# spacetrace — project notes for Claude

A tool that scans disk usage, writes it into a SQLite snapshot and, by
comparing two snapshots, tells you **what grew**. Rust workspace.

Background reading (decisions that are not visible in the code):
[docs/WHY.md](docs/WHY.md) why this product,
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) why this design,
[docs/DECISIONS.md](docs/DECISIONS.md) decisions taken between closed phases
and their rationale, [docs/ROADMAP.md](docs/ROADMAP.md) phases,
[TODO.md](TODO.md) what is next. When weighing a feature proposal, check the
**out of scope** list in WHY.md; before reopening a design decision, check
DECISIONS.md. Installing and configuring the agent:
[docs/AGENT.md](docs/AGENT.md) (ready-made systemd unit
`deploy/systemd/spacetrace-agent.service`); the measured answer to "how does
tool X do this" is [docs/COMPETITORS.md](docs/COMPETITORS.md).

## Commands

```bash
cargo test --workspace                   # 402 tests, all must pass
cargo clippy --workspace --all-targets   # must be warning-free
cargo fmt --all
cargo build --release                    # binary: target/release/spacetrace
cargo check -p spacetrace-scan-core --target x86_64-pc-windows-msvc --all-targets
cargo run -q -p spacetrace-changelog -- markdown --component cli > CHANGELOG.md
```

**`--all-targets` is mandatory:** without it the test code is never compiled
at all, and a platform-dependent error under `#[cfg(test)]` only shows up in
CI (that is what happened on 11 September 2026: `parse_mountinfo` compiled on
every platform in tests and used `std::os::unix` inside).

Windows type checking can only be done for `scan-core`: `agent` and `cli`
depend on C code through zstd, and there is no cross compiler for the msvc
target on macOS. Their Windows behaviour is only visible in CI — **do not say
"it works on Windows" without waiting for CI.**

Rust 1.85+ is required. The tests use a **real file system** in temporary
directories (including hardlink, symlink and permission-error scenarios), no
mocks.

## Invariants that must not break

These can break silently and go unnoticed outside the tests:

0. **Opening a database must not take a write lock.** `store::schema::migrate`
   runs no DDL and no persistent pragma when the schema is up to date. Because
   the agent and the hub open a connection on every request, an unconditional
   `CREATE TABLE IF NOT EXISTS` or `PRAGMA journal_mode` caused a read to
   knock over an in-flight write with SQLITE_BUSY (caught in CI, test in
   `roundtrip.rs`).
1. **Size semantics.** `size` = file bytes only. `alloc` = **what the disk
   actually holds**, including directory blocks. A directory's own inode size
   does **not** enter the logical total; that is why `size` does not match
   `du -sb` (GNU `--apparent-size` adds every directory's inode size) — the
   old wording said it did, and that was wrong.

   **`alloc` is not `du`, `alloc` ≈ `df`.** With no shared blocks the two are
   exactly the same, and a test enforces it. With block sharing `du`
   over-counts, we do not, and **the difference is exactly the shared blocks**
   (that is tested too). There are two cases: for hardlinks `du` deduplicates
   as well, **for an APFS clone it does not** — the clone has its own inode
   and `nlink == 1`, but its blocks sit on disk once. Measured: 3 clones ×
   100 MB = **0 MB** of free-space consumption. Reporting that the way `du`
   does would be the wrong answer to the question "how much room does this
   take on disk".
   This equivalence is a test condition and **the test is
   `crates/scan-core/tests/du_equivalence.rs`** (written on 9 September 2026;
   until that day the claim was verified by hand). The oracle for `alloc` is
   the external `du`; the oracle for `size` is the naive serial walk in the
   same file, because `du` cannot give a logical size (BSD `-A` rounds to
   blocks, GNU `--apparent-size` adds the directory inode). Extend this file
   when you change scan behaviour.
2. **Arena layout: two properties, and only two.** A node's children are
   contiguous (`children_start .. +children_len`), and every child's index is
   **greater** than its parent's. `TreeBuilder::aggregate` aggregates in a
   single reverse pass (that is what the second property is for), `store`
   persists the layout as it is, and `Tree::check` checks exactly these two.

   **Not BFS, and that changed on 14 September 2026.** For years it said "in
   BFS order" because it was: the walk built a separate intermediate tree and
   `flatten` copied it level by level. Now every directory is written into the
   arena as soon as it is listed (B1-K, double storage is gone), so **the
   layout is the order in which directories finish**. Both properties hold by
   construction — a parent has to be in the arena already to be nameable. The
   only way to add a child is `TreeBuilder::push_block`.

   **Consequence: two scans do not produce the same layout.** Two scans of the
   same disk give the same answers (test:
   `the_thread_count_does_not_change_the_answer`, which compares path by path)
   but not the same ids. `diff` matches by name, the desktop ties ids to a
   generation — but **do not write anything new that relies on id order, and
   we have already shipped this once.**

   `dupes` sorted by id in six places, each with a comment saying "so that two
   runs give the same result". That comment had been wrong ever since B1-K
   changed the layout: the released 0.7.0 gave three different orders in three
   runs on a fixed fixture (measured). All six were switched to path in D6;
   the scanner's clone deduplication was already sorting by `(depth, path)`.
   **I once waved this off as "not a bug", and it was one** — when you see a
   comment that relies on id order, do not believe it, measure.

   Testing it is hard too: the existing `the_order_is_the_same_every_run`
   could not catch it, because it calls `find` five times on a **single**
   tree — the ids are the same anyway. The new test builds two different
   layouts with `from_nested`; building them via the thread count gives the
   same layout on a small fixture and loses the bet.
3. **Symlinks are not followed** (they count with their own size),
   **hardlinks are counted once** (`(dev, ino)`; both names appear in the
   tree, one contributes 0 bytes). **Which name carries the bytes is
   undefined** — the walk is parallel, the thread that claims the inode first
   wins, and this varies from platform to platform (on macOS the copy at the
   root was counted, on Linux the inner copy; caught in CI). The guarantee is
   "once", not "the first path" — when writing a test, assert over the pair,
   not over one name.
4. **`Tree::remove_subtree` zeroes the node, it does not drop it from the
   list.** That is what the arena layout means: cutting an entry out of the
   middle renumbers every node after it and forces every client holding ids
   (the desktop) to forget everything. The entry stays addressable and reports
   0 bytes; because `children_len = 0` is set, you cannot descend into it. The
   returned id list is the entries the caller must no longer list.
5. **A cancelled scan returns no tree.** After `ScanProgress::cancel`,
   `scan()` returns `ErrorKind::Interrupted`. A partial tree looks complete
   and reports a wrong total; writing it next to real snapshots is the worst
   outcome. **When writing the test:** the claim "the walk stopped early" is
   established by cancelling *before the scan starts* and verifying
   `progress.files == 0`. Cancelling from a side thread and saying "it cannot
   have finished everything" is betting that the walk is slower than the
   timer, and it loses on a fast machine (it lost on macOS CI).
6. **Which measure something is sorted or drawn by is a parameter, not a
   default.** `SizeBasis` (`Logical` | `OnDisk`) is threaded through
   `children_by`, `Node::measure` and `LayoutOptions.basis`. A sparse file
   reports a length 50 times larger than what it holds (a Docker.raw claiming
   1 TiB holds 19 GiB), and these are the *largest* entries on real disks — so
   the logical measure is most wrong on exactly the entries that matter most.
   The sort and the number beside it must come from the same measure; a list
   that says "largest first" has to say the same thing as the number next to
   it. The desktop default is `OnDisk`, the CLI's is `Logical` (written out
   explicitly at the call sites).
7. **Errors are not swallowed.** An unreadable path is counted and sampled;
   the scan does not stop. **A mount that does not answer is a read error
   too.** `entry.metadata()` does not return on a dead mount and cannot be
   interrupted, so mount points (`mounts.rs`, read at the start of the scan)
   are approached on a thread that can be abandoned; when the deadline passes
   the path is counted as unreadable and the walk continues with its siblings.
   Read the table with `MNT_NOWAIT` — `MNT_WAIT` blocks on a dead mount, which
   turns the precaution into the bug.
8. **Every long-running phase must have a counter that moves.** The watching
   side answers "is it stuck" by looking at the counters alone (the CLI warns
   after 10 seconds without movement), so a phase without a counter looks hung
   while it is working fine. The clone probe did exactly that: 1193 ms of a
   1989 ms scan on `~/github`, without a single counter moving (measured,
   10 September 2026). That is why `clones_probed` exists; do the same when
   you add a new phase. **Saving is a phase too** (14 September 2026): on
   412.983 entries the walk takes 753 ms and writing to the database 571 ms —
   ≈ 14 seconds at 10M entries. `Phase::Saving` and `Phase::Checksumming` (two
   separate passes: 273 ms writing, 208 ms digest) plus
   `ScanProgress::rows_done`/`rows_total` exist for this, and the CLI progress
   line now covers saving as well as scanning. **`StallWatch` reads the
   counters from `ScanProgress` itself**, precisely so that when a new counter
   is added every watcher sees it without being changed.
9. **The agent deletes nothing.** A deliberate decision so that software
   installed on a server can earn trust, not a missing feature.

## Code and repository habits

- **Everything here is English: code, comments, user-visible strings,
  `--help` text, error messages, and all documentation — including this file
  and the `docs/` directory.** There is no i18n layer for the CLI, the agent
  or the hub and none is planned — a translated command is wrong information
  (K1). Commit messages are English going forward; the existing history is
  Turkish and is not being rewritten.
- **One exception, and only in two places: the desktop GUI and the changelog
  texts are in five locales** (`en tr it fr de`, the same set as the website).
  This is not a repeal of K1 but a narrowing of its scope; rationale in
  [docs/DECISIONS.md](docs/DECISIONS.md) K10. The terminal and server surface
  stays English.
- A comment explains not *what* it does but **why it does it that way**. The
  code already says what it does.
- Be stingy about adding dependencies. The agent has to be installable on a
  NAS as a single static binary; before adding a dependency, check whether the
  standard library solves it. Example: the cron parser and the calendar
  arithmetic were written by hand instead of chrono (~200 lines), because the
  only need was "the next matching minute". axum + tokio is a deliberate
  exception (see DECISIONS K3).
- New dependencies are versioned in `[workspace.dependencies]`; crates take
  them with `foo.workspace = true`.
- Commit message bodies explain **why** something was done. See `git log` for
  examples; the older entries there are Turkish.
- **If you made a change a user will see, write a changelog entry**: in
  `crates/changelog/changelog.json`, in the relevant component's `unreleased`
  list, in five locales. It does not replace the commit message — the commit
  tells the code what was done, the changelog tells the user what changed
  (K11). `CHANGELOG.md` is generated, do not edit it by hand; CI breaks if it
  goes stale. The rules are in
  [crates/changelog/README.md](crates/changelog/README.md).
- Do not leave a `cargo clippy` warning behind; CI runs with `-D warnings`.
- **There are two separate lists and one of them is temporary.** `TODO.md` at
  the root is the real list, in the repo. `tasks/` (`todo.md`, `lessons.md`)
  is gitignored — session working notes, not authority. When you mark a task
  "closed", write it in `TODO.md`.

## Claude tooling that lives in the repo

Some of the rules in this file now enforce themselves under `.claude/`
(rationale: `9f09332`). All of it is in the repo and comes with a clone.

| Tool | When |
|------|------|
| `invariant-guard` (agent) | Any diff that touches the invariants above: scan-core, store, diff, dupes, treemap |
| `downstream-api-guard` (agent) | The public API changed and it is going to be pushed to `main` — CI here does not see desktop or hub |
| `code-reviewer` (agent) | Ordinary review, before a commit |
| `test-writer` (agent) | A new test; it reads the repo's style and matches it |
| `changelog-entry` (skill) | A user-visible change: an entry in five locales, then generating `CHANGELOG.md` |
| `release` (skill) | Cutting a release; the full sequence is [docs/RELEASING.md](docs/RELEASING.md) |
| `preflight` (skill) | Everything CI runs, before a push, cheapest first |

**Skills can trigger themselves** (`disable-model-invocation` was removed from
all of them on 16 September 2026): `preflight` before a push and `release`
while a version is being cut run on their own, without waiting for the user to
type `/preflight`. The irreversible steps of `release` (commit, tag, push) are
gated on approval in the skill's body.

**Tools that plug into all five repos live separately.** The
`spacetrace-tools` plugin (`spacetrace-tooling`, next to this repo, its own
private repository) provides all of them under the `spacetrace-tools:` prefix.
The ones relevant here: `doc-drift-auditor` (what a diff turned false in the
docs) and `workspace-audit` (the same thing across all five repos).
`code-reviewer` and `test-writer` have plugin versions too, but **the agents
with the same names here are sharper and stay in place** — the prefix prevents
the collision.

**The full list is not kept here**, it is in the plugin's README; keeping an
inventory in four places produces exactly the drift this file exists to hunt.
The plugin is a private repository, so a clone cannot see it — someone who
cannot install it cannot use the list either, nothing is lost.
The plugin's `CHANGELOG.md` hook is deliberately silent in this repo — the
local hook above is already there and explains the rationale in its own words.

**The vendored `web-design-guidelines` skill is not here**, it is in the
website repo — `4d90a05` took it out together with the site, because it has no
business in a repo that has no website. That commit left behind the symlink
under `.claude/skills/` and `skills-lock.json` at the root; with no symlink
target it was **broken and the skill never loaded at all**. Both were deleted
on 16 September 2026.

Two hooks are active via `.claude/settings.json`: Edit/Write to `CHANGELOG.md`
is blocked (it is a generated file; Bash redirection is deliberately allowed,
the release procedure uses it), and at the end of a session, if `crates/*/src`
changed but `changelog.json` did not, you are asked once per session.

**`.rs` formatting now comes from the plugin**, not from this repo.
`.claude/hooks/rustfmt-on-edit.sh` is still there but nothing references it:
the plugin's hook does the same job in every repo and backs off silently if
`rustfmt` is not installed — the reason for keeping the local copy opt-in (not
requiring a clone to have rustfmt installed) is already met on the plugin
side. The wiring in `settings.local.json` was removed on 16 September 2026,
because both of them ran in sequence on every edit.

## Crates

| Crate | Responsibility |
|-------|----------------|
| `scan-core` | Scanning, the tree model, platform-specific metadata. Depends on nothing. |
| `store` | SQLite snapshot store, ncdu and CSV export, hash cache |
| `diff` | Comparing two snapshots, spotting the "culprit folder" |
| `dupes` | Files with identical content: size → pre-hash → blake3, cache trait |
| `cli` | The `spacetrace` binary (including remote sources) |
| `agent` | The `spacetrace-agent` binary: scheduler + HTTP service |
| `treemap` | Squarified layout + LOD + hierarchical hit-testing (used by the desktop) |
| `changelog` | The changelog of the three components, in five locales; the generator is the same crate's binary |
| `buildinfo` | Stamps the binary with the commit, build date and channel (`build.rs`) |

The dependency direction is one-way. The agent depends on `scan-core`, `store`
and `buildinfo`; it does not depend on `cli` or `diff`.

**`store`'s `dupes` feature is off by default** (`dupes = ["dep:spacetrace-dupes"]`).
The duplicate finder's hash cache pulls in BLAKE3, and the agent is built from
this crate too — it is the one that has to stay a single static binary. The
CLI turns it on, nobody else needs it. Ask the same question when a new heavy
dependency enters `store`.

## Repositories

All four phases are working. Per K2 the code lives in three repositories:

| Repository | Contents | Visibility |
|------|--------|------------|
| this repo | all nine crates above | public, Apache-2.0 |
| [spacetrace-desktop](https://github.com/unalcakir28/spacetrace-desktop) | Tauri v2 + React desktop | private, commercial |
| [spacetrace-hub](https://github.com/unalcakir28/spacetrace-hub) | Fleet dashboard, trends, alerts | private, commercial |

The other two use this repo as a **git dependency**, not a path one. So a
change that breaks the public API here breaks them silently; think about that
before you push to `main`.

## Releases and the website

The full account is [docs/RELEASING.md](docs/RELEASING.md); the parts that
break easily:

- **The downloadable files of all three components are in this repo's
  releases too.** The desktop and the hub build in their own CI and publish
  here (with the `RELEASE_TOKEN` secret — a `GITHUB_TOKEN` in one repo cannot
  write to another, even when the repo is public). Tags: `continuous` / `v*`
  (CLI), `desktop-continuous` / `desktop-v*`, `hub-continuous` / `hub-v*`.
- **Tag and asset names are a fixed contract, and the other side is now in a
  separate repo.** `src/data/releases.ts` in the website repo binds to them
  directly, and `install.sh` builds the file name from the version it is
  given. Renaming them breaks the download page silently — and because they
  are two separate repos, no single CI step catches it; it takes two commits
  on the same day.
- **Container images are built from pre-compiled musl binaries**
  (`.github/docker/Dockerfile.release`), not from the `Dockerfile` at the
  root. Compiling Rust under QEMU makes the arm64 image take tens of minutes
  instead of minutes. The root Dockerfile is still there so that
  `docker build .` works in a clone.
- **musl targets are built with `cross` and `Cross.toml` is mandatory.** The
  version stamp (`SPACETRACE_GIT_SHA`, `SPACETRACE_BUILD_DATE`,
  `SPACETRACE_CHANNEL`) only reaches the container through the passthrough
  list there; if the list is incomplete the binary is built silently
  **without a stamp** and `--version` cannot say so.
- There are three install scripts: `install.sh` (CLI), `install-desktop.sh`
  and `install-desktop.ps1`. All three read asset names from the same
  contract.
- **The website is not in this repo.**
  [unalcakir28/spacetrace-website](https://github.com/unalcakir28/spacetrace-website)
  — Astro, five locales, `spacetrace.teknobakkall.com`. How it works is in
  that repo's `CLAUDE.md`; the only thing you need to know here is the
  download contract above. The rationale for the split is docs/RELEASING.md →
  Site.
- The release workflow does not run on documentation changes
  (`paths-ignore`).

## What is next

The remaining work needs real hardware or real time (full list in TODO.md):
installing the agent on real servers and a week of data, the Windows MFT fast
path, macOS Full Disk Access onboarding, treemap performance on WebKitGTK.

The decisions between phases are closed
([docs/DECISIONS.md](docs/DECISIONS.md)); read the rationale there before
reopening one.

Things to watch out for in the code:

- A snapshot on the wire is **raw SQLite**. `Store::export_snapshot` copies a
  single scan into a separate file with ATTACH; `import_snapshot` reassigns
  the id but preserves the host/root/`started_at` triple — the duplicate check
  relies on that triple.
- **A tree that comes from outside goes through `Tree::from_nested`.** The
  ncdu import (and every format that comes after it) does not build its own
  arena layout: that path calls the walk's `TreeBuilder` + `aggregate`,
  otherwise a second implementation of invariant 2 and of aggregation is born.
  **A directory's own `asize` is discarded** (invariant 1) — real ncdu writes
  it, our exporter does not, so a round-trip test against our own output
  cannot see this bug.
  **And run the test through the store.** The root's parent must be
  `NO_PARENT`; writing `0` produces a tree that looks flawless in memory but
  fails the `save` → `load` round trip with `RootHasParent` and sends
  `remove_subtree` into an infinite loop. A test that compares two trees in
  memory cannot see this — it did not see it on 11 September 2026 and a broken
  `import` shipped.
- **`store::load` is a trust boundary.** A snapshot downloaded from a remote
  goes through this path too, which is why `TreeAssembler::finish` validates
  the arena invariants (`Tree::check`).
  Do not add a path that skips validation: a corrupt `children_start` means an
  index panic, a backward child pointer means an infinite loop.
- **Listing on macOS goes through two paths, and both have to give the same
  number.** `bulk.rs` gets the names and the metadata in a single call with
  `getattrlistbulk` (measured: 2,3–2,4× end to end); the `read_dir` + `lstat`
  path is both the fallback and the only path for directories that contain
  mounts (D1's per-entry protection does not work in the bulk call). **If a
  second metadata source silently diverges** it surfaces years later as "the
  snapshot is corrupt" — `assert_same_answer_as_lstat` compares the two paths
  field by field, so look there too when you add a new field. Directory
  `nlink` especially: `ATTR_DIR_LINKCOUNT` is 1 on APFS, `st_nlink` is
  2+subdirectories, and one `lstat` per directory is paid to reconcile them
  (measured: it costs nothing).
- **Structural validation is not value validation, and the second one is
  `content_hash`.** A bit flip in a `size` field leaves a flawless tree and
  reports the wrong number — `Tree::check` cannot see it. Since schema v3
  every scan carries the SHA-256 of its own logical content
  (`crates/store/src/digest.rs`); if `import_snapshot` recomputes it from the
  incoming rows and it does not hold, it imports **nothing**,
  `export_snapshot` does not send data it knows to be corrupt, and
  `spacetrace verify` checks it when asked. `NULL` = "no digest" (a pre-v3
  snapshot), not "corrupt".
  **It is not authentication:** whoever can change the body can recompute the
  digest too. The threat model is corruption, not an attacker.
- **Every column added to the `scans` table goes at the end of both
  `create_tables` and `migrate_from`, in the same order.** `export_snapshot`
  does `INSERT INTO snap.scans SELECT * FROM main.scans` and `ALTER TABLE`
  can only append at the end; if the orders diverge the copy writes every
  value into the wrong column and says nothing. The test is in
  `crates/store/tests/integrity.rs`.
- **Paths are built on the way down, not by walking up (D6).** `rel_path` is
  right for a single entry and wrong for every entry: its cost is not the size
  of the tree but **the sum of the depths**, and this crate does not decide
  the depth — a snapshot from another machine can be as deep as it likes, so
  looping over it is quadratic on an input the tool did not choose. For many
  entries, `Tree::for_each_path`: descending appends a segment to a buffer,
  ascending truncates it, no allocation per node. Measured: on 412.983 entries
  61 ms → 4,5 ms; CSV export 449 → 400 ms, and the ncdu export, which builds
  no paths, 182 → 183 (the control). It uses a stack rather than recursion,
  for the same reason as `from_nested` — the depth comes from the file; its
  test descends 50.000 levels.
- **CSV row order is strict DFS**: a folder, immediately followed by its
  contents. It used to keep the siblings together with their contents pages
  further down; that changed with D6 and it is written in the changelog.
- A root is scanned only once at a time (`Runner::try_claim`, 409 over HTTP).
- The scheduler works with UTC + a fixed offset; there is no time zone
  database.
- Capacity is reported as **free/total**, not as "% full" (K6). It is exposed
  outside the module as `capacity_of` (not `capacity::of`).
- **`ScanProgress` is not just counters, it is the cancellation switch too.**
  The walk checks it once per directory — checking per entry would put a
  shared atomic read in the hottest loop, and abandoning a directory that has
  already been read gains nothing.

## Known gaps

If you run into these, they are not bugs but known debt (full list in
TODO.md):

- `alloc` and hardlink dedupe on Windows are **written, but verified only in
  CI** (9 September 2026). **One handle** is opened per entry
  (`std::fs::OpenOptions`, `FILE_READ_ATTRIBUTES` + `BACKUP_SEMANTICS` +
  `OPEN_REPARSE_POINT`) and over that handle `FILE_STANDARD_INFO` →
  `AllocationSize` and `BY_HANDLE_FILE_INFORMATION` → nlink + file id + volume
  are read. **Do not try to use `GetCompressedFileSizeW`** — on files that are
  neither compressed nor sparse it returns the logical size; CI said 100.001
  for a 100.001-byte file. The cost: an extra call per entry, measured on the
  order of +36%; what removes it is B4 (`NtQueryDirectoryFileEx`).
  `FileIdentity::Skipped` skips the second query, not the handle.
- **On btrfs/ZFS** the tree walk reports real usage wrongly because of
  reflinks and compression. **APFS clones are deduplicated** (A3,
  9 September 2026, `fcntl(F_LOG2PHYS_EXT)`, on by default) — this line said
  the opposite until 11 September and contradicted invariant 1 above; both are
  in the same file.
- The scan holds the whole tree in memory. The tree itself is **96
  bytes/entry**, linear between 100k and 10M and the same on both platforms
  (10M = 916 MiB).

  **Double storage went away on 14 September 2026 (B1-K)** and the measurement
  was redone that day: `/Applications` peak 91,5 → 57,6 MB, and with the hint
  given 221 → **125 bytes/entry**; on Linux the "non-tree" part halved. The
  measurement tool is in the repo:
  `cargo run --release -p spacetrace-scan-core --example memprobe -- scan <root>`
  and `scripts/bench-walk.sh` for comparison.

  **The remaining difference is platform-dependent and still accumulates on
  macOS.** On Linux the peak is ~1,5× the tree and almost flat across repeated
  scans; on macOS libmalloc does not give fragmented spans back, so the peak
  grows with allocation traffic — and the traffic depends on the corpus:
  `/Applications` +7 MiB over 8 scans, `~/github` (the same entry count, three
  times the name bytes) 205 → 445 MiB over 8 scans. Not a leak;
  `malloc_zone_pressure_relief` changes nothing. The agent is unaffected
  (Linux). "Rescan" in the desktop is affected, but it now passes the
  `expected_entries` hint. The full measurement is TODO.md D4.
- The agent has no built-in TLS; a reverse proxy is recommended. **Rate
  limiting exists** (14 September 2026, `ratelimit.rs`): a token bucket per
  client address, **before** auth — `/health` takes no token and a wrong token
  costs a response too, so the traffic that needs limiting is exactly what
  falls outside the token. Behind a reverse proxy every request comes from the
  proxy's address, so it collapses into a single shared limit;
  `X-Forwarded-For` is deliberately not read.

## Context outside this repo

The project started in Cowork (claude.ai); the session memory there is not
carried over into Claude Code, the two systems are separate. Everything that
needed carrying over was written into the documents in this repo. The summary
of the September 2026 market and technical research is in
[docs/RESEARCH.md](docs/RESEARCH.md).
