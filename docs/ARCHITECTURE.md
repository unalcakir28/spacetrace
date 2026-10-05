# Architecture

This document describes how the code is structured and **why it's structured
that way**. For the product rationale, see [WHY.md](WHY.md); for the plan,
see [ROADMAP.md](ROADMAP.md).

## Overview

```
┌─────────────────┐  ┌───────────┐  ┌─────────────────┐
│ Desktop (Tauri) │  │ CLI       │  │ Hub (Phase 4)   │
│ Phase 3         │  │ ✅ Phase 1 │  │ fleet dashboard │
└───────┬─────────┘  └─────┬─────┘  └────────┬────────┘
        │ in-process       │ direct          │ HTTP
        │                  │                 │
│                    ┌─────┴───────────┐   ┌─┴──────────┐
│                    │ Agent (Phase 2) │   │ Agents     │
│                    │ serve / push    │   │ n machines │
│                    └─────┬───────────┘   └────────────┘
┌───────┴──────────────────┴─────────────────────┐
│ Rust core — all three shells use the same code │
│ scan-core · store · diff                       │
└────────────────────────────────────────────────┘
```

One core, multiple packages. The desktop app, the agent, and the CLI all run
the same scan and comparison code; the only difference between them is the
UI and network layer. As a reference point, Czkawka's `czkawka_core` is
shared the same way across its CLI, GTK4, and Slint interfaces.

## Crates

| Crate | Responsibility | Depends on |
|-------|------------|----------------|
| `scan-core` | Directory scan, tree model, platform-specific backends | rayon |
| `store` | SQLite snapshot store, ncdu import/export, CSV export | scan-core, rusqlite |
| `diff` | Comparing two snapshots | scan-core |
| `dupes` | Identical contents: size → prefix → BLAKE3 | scan-core, blake3, rayon |
| `cli` | The `spacetrace` binary | all of the above, clap |

The dependency direction is one-way: `scan-core` depends on nothing, and
`store`, `diff` and `dupes` only look toward it. The agent (Phase 2) will use
them and will not depend on `cli`.

`dupes` reaches `store` only backwards, through a trait: the duplicate finder
declares what a hash cache has to do and `store` implements it, so the crate
that reads bytes never learns about SQLite. That dependency is behind a
`store` feature the CLI turns on, because `store` is also what the agent is
built from and the agent has to stay a single static binary — a cache for a
command the agent does not have is not worth the bytes.

## Tree model: why an arena

A tree that keeps a `Vec<Child>` per node is expensive at millions of files,
both in memory and in pointer chasing. Instead, a single `Vec<Node>` is used,
and the layout guarantees exactly two things:

1. A node's children occupy a **contiguous** index range
   (`children_start .. children_start + children_len`), so there is no need
   to allocate a separate list per node.
2. Every child has a **larger** index than its parent. Computing subtree
   totals is therefore a single reverse pass (`aggregate`), with no
   recursion.

Nothing more is promised, and in particular the order is **not** breadth-first.
It used to be, because the walk built its own tree and a flatten pass copied it
in level by level. Each directory is now written into the arena as soon as it
has been listed, so the order is the order listings finish in — which varies
with the thread count and between two scans of the same disk. Both properties
above still hold by construction: a parent has to be in the arena already to be
named as one, so its children land after it. `TreeBuilder::push_block` is the
only way to add a child, and it writes `children_start`/`children_len` itself.

Traversal is still cache-friendly — a directory's children are one run — and a
consumer that needs a stable identity across scans uses the path, as `diff`
does.

Names are not stored per node. They are concatenated into one buffer on the
tree, and a node holds a `(u32, u16)` range into it — a `String` per node cost
24 bytes inline plus a heap allocation each, and on a real disk that is 8.6 MB
of text living in 13.2 MB of allocations. Together with counters sized to what
a filesystem can hold (`u32` link and entry counts rather than `u64`), `Node`
is **72 bytes**.

Because a node addresses its name by offset, `Node` cannot be constructed from
outside: [`TreeAssembler`] takes a `&str` per row and interns it, so a wrong
offset is not something a caller can produce.

Reference points: ncdu 2 holds 3.8M files in 162 MB (~25 B/file), and dua-cli
uses a 64-byte arena node. ncdu's figure is not a fair target for us — it does
not keep `own_size`, `own_alloc`, `files` and `dirs` per node — so **dua-cli's
64 bytes is the number to aim at**. Measured peak was 231 B/entry all in while
the walk's intermediate tree and the arena were alive at the same time; that
double storage was removed on 14 September 2026 and the same measurement is now
**125 B/entry** on `/Applications` with an entry-count hint, 142 without one
(`examples/memprobe.rs`; see TODO B1-K).

## Scan

The tree walk is **parallel DFS**: a directory's contents are read on a
single thread (the kernel is fastest for sequential `readdir`), then
subdirectories are dispatched to the rayon pool. This keeps the SSD busy
without the memory bloat of a BFS queue.

The entries a listing produced are accounted for and written into the arena on
that same thread, and only the subdirectories among them become rayon tasks —
about a tenth of the entries on a real disk. Nothing about a scan's result
depends on where that work runs, so it runs where the data already is.

Decisions:

- **Symlinks are not followed.** `DirEntry::metadata()` does not follow the
  link; the link is counted at its own size. This both eliminates cycle risk
  and prevents a tree from being counted twice.
- **Copy-on-write clones are counted once** (macOS/APFS, on by default,
  `--no-clone-dedupe` to disable). A clone has its own inode and `nlink == 1`,
  so hardlink deduplication cannot see it, yet the disk holds its blocks once —
  three 100 MiB clones measured as 0 MiB of consumed free space. APFS names
  the clone family inside the `getattrlistbulk` record the walk already reads
  (`ATTR_CMNEXT_CLONEID`, gated on `EF_MAY_SHARE_BLOCKS`), so finding them
  costs no extra syscall; until 22 September 2026 it was an `open` and an
  `fcntl(F_LOG2PHYS_EXT)` per candidate in a phase of its own, 15–31% of the
  scan. Only the slow listing path still asks one file at a time. The walk
  charges the first member it meets, so the running total is right; after it,
  `Phase::Finishing` moves each family to its member first in `(depth, path)`
  order (`clones.rs`), so two scans of an unchanged disk charge the same file.
  This is the one place `alloc` deliberately parts company with `du`, which
  charges every clone in full.
- **Shared extents are counted once on btrfs and XFS** (Linux, same switch).
  There is no clone family there, and a file can share only part of itself, so
  the unit is the physical byte range: each regular file is opened and asked
  `FS_IOC_FIEMAP` for its extents, and the ones flagged `SHARED` are claimed in
  a process-wide set of merged ranges, keyed by filesystem — the btrfs UUID,
  because every subvolume and snapshot has its own `st_dev` while the extent
  addresses are the filesystem's. A file with no shared extent costs nothing
  beyond that question. Sharing with something outside the scanned root is
  charged inside it, once, the same rule APFS clones follow: `alloc` is what
  the tree references, each block once, not what deleting it would free.
  Measured in Docker, both filesystems: three reflinked 100 MB copies, `df`
  +0 for the copies, `alloc` one copy, `du` three. Code: `scan-core/src/extents.rs`.

  **Which name carries shared blocks is decided after the walk, in `(depth,
  path)` order**, not by whichever thread met them first. With snapshots inside
  the root nearly every file is shared, and first-thread-wins moved the bytes
  between the live tree and its snapshot from scan to scan: five scans of one
  unchanged btrfs put 166, 615, 615, 844 and 897 MB under `live/`, and a diff
  reads that as growth. Now it is 1,300.5 MB every time — the shallowest name,
  so `/usr` rather than `/.snapshots/N/snapshot/usr`. During the walk a shared
  file is charged what it holds alone; the shared rest is added in
  `Phase::Finishing`, which counts the files it goes through in `rows_done`.
  Directories are ranked breadth-first with siblings sorted by name, so no path
  is built per file. Cost on 200,000 snapshot-shared files: +20 ms (btrfs,
  195 → 215) and +7 to +12 MiB peak RSS, about 60–75 bytes per deferred file.

  **Only filesystems that can share are asked.** XFS made without reflink is
  recognised from its geometry (`XFS_IOC_FSGEOMETRY`) and its files are never
  opened — measured with `strace` over 1,000 files: 1 extra `openat`, 1
  `statfs` and 1 `ioctl` per scan, against 1,013 and 1,002 on reflink XFS. The
  `statfs` that identifies a filesystem runs inside the walk's `reading_now`
  guard, with no lock held, and for a mount point under the mount deadline; a
  new device under btrfs that is not a mount point is a subvolume and asks
  nothing.
- **btrfs compression is charged at its compressed size, where that can be
  read.** That can be *more* than `du`: a compressed extent stays whole on disk
  until nothing references any of it, so a file that overwrote 124 KiB of a
  128 KiB compressed extent measured `du` 131,072, `alloc` 204,800, `compsize`
  204,800, and `compressed_bytes_saved` is signed for that reason. `st_blocks` reports a compressed extent uncompressed (a 100 MB log:
  100 MB in `st_blocks`, 2.9 MiB in `compsize`, 3.4 MB of `df`). FIEMAP flags
  the extent `ENCODED` but gives only its logical length; the compressed length
  is in the file extent item, which `BTRFS_IOC_TREE_SEARCH_V2` reads with
  `CAP_SYS_ADMIN`. With it — the agent, usually — each compressed extent is
  charged its on-disk length once, keyed by disk address, and matches
  `compsize` exactly. Without it the extent stays as `st_blocks` counts it, and
  the scan says how many files that left inexact (`compressed_files_inexact`).
- **Hardlinks are counted once.** For files with `nlink > 1`, the
  `(dev, ino)` pair is kept in a shared set; a copy seen again stays visible in
  the tree but contributes 0 bytes. This can be disabled with `--no-dedupe`.
  *Which* name keeps the bytes is unspecified: the walk is parallel, so the
  thread that claims the inode first wins, and that differs between platforms.
  The guarantee is "once", not "the first path" — a test that asserts on one
  of the two names will pass on one OS and fail on another.
- **Errors are not swallowed.** Every unreadable path is counted, and the
  first 64 are stored with their path and error message. A permission error
  does not stop the scan.
- **`one_filesystem`** does not descend into directories that don't share
  the root's `dev` value (`du -x` behavior).

## Size semantics

Two separate sizes are reported, and they are never conflated:

| Field | Meaning | Equivalent |
|------|--------|-----------|
| `size` | Logical size — **file** bytes only | `du -sb` |
| `alloc` | Blocks the disk actually holds, **including directory blocks** | `du -s --block-size=1`, except where blocks are shared |

A directory's own inode size (`len()`, typically 4096) does **not** enter
the logical total: when the user expects "how much space do the files in
this folder take up," adding in the directory's own bookkeeping blocks
would make the number impossible to explain. But because those blocks
genuinely take up space on disk, they are included in `alloc`.

On Unix, `alloc` is computed as `st_blocks * 512` (per POSIX, the unit is
always 512 bytes, independent of the filesystem's block size). This is why
sparse files can show up smaller than their logical size, while small files
show up larger due to block rounding — both are correct.

**Validation:** `crates/scan-core/tests/du_equivalence.rs` compares `alloc`
against `du` byte for byte on a fixture built to make the two measures
disagree — block rounding both ways, a sparse file, a hardlink, a symlink, an
empty directory. `du` is deliberately *not* the oracle for `size`: BSD's `-A`
rounds every file up to a block, and GNU's `--apparent-size` counts each
directory's own inode size, which `size` excludes. `size` is checked against a
naive serial walk in the same file instead, so the oracle shares no code with
the parallel walk or the reverse-pass aggregation.

Windows ships no `du`, so its counterpart is construction-based:
`crates/scan-core/tests/windows_metadata.rs` builds files whose allocated size
cannot equal their logical size and asserts the difference.

### Choosing between them: `SizeBasis`

Both numbers are recorded for every entry, so nothing has to be re-measured to
change which one is being read. What *does* have to be chosen is which one an
ordering or an area is proportional to, and that travels as `SizeBasis`
(`Logical` | `OnDisk`) through `Node::measure`, `Tree::children_by` and
`LayoutOptions::basis`.

It is a parameter rather than a constant because the two orderings genuinely
disagree, and the disagreement is largest exactly where it matters most. A
sparse file reports a length it never allocated — a VM disk image, a database,
a core dump — and those are among the biggest entries on a real disk. A 1 TiB
Docker image holding 19 GiB is a factor of fifty, enough to give it 99% of a
treemap's area and reduce everything genuinely large to a sliver.

Two consequences worth keeping:

- **A list and the figures beside it must share a basis.** "Biggest first" has
  to mean the same thing as the number printed on the row, so `children_by`
  takes the basis rather than assuming one.
- **"Zero" is judged under the basis in force.** The layout drops entries that
  contribute nothing to the total being drawn, so a few-byte file is absent
  logically and present on disk. Both are correct for the question asked.

The desktop defaults to `OnDisk` — the question it exists to answer is what is
filling a disk, and only allocated blocks add up towards what `df` reports as
gone. The CLI stays on `Logical` and says so at its call sites; its summary
line prints both totals either way.

## Snapshot store

The arena layout is written to SQLite **as-is**: `entries.id` is the node's
index, and `children_start`/`children_len` are preserved. The result:
loading a snapshot is a single ordered query with `ORDER BY id` — the tree
is never rebuilt.

```sql
scans(id, host, root, started_at, duration_ms, total_size, total_alloc,
      files, dirs, errors, hardlinks_deduped, scanner_version, label,
      fs_total, fs_available, content_hash)

entries(scan_id, id, parent_id, name, kind, size, alloc, mtime, nlink,
        files, dirs, children_start, children_len)   -- WITHOUT ROWID

rescan_state(scan_id, journal, rescan, flags, digest)  -- only if saved
```

`rescan_state` is what an incremental rescan (B7, macOS) starts from: the
journal cursor, the reason a scan was full or incremental, and the
directories that may not be copied unread (sparse, 5 bytes each). It is
created inside the first save that writes it, never on open, so opening a
database takes no write lock; it is not part of the schema version, so an
older reader still opens the file, and `export_snapshot` leaves it behind.
Its own digest binds it to the scan's `content_hash`: damage means a full
scan, never a wrong copy.

`host` + `root` defines a **target**; comparison and `prune` operate on this
pair. `PRAGMA user_version` holds the schema version; a database written by
a newer version is not opened — it is never silently misread.

### The digest, and what it is not

`content_hash` is a SHA-256 over the scan's **logical content** — its
metadata row and every entry row, each field tagged and every string
length-prefixed. Not the bytes of the file: `export_snapshot` builds a new
SQLite file every time, and page layout or a `VACUUM` can change a file
without changing a value in it. A digest that moves on its own is worse than
none, because the first false alarm teaches everyone to ignore it.

It is checked where a snapshot crosses a boundary — `import_snapshot` refuses
a body that does not match, `export_snapshot` refuses to send one — and on
demand with `spacetrace verify`. A flipped bit leaves a *structurally
perfect* tree, so the structural check cannot see it; that check is
about arena invariants, this one is about values.

**It is not authentication.** Whoever can change the body can recompute the
digest. The threat here is a bad cable, not an adversary; tampering needs a
signature and a way to distribute keys, and that decision has not been taken.

There is also an ncdu-compatible JSON export. The reason is practical: being
able to inspect a recording pulled from a server with `ncdu -f scan.json`
before the desktop app even exists.

## Comparison: the "culprit folder"

A naive diff lists every path that changed — thousands of lines after a
week of work. The question that actually helps isn't "which paths changed,"
it's **"where did the space go."**

The algorithm descends from the root and asks, at every directory: *does a
single subfolder account for almost all of this change?* If yes (default
threshold 90%), it descends into that folder; if no, the change is
genuinely spread out at this level, and this level is reported. Only added
or removed trees are reported as a single row, at their topmost level.

In practice:

```
   +40.1 MiB  grew      42.9 MiB  app/logs/     ← app/ and the root were skipped
   +14.3 MiB  grew      25.7 MiB  backups/
    -1.9 MiB  removed         0 B  uploads/      ← subtree as a single row
```

Children are matched by name via a **merge-join** (both sides are sorted),
so a hash map of the entire tree is never kept in memory.

## Sources other than a local disk

Three ways a tree arrives without this machine walking it, and all three end
in the same `Tree`, so everything after — store, `ls`, `diff`, `age`,
`export` — has one implementation:

| Source | How | Where |
|--------|-----|-------|
| An agent (`--remote`) | Downloads the snapshot as a standalone SQLite file (K4) and loads it through `store::load` | `crates/cli/src/remote.rs` |
| An ncdu or gdu export (`import`) | Nested JSON → `Tree::from_nested` | `crates/store/src/ncdu_import.rs` |
| An S3 bucket (`scan s3://…`) | ListObjectsV2 pages → keys drawn as folders → `Tree::from_nested` | `crates/cli/src/s3/` |

**S3 lives in the CLI**, not in a crate of its own and not in `store`: the
agent is built from `store` and has to stay a small static binary, and the HTTP
client it needs is already in the CLI for `--remote`. SigV4, HMAC-SHA256 and the
XML reader are written by hand (`sha2` was already in the workspace), and are
tested against AWS' published signing vectors, responses captured from MinIO
and AWS, and a live MinIO when `SPACETRACE_TEST_S3_ENDPOINT` points at one.

**Credentials follow botocore's default chain** (`config.rs` decides,
`credentials.rs` fetches): environment keys, a profile's assumed role, a web
identity token, IAM Identity Center, keys in either file, `credential_process`,
then the container and EC2 metadata services. STS calls go through the same
signer; the SSO token is found in `~/.aws/sso/cache` under the SHA-1 of the
session name or start URL, with SHA-1 written by hand too. Temporary keys are
renewed in botocore's two windows (advisory 15 minutes before expiry,
mandatory 10), shrunk to half and a quarter of the lifetime for keys shorter
than an hour. In the advisory window one request renews while the others go
on with the current keys; only past the mandatory deadline do they wait. An
`ExpiredToken` answer renews them once and retries. An
expired SSO token is an error naming `aws sso login`, never a fall-through to
the next source, which would list as somebody else. The precedence was checked
against the aws CLI v2 itself on 24 configurations (`export-credentials` and a
listing, both pointed at one stand-in server): all agree.

What a bucket snapshot means:

- **Identity.** `host` is the service — `s3.amazonaws.com` for AWS, whatever
  the region, or the endpoint's `host:port` — and `root` is `s3://bucket` or
  `s3://bucket/prefix` without a trailing slash. Two machines listing one
  bucket produce one target, which is what `diff --path` and the hub's trends
  group by — and for a bucket `diff --path` takes both snapshots from the
  newest one's service, since one bucket name on two services is two
  buckets. `fs_total`/`fs_available` stay empty: a bucket has no capacity to
  fill, and an empty capacity keeps free-space alerts quiet.
- **Sizes.** An object has no blocks, so `size` and `alloc` are both its
  length and `alloc` totals every byte listed. A folder marker (a key ending
  in `/`) is drawn as its folder; should it — or an object `a` beside a folder
  `a/` — hold bytes, they are the folder's own cost in `alloc` only, the way a
  directory's own blocks are on a disk (invariant 1). `mtime` is
  `LastModified`; a folder takes its newest content's. A key with a `.` or
  `..` segment is left out and counted as a scan error with its bytes,
  because as a name it reads as navigation to every path consumer.
- **Scope.** Current versions only. Old versions, delete markers and
  unfinished multipart uploads are billed and are not in a listing; the CLI
  says so under every total.
- **Progress and cancelling** follow the walk: files, folders and bytes move
  once per page the tree takes in, the folder count moves while the key space
  is being split, and a cancel stops every request before its next page and
  returns `ErrorKind::Interrupted` and no tree.

**Listing in parallel** (`listing.rs`). One stream lists a page per round
trip, so ten million keys are ten thousand round trips in a row. Folders are
disjoint ranges of the key space: past one plain page — kept for a bucket
that fits in it, thrown away otherwise, so that no listing starts in the
middle of a folder, where S3 and MinIO read `start-after` differently —
folders are found with `/`-delimited listings (three levels at most, until
there are four per worker), runs of sibling folders become ranges listed by
`prefix` + `start-after`, and an idle worker takes the second half of the
folders the earliest busy range has not reached. **The tree cannot
change**: workers only fetch, and the objects reach the tree builder on the
calling thread in key order, exactly the sequence one stream gives, so
`Tree::try_from_nested` builds the same arena however the work fell. What is fetched ahead waits in a bounded queue;
a range far ahead stops and its worker moves to the front instead of waiting.
The default is 16 workers on AWS, however addressed, and one stream for any
other endpoint (`Settings::default_workers`): AWS seeks to any key, while
MinIO walks its drive for each new listing, so ranges side by side were
slower there than one stream. The measurements are in
`listing.rs`, beside the constants they chose.

## Platform-specific backends

The portable backend is `read_dir` + `symlink_metadata`, with metadata
reading split out via `cfg`. macOS has had its fast path since 11 September
2026 (B5): `getattrlistbulk` (`bulk.rs`) takes names and metadata in one call, and
the portable path remains only for directories that hold a mount point. Linux
has had its own since 5 October 2026 (B6, `dents.rs`): `getdents64` into one
per-thread buffer and an `fstatat` per entry relative to the directory's
descriptor, with `AT_NO_AUTOMOUNT`. `std` already asked by descriptor, so what
went is the allocations around each entry: about a tenth of a warm walk with
glibc, and up to 3× with the static musl the agent ships as, whose allocator
is slow under threads. `statx` with a narrow mask measured the same as
`fstatat` and was dropped. It falls back to the portable path in the same
place macOS does. The table is the plan it came from:

| Platform | Method | Note |
|----------|--------|-----|
| Windows | Direct NTFS **MFT** read; incremental via the **USN journal** | Requires administrator privileges; ReFS has no MFT. Mandatory in Phase 3: WizTree scans 2 TB in ~14 s. |
| Windows (unprivileged) | `NtQueryDirectoryFileEx`, 64 KB buffer, `FileIdBothDirectoryInformation` | Size + file id in a single call; no extra syscall needed for hardlink dedup |
| macOS | `getattrlistbulk` | Noticeably faster than `readdir + lstat` when size/date is needed |
| Linux | `getdents64` + `fstatat`, DFS per thread | Done (B6); warm 6–14% faster with glibc, 1.3–3.2× with musl; cold 3% and 23% |

`RawMeta::for_path` in `scan-core` is the boundary of this split; the fast
paths will plug in by producing the same `RawMeta`.

### What Windows costs today

A directory listing on Windows carries the logical size but neither the
allocated size nor the file identity. Both come from one handle, opened through
`std::fs::OpenOptions` (`FILE_READ_ATTRIBUTES`, `BACKUP_SEMANTICS`,
`OPEN_REPARSE_POINT`) so that it closes itself: `FILE_STANDARD_INFO` for
`AllocationSize`, and `BY_HANDLE_FILE_INFORMATION` for the link count, file id
and volume serial.

**`GetCompressedFileSizeW` is not the answer**, though it looks like the
path-based call that would avoid the handle. It returns the *logical* size for
any file that is neither compressed nor sparse — CI settled it by reporting
exactly 100001 bytes for a 100001-byte file. The name is the giveaway.

That handle is the price of being correct here, and it is a real one: our own
measurements put an extra syscall per entry at **+36%** wall clock. An
elevated scan of a whole NTFS volume avoids it by reading the master file
table instead (`ntfs/`, TODO B4 — written, awaiting Windows CI); the
unprivileged route in the table above, `NtQueryDirectoryFileEx` with
allocation and file id inside the listing, is still unbuilt. `FileIdentity::Skipped` saves the second query when a scan will
not use the identity, but not the open.

Directories are queried like files, so `alloc` means the same thing on both
platforms. What NTFS reports for a directory may still be 0, because a
directory's index lives in `$INDEX_ROOT`/`$INDEX_ALLOCATION` rather than in the
unnamed data stream; `crates/scan-core/tests/windows_metadata.rs` prints the
observed value rather than asserting a guess about the filesystem.

## Why these technology choices

**Rust.** The agent needs to be deployable to a NAS or a container as a
single static binary; a language with no runtime dependency is a
requirement. It also has the widest ready-made crate ecosystem for
low-level filesystem calls (MFT, getattrlistbulk).

**Tauri v2 (Phase 3).** Once mobile dropped out of scope, Flutter's "five
platforms, one codebase" advantage went away. Tauri is mature on desktop,
its installer is 3–15 MB (vs. 50–150 MB for Electron), and it calls the
Rust core in-process, directly — no FFI bridge. Since the UI is React/TS,
existing web knowledge carries over directly. The risk is WebKitGTK quirks
on Linux; the treemap needs to be tested across all three WebViews.

**Fallback plan: Avalonia 12 + .NET.** If the cost of Rust turns out to be
unacceptable, the desktop app and agent could be rewritten end-to-end in C#
(a single binary via NativeAOT). The architecture and product definition
would not change.

**SQLite.** Snapshots need to be portable as a file (pull it from the
server with `scp`, open it locally). A snapshot diff becomes a single
query. No server setup is required.

## Known limits and technical debt

- Windows pays an extra call per entry for `alloc` and one open handle per
  file for hardlink dedup, and a directory's own blocks are not counted (see
  "What Windows costs today"). None of this is visible from macOS: only CI
  runs the Windows tests.
- **Linux shared and compressed extents are exact for data, not for
  metadata.** What a walk cannot see, each measured or reasoned in Docker:
  btrfs *inline* files (data of up to 2 KiB kept inside metadata; FIEMAP gives
  no address and no `SHARED` flag), so a snapshot of many tiny files still
  counts them once per name — 166 MB of a 1.3 GB, 100k-file corpus;
  RAID copies; and compressed extents without `CAP_SYS_ADMIN`. Those err
  high. One errs **low**, as `du` does: an *uncompressed* bookend extent (partly
  overwritten, still whole on disk until every reference is gone) is charged
  only for the part still referenced, because FIEMAP reports references, not
  extents. Compressed bookends are charged whole when the size can be read.
- **The FIEMAP costs an open and an ioctl per file on btrfs and reflink XFS.**
  Warm cache, 100,000 files, 6 threads: btrfs 14 → 81 ms, XFS 13 → 42 ms
  (2 October); cold cache btrfs 143 → 238 ms. With every extent shared, XFS's answer serialises
  inside the kernel and got *slower* with more threads (271 ms at 1 thread,
  728 ms at 6). Since 5 October 2026 FIEMAP on XFS goes through a gate: one
  thread at a time, in runs of 64 files, armed on the first shared extent,
  so unshared XFS pays nothing (100k shared files, 6 threads: 855 → 200 ms
  warm; TODO A6). Measured on loop devices in a VM; real hardware is still
  to be checked.
  `--no-clone-dedupe` switches it off.
- **ZFS is detected and not corrected** (unverified: no ZFS in the test
  kernel). Its `st_blocks` already reflects compression, but block cloning
  (OpenZFS 2.2+) and deduplication live in pool-wide tables no walk can read,
  so the summary says the total can exceed what the pool holds.
- Sampling the way `btdu` does (`BTRFS_IOC_LOGICAL_INO`, root only) is not
  built. A sampled total carries sampling error and the difference of two
  carries both, so a folder that grew by a fraction of a percent of the disk
  sits inside the noise — and "what grew" is exactly that difference. FIEMAP's
  answer is per file and deterministic, which is what a diff of two snapshots
  needs. What sampling sees and this does not is metadata and inline data, the
  first limit above.
- The scan keeps the entire tree in memory. Memory profile needs to be
  measured on roots with 10M+ files; streaming writes will be added if
  needed.
- `Tree::rel_path` walks up from the root on every call; it should not be
  used in a hot loop on deep trees.
- UI strings are in English and embedded in the code; no i18n layer is
  planned — this is a deliberate scope decision, not debt (see
  [DECISIONS.md](DECISIONS.md)).

## Development

```bash
cargo test --workspace                       # current count: CLAUDE.md
cargo clippy --workspace --all-targets       # should be warning-free
cargo fmt --all
cargo check -p spacetrace-scan-core --target x86_64-pc-windows-msvc --all-targets
```

Tests use a real filesystem in temporary directories, including hardlink,
symlink, permission-error, depth-limit, and diff scenarios. When adding new
scan behavior, extend `crates/scan-core/tests/du_equivalence.rs` — the external
`du` comparison lives there, and a change to block accounting that it does not
cover is a change nothing verifies.
