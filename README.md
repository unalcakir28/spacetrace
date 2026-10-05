# spacetrace

Scan disk usage, keep snapshots, and see **what grew**.

**[spacetrace.teknobakkall.com](https://spacetrace.teknobakkall.com/)**
— what it is, [downloads and install
instructions](https://spacetrace.teknobakkall.com/download/), and the
[usage guide](https://spacetrace.teknobakkall.com/guide/).

Most disk analysers (TreeSize, WizTree, DaisyDisk, ncdu) answer one question:
*"what is on the disk in front of me right now?"*. spacetrace answers the second
one too: *"what changed since last week, and on which machine?"* — from an
open-source agent that runs on your own servers, with nothing leaving them.

The same binary runs on your desktop, on a server, on a NAS and inside a
container.

## Status

| Phase | Scope | State |
|-------|-------|-------|
| 1 | Scanner, SQLite snapshots, diff, CLI | ✅ working |
| 2 | Agent (`serve` / `push`), remote sources, Docker image | ✅ working |
| 3 | Tauri desktop: treemap, remote browser, diff view | ✅ working ([separate repo](https://github.com/unalcakir28/spacetrace-desktop)) |
| 4 | Hub: fleet dashboard, growth trends, fill-up forecasts, alerts | ✅ working ([separate repo](https://github.com/unalcakir28/spacetrace-hub)) |

The desktop app and the hub live in their own repositories because they are the
commercial part; the scanning core, snapshot store, CLI and agent here are and
stay Apache-2.0. The agent runs on your servers, so you should be able to read
it. See [docs/DECISIONS.md](docs/DECISIONS.md) K2.

## Documentation

| Document | Contents |
|----------|----------|
| [docs/WHY.md](docs/WHY.md) | Why this project exists: the gap it fills, target users, explicit non-goals |
| [docs/ROADMAP.md](docs/ROADMAP.md) | Phases, exit criteria, release targets |
| [docs/AGENT.md](docs/AGENT.md) | Running the agent: install, configure, the HTTP API, Prometheus metrics, security notes |
| [docs/SSH.md](docs/SSH.md) | Scanning a machine over ssh with nothing installed: what runs where, cleanup, trade-offs |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | How the code is built and why, technology decisions, known limits |
| [docs/DECISIONS.md](docs/DECISIONS.md) | Settled cross-cutting decisions and their rationale |
| [docs/RESEARCH.md](docs/RESEARCH.md) | September 2026 market and technical research summary |
| [docs/RELEASING.md](docs/RELEASING.md) | How the three components are built, published and downloaded |
| [TODO.md](TODO.md) | Live task list |

Everything in this repository is in English, the documents that record
*reasoning* (WHY, ROADMAP, TODO, DECISIONS, RESEARCH) included.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/unalcakir28/spacetrace/main/install.sh | sh
```

Installs `spacetrace` and `spacetrace-agent` into `/usr/local/bin`. Deliberately
POSIX `sh`, because it also has to run on NAS firmware whose shell is busybox.
`SPACETRACE_BIN_DIR` and `SPACETRACE_VERSION` override where and what.

Prebuilt archives for Linux (musl, x86_64 and aarch64), macOS (both
architectures) and Windows are on the
[download page](https://spacetrace.teknobakkall.com/download/) and in
[releases](https://github.com/unalcakir28/spacetrace/releases). One channel:

| Channel | Tag | What it is |
|---------|-----|------------|
| stable | `v*` | A tagged release |

A `v*` tag is the only thing that builds anything; a push to `main` publishes
nothing. There used to be a `continuous` channel rebuilt from `main` on every
push, removed on 19 September 2026.

The agent's container image is `ghcr.io/unalcakir28/spacetrace` (amd64 and
arm64).

### From source

Requires Rust 1.85+ ([rustup.rs](https://rustup.rs)):

```bash
git clone https://github.com/unalcakir28/spacetrace && cd spacetrace
cargo build --release
./target/release/spacetrace --help
```

To put the binary on your PATH: `cargo install --path crates/cli`

## Usage

```bash
# Scan and print a summary
spacetrace scan ~/projects

# Scan and store the result as a snapshot
spacetrace scan /srv --save --label "weekly"

# List stored snapshots
spacetrace scans

# Compare the last two snapshots: what grew?
spacetrace diff --path /srv

# Compare the latest snapshot against the disk right now
spacetrace diff --since-last /srv

# List folders by size
spacetrace ls /var/lib --top 20
spacetrace ls --scan 3 --subpath docker/overlay2

# Open in ncdu (inspect a snapshot pulled off a server)
spacetrace export --scan 3 --out scan.json && ncdu -f scan.json

# Bring an existing ncdu or gdu export in, including one of a machine that
# no longer exists to be rescanned
ncdu -o old.json /var && spacetrace import old.json --host retired-nas

# A spreadsheet for someone who does not have this installed
spacetrace export --scan 3 --format csv --depth 3 --out report.csv

# What nobody has touched — the other question a full disk raises
spacetrace age /srv --bands 30,365,1095

# The same bytes twice: what deleting the extras would give back. Reads a
# fraction of the tree — length rules most of it out for free, a 16 KiB prefix
# rules out most of the rest, and whole-file hashes are remembered between runs
spacetrace dupes ~/Downloads --min-size 10M

# Check stored snapshots against the digest saved with them
spacetrace verify

# Which installed package the bytes belong to, and what belongs to none —
# usually the part somebody put there by hand
spacetrace pkgs /usr
spacetrace pkgs /usr/bin/python3     # one file: which package owns it
```

### Packages

`pkgs` reads the package databases straight off the disk: dpkg
(`/var/lib/dpkg/info`), pacman (`/var/lib/pacman/local`), apk
(`/lib/apk/db/installed`) and Homebrew, which owns by position (`Cellar/<formula>`,
`Caskroom/<cask>`, and the links in `bin/` and `opt/` that point there). rpm's
database is binary, so `rpm` itself is asked; where it is missing the report
says the database was not read instead of calling its files unowned. On macOS
it also reads the installer receipts `pkgutil` reads (`/var/db/receipts`,
`/Library/Apple/System/Library/Receipts`, `~/Library/Receipts`): everything a
`.pkg` installed — Apple's own, the Command Line Tools, Office, Node.js — with
the same file list `pkgutil --files` gives. Several can be present at once,
Homebrew next to dpkg or next to the receipts for instance.

- **Merged /usr is handled.** A list that says `/bin/ls` is matched against
  `/usr/bin/ls`, where the file actually is. Matching the strings as written
  calls 17% of a Debian 12 `/usr` unowned, `/bin` included.
- **Unowned is what no list names.** That is mostly what somebody installed
  by hand (`/usr/local`, `pip install`, a tarball in `/opt`), but files a
  package's install script generates show up too: Python bytecode, font and
  icon caches, `locale-archive`, busybox's applet links.
- **macOS spells some folders twice.** `/System/Volumes/Data/Applications` is
  `/Applications` through a firmlink, and both give the same answer. On a
  volume that ignores case, as macOS formats them, a name an updater
  re-capitalised still matches its receipt.
- A file two packages both list is counted once, for the first by name, and
  the report says how much was shared. The single-file form lists every
  package that claims it.
- It works on this machine only. A snapshot from another host (`--scan` of a
  pulled one, or `--remote`) is refused: this machine's databases say nothing
  about that machine's files.

### Another machine

Point any read-only command at an agent with `--remote`:

```bash
export SPACETRACE_TOKEN=...

spacetrace --remote https://nas.example.com scans
spacetrace --remote https://nas.example.com diff --path /var
spacetrace --remote https://nas.example.com pull --root /var   # keep a local copy
```

A remote snapshot is downloaded as the same standalone SQLite file the agent
stores, so listing, browsing and diffing it run the identical code as a local
one. See [docs/AGENT.md](docs/AGENT.md) to set the agent up.

No agent there? Scan it over ssh, with nothing installed:

```bash
spacetrace scan --save --ssh admin@nas.lan /volume1
```

A copy of this version is uploaded into a private temporary directory, scans,
sends its snapshot back into your local database, and is removed — also on
failure, on Ctrl-C, and when the CLI is killed outright. The cost: ssh access,
a ~7 MB upload per run, and room in the remote `/tmp` (or `$HOME`) for the
snapshot. Details and limits: [docs/SSH.md](docs/SSH.md).

Every snapshot carries a checksum of its contents, and one that changed on the
way here is refused rather than believed — a flipped bit leaves a structurally
valid tree that reports a wrong number, which is the one kind of damage
nothing else would catch. It is a corruption check, not a signature: whoever
can change the body can recompute the checksum.

Example output:

```
#1 2026-09-06 17:23 [weekly]  →  #2 2026-09-06 19:40
total 23.8 MiB → 76.3 MiB   (+52.5 MiB)

      CHANGE  STATUS          NEW  PATH
   +40.1 MiB  grew       42.9 MiB  app/logs/
   +14.3 MiB  grew       25.7 MiB  backups/
    -1.9 MiB  removed         0 B  uploads/
```

Note what the report says: **`app/logs/`**, not `app/` or `/srv`. Intermediate
folders that merely pass the change through are skipped; the first level where
the change genuinely spreads out is the one reported.

### An S3 bucket

`scan` also takes an S3 bucket — AWS, or any S3-compatible service (MinIO,
Cloudflare R2, Backblaze B2, Wasabi) through `--endpoint`:

```bash
spacetrace scan s3://my-bucket/backups --save                  # AWS
spacetrace scan s3://media --endpoint http://127.0.0.1:9000 --save
spacetrace scan s3://open-data --no-sign-request               # a public bucket

spacetrace diff --path s3://my-bucket/backups                  # what grew
```

The saved snapshot is an ordinary one: `ls --scan`, `diff`, `age --scan` and
`export` work on it unchanged. Credentials and region are found the way the
`aws` CLI finds them — `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`/
`AWS_SESSION_TOKEN`, then `~/.aws/credentials` and `~/.aws/config` with
`AWS_PROFILE` or `--profile`, region from `--region`, `AWS_REGION`, the profile,
then `us-east-1` (a wrong guess is corrected once, from AWS' own answer). Only
static keys are read; for SSO or an assumed role, export keys first with
`aws configure export-credentials`.

- Keys are split on `/` into folders, and a key ending in `/` (a "folder
  marker") is drawn as its folder. The path after the bucket is a folder:
  `s3://b/photos` lists `photos/…`, not `photos2024/…`. A key with a `.` or
  `..` segment is not drawn — as a name it would read as navigation — and is
  reported with its bytes, as a disk scan reports an unreadable path.
- An object has no blocks, so `size` and `alloc` are both its length. The
  on-disk total is every byte listed — the figure `mc du` prints. Bytes the tree can only charge to a folder (a
  marker holding data, an object `a` beside a folder `a/`) count there and stay
  out of the logical total, and the summary says when that happened.
- **Current versions only.** Old versions, delete markers and unfinished
  multipart uploads are not in a listing and are not counted — but they are
  billed, so the total is not the bill.
- The snapshot's host is the service (`s3.amazonaws.com`, or the endpoint's
  `host:port`), not this machine: two machines listing one bucket see one target,
  and `diff --path s3://b` compares two snapshots from the same service — one
  bucket name on MinIO and on AWS is two buckets.

### Watching it happen

When the disk is filling *now*, `watch` answers where, live:

```bash
spacetrace watch ~ --exclude node_modules --interval 1
```

It scans once, then shows which folders grew or shrank since it started, how
fast, biggest first — the same rows `diff` would report, refreshed in place on
a terminal, appended as lines into a pipe, one JSON object per refresh with
`--json`. Filesystem events (FSEvents, inotify, ReadDirectoryChangesW) only say
*where* to look; every number comes from listing that folder again with the
scanner, so hardlinks, symlinks and `--exclude`/`-x`/`--depth` mean exactly
what they mean for `scan`. A hardlinked file is counted once across listings
too: the watch remembers which folders hold a name of it, so a build that
links its output (cargo's `target/`, pnpm) is followed folder by folder. Any
events the system reports as dropped, and a hardlink whose other names no
listing has met, trigger a full rescan, capped at a tenth of the time, and the
screen says so; a cheap full rescan also runs every minute or so to catch
losses nobody reported. Clones are counted at their full size
(`--no-clone-dedupe`). The `filesystem … free` line is the filesystem's own
count, to compare against.

On Linux each watched folder takes one inotify watch. If
`fs.inotify.max_user_watches` is too low, `watch` stops and says so, with the
`sysctl` to raise it; `--exclude` and `--depth` also reduce how many it needs.

### Common options

| Option | What it does |
|--------|--------------|
| `--exclude node_modules` | Never descend into folders with that name (repeatable) |
| `-x`, `--one-file-system` | Do not cross mount points (like `du -x`) |
| `--depth N` | Do not descend below N levels |
| `--min 10M` | Ignore changes smaller than this in a diff |
| `--files` | Report files in a diff, not just folders |
| `--threads N` | Walk with N threads (default: `min(cores, 6)`, measured) |
| `--mount-timeout S` | Give a mounted filesystem S seconds to answer before recording it as unreadable and moving on (default 60; `0` waits forever) |
| `--json` | Emit JSON (available on every command) |
| `--db path.sqlite` | Use a different snapshot database |

Default database: `~/Library/Application Support/spacetrace/` on macOS,
`$XDG_DATA_HOME/spacetrace/` on Linux. Override it with `SPACETRACE_HOME`.

## Architecture

```
crates/
├── scan-core/   Parallel scanner + arena tree model (platform-specific backends)
├── store/       SQLite snapshot store + ncdu import/export + CSV export
├── diff/        Snapshot comparison, "culprit folder" detection
├── dupes/       Identical contents: size → prefix → BLAKE3, with a cache trait
├── cli/         the spacetrace binary
├── agent/       the spacetrace-agent binary: scheduler + HTTP service
├── treemap/     squarified layout with level-of-detail and cushion surfaces, for the desktop app
├── changelog/   the one changelog source, in five locales, and its generator
└── buildinfo/   commit, channel and build time, stamped in at compile time
```

The tree is stored as an **arena** whose children occupy a contiguous index
range: no `Vec` per node, aggregation finishes in a single reverse pass, and the
layout is cache-friendly for treemap rendering. That same layout is written to
SQLite as-is, so loading a snapshot is one ordered query — the tree is never
rebuilt.

The desktop app and the agent use the **same core**; the agent ships as a single
static binary built from `scan-core` + `store`.

### Size semantics

- **logical (`size`)**: file bytes only. Matches `du -sb` exactly.
- **on disk (`alloc`)**: blocks actually allocated, directory blocks included.
  Matches `du -s --block-size=1` exactly, except where blocks are shared: APFS
  clones and btrfs/XFS reflinks and snapshots are charged once, so `alloc`
  follows `df` and `du` over-counts. Run as root on btrfs, compressed files are
  charged at their compressed size. `--no-clone-dedupe` gives back `du`'s number.
- **filesystem capacity**: reported as *free of total*, which matches `df`'s
  Avail column exactly. Deliberately not "% used": on a filesystem whose space
  is shared between volumes (APFS containers, btrfs subvolumes, thin LVM) the
  used figure would include the siblings and disagree with `df` on the same
  mount.
- Hardlinks are counted once by default; disable with `--no-dedupe`. Symlinks are
  never followed and are counted at their own size.
- An S3 object has no blocks: both sizes are its length (see
  [An S3 bucket](#an-s3-bucket)).

Verified: on `/usr` (141k files), `/usr/share` and `/etc`, both totals match `du`
**exactly**. This is a test condition, not an aspiration.

## Development

```bash
cargo test --workspace     # the whole suite, on a real filesystem
cargo clippy --workspace --all-targets
cargo fmt --all
```

Tests use a **real filesystem** in temporary directories — hardlink, symlink,
permission-error and depth-limit scenarios included. There are no mocks.

## License

Apache-2.0. The scanning core, the snapshot store, the CLI and the agent are and
will stay open source — the agent runs on your servers, so you should be able to
read it. See [docs/DECISIONS.md](docs/DECISIONS.md) for the licensing rationale.
