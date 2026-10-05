# To do

Live work list. For phase definitions and exit criteria see
[docs/ROADMAP.md](docs/ROADMAP.md), for rationale see [docs/WHY.md](docs/WHY.md),
for where competitors are ahead see [docs/COMPETITORS.md](docs/COMPETITORS.md).

Last updated: 2 October 2026 (**The idea pool and two long-open items
closed, nothing released yet:** `scan --ssh`, `scan s3://`, `watch`, `pkgs`,
the agent's `/metrics`, A6 shared and compressed extents on btrfs and XFS, the
cushion surfaces in the treemap layout, and the site's fonts served from its
own origin. Every one went through a review whose fixes are in. On
5 October the desktop and hub pins moved onto it, and the desktop gained the
cushion treemap's drawing half.)

Before that, 14 September 2026 (**Version cut: CLI 0.7.0, desktop 0.7.0,
hub 0.5.0.** Since it stayed fixed at v3, the schema had no order requirement.
Also **B1-K done** — the double storage is gone, the measurement
infrastructure is in the repository. Before that: **Order 1–4 closed** — E1, E2, A4, A1, A2,
A4w, B1, A3, A5. CI green on all three platforms. 10 September was also
a release day: desktop 0.4.0 → 0.4.2, then desktop 0.5.0 together with A5,
hub 0.4.0 and CLI 0.5.0. Order was required because of a schema jump: first
desktop and hub, then CLI (rationale in docs/RELEASING.md). Also macOS FDA
onboarding and a **stable signing identity** landed — the free half of E3,
details below. Next up → Order 5: **B2**, then B3)

---

## Open decisions ✅ closed

All five were decided on 7 September 2026. Rationale and measurements are
in [docs/DECISIONS.md](docs/DECISIONS.md); summary:

- [x] **Interface language** → English (K1). CLI, README, ARCHITECTURE translated;
      rationale documents stayed Turkish. i18n declared out of scope.
- [x] **License model** → core + agent Apache-2.0 in this monorepo; desktop and
      hub commercial in a separate repository (K2). The "Pro = unlimited agent"
      hypothesis in WHY.md was corrected because it was unimplementable.
- [x] **Agent protocol** → HTTP + JSON, axum framework; snapshot body
      `application/octet-stream` (K3).
- [x] **Snapshot portability** → raw SQLite, `VACUUM INTO` + zstd (K4).
      Measured: 49.5 B raw per entry, 12.4 B zstd → 1M files ≈ 12 MB.
- [x] **GitHub repository** → public (K5).

---

## Phase 1 — Core and CLI ✅

- [x] Workspace scaffold, CI (ubuntu/macos/windows), Apache-2.0
- [x] `scan-core`: parallel DFS, arena tree (contiguous children, child index
      greater than the parent's — originally in BFS order, not since B1-K)
- [x] Hardlink deduplication `(dev, ino)`, can be disabled with `--no-dedupe`
- [x] Symbolic links are not followed, counted with their own size
- [x] `alloc` = `st_blocks * 512`; `size` = file bytes only
- [x] `--exclude`, `-x/--one-file-system`, `--depth`
- [x] Counting and sampling permission errors (the scan does not stop)
- [x] `store`: SQLite schema, storing the arena as-is, `PRAGMA user_version`
- [x] `store`: `list`, `latest_for`, `last_two_for`, `delete`, `prune`
- [x] ncdu-compatible JSON export
- [x] `diff`: culprit folder detection (concentration threshold), added/removed subtrees
- [x] `cli`: scan / ls / scans / diff / export / prune / rm
- [x] Live progress indicator, `--json` on every command
- [x] 32 tests; exact verification against `du` (`/usr`, `/usr/share`, `/etc`)
- [x] clippy warning-free, `cargo fmt` clean, Windows target type-checked
- [x] README, WHY, ROADMAP, ARCHITECTURE

---

## Phase 2 — Agent ✅ core done

### Core
- [x] `agent` crate (added to the workspace, `spacetrace-agent` binary)
- [x] `agent scan` — one-off, no network (single root with `--root`)
- [x] Configuration file (TOML): roots, exclusions, scheduling, token
      — unknown keys are rejected, pre-validated with `agent check`
- [x] Built-in scheduler — hand-written 5-field cron (chrono not added),
      including Vixie's dom/dow union rule
- [x] Snapshot rotation (`keep` per root; `store::prune_target`)

### Network
- [x] `agent serve` — axum HTTP service
  - [x] `GET /health` (no token, liveness+version only), `GET /status`
  - [x] `GET /scans`, `GET /scans/:id`, `GET /scans/:id/download`
  - [x] `POST /scans` (202 + background scan), `POST /snapshots` (receiving endpoint)
  - [x] Bearer token validation (constant-time comparison), file/env/config
  - [x] Body size limit, clean shutdown on SIGTERM
  - [x] Optional built-in TLS — **D2 done** (21 September 2026); a reverse
        proxy is still the recommendation where a domain name exists
- [x] `agent push <url>` — send the snapshot to the hub/another agent (zstd)
- [x] Concurrent scan lock (the same root is not scanned twice → 409)
- [x] Rate limiting — **D3 done** (14 September 2026); the reason for deferring it
      was wrong, details there

### Client side
- [x] Remote source in the CLI: `--remote <url|name>` (scans / ls / diff / export)
- [x] `spacetrace pull` — pull a remote snapshot into the local database
- [x] Remote source definitions `remotes.toml`
- [x] SSH mode: running a temporary agent on the other side without requiring
      installation — `spacetrace scan --ssh DEST PATH` *(2 October 2026)*. A
      flag on `scan`, not a command: `--save`, `--label`, walk flags,
      `--ncdu`, `--json` mean what they already mean. A copy of the same
      version goes into a `mktemp -d` dir over ssh stdin (no scp/sftp), runs
      `scan --save`, the db comes back over stdout and is imported through
      `import_snapshot` (digest checked). The remote dir belongs to a lease
      session that cleans up when the CLI's pipe closes — so also on
      `kill -9` and network loss — deleting files by name and `rmdir`, from
      its own copy of the path. One ControlMaster, clients in their own
      process group, so Ctrl-C reaches only the CLI, which waits for
      "removed" and exits 130. Binary: self if portable, else the release
      asset of this version (SHA256SUMS via `update::fetch_release`, cached
      by version+target), `--binary` for dev builds, `-V` must match.
      Measured against Docker sshd: v0.9.1 linux-aarch64 asset 7.2 MiB;
      success/fail/SIGINT/pgrp Ctrl-C/SIGKILL all leave /tmp empty; noexec
      /tmp → $HOME; hostile path stored byte-exact; 600,601 entries → 19.7 MB
      db. **Still open:** Windows (no ControlMaster, no process groups —
      refused today); error paths and clone count are not in the snapshot; a
      POSIX login shell is required.

### Distribution
- [x] Static binary (musl) — linux/amd64, linux/arm64 (release workflow)
- [x] Docker image (scans the host via a read-only mount)
- [x] systemd unit + example configuration (hardened, ProtectSystem=strict)
- [x] `curl | sh` install script (POSIX sh, busybox-compatible)
- [x] GitHub Releases automation (tag → build → artifact + SHA256SUMS)

### Validation (exit criterion)
- [x] End-to-end local validation: agent + CLI `--remote diff` produces meaningful
      output, correctly finds the culprit folder
- [ ] **Install on our own Hetzner servers, the Proxmox host, and a container**
- [ ] **Collect real data for a week** — these two can only be done on
      real machines, the code side is ready

---

## Phase 3 — Desktop ✅

Separate repository: [spacetrace-desktop](https://github.com/unalcakir28/spacetrace-desktop) (K2).
The layout engine stayed here (`crates/treemap`), because it is core and testable.

- [x] Tauri v2 + React/TS scaffold, calling the Rust core in-process
- [x] Squarified treemap — layout in Rust (`crates/treemap`), drawing in Canvas2D
- [x] LOD: rectangles below min_area are not split; the tile count depends
      on the screen's size, not the disk's
- [x] Culling and hit-testing — the hierarchy itself was used instead of a quadtree
      (a separate index is unnecessary since a child is always inside its parent)
- [x] Folder tree panel (lazily expanding), bidirectional selection sync
- [x] Coloring by file type, send to trash, open in Finder/Explorer
- [x] Remote source flow: download a snapshot from the agent, browse it as if local
- [x] Diff view (comparison table for two snapshots)
- [x] Node ids tied to generation — an id belonging to an old tree is rejected
- [~] **Windows MFT fast path** — written, behind admin, awaiting Windows CI (→ **B4**)
- [x] macOS Full Disk Access onboarding screen — banner on the welcome screen,
      button in the settings panel, six usage descriptions in `Info.plist`
- [x] Timeline view (a target's full history) — **C3**, released in desktop
      0.6.0
- [ ] Treemap performance testing across three WebViews (including WebKitGTK) — only
      verified on macOS

---

## Phase 4 — Hub ✅

Separate repository: [spacetrace-hub](https://github.com/unalcakir28/spacetrace-hub) (K2).

- [x] axum + SQLite service, self-hosted, single static binary
- [x] Multi-agent dashboard — sorted by urgency (the one filling up soonest first)
- [x] Per-folder growth trend (least squares, with fit quality)
- [x] "Fills up in N days at this rate" estimate — not shown if the basis is weak
      (≥3 samples, ≥1 day, r² ≥ 0.5, measured capacity, ≤10 years)
- [x] Threshold alerts: webhook (free space, growth rate, fill horizon) + cooldown
- [x] Team access and token management — agent tokens are hashed and can be
      revoked; an agent token cannot read the dashboard, an admin token cannot push
- [x] One-command setup with docker-compose
- [x] Capacity measurement added to the core (schema v2) — a prerequisite for the estimate
- [x] Email alerts — **C1**, released in hub 0.5.0 (`mailto:` target,
      `[smtp]` config section)
- [x] Per-person accounts — **C2**, released in hub 0.5.0 (viewer/admin)

---

## Competitive gaps

Work list drawn from the 9 September 2026 competitor analysis. Rationale, measurements and
sources are in [docs/COMPETITORS.md](docs/COMPETITORS.md); every item is labeled with **which
competitor is better than us**. The ordering is below, under the "Order" heading.

### A. Accuracy — meet our claim on three platforms

These are not missing features, they are **failing to keep our word**. A speed
shortfall is a competitive disadvantage; a wrong number refutes the product itself.

- [x] **A1 Windows `alloc` real value** — written *(9 September 2026)*,
      **awaiting CI confirmation** (only type-checking is possible on macOS).
      `FILE_STANDARD_INFO` → **`AllocationSize`** is used.
      *`GetCompressedFileSizeW` was tried first and CI disproved it:* it
      returns the logical size for uncompressed, non-sparse files — it said
      100,001 for a 100,001-byte file. The name already gave it away. So a
      path-based call doesn't work, a handle is required. Directories are
      queried too, so `alloc` means the same thing on both platforms.
      *Competitor:* TreeSize, WizTree, WinDirStat 2.5.0 give the correct number.
- [x] **A2 Windows hardlink dedupe** — written *(9 September 2026)*, **awaiting
      CI confirmation**. `GetFileInformationByHandle` gives `nNumberOfLinks` +
      `nFileIndexHigh/Low` + `dwVolumeSerialNumber` in a single call, i.e. nlink,
      ino and dev arrive together. The handle is opened with
      `std::fs::OpenOptions` instead of `CreateFileW` (`FILE_READ_ATTRIBUTES`,
      `BACKUP_SEMANTICS`, `OPEN_REPARSE_POINT`): RAII closes it, no leak on
      early return, the unsafe surface is small, and **no new windows-sys
      feature was needed**.
      *Side benefit:* `-x/--one-file-system` now actually works on Windows —
      previously it was silently a no-op because every entry reported volume 0.
      *Cost:* one handle per file. Only paid when dedupe or `-x` is on
      (`FileIdentity::Skipped`); measured order of magnitude +36%, removed by B4.
      *Competitor:* WinDirStat 2.5.0 (January 2026).
- [x] **A3 APFS clone deduplication** — done *(9 September 2026)*, on by
      default, turned off with `--no-clone-dedupe` (`dedupe_clones` in the agent).
      A clone has its own inode and `nlink == 1`, so hardlink deduplication
      can't see it; but its blocks sit on disk only once. Controlled
      measurement: **3 clones × 100 MB = 0 MB** of free-space consumption,
      while `du` says 400 MB. Detection is via `fcntl(F_LOG2PHYS_EXT)`: files
      starting at the same physical offset share an extent. Compressed files
      return `ENOTSUP`, which safely means "not a clone". Only files whose
      **size collides with another file's** are queried, because every query
      is an `open` + `fcntl`.

      **Measurement correction — correcting my own number.** My first
      measurement said "we're overcounting by 31.7%" and it **was wrong**: my
      measurement script wasn't deduplicating hardlinks, while the scanner
      eliminates 68,000 hardlinks in `~/github`. Since two hardlinks share the
      same inode, they naturally report the same physical offset — meaning
      most of what I counted as "clones" were actually hardlinks we'd already
      handled. After hardlink deduplication, the **real number: 0.76 GiB /
      15.6 GiB = 4.9%**, 430 files. The independent Python tool and the
      scanner give the exact same number (430).

      | Tree | Recovered | Cost |
      |------|-----------|---------|
      | `/Applications` (412k entries) | 0 (no clones at all) | +91 ms (+8%) |
      | `~/github` (138k files) | 0.76 GiB (4.9%) | +72 ms (+21%) |

      **Invariant #1 rewritten:** `alloc` is no longer "identical to `du`",
      but "what the disk actually holds". When there are no shared blocks it's
      identical to `du` (enforced by a test); when there are, the difference
      is **exactly the shared blocks** (also tested).
      *Competitor:* **DaisyDisk 4.34** already did this; now we do too.
- [x] **A4 (Unix) `du` comparison test — written.** *(9 September 2026)*
      **The item was framed wrong:** it's not "the test only runs on macOS",
      **there was no test at all**. `totals_match_the_files_on_disk` was
      comparing against constants the test itself wrote, and the `alloc`
      claim was `>= 4096`; `du` verification was done by hand. Meanwhile
      CLAUDE.md, ARCHITECTURE.md and WHY.md said "this is a tested condition"
      — all three are now true.
      New: `crates/scan-core/tests/du_equivalence.rs`, 6 tests, running in CI
      on ubuntu + macos. The `alloc` oracle is external `du`; the `size`
      oracle is a **naive serial walk** over the same file, because `du`
      can't give the logical size (BSD `-A` rounds to the block, GNU
      `--apparent-size` adds the directory inode — our `size` doesn't add it).
      Verified with mutation testing: `alloc`=`size` → 3 tests fail, breaking
      dedupe → 3 tests, adding directory-inode summing → 1 test (only the
      naive walk catches it; `du` would approve that mutation).
- [x] **A4w Windows counterpart of the `du` comparison** — written
      *(9 September 2026)*, in the same commit as A1/A2.
      `crates/scan-core/tests/windows_metadata.rs`, 6 tests.
      There's no `du` on Windows, no external oracle; instead a
      **construction-based** test. Since the cluster size can't be queried
      portably, the trick is: a length that's **not a multiple of 512** is
      chosen (100,001). Since an NTFS cluster is at least 512 bytes, a real
      allocation number **can't equal** that length — meaning the
      `alloc != size` claim proves the logical size isn't being returned,
      without knowing the cluster size. The file content is deliberately made
      **incompressible** (a file full of zeros would unfairly fail the test
      on a volume with compression turned on).
      The `stats.errors == 0` assertion is also a canary: if there were a
      systematic API error, `alloc` would silently fall back to the logical
      size, and the test catches that.
- [x] **A5 Snapshot integrity check** — done *(10 September 2026)*. In schema
      v3, `scans.content_hash`: the SHA-256 of the scan's *logical content*
      (the metadata row + every `entries` row, fields tagged, strings
      length-prefixed). Not the file's bytes — `export_snapshot` builds a new
      SQLite file every time, and `VACUUM` changes the file without changing
      its content; a hash that moves on its own is worse than none. If
      `import_snapshot` doesn't match, it takes nothing; `export_snapshot`
      doesn't ship something it knows is corrupt; `spacetrace verify` checks
      it on request. `NULL` = "no hash" (pre-v3), not "corrupt". **Not
      authentication** — it also computes a hash for a body that could be
      tampered with; the threat model is corruption, not an attacker.
      No new dependency (`sha2` is already in the workspace).
- [x] **A6 btrfs/XFS shared and compressed extents** — done *(2 October
      2026)*, on by default with clone dedupe, off with `--no-clone-dedupe`.
      Measured in Docker on loop-mounted btrfs and XFS before writing code: 3
      reflinked 100 MB copies move `df` +0 for the copies while `du` says
      300 MB; a 100 MB zstd log reports 100 MB in `st_blocks`, 2.9 MiB in
      `compsize`, 3.4 MB of `df`.
      **Shared:** one `open` + `FS_IOC_FIEMAP` per regular file
      (unprivileged); `SHARED` extents claimed in a merged byte-range set per
      filesystem. Ranges, not extents, because a partly overwritten copy
      reports the old extent in two pieces (measured). Keyed by btrfs UUID
      (`BTRFS_IOC_FS_INFO`), not `st_dev`: every subvolume and snapshot has
      its own device. Sharing outside the root is charged inside it, once
      (same rule as APFS).
      **Compressed:** FIEMAP gives only the logical length of an `ENCODED`
      extent; the on-disk length needs `BTRFS_IOC_TREE_SEARCH_V2`
      (`CAP_SYS_ADMIN`). As root: charged on-disk, once per disk address,
      equal to `compsize`. Unprivileged: left as `du` counts it and reported
      (`compressed_files_inexact`).
      **ZFS:** detected by `statfs` magic, summary warns; unverified, no ZFS
      in the test kernel. **Sampling (btdu):** not built; a diff of two
      sampled totals sits in the noise, FIEMAP is exact per file.
      **Which name carries it:** the walk charges the first name it meets so
      the running total is right; `Phase::Finishing` then moves every shared
      range to its owner first in `(depth, path)` order, so two scans of an
      unchanged disk attribute the same bytes to the same folder (the APFS
      clone pass does the same since 2 October). XFS made without reflink is
      recognised by `XFS_IOC_FSGEOMETRY_V1` and skipped. `compressed_bytes_saved`
      is signed: a compressed extent can cost more on disk than `du` says.
      **Tests:** 9 in `du_equivalence.rs` assert `du − alloc == shared +
      compressed` against `du`, `df`, `compsize`; skip on ext4, run by the CI
      job `test (btrfs + XFS)` with `SPACETRACE_REQUIRE_REFLINK=1`. Passed in
      Docker on btrfs as root and as an ordinary user and on XFS; the whole
      file passes on XFS with `reflink=0`. 4 mutations each caught.
      **Cost** (100k files, warm, 6 threads): btrfs 16 → 101 ms, XFS 18 →
      59 ms; cold btrfs 143 → 238 ms. Memory +0.8 MiB for 200k shared files.
      **XFS FIEMAP capped** *(5 October 2026)*: the kernel serialises FIEMAP
      on shared XFS extents, so more threads made it slower. A gate per
      XFS volume (`fiemap_cap`: XFS 1, btrfs none) arms on the first shared
      extent seen and then lets one thread map files at a time, 64 per
      permit; before it is armed FIEMAP stays inline, so unshared XFS pays
      nothing. A waiter abandons the gate only if no file finished during
      its whole wait (a stuck holder, not a slow one). Docker, 100k files,
      6 threads: hot 855 → 200 ms warm, 971 → 248 ms cold; pairs 261 → 198;
      half 457 → 171; unshared and 1 thread unchanged. A permit per file
      was ~3× slower, cap 2 convoys. **Still open:** re-measure on real
      hardware; `GETFSMAP` as root. btrfs inline files (≤2 KiB, in metadata)
      still count once per name under a snapshot (166 MB of 1.3 GB, 100k
      files). Bookend extents and RAID copies are not visible. bcachefs/OCFS2
      not covered. The new CI job has not run on GitHub Actions yet.
      *Competitor:* btdu (Monte Carlo, 1% resolution at ~100 samples).

### B. Speed — measured gaps

- [x] **B8 The clone probe leaves the walk** — *(22 September 2026)*

      After B5 removed the per-entry `lstat` on macOS, the remaining
      per-entry syscall was the clone probe: an `open` plus an
      `fcntl(F_LOG2PHYS_EXT)` for every file whose logical size collided
      with another's, run in a phase of its own after the walk. Ablation put
      it at **15–31% of the scan**: 23 ms of 74 on `/usr`, 90 of 596 on
      `/Applications`, 551 of 2400 on `~/Desktop/Projects`.

      APFS names the clone family inside the bulk record. `forkattr` gains
      `ATTR_CMNEXT_CLONEID | ATTR_CMNEXT_EXT_FLAGS` under
      `FSOPT_ATTR_CMN_EXTENDED`; the family is `(device, clone id)`, gated on
      `EF_MAY_SHARE_BLOCKS` because APFS hands *every* file a clone id and
      without the gate every file would join a family. Charging moved into
      `place()` beside the hardlink claim, both resolved once per directory
      so each process-wide lock is taken once rather than per entry.

      **The one hazard, and it is silent:** a name dropped as a repeat
      hardlink must not also consume its clone family's claim, or the
      family's blocks are charged to nobody and the total comes out short.
      `Ctx::claim` resolves the two in that order for exactly this reason.

      Results, interleaved random order, warm cache, median `[measured]`:

      | Corpus | before | after | dua-cli 2.45.0 |
      |---|---|---|---|
      | `/usr`, 50,189 | 94 ms | **78 ms** | 77 ms |
      | `/Applications`, 415,503 | 485 ms | **412 ms** | 448 ms |
      | `~/Desktop/Projects`, 1,064,452 | 2133 ms | **1431 ms** | 1566 ms |

      Peak RSS on the largest fell 299 → 200 MiB in the same change, because
      the walk stopped building a `PathBuf` per entry (see D4). The thread
      default moved 8 → 6 for the same underlying reason: less work per entry
      makes coordination a larger share, and 8 now costs 22% at 1M entries.

      Two consequences worth knowing about:

      - **The 64 KiB floor is gone.** It existed because each check cost an
        open; small clones are now deduplicated too, so `clones_deduped`
        rises and a tree of many small clones reports slightly less.
      - **`clones_probed` is 0 on an ordinary macOS scan.** The counter stays
        because it is on the agent's wire and because the slow listing path
        (a directory holding a mount point) still asks one file at a time,
        now with `getattrlist` rather than an `open` + `fcntl`.

- [x] **B1 Memory** — *(9 September 2026: measured, broken down, four done.
      The fifth and main one, **B1-K**, finished on 14 September 2026.)*

      **Distribution — before the fix** (`/Applications`, 412,232 entries,
      peak RSS 119 MB = **290 B/entry**), measured phase by phase with an
      RSS probe:

      | Item | MB | B/entry |
      |---|---|---|
      | baseline (binary + runtime) | 8 | — |
      | `RawEntry` intermediate tree (walk phase) | 36.3 | 88 |
      | name `String`s | ~13.2 | ~32 |
      | fragmentation, malloc headers, temporary `PathBuf`s | ~19 | ~46 |
      | arena (`Node` 104 B) | 42.9 | 104 |

      **Result (same day):** 298 → **231 B/entry** (117 → 91 MB), i.e.
      **22.5%**. No speed regression — in the alternating A/B measurement,
      at 8 threads the best went from 0.976s → 0.894s, i.e. it **sped up**
      slightly (one fewer malloc per entry). Note: a sequential measurement
      had first shown a 12% regression; that was drift caused by machine
      warm-up, alternating runs eliminated it.

      **Critical observation:** 77 MB when the walk finished, 119 MB when
      flatten finished. So the `RawEntry` tree and the arena **live at the
      same time**, and even when `RawEntry` is freed it isn't returned to
      the operating system. 192 of the 290 B/entry is this **double
      storage**.

      - [x] **Pre-allocate the arena's capacity.** The entry count is
            already known exactly by flatten time (`progress.files +
            dirs`). Before, it grew from 1024 by doubling up to 524,288.
            Measured gain 298 → 290 B (3%) — much smaller than expected,
            because the peak forms during the walk, not during flatten.
      - [x] **Shared name arena.** Names live in a single `String` inside
            `Tree`; the node holds `(offset: u32, len: u16)`. `u16`, not
            `u8`, because the root's name is a full path and can exceed
            255 bytes. The `Node.name` field is gone, replaced by
            `Tree::name(id)`; the only way to build a node is now
            `TreeAssembler` + `StoredNode`, so **a bad offset can't be
            constructed structurally**. The schema didn't change (SQLite
            still stores TEXT per row).
      - [x] **Field narrowing:** `nlink`, `files`, `dirs` `u64` → `u32`.
            `Node` 104 → **72 B** (measured). `mtime` stayed `i64` —
            pre-1970 files are real and carry a negative timestamp.
      - [x] **`RawEntry` 88 → 48 B.** Names in a single buffer per
            directory (`Children { names, entries }`), children
            `Option<Box<Children>>`. Since `to_string_lossy()` returns
            borrowed on valid UTF-8, the per-entry `String` allocation is
            gone entirely: **412k → 35k allocations.**
      - [x] **B1-K Remove double storage** — done *(14 September 2026)*,
            the research described in the note below was carried out and
            **the problem itself was found to be misframed**.

      **Target correction:** the **~25 B/file** target from RESEARCH.md is
      **not reachable** with our field set, and the comparison is apples
      to oranges. ncdu 2 doesn't keep `own_size`/`own_alloc`/`files`/
      `dirs` per node. Our baseline arithmetic: with the most aggressive
      narrowing, `Node` 72 B + name ~21 B = **~93 B/entry**, plus double
      storage. The realistic target is on the order of **dua-cli's 64 B
      arena node**, not 25.
      *Competitor:* dua-cli 64 B arena node + shared name store, RSS 49%
      lower; ncdu 2: 25 B per file, 56 B per directory.

      ### B1-K — double storage ✅ *(14 September 2026)*

      **Answer: there was no dilemma.** The four questions below were the
      research's starting point, and the first made the others
      unnecessary.

      *"Can double storage be removed while preserving invariant #2?"* —
      Yes, and invariant #2 turned out to be weaker than assumed. What the
      arena needs isn't BFS but two properties: children are contiguous,
      and each child's index is greater than its parent's. Every consumer
      in the repository was checked one by one and **none of them wants
      level order**: `aggregate` and `median_bands` are reverse passes
      that rely only on the second property, `Tree::check` checks exactly
      those two properties, `store` stores the layout as-is, `diff`
      matches children **by name**, the desktop ties ids to a generation.
      In the code, "BFS" only appeared in comments and docs.

      Since a directory's listing is already completed on a single
      thread, a contiguous block can be opened and written in the arena
      at that point (`TreeBuilder::push_block`). For the parent to be
      nameable it must already be in the arena, so the second property
      holds structurally. The intermediate tree is gone entirely.

      Level-synchronized BFS, risking the work-stealing DFS, a chunked
      arena — **none of them were needed** — all three were solutions to
      a constraint that had been misframed. dua-cli's approach was also
      read (`traverse.rs`): they don't keep an intermediate tree either,
      they append entries to the arena as they stream; their difference
      is a single consumer thread + a bounded channel. It didn't fit us,
      because the listing thread has to know the children's ids **before**
      recursion.

      **Measurement** (`examples/memprobe.rs`, `scripts/bench-walk.sh`;
      interleaved 9 runs, median, M3 Max):

      | | baseline | after |
      |---|---|---|
      | `/Applications` peak | 91.5 MB | **57.6 MB** (-37%) |
      | `/Applications` B/entry, hinted | 221 | **125** (-43%) |
      | `~/github` peak | 234.6 MB | **201.6 MB** (-14%) |
      | Linux 75k, peak | 15.0 MiB | **10.7 MiB** (-29%) |
      | Linux 75k, "non-tree" | 8.4 MiB | **4.1 MiB** (-51%) |

      None of the four distributions overlap. **Speed is the same in both
      corpora**: 2022 vs 1956 ms and 739 vs 750 ms, with the ranges nested
      in both — meaning what should be said is "the same," not the
      difference between the two medians.

      **The gain staying small in `~/github` is not B1-K's doing.** There,
      ~140 MiB of the peak isn't live data: even the baseline binary stays
      at 201 MiB after dropping the tree. Pages macOS libmalloc doesn't
      return, i.e. D4. On Linux the same code halves the "non-tree"
      portion, and that is the structural result.

      **`ScanOptions::expected_entries`** was added: since the arena now
      fills during the walk, its final size isn't known up front, and a
      doubling `Vec` holds two buffers at once on its last move (57 MB at
      412k; twice the arena's size at N = 2^k+1). Not a flag — its only
      honest source is a previous scan of the same root. The CLI and
      agent ask the store, the desktop already supplies the figure it
      keeps for the progress bar.

      **Observable change:** two scans no longer produce the same ids
      (they produce the same answers). `dupes`'s group representative is
      deterministic within a scan, and can shift between two scans. The
      scanner's own clone deduplication therefore sorts by
      `(depth, path)`. *(That stopped being true on macOS the day this
      landed: the in-walk clone dedupe charged whichever member a thread met
      first, and 40 scans of one fixture gave five answers. Restored on
      2 October 2026 by a settlement pass in `Phase::Finishing`, shared with
      A6.)*

      **Verification:** `/Applications`'s 412,983-row CSV export is
      byte-for-byte identical. On `/usr` the tree's shape is identical,
      only which hardlink name carries the bytes changes — invariant 3
      already declares this undefined, and **the old binary itself
      changes in 245 rows between its own two runs** (measured).

      **The target correction still stands:** ~25 B/file from
      RESEARCH.md isn't reachable with our field set (ncdu 2 doesn't keep
      `own_size`/`own_alloc`/`files`/`dirs` per node). Our baseline
      arithmetic is `Node` 72 B + name ~21 B = ~93 B/entry, and the
      hinted measurement is now **125 B/entry**, i.e. on the order of
      dua-cli.

- [x] **B2 Thread count tuning** — done *(10 September 2026)*.
      `--threads N`, `threads` per root in the agent, and the walk now
      runs in its own pool instead of rayon's global pool (a library
      can't own a process-wide setting). Default `min(cores, 8)`.
      **There's no such thing as a single best thread count:** the
      optimum shifts with the tree's size — 6 at 50k entries, 12 at 412k.
      The chosen count isn't the best at either, but it beats the old
      default at both (39% and 11%). Table and method in
      docs/COMPETITORS.md §1.2.
      *Competitor:* erdtree, empirically 3 threads; TreeSize tunes by CPU
      load.
      **Left open:** not measured above 412k. The synthetic 1.2M attempt
      was discarded because its shape came out malformed; a real large
      corpus is needed.
- [x] **B3 HDD / network drive mode** — done for spinning disks *(5
      October 2026)*. `--disk auto|ssd|hdd` (agent: `disk` per root).
      `hdd` walks with one thread and, on Linux, stats each directory's
      entries in inode order (`dents.rs`, a per-thread (inode, offset)
      scratch). `auto` reads `/sys/.../queue/rotational` for the root's
      device — through `/sys/fs/btrfs/<uuid>/devices` on btrfs, any
      spinning member counts — on an abandonable thread with a deadline
      (≤ 2 s, invariant 7); undetected means flash. `ScanStats.pace` and
      `--json` say how it was decided (`asked`/`detected`/`undetected`).
      Simulated spinning disk (Docker, cold): 1 thread in listing order
      11.5 s, in inode order 0.9 s; 2 and 6 threads 12.6–13.6 s either way;
      every setting issued the same reads. No SSD regression. Network
      filesystems are **not** paced like HDDs: virtiofs went 2.9 s at 1
      thread → 1.5 s at 6.
      - [ ] Pace and inode order per volume: an HDD mounted below an SSD
            root walks at the root's pace, and an SSD below an HDD root in
            inode order with one thread.
      - [ ] Verify on a real HDD and on btrfs over a spinning disk.
      *Competitor:* gdu `--sequential`; QDirStat sorts entries by inode
      before stat'ing them.
- [~] **B4 Windows MFT fast path** — written *(5 October 2026)*, **awaiting
      Windows CI**; no changelog entry until it passes. Raw MFT, not USN (no
      sizes). The parser in `scan-core/src/ntfs/` reads any `Read + Seek`
      and is tested entry for entry against three ntfs-3g images
      (`scripts/ntfs-fixtures.sh`, committed as `tests/fixtures/ntfs.tar.gz`)
      and a 200k-file image: fixups (torn records re-read once), extension
      records via base reference + sequence check (covers
      `$ATTRIBUTE_LIST`), sizes only from the VCN-0 instance, DOS names
      dropped, records < 16 hidden, a reparse tag read through the full run
      list (one unreadable tag is one bad record, kept as a link, never a
      failed table). 202,104 records parse in 19–20 ms and build in 5–6 ms
      from page cache (M3 Max); disk speed unmeasured. Built through
      `place()`, so dedupe, exclusion, depth and counters are the walk's;
      records read move `rows_done`. Whole-volume roots only (record 5), as
      administrator; everything else — no elevation, ReFS, FAT, network,
      a geometry or serial mismatch, more than 1% bad records — walks.
      `--no-mft` / `ScanOptions::read_mft`.
      **Open:** the `volume.rs` differential test (the runner's C:, walk vs
      table, field for field; CI sets `SPACETRACE_REQUIRE_MFT`) has to
      pass — it settles directory `AllocationSize`, the 8-byte rounding of
      in-record data and WOF. Then: speed on a real disk; subfolder roots
      (the subtree code works, the cost is reading the whole table);
      a fallback reason the user can see (today a fallback looks like the
      table except in time taken); read-ahead (read and parse do not
      overlap); the cost of the `FlushFileBuffers` before each read; name
      text is indexed with `u32`, so past 4 GiB of names the read fails
      late and falls back. The non-elevated tier (point 5 below) is
      untouched.
      *Original note:* needs administrator, absent on ReFS and
      network/FAT → falling back to the normal path is mandatory, and that
      path gets fixed in A1/A2. **The order is therefore after A.**
      *Competitor:* WizTree (raw MFT), TreeSize Free (administrator),
      WinDirStat 2.5.0.

      **Entry note** *(17 September 2026, written before the Windows
      session, from a macOS checkout — nothing here is measured.)*
      Settle these in order; the first answer sets the size of the job.

      1. ~~**Does the enumeration carry sizes?**~~ **Answered: no.**
         *(21 September 2026, from the Win32 ABI — this one did not need
         the volume, because a structure layout is not
         machine-dependent.)* `USN_RECORD_V2` and `USN_RECORD_V3` carry,
         in full: `RecordLength`, `MajorVersion`, `MinorVersion`,
         `FileReferenceNumber`, `ParentFileReferenceNumber`, `Usn`,
         `TimeStamp`, `Reason`, `SourceInfo`, `SecurityId`,
         `FileAttributes`, `FileNameLength`, `FileNameOffset`,
         `FileName`. No size, no allocated size, no end-of-file offset.
         The enumeration reconstructs the *tree* and says nothing about
         *bytes*.

         **So B4 is the larger job, and it is not the job this item's
         title describes.** Three routes to the bytes, and two are dead:
         an open handle per entry is what we already do (`FileIdentity`,
         measured ~+36%, the cost B4 exists to remove), and
         `OpenFileById` + `GetFileInformationByHandleEx` is the same
         handle under a different name. The one route that avoids the
         per-entry handle is parsing the raw MFT: `FSCTL_GET_NTFS_VOLUME_DATA`
         for the record size and the `$MFT` location, then
         `$STANDARD_INFORMATION` and `$DATA` per record. That is what
         WizTree does. Names and parents live in `$FILE_NAME` in the same
         records, so **once the MFT is parsed, `FSCTL_ENUM_USN_DATA` is
         redundant for B4** — it stays relevant only to B7.

         What still wants the real volume is the parser, not this
         question: fixups (an unapplied update sequence array is silently
         wrong every 512 bytes), resident versus non-resident `$DATA`,
         `$ATTRIBUTE_LIST` for fragmented records, and the DOS/Win32
         namespace flags in `$FILE_NAME` (ignore them and every file
         appears twice). None of that is a type error, so `cargo check`
         against the msvc target cannot catch any of it — which is why
         `mftprobe.rs` is written **on** the Windows machine, first thing
         that trip, not before it. B5's own note below records what
         writing this kind of code blind costs.
      2. **It does not plug in where B5 does** — no longer a guess, it
         follows from the answer above. The macOS fast
         path enters at `bulk_list(dir) -> Option<Vec<NamedMeta>>` in
         `crates/scan-core/src/scan.rs`: one call per directory,
         returning `None` to fall back. The MFT is read **per volume**,
         not per directory — the table is enumerated once and the tree
         is joined on parent references. So B4 is an alternative
         *source* for a whole root, not a faster listing inside the
         existing walk, and it wants its own entry point above the walk.
         Forcing it into `bulk_list` is the wrong shape.
      3. **The differential test is not optional.** B5's real risk was a
         second metadata path silently diverging, not speed — the same
         thing `store::digest` warns about in its own header. Whatever
         B4 produces must be compared field for field against the normal
         walk, the Windows counterpart of `assert_same_answer_as_lstat`:
         hardlinks, reparse points, junctions, a sparse file, a
         compressed file, and a directory larger than one batch.
      4. **The fallback matrix is part of the feature, not a caveat.**
         No administrator, ReFS, FAT/exFAT, network drive → the normal
         path, with nothing conditional leaking out to the caller.
         Windows metadata already costs an open handle per entry
         (`FileIdentity` in `crates/scan-core/src/meta.rs` exists for
         exactly that reason), and elevation is not the common case, so
         the normal path stays the one most users hit. B4 does not
         excuse leaving it slow.

      5. **The non-elevated tier, and the one thing that decides its
         shape** *(22 September 2026, read off the Win32 ABI in
         `windows-sys` 0.61 — a structure layout is not machine-dependent,
         so this much did not need the volume.)*

         `GetFileInformationByHandleEx(FileIdBothDirectoryInfo)` answers a
         whole directory from the **directory's** handle: per entry it
         carries `EndOfFile`, `AllocationSize`, `FileAttributes` and
         `FileId`. No administrator, works on ReFS, FAT and network
         drives. That is `alloc` and the file identity without opening
         anything per entry, which is what the +36% is spent on.

         **What it does not carry is the link count**, and neither does
         `FILE_ID_EXTD_DIR_INFO`, the newer class. `NumberOfLinks` exists
         in exactly two places, `FILE_STANDARD_INFO` and
         `BY_HANDLE_FILE_INFORMATION`, and both want an open handle. So
         there is no Windows directory enumeration that answers everything
         the Unix `stat` in the walk already answers, and the plan's
         "drop `nlink` in favour of the `(volume, FileId)` set" is a
         **trade, not a simplification**. What it costs, precisely:

         - *Deduplication stays correct.* `(volume serial, FileId)` is
           exactly what a hardlink shares, and the volume serial is one
           call per directory, not per entry. `Ctx::claim` already keys on
           a pair, so nothing in the accounting changes.
         - *Three consumers read the value itself.* `store::ncdu` writes
           `"nlink":N,"hlnkc":true` for files above 1 — the interop
           contract ncdu and gdu read; `dupes` gates on `nlink <= 1`; and
           `store::digest` hashes it, so **a Tier 1 scan and an ordinary
           scan of the same tree would carry different content hashes**.
           That last one is what stops this from being a free win: the
           whole point of `assert_same_answer_as_lstat` on macOS is that
           a second metadata path must not disagree with the first.
         - *A tree-scoped count is not the same count.* "Seen twice under
           this root" misses a file whose other name is outside it, which
           the real `nlink` does not.

         **Two smaller wins that are not trades and should land first.**

         - `query()` calls `GetFileInformationByHandleEx(FileStandardInfo)`
           for `AllocationSize` and then `GetFileInformationByHandle` for
           the link count — but `FILE_STANDARD_INFO` **already carries
           `NumberOfLinks`**. The second call adds only the file index and
           the volume serial, both of which the directory listing above
           supplies. So the listing plus one `FileStandardInfo` per file
           replaces three per-entry calls with one, with no semantic
           change at all.
         - With `--no-dedupe`, or for a directory, nothing outside the
           listing is wanted and the handle disappears entirely.

         **And one route worth a look on the machine:**
         `GetFileInformationByName` (Windows 11 22H2+) answers
         `FILE_STAT_BASIC_INFORMATION` **by path, with no handle** — and
         that structure has `AllocationSize`, `FileId` *and*
         `NumberOfLinks` together. It would be the exact counterpart of
         the `getattrlist` call that replaced the `fcntl` probe on macOS
         in B8. `windows-sys` 0.61 does not expose it, so it needs a
         hand-written `extern "system"` and a `GetProcAddress` fallback
         for older Windows — neither of which is worth writing blind.

         **None of this is written.** It is type-checkable here and
         nothing else, and the repository's own rule applies: no claim
         that anything works on Windows before CI says so. The
         differential test in point 3 is what settles the `nlink`
         question, because it will fail loudly on exactly that field.

      Same trip, while the machine is there: **B7** is the same API
      family and belongs directly after this, and the desktop's treemap
      performance on the Windows WebView is still unverified (Phase 3) —
      cheap, and only doable here.
- [x] **B5 macOS `getattrlistbulk` fast path** — done *(11 September
      2026)*. No separate crate was needed: `libc` already exposes
      `getattrlistbulk`. Instead of `readdir` per directory +
      **`lstat` per entry**, both names and metadata in a single call.

      **Measurement, interleaved with medians, on two corpora:**

      | Tree | Old | New | Gain |
      |------|------|------|--------|
      | `~/github` (297,695 entries) | 1293 ms | 556 ms | **2.33×** |
      | `/Applications` (412k entries) | 1554 ms | 646 ms | **2.41×** |

      The distributions don't overlap at all (`~/github`: new max 598,
      old min 1225), meaning this isn't a number this machine's noise
      could produce. When the listing layer alone is measured
      single-threaded, 3.3×; end-to-end in the parallel walk, 2.3–2.4×,
      because tree building and the clone probe stay the same. Across
      three roots (`~/github`, `/Applications`, `/usr`) the two binaries'
      output is byte-for-byte identical.

      **The real risk was a second code path**, not speed: two metadata
      sources silently diverging is the error `store::digest` warns about
      in its own header. So the fast path produces the same `RawMeta`,
      and `assert_same_answer_as_lstat` compares the two paths field by
      field — including symlinks, broken symlinks, a symlink to a
      directory, hardlinks, and a directory that doesn't fit in a single
      batch.

      **Directory `nlink` had to be corrected.** `ATTR_DIR_LINKCOUNT` on
      APFS gives the real hardlink count (1), while `st_nlink` gives the
      2+subdirectories every Unix tool shows. Reconciled with one `lstat`
      per directory; **no measured cost** (3.31× vs 3.29×), because the
      kernel has just read that inode.

      **Not used in a directory containing a mount.** The whole directory
      comes back in one call, so there's no longer a per-entry moment
      where a non-responding file system could be given time — D1's
      protection wants the old path, and the walk hands it those
      directories.

      **The note that "none of the competitors do this on macOS" was
      wrong.** The table in COMPETITORS.md §2 already said DiskRaptor
      uses `getattrlistbulk`, so this item is catching up, not getting
      ahead. `dumac`'s 6.39× figure against `du` also isn't comparable to
      our 2.3×: their baseline is single-threaded `du`, ours is already
      our own scanner walking in parallel with 8 threads.

      **In the first attempt I had `attribute_set_t` shifted by one
      slot** (confusing it with `attrlist`; that one has a header, this
      one doesn't), and every entry looked wrong. My comparison function
      also passed silently, because `zip` of 9 against 0 iterates zero
      times — the count-equality assertion was added afterward.
- [x] **B6 Linux `getdents64` fast path** — done *(5 October 2026)*.
      `dents.rs`: one per-thread `getdents64` buffer, `fstatat` relative to
      the dirfd, `openat` for FIEMAP, no allocation per entry; `read_dir` +
      `lstat` stays the path for a directory holding a mount point; an open
      failure is reported once, in the ordinary path's words. `statx` was built, measured and
      dropped: equal to `fstatat` on glibc and musl (177 vs 168 ms, 651 vs
      654 ms at 1M entries). Malformed records are an EIO on the
      directory, `d_ino == 0` skipped, EINTR retried. Docker linux/aarch64,
      interleaved medians: glibc 6–14% (1M entries 188 → 161 ms at 6
      threads); the static musl agent build 1.3–3.4× (1M 2061 → 682 ms,
      real tree 307 → 122 ms, cold 420 → 323 ms). Exports identical to main
      at 1 and 6 threads; `assert_same_answer_as_lstat` in `dents.rs`
      holds it field by field.
      - [ ] The musl build is still ~3× slower than glibc at 6 threads —
            most likely musl's allocator under threads.
      *Competitor:* `dut`, in warm cache, 6.87× over `du`, 2.8–3.75× over
      dust/dua/gdu.
- [~] **B7 Incremental rescan** — macOS done *(5 October 2026)*; USN Journal (Windows) and Linux open. For an agent
      that scans nightly, scanning everything every time is wasteful.
      Strategically the biggest speed gain. *Competitor:* **SpaceObServer**
      — our closest architectural competitor, and ahead right at this
      point.

      **B7 does not depend on B4** *(21 September 2026)*. B4's blocker is
      that the USN records carry no sizes (see its note above), and B7
      does not need them to: the journal says *which* files changed, and
      the size of those few comes from the ordinary per-entry path. The
      handle cost that makes B4 expensive is proportional to the changed
      set here, not to the volume. So B7 wants the journal only for what
      the journal actually provides, and needs no MFT parser.

      "Directly after B4" in the order below is therefore **logistics,
      not a dependency** — the same machine, the same trip, the same API
      family. B7 can be taken first, and on this reading it is the
      cheaper of the two.

      What it does need, and neither exists yet: a **journal cursor
      persisted per scan** (journal ID plus next USN — a `scans` column,
      so invariant "new column at the end of both `create_tables` and
      `migrate_from`" applies), and a way to **build the new tree from
      the previous snapshot plus a delta**. The second one is the real
      design question, because arena ids are not stable across scans
      (invariant 2) — the delta cannot patch an arena in place, it has to
      feed `TreeBuilder`. A stale or rolled-over journal ID must fall
      back to a full scan, silently and always.

      **macOS comes first, and its half is now measured** *(22 September
      2026)*. The same feature wants three journals — USN on Windows,
      FSEvents on macOS, fanotify on Linux — and FSEvents is the one that
      can be tried on the machine this is written on. `examples/fsprobe.rs`
      is that experiment; it is kept because the numbers below are the
      whole case for the feature.

      **FSEvents answers the question, including the case a directory
      mtime cannot.** Replaying from a stored event id returns the paths
      that changed under a root, and a file *grown in place* comes back
      with `ItemModified` — which is what rules out the obvious cheap
      alternative of comparing directory mtimes, since writing into an
      existing file changes no directory's mtime and would be missed.

      **The cost is proportional to how far back the cursor is, not to
      how much changed** `[measured]`, replaying under `~/Desktop/Projects`:

      | Event ids back | Replay | Events returned |
      |---|---|---|
      | 1,000 | 7.7 ms | 0 |
      | 10,000 | 10.2 ms | 5 |
      | 100,000 | 29.9 ms | 633 |
      | 1,000,000 | 835 ms | 24,616 |
      | all history (id 1) | 19.4 s | 6 |

      Against a full scan of the same tree at **1431 ms**, a rescan an
      hour or a day later costs tens of milliseconds — a 20–50× win, and
      the strategic case for the feature holds up. The last row is the
      shape of the failure: an id old enough that the whole `/.fseventsd`
      log is read takes far longer than rescanning, and it reports the
      condition **by not finishing** rather than by a flag. So the
      fallback rule writes itself: **spend at most as long asking as the
      last full scan took** — `scans.duration_ms` is already stored — and
      walk everything when the deadline passes. Every other loss mode
      (`MustScanSubDirs`, `UserDropped`, `KernelDropped`,
      `EventIdsWrapped`, `RootChanged`, `Mount`, `Unmount`) is a flag on
      an event and lands in the same branch.

      **The blocker is not the journal. It is that the arena stores no
      identity.** To reuse an unchanged subtree the new scan has to splice
      the old nodes in — which is easy, a breadth-first pass through
      `TreeBuilder::push_block` keeps invariant 2 by construction — but it
      must also restore the *accounting* state, and it cannot. `Ctx::claim`
      keys hardlinks on `(dev, ino)` and clone families on
      `(dev, clone id)`; `Node` carries neither, and never has. The
      failure is silent and specific: a hardlink or clone family that
      **straddles the boundary**, one name inside a spliced subtree and
      one inside a re-walked one, is charged twice — the spliced copy
      still carries the bytes it was charged last time, and the re-walked
      copy claims an identity this run has not seen.

      Three ways out, costed:

      1. **Store the identity per node.** Principled, and expensive: `ino`
         is 8 bytes on a 72-byte `Node` and a column on every row of every
         snapshot, and it changes `content_hash`.
      2. **One bit per node: "something below me has a shared identity".**
         Computed in the reverse pass `TreeBuilder::aggregate` already
         makes, and splicing is allowed only where the bit is clear. Sound,
         and **free in memory** — `Node` is exactly 72 bytes with five
         bytes of padding after `kind`, so a `u8` flag fits in space that
         is already being paid for. This is the one to build.
      3. **Refuse incremental when the previous scan deduplicated
         anything.** Sound and trivial, but on a developer's Mac
         `clones_deduped` is never zero, so it would switch the feature off
         exactly where it is worth most. `clones_deduped` is not even in
         the `scans` table today; only `hardlinks_deduped` is.

      So the build order is: the flag from (2), then the `scans` cursor
      column, then the splice, then the safety test — **a rebuilt tree and
      a full scan of the same filesystem state must have the same
      `content_hash`**, which is the only assertion strong enough to be
      worth having here.

      **macOS done** *(5 October 2026)*, built in that order. `scan --save`
      and scheduled agent scans start from the root's last snapshot, list
      only the directories on the path to what FSEvents reported, and copy
      every other subtree through `push_block`. Option (2) as planned:
      `SHARED`, `ERRORS`, `MOUNT`, `MOUNT_POINT` flags in `Node`'s padding.
      FSEvents does report the source side of a straddling family — the
      folder of a new hardlink's original, the file of a new clone — so
      the copied side is reread; a test on a fresh APFS image holds it.
      **No `scans` column after all**: the cursor, the sparse flags (5 B
      per flagged directory, 104 KB on 1.4M entries) and a digest binding
      both to `content_hash` live in a `rescan_state` side table created
      inside the save transaction. Schema stays v3, because a v4 database
      is refused by every older desktop, and exports carry none of it.
      Fallbacks, each recorded on the snapshot: no cursor, another volume,
      journal past its budget (max(last full walk, 500 ms)), a loss flag,
      more than 100,000 changes, a cursor older than two days (no API
      dates the retained history: `FSEventsGetLastEventIdForDeviceBeforeTime`
      returned 0 for every time asked), other options, a damaged or
      imported base. The save now hashes rows as it writes them
      (/Applications save 588 → 487 ms), and the base is checked in the
      pass that loads it.
      *Measured* (M3 Max, 5 interleaved pairs, medians, full vs
      incremental): /Applications 459 vs 241 ms; ~/Library 2067 vs 1981 ms;
      ~/Desktop/Projects 5357 vs 3174 ms. Not the 20–50× above: base load
      (216 / 417 / 923 ms) and rereads dominate — replay itself is ~10 ms.
      - [ ] USN (Windows) and fanotify (Linux) as further `Journal`
            implementations; a Linux journal must flag `files_unmapped`
            directories first.
      - [ ] ~/Library rereads ~1,250 privacy-protected (TCC) folders every
            time, ~1.2 ms each, so it gains almost nothing.
      - [ ] Trees full of hardlinks reuse little (Projects reuses 33%).
      - [ ] An APFS image attached `-nobrowse` gets no FSEvents history, so
            it never scans incrementally.
      - [ ] Only saved scans continue the chain; pushed snapshots carry no
            state. Base load is now the largest cost.

### C. Feature gaps

- [x] **C1 Email alert (hub)** — done *(11 September 2026)*. A rule's
      target is either an http(s) URL or an email address; **a single
      column**, not two side-by-side nullable fields, because "exactly one
      target" isn't a rule that needs remembering but the shape of the data.
      The `mailto:` scheme was chosen because every webhook row in v1 was
      already a valid value — not a migration transform but a rename (hub
      schema v1 → v2, its test in `db.rs`, against a database actually set up
      the way v1 wrote it).
      **Credentials aren't in the database**, they're in the config file
      (via `password_file`, the same pattern as `admin_token`): the database
      is the file that gets backed up and attached to bug reports.
      **The `lettre` dependency is deliberate** — we hand-wrote the cron
      parser, but SMTP isn't that kind of list: TLS isn't in std, and the
      part that actually bites is a hostname coming off the network landing
      in a header. A bare CRLF ends a header block and the rest gets read as
      a header. rustls-only was chosen, no openssl/native-tls in `cargo
      tree` (mandatory for the musl static build).
      *Competitor:* SpaceObServer.
      - [ ] **Real delivery testing is on you.** What's verified here: the
            routes (28 integration tests, on a real socket), the password
            not leaking to the page, rejection of a CRLF-laced recipient,
            the v1→v2 migration. **Not verified: real mail to a real
            relay.** Settings → "Send a test message" exists exactly for
            this and follows the same path an alert takes. To check:
            587 with STARTTLS, 465 with implicit TLS, and how readable the
            error is for a wrong password.
- [x] **C2 Per-person accounts (hub)** — done *(11 September 2026)*. A
      token per person, two roles: `viewer` sees everything and can change
      nothing, `admin` changes everything, including who has access.
      **A token, not a password, and this is a boundary, not a shortcut**:
      a generated 256-bit token can be stored as plain SHA-256 because
      there's nothing to brute-force; a human-chosen password couldn't be
      stored that way and would need a slow KDF plus everything built
      around it. The login form already took a token, so this became
      adding a name and a role to that mechanism.
      **Authorization is the router's shape, not a check in the handler.**
      Write routes sit in their own group behind the `require_write` layer;
      a check inside a handler can be forgotten, and forgetting it means a
      read-only account can delete every alert rule in the fleet. There are
      two guards and both were proven to have teeth: moving a write route
      into the read group fails
      `a_viewer_can_read_everything_and_change_nothing`, and a new POST
      route not added to the list fails
      `every_write_route_is_in_this_list` (that test reads `web.rs`,
      because axum doesn't expose its routes).
      Alert rules carry who added them (`created_by`, NULL on v2 rows —
      backfilling it would mean writing someone's name over work they
      didn't do). The last admin can't remove themselves: the fallback
      path is a file on the server, and that's exactly the spot where
      someone locking themselves out from the web UI doesn't exist.
      *Competitor:* SpaceObServer (Client/Web Access).
- [x] **C3 Timeline view (desktop)** — done *(11 September 2026)*.
      A target's `(host, root)` history on a single line, the change
      between points shown alongside it, and one click from every step
      straight to that jump's diff.
      **The axis starts at zero** — an axis cropped to its own data turns
      a 2% wobble into a cliff, and this view exists precisely to answer
      "is it growing?".
      **Grouping is in Rust** (`src-tauri/src/history.rs`), because the
      desktop has no JS test runner; anything that carries a rule shouldn't
      sit on the side that can't be tested (treemap layout is in Rust for
      the same reason). The root path is normalized: `/data` versus
      `/data/` would split a target's history in two.
      **Scope boundary:** the desktop shows *what happened*, the hub
      *predicts*. The hub's `trend.rs` predicts with least squares plus an
      `r2` threshold and rejects on a bad fit; putting a second copy of it
      on the desktop would mean two threshold sets drifting apart from
      each other. If it's ever moved, `trend` should go into the core
      repository first — separate work.
- [x] **C4 Duplicate finder** — done *(11 September 2026)*. `spacetrace
      dupes`, a new `crates/dupes`. Three tiers, each paying only for what
      the previous one couldn't rule out: size (free, the scan already
      knows it) → first 16 KiB → full file. **Measured:** `~/github`
      (375,585 files, 33.8 GiB) → the answer cost 2.6 GiB of reading
      (7.7%); the second run read 30 MiB and re-hashed nothing.
      **BLAKE3, and no byte comparison afterward** — at a 256-bit output
      the odds of a collision are far below the disk returning the wrong
      bytes, and a verification pass would double the reads for a
      non-binding risk. This is a claim about a *cryptographic* hash and
      couldn't be defended with a 64-bit one — that's why it isn't xxh3.
      **Hardlinks aren't duplicates**: they already share their bytes, and
      counting them as recoverable would promise space that won't appear
      on deletion. They're listed in a separate group with zero savings.
      Watch out: the scan writes the hardlink's second name as 0 bytes
      (invariant 3), so the size tier looks at the disk for anything with
      `nlink > 1`.
      **The cache snapshot isn't in the database**, it's in a separate
      file next to it (`<db>.hashes`): the snapshot travels to another
      machine and local inode numbers would look valid there while
      belonging to a different disk — also, adding a table would trigger
      `SCHEMA_VERSION` and, with it, RELEASING.md's version ordering, for
      a cache that can be deleted at any time.
      **Nothing gets deleted**, same reasoning as the agent: which copy
      stays is a decision that needs context this crate doesn't have.
      *Competitor:* DiskRaptor (xxh3), WinDirStat 2.5.0, Czkawka.
- [x] **C5 Tree that grows live during a scan (desktop)** — done
      *(11 September 2026)*, **and replaced *(19 September 2026)***.
      `scan-core/src/live.rs` and `LiveMap.tsx` are both deleted; a
      running scan now hands back an ordinary `Tree` through
      `ScanProgress::partial` and the window draws it with the same
      `Treemap` it draws a finished scan with.
      **What the replacement cost the argument below:** the single-level
      decision was sound for a structure built alongside the arena, and
      wrong once the arena itself could be read. `PartialTree` publishes
      no second structure at all — it takes a consistent view of the
      nodes the walk has already written — so "a structure that grows on
      disk for squares that can never be seen" stopped being the choice
      on offer. Everything else here still holds and is kept because it
      is the reasoning, not the code.
      The original entry follows.
      **A single level, and that's a decision, not a rough draft**: a
      treemap doesn't draw a floor under minimum area, so the only part
      still readable while a scan is flying is the top level, and the
      question asked of a running scan is always "which of these is big".
      Carrying every level would mean a structure that grows on disk for
      squares that can never be seen, and that locks from every worker.
      **Bytes are added per directory, not per file** — the hottest loop
      already paid, per entry, to avoid putting a shared write there.
      **Cost was measured:** 4 ns per publish (the first version was
      203 ns with `RwLock`; "written once, then read" is already what
      `OnceLock` means). 0.13 ms total for `/Applications` with 34,000
      directories. An A/B of the whole scan can't resolve this
      (-3.45% / -0.45% — noise floor), so the correct form is
      per-operation measurement times operation count.
      **The same `squarify` routine** draws both the live preview and the
      finished map (made public in the treemap crate), so the picture
      doesn't rearrange itself once the scan finishes — the handoff is a
      data change, not an algorithm change. *(The replacement goes
      further: it is the same `layout` call on the same kind of tree, so
      there is no handoff left to get wrong.)*
      **Only while the window is idle**: a rescan doesn't touch a map
      that's currently being read, because this app's rule is "work never
      takes the window out of your hands".
      The accuracy claim is in a separate test: live totals match the
      finished tree's totals exactly (two answers arriving by two
      completely different paths).
      - [ ] **Visual verification is on you.** I can't take screenshots.
            To check: that the squares really do grow and move, that
            labels don't overflow in a narrow square, and that the
            picture doesn't jump when the scan finishes and the real map
            arrives. The logic side is tested (layout, container,
            measurement, category, area bounds).
- [x] **C6 Sunburst view (desktop)** — done *(11 September 2026)*.
      `treemap/src/sunburst.rs` + `Sunburst.tsx` on the desktop. One level
      per ring, an arc's angle is **its share of its parent** (not of the
      root) — that's why the rings line up underneath each other.
      **Angle is size, not area.** An arc's area grows with the radius, so
      a small folder sitting further out covers far more ink than a
      bigger one sitting further in. That's the view's limitation, not the
      app's — it's also why the treemap stays the default, and the
      interface says so in its hint text.
      The hole in the middle isn't only for the label: the innermost
      ring's arcs would otherwise meet at a point, and the level that gets
      read the most would be the hardest to aim at.
      Three behaviors were verified by mutation, and all three were
      caught: root share instead of parent share, the hit-test's angle
      rule, and pruning of arcs that are too thin.
      *Competitor:* FreeSize (treemap + sunburst + heatmap), Filelight.
      - [ ] **Visual verification is on you.** To check: that the rings
            line up, that labels don't overflow on a narrow arc, that the
            cursor selects what it's pointing at (especially at the
            12 o'clock seam), and that the circle isn't an ellipse in a
            narrow/wide window.
- [x] **C7 File age** — done *(11 September 2026)*. `spacetrace age`
      (`--bands`, `--json`) and, on the desktop, a map colored by age.
      Computed in the core (`scan-core/src/age.rs`), the same pattern as
      C3: no JS testing on the desktop, so anything carrying a rule stays
      on the side that can be tested.
      **Weighted by bytes, not by file count** — a hundred thousand old
      source files isn't the answer, a disk image is. **Directories don't
      enter the profile:** a directory's `mtime` changes the moment
      something is added next to it, and has nothing to do with the age
      of what's inside it. **A separate band when there's no `mtime`** —
      a snapshot coming from ncdu doesn't carry it, and reading 1970 would
      mean "untouched for fifty years".
      **Directories are colored on the map too**, and their band is the
      subtree's *median byte* (`median_bands`) — not their own `mtime`.
      This is what separates a heat map from a recolored category map: at
      every depth worth looking at, most of the area is folders, and the
      unit you can act on is a folder. Median was chosen over "which band
      has the most bytes" because the latter paints a folder split 51/49
      a single color, and the color flips on a single file.
      *Competitor:* FreeSize (heatmap).
      - [ ] **Visual verification is on you.** No screenshot permission
            on this machine, so the ramp's readability, whether the
            legend overflows in a narrow window, and whether the folder
            tint (55% alpha) buries its children **haven't been tried**.
            The computation side is verified: on `/Applications`'s
            300 largest directories, `median_bands` was checked against
            the subtree profile's median with two independent walks, zero
            mismatches.
- [x] **C8 ncdu/gdu JSON import** — done *(11 September 2026)*.
      `spacetrace import <file>`; `--root`, `--host`, `--label`. No new
      dependency, `serde_json` was already in `store`.

      **Aggregation wasn't written a second time.** The importer feeds
      `Tree::from_nested`, which calls the same `TreeBuilder` +
      `aggregate` the walker uses. Arena invariants and aggregation stay
      in one implementation.

      **Mutation testing turned up two real bugs.** First: `from_nested`
      wasn't filling `children_start`/`children_len` (it doesn't `push`,
      the scanner does that in `flatten`) — the tree's totals were
      correct, but every `children()` call came back empty. The layout
      test was therefore **a hollow claim**: with `children_len = 0`,
      neither loop ever ran. The fixture had no sibling *after* a
      subdirectory, so even the DFS mutation still passed; fixing the
      fixture surfaced the bug.

      Second, and more serious: **real ncdu also writes `asize` on
      directories**, and I was adding it into the logical aggregate —
      **a violation of invariant 1**, exactly the point where GNU
      `du --apparent-size` parts ways with us. Our own exporter writes
      `asize: 0` on directories, so the round-trip test could never have
      caught this; a realistic ncdu fixture written from the spec caught
      it.

      **The one thing not verified:** real `ncdu` output. ncdu isn't
      installed on this machine (installing it would need permission),
      the fixture was written from the spec.
- [x] **C9 CSV export** — done *(11 September 2026)*.
      `spacetrace export --format csv`, `--depth` to stop at the upper
      tiers.
      **Both measures are columns, not a setting** — since there's no
      ordering in the file and no label next to it, invariant 6's
      rationale doesn't apply here; let the reader choose.
      Alongside subtree totals there are `own_*` columns, otherwise
      summing every row would count a file again for every folder above
      it.
      RFC 4180 escaping was hand-written and tested against a CSV
      *reader*: a golden-string comparison would also have let through a
      bug that produces consistently wrong output.

### D. Robustness

- [x] **D1 Timeout and continue on a slow/unresponsive mount** — done
      *(visibility 10, timeout 11 September 2026)*.
      *Competitor:* DiskRaptor lived this bug in production (issue #46: "no
      progress for 30 seconds on a 1TB USB HDD") and added timeout/retry.

      **Problem.** `read_dir_parallel` calls `entry.metadata()` for every
      entry, i.e. an `lstat`, and this happens **before** the `dev`
      comparison. On a dead mount that syscall never returns and can't be
      interrupted portably. There were three consequences: `--one-file-system`
      didn't protect (the check uses the data from the `lstat` that causes the
      hang), one entry locked up the **entire** directory it was in (directory
      reading is deliberately single-threaded), and cancellation didn't help
      (`is_cancelled()` only runs before `read_dir`).

      **Solution.** Know the boundary before touching it: `mounts.rs` reads
      the mount table at the start of the scan (macOS `getmntinfo(MNT_NOWAIT)`
      — **not `MNT_WAIT`**, which also blocks on a dead mount; Linux
      `/proc/self/mountinfo`, pure std; others empty set = old behavior). The
      mount point is approached through a thread that's accepted as
      disposable (`timeout.rs`); if it doesn't answer it counts as an
      **unreadable path** (invariant 7) and the walk continues with its
      siblings. A partial tree is never returned (invariant 5).

      **`libc` was already there.** What I wrote as this item's blocker —
      "`libc` needs to be added to `scan-core` first" — was wrong:
      `capacity.rs` and `meta.rs` already use it.

      **Default 60 s, deliberately generous.** The two errors don't cost the
      same: waiting too long slows the scan, giving up too early silently
      drops **an entire volume** from a total that claims to be complete.
      `--mount-timeout 0` restores the old behavior; in the agent,
      `mount_timeout` per root.

      **Cost: one hash lookup per directory, ~95 ns.** For `~/github`
      (297,695 entries, 10,856 directories) that's **1.03 ms**, i.e. ~0.09%.
      Per directory, not per entry, because `Mounts` also holds the *parents*
      of mount points: a directory with no mount under it never gets any of
      its entries looked up. **A whole-scan A/B can't do this job** — it
      showed +9.19%, i.e. 100 times the real cost; 1 ms doesn't show up
      inside ±150 ms of run-to-run noise. The number comes from the
      micro-benchmark and the directory count.

      **The one thing that can't be verified:** that a real kernel-level hang
      goes through this path. Because the probe is injectable, the whole
      mechanism is tested (it's skipped, siblings are scanned, the total
      doesn't inflate, a healthy mount scans normally, the probe is never
      called when disabled) — but producing an actually-hung mount needs a
      second machine.

- [x] **D2 Built-in TLS in the agent** — done *(21 September 2026)*.
      `crates/agent/src/tls.rs`. `server.tls_cert_file` plus
      `server.tls_key_file`, both or neither; half a pair is refused at config
      load, because the alternative is serving plaintext on a port its
      operator has decided is HTTPS. A reverse proxy stays the recommendation
      wherever a domain name exists — Caddy renews certificates and the agent
      does not.

      **No new dependency, and that decided the shape.** rustls and
      tokio-rustls already arrive through `reqwest`, hyper and hyper-util
      through `axum`; declaring them added nothing. `cargo tree -p
      spacetrace-agent --edges normal` is byte-identical before and after
      (195 entries). `rcgen` is a dev-dependency only, on the `aws_lc_rs`
      feature rather than its default `ring`, because aws-lc-rs is already
      compiled through rustls and ring is not.

      **Handshakes had to come off the accept path.**
      `axum::serve::Listener::accept` is awaited one connection at a time, so
      doing the handshake inside it serialises every new connection behind the
      slowest: a peer that opens a socket and never sends a ClientHello would
      block all other clients for the whole handshake timeout. `TlsListener`
      therefore runs handshakes in their own tasks and queues only finished
      streams over an mpsc of depth 64 — the queue is backpressure, not a
      buffer for load.

      **The rate limiter was the real trap.** It keys on the peer address,
      which axum supplies through `ConnectInfo` — and axum implements
      `Connected` only for its own `TcpListener`. The orphan rule forbids
      adding it for `SocketAddr`, and the available trick (wrapping the
      listener in a no-op `tap_io` to borrow axum's blanket impl) reads as
      dead code to the next person, whose deletion would leave the limiter
      with no address and no compile error to say so. Hence a local
      `serve::PeerAddr`, which makes both listeners provably produce the same
      key. `the_rate_limiter_still_sees_the_peer_address_over_tls` is the test
      that fails if this regresses.

      **Client side is `ca_file` in `remotes.toml`, and no fingerprint
      pinning.** For a self-signed certificate the two are the same trust
      decision — naming the certificate as the only root for that remote *is*
      pinning it — and a fingerprint would need `use_preconfigured_tls` and a
      custom `ServerCertVerifier` written twice, in the CLI and in the agent,
      to buy nothing. `ca_file` is read only for a named remote; a bare
      `--remote https://…` URL carries no trust configuration.

      **Out of scope, deliberately:** mTLS (the token stays the only client
      credential), ACME (a box with no public DNS name has nothing for ACME to
      prove), certificate generation by the agent, and trust options for
      `agent push`, whose target is a hub and therefore has a domain and a
      proxy. Eleven new tests: four in `tls.rs` on the errors an operator has
      to act on, two in `config.rs` on the half-configured refusal, five in
      `tests/api.rs` over a real TLS socket.
- [x] **D3 Rate limiting in the agent** — done *(14 September 2026)*.
      `crates/agent/src/ratelimit.rs`, a hand-written token bucket, no new
      dependency (same rationale as the cron parser).

      **The reason it was deferred was wrong.** "A token is already
      required" answers a different question: the token decides *who* can
      read, not *how often*. Two things sit entirely outside the token:
      `/health` is deliberately tokenless, and even a wrong token costs a
      header parse, a comparison and a response. That's why the limiter runs
      **before auth** — that's the only place it can limit those, and it's
      verified by mutation (exempting `/health` makes the test fail).

      A bucket, not a window: at a window boundary you can exceed double the
      limit (the last instant of one window plus the first instant of the
      next), and "bursts happen, sustained flooding doesn't" can only be
      expressed with a bucket.

      **The reverse-proxy warning is written in the doc.** The project
      recommends a reverse proxy, and there every request comes from the
      proxy's address, so a per-client limit turns into a single limit
      everyone shares. `X-Forwarded-For` is **not read**: it's a header
      anyone can write, and writing it would be a way to grant yourself a
      new allowance.

      The table is hard-limited (4096 addresses). Once it's full and
      everyone is still in debt, new addresses are rejected — forgetting one
      that's mid-stream would hand it back a full allowance, which would be a
      way to route around the table's limit. My own test caught this: in the
      first version only "full" buckets were evicted, and since every new
      client immediately spent its token, none of them were ever full, so the
      table grew unboundedly.

      Default 120/minute + 60 burst; `0` disables it. Eight unit tests (the
      clock is in the test's hands, otherwise every assertion would be a
      `sleep` and a guess) and three integration tests on a real socket.
- [~] **D4 Memory profile at 10M+ files** — **measured** *(11 September
      2026)*. This used to be a single-point extrapolation (~2.8 GB); the
      real answer is both better and more interesting than that.

      **The tree itself: 96 bytes/entry, perfectly linear from 100k to 10M,
      and identical on macOS and Linux** (10M entries = 915–916 MiB). The
      synthetic tree was built with `TreeAssembler`, i.e. the same path
      `store::load` uses — the structure itself, not a model of it.

      **But RSS isn't the tree, and the gap between them is
      platform-dependent.** For the same 250k-entry tree:

      | | macOS | Linux (glibc) |
      |---|---|---|
      | single-scan RSS | 125 MiB (~4× tree) | **35 MiB** (~1.2× tree) |
      | after 12–20 scans | **941 MiB** | **37.9 MiB** |
      | per scan | +42 MiB | +0.2 MiB |

      **Same code.** Linux always uses the `read_dir` + `lstat` path and is
      flat; on macOS *the same path* (measured with bulk disabled) grows
      +26 MiB per scan. So this isn't a leak, it's macOS libmalloc not giving
      back fragmented spans. `malloc_zone_pressure_relief` was tried: **it
      does nothing** (RSS is bit-for-bit identical). The `getattrlistbulk`
      path raises the growth from +26 to +45 MiB because it increases
      allocation traffic — a multiplier, not the cause.

      Explanations ruled out: thread count (138 MiB at 1, 163 at 16 — noise),
      a thread-pool leak (RSS flat over 20 scans on a 3-entry tree).

      *Conclusion:* **no problem for the agent** — on Linux 10M entries ≈
      1.1 GB and repeated scans don't accumulate. *Remaining:* **"Rescan" in
      the desktop** scans again in the same process and grows every time on
      macOS; visible over a long session. There's no cheap fix (relief does
      nothing); the real fix is reducing allocation traffic in the walk —
      i.e. B1.

      **Correction (14 September 2026): "+42 MiB per scan" turned out to be
      corpus-dependent, and this line didn't say so.** B1-K's baseline
      measurement ran the same probe on two trees, 8 rounds, in a single
      process:

      | | `/Applications` | `~/github` |
      |---|---|---|
      | entries | 412,983 | 428,731 |
      | round 1 | 87.4 MiB | 204.8 MiB |
      | round 8 | 94.8 MiB | 445.0 MiB |
      | per scan | +1.0 MiB (flattens) | +34 MiB (doesn't flatten) |

      Nearly the same entry count, completely different behavior. What
      distinguishes them is allocation traffic: `~/github`'s names are
      21.7 MiB, `/Applications`'s are 8.2 MiB. So "+42 MiB per scan on
      macOS" is an upper bound, not a rule.

      **After B1-K (14 September 2026):** the peak dropped too since
      allocation traffic dropped — `/Applications` 91.5 → 57.6 MB, and on
      Linux the "non-tree" portion was cut in half. The accumulation on
      macOS still stands (the cause is libmalloc, not the code) but it
      starts from a lower baseline, and the desktop now passes the
      `expected_entries` hint.
- [x] **D5 Progress feedback for `store::save`** — done
      *(14 September 2026)*. **And re-measured first, because the old number
      was wrong.** The 11 September "at most ~290 ms" was an indirect upper
      bound (subtracting the scan from the total time). Direct A/B: on
      `/Applications`, 753 ms without saving, 1324 ms with saving →
      **571 ms**, i.e. double the estimate. At 10M entries ≈ **14 seconds**.
      The "not urgent" assessment doesn't hold up against this number.

      The internals were measured too: row writing **273 ms**, `digest`
      **208 ms**, commit 18 ms. Since both passes are the same order of
      magnitude, there are two separate phases: `Phase::Saving` and
      `Phase::Checksumming`. With a single phase, the bar would reach the
      end and start over. *(Since 5 October 2026, B7, the digest is
      computed while the rows are written and `Checksumming` is never
      entered.)*

      `rows_done`/`rows_total` were added to `ScanProgress` and entered
      `StallWatch`'s counter array — that array's own read from
      `ScanProgress` was designed exactly for this, so every watcher was
      covered without being changed. The CLI's progress line is now an RAII
      guard: it used to be cleared the instant the walk finished, and the
      command went silent for half a second. The agent's `/status` also
      responds throughout the save (`rows_done`, `rows_total`, `phase:
      "saving"`).

      The test **watches the save from a side thread**: "finished with the
      right count" would also be true of a counter that jumps in one step at
      the very end, and such a counter would look frozen for the whole
      phase. Verified with two mutations.
- [x] **D6 `Tree::rel_path` walks from the root on every call** — done
      *(14 September 2026)*. Neither documented nor cached: **the direction
      was reversed.** `Tree::for_each_path` builds the path while descending
      — entering a directory appends a segment to the buffer, leaving it
      truncates it — so it's written once, not once per entry under that
      name. Caching wasn't even considered: the memory had been hard-won in
      B1-K.

      **Measured:** on `/Applications`'s 412,983 entries, **61 ms** with
      `rel_path`, **4.5 ms** descending — **13.6×**, and over five rounds the
      distributions never overlap. End-to-end CSV export 449 → **400 ms**
      (median, 7 interleaved rounds; OLD min 445 > NEW median). As a
      control, the ncdu export, which doesn't build a path, went 182 →
      183 ms, i.e. the change landed exactly where expected.

      **No measurable difference in `dupes`** (2970 → 2909 ms, distributions
      overlap) because the command's duration is dominated by the scan and
      `stat`. The rationale there isn't speed but worst case: `Tree::path` is
      linear in depth, and this crate doesn't control the depth — a snapshot
      from another machine can be as deep as it likes, so calling it per
      file is quadratic on adversarial input.

      **A side finding, and a real bug:** `dupes` sorted by node id in six
      places, and each one's comment said "so two runs give the same result"
      — but ids have changed between scans since B1-K. The published 0.7.0
      gives a different order on every run against a fixed fixture
      (measured). All six were converted to sort by path. The existing test
      couldn't have caught this: it called `find` **on a single tree** five
      times, so the ids were already identical.

      CSV row order changed too (now strict DFS: a folder immediately
      followed by its contents) — noted in the changelog.

### E. Distribution and documentation

- [x] **E1 Link from `docs/RESEARCH.md` to COMPETITORS.md** and updating the
      §1 competitor table with measurements. *(9 September 2026)* dua-cli got
      its own row; the TreeSize Free MFT and WinDirStat 2.5.0 corrections were
      applied; next to the §3 ~25 B/file target, the measured 276–437 B and
      the 16-thread regression were written in. The falsified item (b) was not
      deleted, it was left **marked as falsified** — so which decision was
      made with which information doesn't get lost.
- [x] **E2 WHY.md correction** — the sentence *"Remote machine + scan history
      exist only in SpaceObServer and $600+/year; below that there's
      nothing"* is **now wrong**: FreeSize Pro at CHF 29/year makes the same
      promise, and the dua-cli diff is free. The pricing hypothesis was also
      based on this sentence.
      *(9 September 2026)* Five places were corrected: the dua-cli column and
      the **"self-hostable" row** were added to the comparison table (the
      bold row is no longer "history", it's this now); the "a huge gap"
      paragraph was narrowed to the **triple intersection**; the sentence
      "FreeSize would need to write a server to copy this" was corrected
      (they did write one); CHF 29 was added to the price anchors; a
      measurement was added to the speed-gain condition. **Also:** the
      README's claim *"**Every** disk analyser … FreeSize"* was also wrong,
      and that was corrected too.
- [ ] **E3 Code signing** — macOS Developer ID + notarization ($99/year),
      Windows OV ($150–300/year). *Competitors:* FreeSize, Diskaroo, TreeSize,
      WizTree — all signed. An unsigned program that reads every corner of
      the disk = SmartScreen/Gatekeeper warning = low install rate.
      **Half of it closed for free on 10 September 2026:** Gatekeeper and TCC
      are separate systems, and the latter only wanted a *stable identity*. A
      self-signed certificate was introduced, and permissions now survive
      updates. What's left open is just the Gatekeeper/SmartScreen warning,
      and its cost is money.
      As a temporary patch there's `install-desktop.sh` / `.ps1` — since curl
      doesn't write the quarantine flag, the warning never triggers. What to
      delete once payment is made is written item by item in
      docs/RELEASING.md.
- [ ] **E4 Homebrew / Scoop / winget / AUR** — *manifests written and
      verified 22 September 2026; the four repositories and accounts do not
      exist yet.*

      **Why it is the widest gap and not the smallest.** On 22 September 2026
      spacetrace was in **no package manager at all**: 121 downloads across 14
      releases, 0 stars, 4 unique visitors in 14 days. Every tool it is
      measured against is one command away — `brew install dua-cli`,
      `scoop install gdu`, `pacman -S ncdu`. Being measurably faster than them
      (B8) reaches nobody who installs software the way most people do.

      Done: `packaging/` holds a template per manager and `render.sh`, which
      fills them from a **published** release and refuses a tag that was never
      published. Rendered against v0.9.0 and checked — Ruby parses the
      formula, a YAML parser the three winget manifests, the digests match the
      files downloaded from the release. A `packaging` job in `release.yml`
      renders and re-verifies on every release and attaches the result as an
      asset, which also makes it the one check that notices an asset rename.

      **The layout trap, worth stating once:** the `tar.gz` archives put the
      binaries at the root, the Windows `.zip` puts them in a versioned
      subdirectory. Scoop needs `extract_dir` and winget a `RelativeFilePath`
      with the version in it; a manifest written from the tar.gz shape
      installs nothing on Windows and says nothing about it.

      Left, and each is an account or a repository somebody has to create:

      - `unalcakir28/homebrew-tap` — public repo, `Formula/spacetrace.rb`
      - `unalcakir28/scoop-bucket` — public repo, `bucket/spacetrace.json`
      - winget — a pull request per version to `microsoft/winget-pkgs`
      - AUR — an SSH key at aur.archlinux.org, then `spacetrace-bin`;
        `.SRCINFO` needs `makepkg` and therefore an Arch machine

      homebrew-core instead of a tap would drop the tap prefix, but its
      acceptance criteria are about notability and 0 stars does not clear
      them. Flathub is deliberately excluded: a desktop store for a
      command-line tool, sandboxed away from the filesystem it exists to read.
      Detail and rationale: [packaging/README.md](packaging/README.md).
- [x] ~~**E5 Migration guides**~~ — **out of scope** *(14 September 2026,
      user decision)*. ROADMAP had listed it as a 1.0 requirement; it isn't
      one. The product itself already imports ncdu output (C8), and `--help`
      and the README are in English, so the answer to "how do I migrate"
      already lives in the tool. Writing a guide is marketing, not
      engineering, and it doesn't belong in this queue.

### Order

| Order | What | Why it's here |
|------|-----|--------------|
| ~~1~~ ✅ | ~~E1, E2~~ | Half an hour, and the input to every other decision — no plan should be built on a wrong competitive map. **Done (9 September 2026)**, including the README correction |
| ~~2~~ ✅ | ~~A4, A1, A2, A4w~~ | Our accuracy claim wasn't being met on Windows. A4 (Unix) was done first because there were no tests at all. **Done (9 September 2026), Windows CI green** |
| ~~3~~ ✅ | ~~B1~~ | A competitor solved this 10 days ago and wrote up how; the wall in front of the 10M-file target. **Four of them done (9 September 2026), B1-K on 14 September** |
| ~~4~~ ✅ | ~~A3, A5~~ | We were wrong on macOS (DaisyDisk was right) — A3 is done; corruption over the network is no longer silent. **Done (10 September 2026)** |
| ~~5~~ ✅ | ~~B2, B3~~ | Cheap and measured — B2 is done (10 September 2026), B3 on 5 October (simulated disk; a real HDD still to confirm) |
| ~~6~~ | ~~C3, D1~~ | Both done |
| 7 | B4, ~~B5~~ ✅, ~~B6~~ ✅, ~~B8~~ ✅ | Platform-specific fast paths — B5 is done (11 September 2026), B8 on 22 September, which is where the lead over dua-cli came from, and B6 on 5 October; B4 is written and waits for Windows CI |
| 8 | B7 | The biggest strategic win. Listed after B4 for the machine, not for a dependency — and since 21 September 2026 it reads as the cheaper of the two, because it needs no MFT parser |
| 9 | ~~C1–C9~~ ✅ | Feature parity completed *(11 September 2026)* |
| 10 | E3, **E4 ← half done**, ~~D2~~ ✅, ~~D3~~ ✅ | Release prep (E5 left out of scope). D3 done 14 September 2026, D2 on 21 September; E3 needs money. E4's manifests and CI job landed 22 September — what is left is creating four repositories and accounts, which is the widest remaining competitive gap and needs no engineering |
| ~~last~~ ✅ | ~~B1-K~~ | Double storage. Research was done and **the problem had been framed wrong**: the arena didn't want BFS, it wanted two properties. The intermediate tree was removed, speed is the same, peak memory is -37% on `/Applications`. **Done (14 September 2026)** |

---

## Technical debt

Doesn't block the product but shouldn't pile up. The starred ones are now
tracked in the "Competitive gaps" section together with their competitive
context and order — the work item lives there, here they only stand as a
debt record.

- [x] ~~Windows `alloc` isn't real~~ → **A1 done** *(9 September 2026)*.
      This line stayed open until 11 September, and the `TODO(win)` marker it
      pointed to had already been removed from the code — meaning the debt
      list contradicted the same file's roadmap section. Showing a closed
      debt as open means someone redoing work that's already done.
- [x] ~~Windows hardlink dedupe is off~~ → **A2 done** *(9 September 2026)*.
- [x] ~~No APFS clone deduplication~~ → **A3 done** *(9 September 2026)*, via
      `fcntl(F_LOG2PHYS_EXT)`, on by default.
- [x] ~~btrfs/ZFS: tree walking is wrong because of reflink and compression~~
      → **A6 done** *(2 October 2026)* for btrfs and XFS; ZFS is detected and
      warned about, not measured.
- [x] ~~Memory profile on 10M+ file roots was never measured~~ →
      **measured** (9 September 2026): `Node` 104 B, real peak
      **276–437 B/entry**. Then fixed: `Node` **72 B** (B1), double storage
      removed (B1-K), and on 14 September 2026 the peak on `/Applications` is
      **125 B/entry** (hinted). The measurement tool is now in the
      repository: `examples/memprobe.rs`. Remaining → **D4** (macOS
      libmalloc accumulation, not code)
- [x] ~~`Tree::rel_path` walks from the root on every call~~ → **D6 done**
      (14 September 2026): `Tree::for_each_path` builds the path while
      descending, 13.6× faster, and `dupes`'s id-based sort was switched to
      path.
- ~~Interface strings are embedded in code, no i18n~~ → not debt, a decision
      ([DECISIONS.md](docs/DECISIONS.md) K1). Strings stay English and
      embedded.
- [x] ~~`store::save` is a single transaction on large trees — no progress
      feedback~~ → **D5 done** (14 September 2026). The single transaction
      stays; what was missing was the counter. Real cost is 571 ms/412k
      (twice the old "≤290 ms" estimate), ≈14 s at 10M.

---

## Post-launch — SEO and AISEO

Our own domain and the site repository **were done on 9 September 2026**;
the rest is still deferred. So the audit doesn't need to be redone from
scratch when its turn comes, the measurements are here.

Status check (9 September 2026, measured on `dist`) — **the infrastructure
is correct**: 31 pages turn into HTML during build, 26 of them never fetch
any framework JS, React only ships on the 5 main pages that have the
treemap demo. This is critical, because most AI crawlers (GPTBot, ClaudeBot,
PerplexityBot) don't run JavaScript; if this were an SPA they'd see a blank
page. One `<h1>` per page, proper `h1→h2→h3`, `canonical`, `hreflang` +
`x-default` for five languages, a 30-URL sitemap and Open Graph titles are
ready.

- [x] **Own domain** → `spacetrace.teknobakkall.com`. CNAME in Cloudflare,
      **proxy off** (with the orange cloud on, GitHub can't issue a
      certificate). `base` became `/`, `public/CNAME` carries the domain.
- [x] **The site was moved into its own repository** →
      [unalcakir28/spacetrace-website](https://github.com/unalcakir28/spacetrace-website).
      Done in the same move as the domain, because the repository name was
      part of the URL.
- [x] **`robots.txt` and `llms.txt`** — done *(21 September 2026)*, both in
      `public/` in the site repository. `robots.txt` is allow-all and the
      `Sitemap:` line is the only reason it exists; eleven AI crawlers are
      named explicitly, which changes no behaviour today because the wildcard
      already permits them, and makes narrowing it later a decision about each
      one rather than a silent drop. `llms.txt` carries **no version number**:
      nothing would keep it true, so it points at the download and changelog
      pages, which are generated from the release itself.
- [x] **JSON-LD structured data** — done *(21 September 2026)*,
      `src/data/structuredData.ts`. One `@graph` per page with `@id`
      cross-references: `Organization` and `WebSite` (one node for the domain,
      not one per language) plus `WebPage` everywhere, `BreadcrumbList` off the
      home page, `SoftwareApplication` on the three component pages.

      **Three things it deliberately does not say.** No `softwareVersion`,
      which would be a third place a release has to be bumped and the only one
      nothing guards. No `offers` on desktop or hub: the desktop is free *while
      it is in phase 3* and the hub is commercial, and structured data cannot
      say "for now", so a price there would be a claim that expires quietly —
      the CLI, which is Apache-2.0 and will stay free, prices itself. No
      `FAQPage` and no invented `sameAs`, because the guide has no real
      question-and-answer structure and the project has no social account.

      `operatingSystem` differs per component: the hub ships Linux and macOS
      binaries and no Windows one. Copying the desktop's list across was the
      easy mistake.
- [x] **`og:image` and Twitter card** — done *(21 September 2026)*. One
      1200×630 card, `public/og.png`, rendered from `scripts/og-card.html` so
      it uses the site's own palette and typography rather than a copy of them
      in a design tool. `twitter:card` is `summary_large_image`, with no
      `twitter:title` or `twitter:description` — X falls back to the Open Graph
      ones — and no `twitter:site`, because there is no account to name.

      **One image for every page and language, on purpose.** The
      page-specific part of a share is already carried by the translated
      `og:title` and `og:description`. Seven per-page cards would each bake a
      page name into pixels, and no check can read text out of a PNG, so a
      renamed route would leave a wrong image behind with nothing to catch it.
- [ ] **Google Search Console + Bing Webmaster Tools registration and
      sitemap submission.** Still open, and still an account task rather than a
      code one — but the code side is now ready: `VERIFICATION` in
      `src/data/seo.ts` renders `google-site-verification` and `msvalidate.01`
      when a token is pasted in, and **no tag at all when the string is empty**,
      because an empty meta tag is a failed verification that looks like a
      finished one. Bing imports a verified property straight from Search
      Console, so the Google token alone is usually enough. Bing also feeds
      ChatGPT search.
- [x] ~~Minor: `og:locale` isn't in the `en_US` format the OG spec wants~~ →
      fixed *(21 September 2026)* with a second map, `OG_LOCALES`.
      `LOCALE_TAGS` keeps the bare codes, which are what `hreflang` and the
      `lang` attribute correctly want; the two are separate on purpose, and
      collapsing them breaks one of them silently, because a scraper ignores
      the short form without complaining. The other four languages are now
      declared as `og:locale:alternate`.
- [x] ~~Minor: Google Fonts is fetched as an external stylesheet~~ → done
      *(2 October 2026)*, site commit `1e767d5`: the woff2 files are served
      from `public/fonts/` (Google's own files for Archivo and JetBrains
      Mono, still variable, latin and latin-ext only) with `@font-face` in
      `global.css`; Archivo latin is preloaded, plus latin-ext on Turkish
      pages, and the OG card renders from the same bytes. No request goes
      to a Google domain any more; first screen and `og.png`
      pixel-identical to before. Live once the site is pushed. **Kept separate from the work above** *(21 September 2026)*:
      that was discoverability, this is performance, and they share nothing
      but the `<head>` they live in.

**A guard came with it**, because every failure in this area is silent: a
dropped `og:image` turns a share back into a bare link and the page looks
identical, a relative one is ignored rather than rejected, a JSON-LD typo is
skipped rather than reported, and a renamed route leaves `llms.txt` handing a
404 to exactly the crawlers it exists to serve. `scripts/verify-seo.mjs`
(`yarn verify:seo`, in the Pages workflow) reads the built `dist/`, like the
site's other two verification scripts and for the same reason. Measured by
mutation: dropping `og:image`, shortening `og:locale` to `en`, putting
non-JSON in the `ld+json` script and renaming a route inside `llms.txt` were
four for four.

**What was already right** (the 9 September audit above) carried the rest, and
re-measured on `dist` for this work: of 36 built pages, **31 carry no React
island at all** and the 5 that do are the home page in its five languages,
where the treemap demo lives. That is what makes any of this reach GPTBot,
ClaudeBot and PerplexityBot — none of them runs JavaScript, so a static graph
in the markup is the only form they ever see. Had this been an SPA, all of the
above would have been invisible to them.

Setting expectations: the technical side ensures **there's nothing blocking
indexing** and makes the content maximally readable. Climbing the rankings
is a content and backlink job; no technical tweak moves a new site up the
rankings quickly.

---

## Idea pool

No decision made yet, will be discussed when its turn comes.

- [x] Duplicate finder → **C4**, done 11 September 2026
- [x] ncdu/gdu JSON **import** → **C8**, done 11 September 2026
- [x] File age heat map → **C7**, done 11 September 2026
- [x] **Cushion-shaded treemap** — done: core side *(2 October 2026)*,
      desktop side with the pin bump *(5 October 2026)*. van Wijk & van de
      Wetering 1999 (h = 0.5, f = 0.75, Ia = 40, Is = 215, light [1, 2, 10]
      with y flipped for screen space). The four coefficients per tile are
      accumulated in the core's layout pass (`treemap/src/cushion.rs`),
      tested there, and sent only on request as f32 (+43 B/tile, 52 → 95).
      The desktop lights each device pixel once, from an owner map, and multiplies that onto the cached
      flat layer, so it composes with every colour mode. At DPR 2 in
      Chromium: repaint 4 → 28 ms (16k tiles) and 17 → 42 ms (147k); hover
      unchanged. Default stays Flat.
      **Still open:** frame time on WKWebView,
      WebKitGTK and WebView2; a cheaper owner pass (12 ms, 7.2× overdraw) if
      those turn out slow.
- [x] **Cloud roots — S3** — done *(2 October 2026)*. `spacetrace scan
      s3://bucket/prefix`, for AWS and S3-compatible services via
      `--endpoint` (path-style). It lives in the CLI: hand-written SigV4,
      HMAC and XML, so the agent stays small. Keys become folders, a
      trailing `/` marks a folder, an object shadowed by a same-named folder
      is charged to it (`alloc` only, reported), and `size` = `alloc` =
      object length. Current versions only, and the output says so. `host` =
      the service and `root` = `s3://bucket[/prefix]`, so diff finds the
      previous snapshot.
      *Measured:* totals equal `mc du` to the byte before and after a change
      (7,349,860 → 12,612,860 B). Real AWS: redirected us-east-1 →
      us-west-2, then 4,885 objects / 54.8 GB in 5 pages, matching an
      independent paginator. The signer passes AWS' published vectors.
      **Found on the way:** `age --scan ID` reported 0 B in every band for
      every stored snapshot — `age_profile_at` and `median_bands` summed
      `own_size`/`own_alloc`, which `store::load` returns as zero by design.
      Fixed in scan-core; the desktop's age colouring of a loaded snapshot is
      fixed with the next pin bump.
      **Credentials** *(5 October 2026)*: botocore's default chain, in its
      order — environment, `role_arn` (with `source_profile` or
      `credential_source`, through STS AssumeRole), web identity, SSO
      (`sso_session` and legacy; the token cache found by SHA-1 of the
      start URL), credentials-file keys, `credential_process`, config-file
      keys, ECS/EKS container credentials, IMDSv2. One renewal for all
      workers: in the advisory window one thread renews while the others
      keep the current keys; past the mandatory deadline they wait,
      cancellably, and share one outcome (one MFA prompt, not sixteen).
      Checked against aws CLI v2.36.47 on 24 configurations. Hardened past
      botocore: `AWS_CONTAINER_CREDENTIALS_RELATIVE_URI` must start with
      `/` and the result goes through the loopback/allow-list check;
      addresses in errors drop userinfo and query; the IMDS role name must
      be a valid IAM name; `credential_process` stdout is capped at 1 MiB.
      Live service checked: MinIO STS AssumeRole only; web identity, SSO,
      ECS and IMDS against local stand-ins. No MFA prompt, no SSO OIDC
      refresh, no assume-role cache file.
      **Parallel listing** *(5 October 2026)*, `s3/listing.rs`: delimiter
      discovery (≤ 3 levels) finds folders, runs of siblings become
      `prefix` + `start-after` ranges, an idle worker takes the second half
      of the earliest busy range, and pages reach the tree builder in key
      order, so the tree is identical for any worker count. Fetched-ahead
      objects are capped at 100,000; a range that runs out of room pauses
      and resumes after its last key (waiting inside the range took 111 s
      against 27 s). A static partition with in-order claiming was tried
      and lost (587k keys 130 s vs 19 s at 16 workers): a range cannot be
      split midway. AWS `sentinel-cogs` (us-west-2, link-bound past 8
      workers): 587k keys 175 s → ~30 s at 16, 78k 22 s → 7 s. MinIO (1M
      keys, one drive) is slower in parallel (207 s → 490 s at 16), so any
      non-AWS `--endpoint` defaults to one stream; `--threads` overrides,
      clamped to 64.
      - [ ] Still open: OneDrive and Google Drive (need a registered OAuth
            client); memory and time at 10M keys (extrapolated ~1.7 GB, not
            measured); a flat bucket (no `/`) gets no parallelism;
            in-region AWS, Ceph, R2, B2, Wasabi unmeasured; virtual-hosted
            addressing for `--endpoint`; billed bytes beyond current
            versions (ListObjectVersions / ListMultipartUploads); Ctrl-C
            saves nothing (no signal handler).
- [x] **C10 Package manager awareness** — done *(2 October 2026)*.
      `spacetrace pkgs [PATH]` credits every file under a root to the
      package that installed it and lists the rest as unowned, in pieces
      that partition the unowned total; a file instead of a folder names its
      owner. dpkg, pacman, apk and Homebrew are read off the disk; rpm's
      binary database through `rpm -qa --qf '[%{=NAME}\t%{FILENAMES}\n]'`
      (`%{NAME}` without `=` fails on every multi-file package), and a
      missing `rpm` is reported, never silently unowned.
      **Merged /usr is the whole difference**: listed directories are
      canonicalised (once each, cached), the last component is not, so a
      shipped symlink matches as a link. Without it bookworm's `/usr` was 17%
      "unowned" (297 files, 22.5 MB — all of `/bin`); `dpkg -S /usr/bin/ls`
      itself fails there. A file two packages list goes to one (manager
      order, then name) and the overlap is printed. Another host's snapshot,
      an imported one (marked `scanner_version = "import/…"`, older imports
      recognised by having no scan time and no filesystem size) and
      `--remote` are refused. A review closed the hangs: listed paths are
      resolved one component at a time and stop at the scope, a mount inside
      it is asked once through `probe_mount`'s deadline, `rpm -qa` runs
      under a deadline (stale BDB lock), and one damaged entry is counted
      and sampled instead of ending the report.
      *Measured:* per-package bytes equal an independent `stat` sum over the
      package's own list on bookworm, Arch and Fedora; openjdk in
      /opt/homebrew equals its Cellar plus its link. 97,907 listed paths cost
      +90 ms and +13 MiB RSS over a plain scan of `/usr`.
      **macOS receipts** *(5 October 2026)*: the BOM and plist files are
      parsed directly (`pkgs/receipt.rs`), not through `pkgutil`. On 147
      receipts / 942,958 paths `pkgutil --files` per package took 7.7 s and
      the parser 0.09 s, with identical paths and folders for every package
      (ignored oracle test). Paths resolve against the volume plus
      `InstallPrefixPath`; folders are claimed and the tree decides (25
      Highlights paths were bundles when installed and plain files after an
      update). The per-package `stat` cross-check found two bugs, both
      fixed: matching is case-folded when `pathconf(_PC_CASE_SENSITIVE)`
      says the volume ignores case (Office re-capitalised 442 Word files),
      and a scan under `/System/Volumes/Data` is translated through
      `/usr/share/firmlinks`. A receipt is untrusted input: the leaf chain
      stops at the recorded count, paths over `MAXPATHLEN` and plist sizes
      past the file refuse the receipt, XML comments/CDATA are skipped
      properly; a damaged receipt credits nothing and is counted and
      sampled. Every package in /usr/local (27), /Applications (28) and
      /Library (23) equals its `stat` sum. Matching costs 72-465 ms and
      +83-215 MiB peak RSS over the scan (Xcode's 36 MB BOM is installed at
      `/` and read for every scope).
      - [ ] Still open: snap/flatpak/nix, cask links outside the Caskroom,
            Homebrew on Linux verified only on a hand-built layout; receipts
            on external volumes; a real per-user install into
            `~/Library/Receipts` (only `pkgutil --volume` verified); case
            folding is decided once per scan, so a scan crossing into a
            volume with the other setting folds wrongly there; Linux
            casefold directories (ext4/f2fs `+F`) are matched
            case-sensitively; each BOM is read whole, not mapped. Files generated by install scripts
            (`__pycache__`, `hwdb.bin`, `modules.alias`) are honestly unowned
            — no list names them.
      *Competitor:* QDirStat.
- [x] **C12 `spacetrace watch`** — done *(2 October 2026)*. Live "what is
      growing": a fresh scan as baseline (a stored snapshot's distance is
      `diff --since-last`'s job), then the folders that changed since, with a
      10-20 s rate, by `diff`'s culprit rule — a test holds the rows to `diff`
      itself. **Events only say where to look**: a dirty folder is relisted
      with `scan(dir, max_depth = 1)`, a new one scanned whole, so scan-core
      does all size accounting. **Hardlinks** cannot be settled by one listing
      (`Node` keeps no inode), so a `(dev, ino)` ledger beside the model
      settles them (see below). **Clones** are counted
      at full size, same reason. **Loss**: inotify overflow → full rescan,
      FSEvents MustScanSubDirs → subtree rescan, plus an unprompted full
      rescan every max(60 s, 30× scan) that corrects and reports drift —
      needed because notify 8.2 can drop a Windows overflow without telling
      anyone. inotify gets one watch per descended folder, added before the
      first scan (8 vs 29 watches with/without `--exclude node_modules`,
      measured); exhausting `max_user_watches` stops the command with the
      sysctl (shown in Docker). New crate: notify =8.2.0, CLI only, since 5
      October 2026 off Linux only.
      Measured on 1.26M entries / 83k folders: steady RSS ~250 MB (a scan's
      retained peak); with cargo builds inside the root, hardlink rescans cost
      ~9 s CPU each and peak at a 520 MB footprint.
      A review closed four gaps: a folder deleted and made again or swapped
      for another (`npm install`) kept the old totals and, on inotify, lost
      its watches — a create or rename naming a tracked folder now rescans
      its subtree and watches it again; watch setup approached mount points
      before the guarded scan (invariant 7), now through the mount table and
      `probe_mount`; the periodic check and compaction never ran on a disk
      written to every tick; non-UTF-8 names were tracked by their lossy
      form. The event queue is bounded and a full one is a loss.
      **Hardlink ledger** *(5 October 2026)*: `watch/links.rs` keeps every
      hardlinked file by `(dev, ino)` with the folders holding its names,
      fed by scan-core's `scan_recording` hook from the same `stat` that
      charged the name. A file is charged to one folder, the first by
      (depth, path), once all `nlink` names are placed; a name that cannot
      be placed is doubted for two ticks and only then costs one capped full
      rescan. Files sit in a slab with the first two folders inline (116 B
      per file, 36.7 → 22.2 MiB for 200k files with a link each); folders
      keep running charged sums, so a refresh no longer walks the ledger.
      Measured with 6 cargo builds inside a 1.2M-entry root on macOS: CPU
      per build 9.33 s → 0.02 s, full rescans 6 → 0, peak 281 → 133 MiB;
      totals equal a fresh `scan` byte for byte.
      **inotify directly** *(5 October 2026)*: `watch/inotify.rs` asks for
      changes only (no IN_OPEN), coalesces one change per kind per folder per
      read, takes a moved folder's watches off with it, and a reader failure
      reaches the user with its cause. On Linux (Docker, 500k files, 8
      builds) CPU per build 2.60 s → 0.01 s, full rescans 15 → 0 (two of
      notify's were a startup queue overflow), peak RSS 75 → 58 MiB.
      - [ ] A link into an excluded, too-deep or out-of-root folder made
            between full scans leaves the file's bytes out for about two
            ticks, then costs one capped full rescan; a partial scan cannot
            tell an outside name from one listed earlier. Linking a file
            that sat unchanged for a while costs the same on Linux.
      - [ ] Android stays on notify's inotify backend (not built here).
      - [ ] **Windows verified only by CI**, overflow behaviour untested.
- [x] **Prometheus metrics endpoint** — done *(2 October 2026)*.
      `crates/agent/src/metrics.rs`, text format 0.0.4, hand-written, no new
      crate: thirteen gauge families, per configured root from the newest
      stored snapshot and the runner's claim table, plus build info and start
      time. Behind the token (root paths and fill level are the inventory it
      guards) and behind the rate limiter (15 s scrape = 4 of 120/min).

      **A scrape touches no scanned filesystem.** Capacity is the snapshot's,
      not a fresh `statvfs`, which hangs on a dead share (invariant 7). The
      configured-vs-canonical root mapping that a restart loses is resolved
      once per root on an abandonable thread (`Runner::recorded_roots`), at
      most 500 ms of waiting per scrape, overwritten by every scan.

      **Never-scanned roots stay visible** (`scan_running`, `snapshots` = 0);
      snapshot families are absent rather than 0. A root listed twice is
      reported once — a duplicate series fails the whole scrape.

      **Measured, and it found a slow query.** `latest_for(root, Some(host))`
      read the whole index because `(?2 IS NULL OR host = ?2)` is planned for
      every binding; split into two statements. Median scrape, two roots:
      29 snapshots 0.81 ms; 20,005: 10.40 → 2.18 ms; 200,005: 102.28 →
      14.79 ms (rest is COUNT over each root's index range). Proven against
      prom/prometheus 3.15.0: `up` 1, values equal `/scans`, a label with
      `\`, `"` and a newline round-tripped. 18 new tests.

      **Still open:** the dead-mount path of `recorded_roots` is reasoned, not
      exercised against a real hung NFS mount; `last_two_for` has the same
      `IS NULL OR` plan and was left alone; `/status` still reads every
      snapshot row for its count (220 ms at 200,005); no scan-failure counter.
