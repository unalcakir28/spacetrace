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

- `--threads N` chooses how wide a scan runs, and the agent takes the same setting per root. There is no best number: the optimum moves with the size of the tree, and someone who knows their disk will choose better than any built-in default.
- When a scan stops making progress, the progress line now says so and names the directory it is waiting on, instead of going on claiming to be scanning. A network share that has stopped answering blocks in the kernel and no timeout in this program can lift that — but knowing what it is waiting on is what lets you decide whether to wait or quit.
- `spacetrace import` reads an ncdu or gdu JSON export and stores it as a snapshot, so scans you already have become something to compare against — including scans of a machine that no longer exists to be rescanned. A directory's own apparent size is dropped on the way in: counting it is what makes `du --apparent-size` disagree with the figure this tool reports.
- `spacetrace export --format csv` writes one row per entry for a spreadsheet, with `--depth` to stop at the top few levels of a tree no spreadsheet would open whole. Both measures are columns rather than a setting — what files claim and what the disk holds — alongside each entry's own cost, so a total can be rebuilt without counting a file once for every folder above it. Commas, quotes and newlines in names are quoted properly; they are legal in a filename and a naive export turns them into silently wrong rows.

### Changed

- The agent's `/status` now says whether a running scan is working or wedged. `scanning` carries one object per scan instead of a bare path: live counters, the phase (`walking` or `finishing` — after the walk only the clone probe moves, so counting files alone reads a healthy scan as a stuck one), and, once every counter has stood still, how long for and which directories it is waiting on. The stall is timed by a watcher inside the agent, so polling `/status` rarely does not inflate it.

### Performance

- Scans now use at most eight threads instead of one per core, which was faster on every directory tree measured — 39% on a small one, 11% on a large one. A walk is syscall-bound: past a point the threads queue in the kernel rather than work. `--threads N` overrides it.
- Scans on macOS are about 2.3 times faster. The old walk asked the kernel for a directory's names and then asked again, once per entry, for each entry's size and dates; macOS can answer both in a single call, and now does. Measured end to end on two real trees: 1293 ms to 556 ms on one, 1554 ms to 646 ms on the other, with both versions reporting byte-for-byte the same totals.

### Fixed

- The progress display appeared to freeze near the end of every scan. After the walk there is a second pass looking for copy-on-write clones, and it moved no counter at all — on `~/github` that was 1193 of 1989 milliseconds. That phase now says what it is and counts what it checks.
- A network share whose server has gone away no longer wedges the whole scan. The first lookup into a mounted filesystem cannot be interrupted once it hangs, and because a directory is listed on one thread, one dead share used to take every one of its siblings with it. Scans now read the mount table first, approach a mount point through a thread they are willing to abandon, and after 60 seconds record it as an unreadable path and carry on — so the result is short by that one filesystem and says so, instead of never arriving. `--mount-timeout 0` restores the old behaviour; measured cost of the check is about a millisecond on a 300,000-entry tree.

## 0.5.0 — 2026-09-10

### Added

- Snapshots now carry a checksum of their content, and `spacetrace verify` checks it. A snapshot that changed on the way here is refused on import rather than believed: a flipped bit leaves a perfectly valid tree that reports a wrong number, which is precisely the failure nothing could see before.

### Changed

- The snapshot database moves to a new schema so the checksum has somewhere to live. Older builds will no longer open a database this one has written — they say so plainly rather than misreading it. If you use the desktop app on the same history, update it too.

## 0.4.1 — 2026-09-09

### Fixed

- `install.sh` and `spacetrace update` find the right release again. All three components publish into one repository, so GitHub's idea of the latest release briefly meant the hub's — which `install.sh` could not install, and which the update check read as no version at all.

## 0.4.0 — 2026-09-09

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
