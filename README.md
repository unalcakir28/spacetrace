# spacetrace

Scan disk usage, keep snapshots, and see **what grew**.

Every disk analyser on the market (TreeSize, WizTree, DaisyDisk, FreeSize, ncdu)
answers one question: *"what is on the disk in front of me right now?"*.
spacetrace answers the second one too: *"what changed since last week, and on
which machine?"*

The same binary runs on your desktop, on a server, on a NAS and inside a
container.

## Status

| Phase | Scope | State |
|-------|-------|-------|
| 1 | Scanner, SQLite snapshots, diff, CLI | ✅ working |
| 2 | Agent (`serve` / `push`), remote sources, Docker image | ✅ working |
| 3 | Tauri desktop: treemap, remote browser, diff view | ⏳ |
| 4 | Central service: multi-machine dashboard, growth alerts | ⏳ |

## Documentation

| Document | Contents |
|----------|----------|
| [docs/WHY.md](docs/WHY.md) | Why this project exists: the gap it fills, target users, explicit non-goals |
| [docs/ROADMAP.md](docs/ROADMAP.md) | Phases, exit criteria, release targets |
| [docs/AGENT.md](docs/AGENT.md) | Running the agent: install, configure, the HTTP API, security notes |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | How the code is built and why, technology decisions, known limits |
| [docs/DECISIONS.md](docs/DECISIONS.md) | Settled cross-cutting decisions and their rationale |
| [docs/RESEARCH.md](docs/RESEARCH.md) | September 2026 market and technical research summary |
| [TODO.md](TODO.md) | Live task list |

Project documents under `docs/` that record *reasoning* (WHY, ROADMAP, TODO,
RESEARCH) are kept in Turkish; everything user-facing — the CLI, this README and
ARCHITECTURE — is English.

## Install

Requires Rust 1.85+ ([rustup.rs](https://rustup.rs)):

```bash
git clone https://github.com/unalcakir/spacetrace && cd spacetrace
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
└── agent/       the spacetrace-agent binary: scheduler + HTTP service
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
- Hardlinks are counted once by default; disable with `--no-dedupe`. Symlinks are
  never followed and are counted at their own size.

Verified: on `/usr` (141k files), `/usr/share` and `/etc`, both totals match `du`
**exactly**. This is a test condition, not an aspiration.

## Development

```bash
cargo test --workspace     # 107 tests
cargo clippy --workspace --all-targets
cargo fmt --all
```

Tests use a **real filesystem** in temporary directories — hardlink, symlink,
permission-error and depth-limit scenarios included. There are no mocks.

## License

Apache-2.0. The scanning core, the snapshot store, the CLI and the agent are and
will stay open source — the agent runs on your servers, so you should be able to
read it. See [docs/DECISIONS.md](docs/DECISIONS.md) for the licensing rationale.
