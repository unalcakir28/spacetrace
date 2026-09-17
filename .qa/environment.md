# QA environment manifest — spacetrace core

Rebuilt when something here stops being true. Never put a secret in this file;
name where the credential lives instead.

## Target

| | |
|---|---|
| What | Local development checkout, `crates/` workspace at **0.7.0** |
| Machine | macOS (Darwin 27), **APFS**, `/` sealed and read-only |
| Toolchain | cargo 1.98.1, rustc 1.98.1 |
| Production? | **No.** Nothing here touches a production host. Destructive cases are fair game. |

## External dependencies

The core has none worth mocking — that is the point of the design, and it makes
this table short.

| Dependency | State here | Note |
|---|---|---|
| SQLite | **real** | `rusqlite` with `bundled`, no system library |
| HTTP server (agent) | **real** | axum, bound to loopback on a test port |
| Identity provider / mail / payment | **absent** | none exist in this product |
| Remote agent (for `pull`, `push`) | **real, self-hosted** | a second local agent process on another port |

## Oracles — what "correct" is measured against

| Claim | Oracle | Available here |
|---|---|---|
| `alloc` matches `du` | external `du` | **BSD du only.** GNU coreutils is **not installed** — `du -sb` and `--block-size=1` from README do not exist on this machine. `du_equivalence.rs` already handles this; ad-hoc cases must not paste the README's GNU flags. |
| `size` | naive serial walk in `du_equivalence.rs` | yes — deliberately shares no code with the parallel walk |
| capacity free/total | `df` | BSD `df`, `-k` |

## Access

| Surface | Method | Where the credential comes from |
|---|---|---|
| Agent, all routes except `/health` | `Authorization: Bearer <token>` | `agent.toml` — `server.token` or `server.token_file`. Tests generate their own throwaway token per run. |
| `/health` | none, by design | — |

## Fixtures

Every case builds its own tree in a temp directory; there is no shared seed
database to poison. What a case needs and cannot make here:

| Needed | State |
|---|---|
| hardlinks, symlinks, broken symlinks, deep trees, unicode names, sparse files | **creatable** on APFS |
| unreadable directory (permission error path) | creatable, but **not as root** — skip if the run is elevated |
| NTFS / ReFS / MFT | **absent** — Windows only |
| btrfs / ZFS reflinks (A6) | **absent** — Linux only |
| spinning disk, NFS mount (B3) | **absent** |
| a tree over ~1M entries | **absent** — B2's open question needs a real corpus |

## Platform-blocked at design time

These are `BLOCKED (ortam)` before the run starts, not discoveries:
Windows metadata path (`windows_metadata.rs` compiles to zero tests here),
`getdents64`/`statx` (B6), MFT (B4), USN journal (B7), btrfs/ZFS (A6),
HDD/NFS sequential mode (B3), Linux `/proc` mount parsing.
