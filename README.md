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
| [docs/AGENT.md](docs/AGENT.md) | Running the agent: install, configure, the HTTP API, security notes |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | How the code is built and why, technology decisions, known limits |
| [docs/DECISIONS.md](docs/DECISIONS.md) | Settled cross-cutting decisions and their rationale |
| [docs/RESEARCH.md](docs/RESEARCH.md) | September 2026 market and technical research summary |
| [docs/RELEASING.md](docs/RELEASING.md) | How the three components are built, published and downloaded |
| [TODO.md](TODO.md) | Live task list |

Project documents under `docs/` that record *reasoning* (WHY, ROADMAP, TODO,
RESEARCH) are kept in Turkish; everything user-facing — the CLI, this README and
ARCHITECTURE — is English.

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
[releases](https://github.com/unalcakir28/spacetrace/releases). Two channels:

| Channel | Tag | What it is |
|---------|-----|------------|
| stable | `v*` | A tagged release |
| continuous | `continuous` | The newest `main`, rebuilt on every push. Passed CI and nothing else |

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

# Check stored snapshots against the digest saved with them
spacetrace verify
```

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

### Common options

| Option | What it does |
|--------|--------------|
| `--exclude node_modules` | Never descend into folders with that name (repeatable) |
| `-x`, `--one-file-system` | Do not cross mount points (like `du -x`) |
| `--depth N` | Do not descend below N levels |
| `--min 10M` | Ignore changes smaller than this in a diff |
| `--files` | Report files in a diff, not just folders |
| `--threads N` | Walk with N threads (default: `min(cores, 8)`, measured) |
| `--mount-timeout S` | Give a mounted filesystem S seconds to answer before recording it as unreadable and moving on (default 60; `0` waits forever) |
| `--json` | Emit JSON (available on every command) |
| `--db path.sqlite` | Use a different snapshot database |

Default database: `~/Library/Application Support/spacetrace/` on macOS,
`$XDG_DATA_HOME/spacetrace/` on Linux. Override it with `SPACETRACE_HOME`.

## Architecture

```
crates/
├── scan-core/   Parallel scanner + arena tree model (platform-specific backends)
├── store/       SQLite snapshot store + ncdu-compatible export
├── diff/        Snapshot comparison, "culprit folder" detection
├── cli/         the spacetrace binary
├── agent/       the spacetrace-agent binary: scheduler + HTTP service
└── treemap/     squarified layout with level-of-detail, for the desktop app
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
  Matches `du -s --block-size=1` exactly.
- **filesystem capacity**: reported as *free of total*, which matches `df`'s
  Avail column exactly. Deliberately not "% used": on a filesystem whose space
  is shared between volumes (APFS containers, btrfs subvolumes, thin LVM) the
  used figure would include the siblings and disagree with `df` on the same
  mount.
- Hardlinks are counted once by default; disable with `--no-dedupe`. Symlinks are
  never followed and are counted at their own size.

Verified: on `/usr` (141k files), `/usr/share` and `/etc`, both totals match `du`
**exactly**. This is a test condition, not an aspiration.

## Development

```bash
cargo test --workspace     # 150 tests
cargo clippy --workspace --all-targets
cargo fmt --all
```

Tests use a **real filesystem** in temporary directories — hardlink, symlink,
permission-error and depth-limit scenarios included. There are no mocks.

## License

Apache-2.0. The scanning core, the snapshot store, the CLI and the agent are and
will stay open source — the agent runs on your servers, so you should be able to
read it. See [docs/DECISIONS.md](docs/DECISIONS.md) for the licensing rationale.
