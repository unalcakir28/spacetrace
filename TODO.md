# To do

Live work list. For phase definitions and exit criteria see
[docs/ROADMAP.md](docs/ROADMAP.md), for rationale see [docs/WHY.md](docs/WHY.md),
for where competitors are ahead see [docs/COMPETITORS.md](docs/COMPETITORS.md).

Last updated: 14 September 2026 (**Version cut: CLI 0.7.0, desktop 0.7.0,
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
  - [ ] Optional built-in TLS — a reverse proxy is recommended for now (→ **D2**)
- [x] `agent push <url>` — send the snapshot to the hub/another agent (zstd)
- [x] Concurrent scan lock (the same root is not scanned twice → 409)
- [x] Rate limiting — **D3 done** (14 September 2026); the reason for deferring it
      was wrong, details there

### Client side
- [x] Remote source in the CLI: `--remote <url|name>` (scans / ls / diff / export)
- [x] `spacetrace pull` — pull a remote snapshot into the local database
- [x] Remote source definitions `remotes.toml`
- [ ] SSH mode: running a temporary agent on the other side without requiring installation

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
- [ ] **Windows MFT fast path** (`usn-journal-rs`) — behind admin (→ **B4**)
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
- [ ] **A6 btrfs/ZFS awareness** — tree walking is wrong because of reflinks
      and compression. Long-term; doing it right requires sampling.
      *Competitor:* btdu (Monte Carlo, 1% resolution at ~100 samples).

### B. Speed — measured gaps

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
      `(depth, path)`.

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
- [ ] **B3 HDD / network drive mode** — on spinning disks and NFS, a
      parallel walk can be worse than single-thread (seek thrash); we
      have nothing for this case.
      *Competitor:* gdu `--sequential`; QDirStat sorts entries by inode
      before stat'ing them.
- [ ] **B4 Windows MFT fast path** (`usn-journal-rs`) — needs
      administrator, absent on ReFS and network/FAT → falling back to the
      normal path is mandatory, and that path gets fixed in A1/A2.
      **The order is therefore after A.**
      *Competitor:* WizTree (raw MFT), TreeSize Free (administrator),
      WinDirStat 2.5.0.
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
- [ ] **B6 Linux `getdents64` + `statx` fast path.**
      *Competitor:* `dut`, in warm cache, 6.87× over `du`, 2.8–3.75× over
      dust/dua/gdu.
- [ ] **B7 Incremental rescan via USN Journal (Windows)** — for an agent
      that scans nightly, scanning everything every time is wasteful.
      Strategically the biggest speed gain. *Competitor:* **SpaceObServer**
      — our closest architectural competitor, and ahead right at this
      point.

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
      *(11 September 2026)*. `scan-core/src/live.rs` + `LiveMap.tsx` on
      the desktop.
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
      data change, not an algorithm change.
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

- [ ] **D2 Built-in TLS in the agent** — a reverse proxy is currently
      recommended (also in Phase 2).
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
      end and start over.

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
- [ ] **E4 Homebrew / AUR / Microsoft Store.**
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
| 5 | ~~B2~~ ✅, **B3 ← next up** | Cheap and measured — B2 is done (10 September 2026); B3 needs a real HDD or a network drive |
| ~~6~~ | ~~C3, D1~~ | Both done |
| 7 | B4, ~~B5~~ ✅, B6 | Platform-specific fast paths — B5 is done (11 September 2026); B4 needs a Windows machine, B6 a Linux machine |
| 8 | B7 | The biggest strategic win, but also the biggest job |
| 9 | ~~C1–C9~~ ✅ | Feature parity completed *(11 September 2026)* |
| 10 | E3, E4, D2, D3 | Release prep (E5 left out of scope) |
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
- [ ] btrfs/ZFS: tree walking is wrong because of reflink and compression; a
      "filesystem-aware mode" is needed → **A6**
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
- [ ] **`robots.txt` and `llms.txt`** — both are now possible since the
      domain arrived. They go into `public/` in the site repository.
      `llms.txt` is the emerging convention for introducing the project to
      AI agents.
- [ ] **JSON-LD structured data** (currently 0). The `SoftwareApplication`
      schema is the machine-readable answer to "what is this, which OS,
      which license, is it free" questions. Should be generated per page and
      per language. Note: astro.build's own site doesn't have it either, so
      it's not a universal practice — putting it ahead of `og:image` would
      be too presumptuous.
- [ ] **`og:image` and Twitter card** (neither exists). Right now every
      share on LinkedIn/X/Slack/Discord shows up as a bare link. A 1200×630
      image is needed; the treemap itself is the natural candidate.
      astro.build has one.
- [ ] **Google Search Console + Bing Webmaster Tools registration and
      sitemap submission.** This is the real answer to "fastest indexing"
      and it's an account task, not a code task. Bing also feeds ChatGPT
      search.
- [ ] Minor: `og:locale` isn't in the `en_US` format the OG spec wants (it
      says `en`); Google Fonts is fetched as an external stylesheet —
      serving the woff2 files ourselves removes one render-blocking
      third-party request.

Setting expectations: the technical side ensures **there's nothing blocking
indexing** and makes the content maximally readable. Climbing the rankings
is a content and backlink job; no technical tweak moves a new site up the
rankings quickly.

---

## Idea pool

No decision made yet, will be discussed when its turn comes.

- Duplicate finder (size → pre-hash → blake3, cached)
- ncdu/gdu JSON **import** (existing users' old scans)
- Cushion-shaded treemap (SequoiaView/WinDirStat look)
- Cloud roots: S3, OneDrive, Google Drive each as a "remote source"
- Package manager awareness (like QDirStat: "this file belongs to that package")
- File age heat map ("400 GB untouched for 2 years")
- `spacetrace watch` — live updates via inotify/FSEvents
- Prometheus metrics endpoint (agent `/metrics`)
