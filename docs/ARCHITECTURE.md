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
| `store` | SQLite snapshot store, ncdu export | scan-core, rusqlite |
| `diff` | Comparing two snapshots | scan-core |
| `cli` | The `spacetrace` binary | all of the above, clap |

The dependency direction is one-way: `scan-core` depends on nothing, and
`store` and `diff` only look toward it. The agent (Phase 2) will use all
three and will not depend on `cli`.

## Tree model: why an arena

A tree that keeps a `Vec<Child>` per node is expensive at millions of files,
both in memory and in pointer chasing. Instead, a single `Vec<Node>` is used,
with nodes laid out in **BFS order**. This has three consequences:

1. A node's children occupy a **contiguous** index range
   (`children_start .. children_start + children_len`), so there is no need
   to allocate a separate list per node.
2. Every child has a **larger** index than its parent. Computing subtree
   totals is therefore a single reverse pass (`aggregate`), with no
   recursion.
3. Treemap layout and rendering scan the array in order — cache-friendly.

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
64 bytes is the number to aim at**. Measured peak is 231 B/entry all in, and
the remaining gap is not the arena: the walk's intermediate tree and the arena
are alive at the same time (see TODO B1).

## Scan

The tree walk is **parallel DFS**: a directory's contents are read on a
single thread (the kernel is fastest for sequential `readdir`), then
subdirectories are dispatched to the rayon pool. This keeps the SSD busy
without the memory bloat of a BFS queue.

Decisions:

- **Symlinks are not followed.** `DirEntry::metadata()` does not follow the
  link; the link is counted at its own size. This both eliminates cycle risk
  and prevents a tree from being counted twice.
- **Copy-on-write clones are counted once** (macOS/APFS, on by default,
  `--no-clone-dedupe` to disable). A clone has its own inode and `nlink == 1`,
  so hardlink deduplication cannot see it, yet the disk holds its blocks once —
  three 100 MiB clones measured as 0 MiB of consumed free space. They are found
  by asking `fcntl(F_LOG2PHYS_EXT)` where a file's data physically starts:
  files sharing that offset share their extents. Only files whose size collides
  with another file's are probed, because each probe is an open and an `fcntl`.
  Measured on a developer's tree: 430 clones, 0.76 GiB of 15.6 GiB (4.9%), at a
  cost of ~70 ms. This is the one place `alloc` deliberately parts company with
  `du`, which charges every clone in full.
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
      files, dirs, errors, hardlinks_deduped, scanner_version, label)

entries(scan_id, id, parent_id, name, kind, size, alloc, mtime, nlink,
        files, dirs, children_start, children_len)   -- WITHOUT ROWID
```

`host` + `root` defines a **target**; comparison and `prune` operate on this
pair. `PRAGMA user_version` holds the schema version; a database written by
a newer version is not opened — it is never silently misread.

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

## Platform-specific backends

Right now there is a single portable backend (`read_dir` +
`symlink_metadata`), and metadata reading is split out via `cfg`. Planned
fast paths:

| Platform | Method | Note |
|----------|--------|-----|
| Windows | Direct NTFS **MFT** read; incremental via the **USN journal** | Requires administrator privileges; ReFS has no MFT. Mandatory in Phase 3: WizTree scans 2 TB in ~14 s. |
| Windows (unprivileged) | `NtQueryDirectoryFileEx`, 64 KB buffer, `FileIdBothDirectoryInformation` | Size + file id in a single call; no extra syscall needed for hardlink dedup |
| macOS | `getattrlistbulk` | Noticeably faster than `readdir + lstat` when size/date is needed |
| Linux | `getdents64` + `statx`, DFS per thread | The current approach already follows this model |

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
measurements put an extra syscall per entry at **+36%** wall clock. The fast
path above removes it, because `NtQueryDirectoryFileEx` returns allocation and
file id inside the listing itself — which is the strongest argument for
building it. `FileIdentity::Skipped` saves the second query when a scan will
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
- On btrfs/ZFS, reflinks, compression, and dedup mean the tree walk
  misreports actual disk usage. Getting it right requires sampling, like
  `btdu` does; for now, a "filesystem-aware mode" is planned for Phase 5.
- APFS clones are not yet deduplicated (`alloc` can be inflated on macOS).
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
cargo test --workspace                       # 177 tests
cargo clippy --workspace --all-targets       # should be warning-free
cargo fmt --all
cargo check -p spacetrace-scan-core --target x86_64-pc-windows-msvc
```

Tests use a real filesystem in temporary directories, including hardlink,
symlink, permission-error, depth-limit, and diff scenarios. When adding new
scan behavior, extend `crates/scan-core/tests/du_equivalence.rs` — the external
`du` comparison lives there, and a change to block accounting that it does not
cover is a change nothing verifies.
