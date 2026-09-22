<!-- Generated from crates/changelog/changelog.json in unalcakir28/spacetrace.
     Do not edit by hand. From a checkout of that repo:
       cargo run -p spacetrace-changelog -- markdown --component cli > CHANGELOG.md -->

# Changelog

What changed in spacetrace CLI and agent, newest first.

Versions marked *development milestone* were never tagged and have no
downloadable files. They are recorded because the work happened, not
because anyone can install them.

## Unreleased

### Changed

- Clones smaller than 64 KiB are deduplicated too, and the default thread count dropped from 8 to 6. The size floor existed because each clone check cost an open file; it costs nothing now, so a tree of many small clones reports less than it did - closer to what `df` says the disk holds. `clones_deduped` rises accordingly. The thread default was re-measured after the walk got cheaper: 6 is at or near the best on all three test corpora, where 8 now costs 22% on the largest of them. Use `--threads` to override.

### Performance

- macOS scans are 15-33% faster and hold a third less memory. Copy-on-write clones used to be found in a phase of their own after the walk, opening every candidate file one at a time; APFS reports the clone family inside the directory listing the walk was already reading, so that phase is gone. Measured against the previous build, warm cache, interleaved runs: 50,189 entries 94 to 78 ms, 415,503 entries 485 to 412 ms, 1,064,452 entries 2133 to 1431 ms. Peak memory on the largest of those fell from 299 to 200 MiB, because the walk no longer builds a full path for every entry it lists - only for the directories it descends into. Clone deduplication is now free rather than 15-31% of the scan, so `--no-clone-dedupe` no longer buys any speed.

## 0.9.0 — 2026-09-21

### Added

- The agent can now serve its API over HTTPS itself: `tls_cert_file` and `tls_key_file` under `[server]` in the configuration, both or neither. Until now that needed a reverse proxy in front, which on a NAS with no domain name of its own meant putting the token and the file inventory on the network in plaintext. A self-signed certificate is enough; the client is told which one to trust with `ca_file` in `remotes.toml`. Where a domain name exists a reverse proxy is still the better answer, because it renews certificates and the agent does not: a renewed certificate takes a restart. The certificate needs the address clients use as a subject alternative name.

## 0.8.0 — 2026-09-19

### Added

- The agent now limits how many requests one client address may make, answering 429 with a `Retry-After` above the limit. It counts before checking the bearer token, because the two things a token cannot bound are exactly the ones that need it: `/health` needs no token by design, and a wrong token still costs a reply. The default of 120 a minute is far above ordinary use; what it stops is a client stuck in a retry loop spending the disk the agent exists to measure. Behind a reverse proxy every request arrives from the proxy, so the limit becomes one shared allowance — set `rate_limit_per_minute = 0` and limit in the proxy where that matters.

### Changed

- The CSV export now lists each folder immediately followed by what is inside it. It used to write a whole set of siblings together and their contents further down, which put a folder and its files pages apart in the spreadsheet. The columns and the rows themselves are unchanged.
- `spacetrace scan --save` now says what it is doing while it writes the snapshot to the database. That write is not a quick tail on the end of a scan — on a folder of 412,983 entries the walk takes 0.8 seconds and the write another 0.6, and at ten million entries it is about fourteen seconds — and until now the progress line was cleared the moment the walk ended, leaving the command apparently frozen. The agent reports the same two stages on `/status`.

### Fixed

- `spacetrace dupes` listed the same duplicates in a different order every time it ran, so comparing two reports showed differences that were not there. The order now follows the paths, which do not change between runs.
- A directory tree nested past about 210 levels no longer aborts the scan. The walk recurses once per level and its threads had the ordinary default stack, so a deep enough tree overflowed it and the process died outright — in the agent that took the HTTP API and the scheduler with it, and no snapshot was written. The walk threads now get a 16 MiB stack and stop at 1024 levels, recording the directory as unreadable and carrying on; a tree deeper than the operating system can name is reported the same way, with its path and its reason.

## 0.7.0 — 2026-09-14

### Added

- `spacetrace dupes` finds files holding identical contents and says what deleting the extras would give back. It reads as little as it can: files of different lengths cannot match and the scan already knows every length, files that share a length are separated by their first 16 KiB, and only what survives both is read in full. On a 33.8 GiB tree of 375,585 files that came to 2.6 GiB of reading — 7.7% — and a second run read 30 MiB, because whole-file hashes are remembered between runs. Hardlinked names are listed separately and counted as reclaiming nothing, since they already share their bytes. Nothing is deleted; which copy to keep is not a decision this tool has the context to make.

### Performance

- Scanning uses markedly less memory. The walk used to build a tree of its own and copy it into the final one, so both were in memory at the peak; it now writes entries into the final tree as each directory is read. On a 412,983-entry folder the peak fell from 91.5 MB to 57.6 MB, and to 49.3 MB when a previous scan of the same folder is on record to size the structure from. Scanning takes the same time as before, and every figure it reports is unchanged.

## 0.6.1 — 2026-09-11

### Fixed

- A snapshot created by `spacetrace import` could be stored and then never opened again: every attempt failed with "entry 0 is not a root: it claims a parent". The import wrote a malformed root and nothing noticed until the read. Snapshots already imported are readable by this version without being imported again.

## 0.6.0 — 2026-09-11

### Added

- `--threads N` chooses how wide a scan runs, and the agent takes the same setting per root. There is no best number: the optimum moves with the size of the tree, and someone who knows their disk will choose better than any built-in default.
- When a scan stops making progress, the progress line now says so and names the directory it is waiting on, instead of going on claiming to be scanning. A network share that has stopped answering blocks in the kernel and no timeout in this program can lift that — but knowing what it is waiting on is what lets you decide whether to wait or quit.
- `spacetrace import` reads an ncdu or gdu JSON export and stores it as a snapshot, so scans you already have become something to compare against — including scans of a machine that no longer exists to be rescanned. A directory's own apparent size is dropped on the way in: counting it is what makes `du --apparent-size` disagree with the figure this tool reports.
- `spacetrace export --format csv` writes one row per entry for a spreadsheet, with `--depth` to stop at the top few levels of a tree no spreadsheet would open whole. Both measures are columns rather than a setting — what files claim and what the disk holds — alongside each entry's own cost, so a total can be rebuilt without counting a file once for every folder above it. Commas, quotes and newlines in names are quoted properly; they are legal in a filename and a naive export turns them into silently wrong rows.
- `spacetrace age` answers the other question a full disk raises: not what is big, but what nobody has touched. Bytes are grouped by when they were last modified — the bands are yours to set with `--bands` — and weighted by size rather than by file count, because a hundred thousand stale source files are not the answer and one stale disk image is. Files whose modification time was never recorded are reported separately instead of being counted as fifty years old.

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
