<!-- Generated from crates/changelog/changelog.json in unalcakir28/spacetrace.
     Do not edit by hand. From a checkout of that repo:
       cargo run -p spacetrace-changelog -- markdown --component cli > CHANGELOG.md -->

# Changelog

What changed in spacetrace CLI and agent, newest first.

Versions marked *development milestone* were never tagged and have no
downloadable files. They are recorded because the work happened, not
because anyone can install them.

## Unreleased

### Added

- The agent reports on `/status` when a newer release exists. A notice only: it never installs anything itself. Once a day, sending nothing about the machine, and `update_check = false` turns it off entirely.
- `spacetrace update` installs the newest release over the running one, after verifying the download against the published `SHA256SUMS`.
- A once-a-day check for a newer release, printed as one line on stderr. It runs only on a tagged build in a terminal, explains itself the first time, and stops entirely with `SPACETRACE_NO_UPDATE_CHECK=1`.
- `--version` now says which build it is: the commit, the date it was built and the channel. The agent reports the same on `/health` and `/status`.
- Every release now comes with written release notes in five languages, and the repository has a CHANGELOG.md.

### Changed

- The promise that on-disk sizes agree with `du` where no blocks are shared is now enforced by a test that runs real `du`, rather than being a claim in the documentation.

### Performance

- Scanning a large tree uses noticeably less memory: a tree node shrank from 104 to 72 bytes and names moved into one shared arena.

### Fixed

- `install.sh` verifies the download against the published `SHA256SUMS`. The file was published with every release and never read.
- APFS clones are counted once. Three 100 MB clones take 0 MB of extra room on the disk, and spacetrace now says so where `du` still reports 400 MB. Use `--no-clone-dedupe` to turn it off.
- On Windows, spacetrace now reads the space a file really occupies and counts a hard-linked file once, instead of falling back to the length the file claims.

## 0.3.0 — 2026-09-08 · *development milestone*

### Added

- A running scan can be cancelled, and a subtree can be dropped from a loaded tree without invalidating the entries around it.
- Sorting and drawing now name their measure explicitly, so a list that says largest first agrees with the number printed beside it.
- Every push leaves a downloadable build behind, and `install.sh` installs it on Linux and macOS.

## 0.2.0 — 2026-09-07 · *development milestone*

### Added

- `spacetrace-agent`: a read-only agent for servers and NAS boxes, with a built-in scheduler and an HTTP API. It never deletes anything.
- Filesystem capacity, reported as free and total rather than a percentage full, because on APFS and thin pools a percentage disagrees with `df`.
- The treemap layout engine: squarified tiles, level of detail, and hit-testing that follows the hierarchy.

### Fixed

- Opening a database no longer takes a write lock, so a read can no longer knock over a scan that is still writing.

## 0.1.0 — 2026-09-06 · *development milestone*

### Added

- Scan a directory and see where the space went, with both measures side by side: what the files claim, and what the disk actually holds.
- Snapshots stored in SQLite, so a scan taken today can be compared with one taken next month.
- `spacetrace diff` names the folder responsible for a change instead of listing every file underneath it.
- Export to the ncdu format, so a spacetrace snapshot can be opened by tools that already exist.
- Symbolic links are not followed, hard links are counted once, and a sparse file reports what it allocates rather than the length it claims.
