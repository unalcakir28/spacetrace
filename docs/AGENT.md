# The spacetrace agent

`spacetrace-agent` scans configured roots on its own schedule, stores each
result as a snapshot, and serves them over HTTP. It runs on servers, NAS boxes
and inside containers.

**It only ever reads.** The agent has no code path that deletes anything outside
its own snapshot database. That is a deliberate limit rather than a missing
feature: software that runs unattended on someone's server earns trust by not
being able to do damage, not by promising it won't.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/unalcakir28/spacetrace/main/install.sh | sh
```

Or build it: `cargo build --release --bin spacetrace-agent`.

## Configure

```bash
spacetrace-agent init > /etc/spacetrace/agent.toml
head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' > /etc/spacetrace/token
chmod 600 /etc/spacetrace/token
```

```toml
db = "/var/lib/spacetrace/snapshots.sqlite"

# Schedules are matched against UTC plus this offset. The agent ships no
# timezone database, so DST is not handled: pick the offset you want scans to
# happen at, or leave it 0 and think in UTC.
utc_offset_minutes = 0

[server]
listen = "127.0.0.1:7878"
token_file = "/etc/spacetrace/token"

[[roots]]
path = "/var"
schedule = "0 3 * * *"       # 03:00 daily
label = "nightly"
exclude = ["node_modules", ".git"]
one_file_system = true
threads = 4                  # fewer than the default, to leave the box alone
keep = 14                    # snapshots of this root to retain
```

`threads` is per root rather than per agent because it is really a property of
the disk: an NVMe root and a spinning-disk root on the same machine want
different numbers. The default is `min(cores, 8)` — not one per core, which
measured slower on every corpus tried (docs/COMPETITORS.md §1.2).

Then check what it will do before starting it:

```bash
$ spacetrace-agent --config /etc/spacetrace/agent.toml check
config       ok
database     /var/lib/spacetrace/snapshots.sqlite
host         nas
listen       127.0.0.1:7878
token        configured
ad-hoc scans refused

ROOT                          SCHEDULE        NEXT RUN
/var                          0 3 * * *       2026-09-08 03:00

Times are UTC.
```

`check` reads the config, resolves the token and computes the next run for every
root. A schedule that can never fire (`0 0 30 2 *`) reports `never` rather than
failing silently.

### Configuration reference

| Key | Default | Meaning |
|-----|---------|---------|
| `db` | required | Snapshot database; every root lands in this one file |
| `utc_offset_minutes` | `0` | Minutes added to UTC when matching schedules |
| `server.listen` | `127.0.0.1:7878` | Bind address |
| `server.token` / `server.token_file` | — | Bearer token, inline or from a file |
| `server.allow_adhoc_scans` | `false` | Let `POST /scans` name a path outside `[[roots]]` |
| `server.max_upload_bytes` | 512 MiB | Largest snapshot `POST /snapshots` accepts |
| `roots[].path` | required | Absolute path to scan |
| `roots[].schedule` | — | Five-field cron; omit to scan only on request |
| `roots[].label` | — | Free text stored with the snapshot |
| `roots[].exclude` | `[]` | Directory names never descended into |
| `roots[].one_file_system` | `false` | Do not cross mount points (`du -x`) |
| `roots[].depth` | — | Stop descending below this depth |
| `roots[].dedupe_hardlinks` | `true` | Count hardlinked files once |
| `roots[].threads` | `min(cores, 8)` | Threads to walk this root with |
| `roots[].keep` | — | Snapshots of this root to retain; unset keeps all |

The token is resolved in this order: `server.token`, `server.token_file`, then
the `SPACETRACE_TOKEN` environment variable. `serve` refuses to start without
one — an unauthenticated agent hands its whole filesystem inventory to anyone
who can reach the port.

Unknown keys are rejected at startup. A typo that silently does nothing on a
machine nobody watches is worse than a failure to start.

### Cron expressions

Five fields: `minute hour day-of-month month day-of-week`, with `*`, `a-b`,
`*/n`, `a-b/n` and comma-separated lists. Sunday is both `0` and `7`. As in
every other cron, when *both* day-of-month and day-of-week are restricted a day
matches if *either* does.

There are no seconds and no `@daily` aliases.

## Run

```bash
systemctl enable --now spacetrace-agent      # deploy/systemd/spacetrace-agent.service
```

The bundled unit runs the agent as its own unprivileged user with
`ProtectSystem=strict`, so it can read the filesystem and write nothing but
`/var/lib/spacetrace`. It also runs at `Nice=10` with idle I/O priority: a scan
should never get in the way of what the machine is actually for.

In Docker, mount the host read-only and scan that:

```bash
docker run -d --name spacetrace \
  -v /:/host:ro \
  -v spacetrace-data:/var/lib/spacetrace \
  -v /etc/spacetrace:/etc/spacetrace:ro \
  -p 7878:7878 \
  ghcr.io/unalcakir28/spacetrace
```

## Use it from the CLI

```bash
export SPACETRACE_TOKEN=...

spacetrace --remote https://nas.example.com scans
spacetrace --remote https://nas.example.com diff --path /var
spacetrace --remote https://nas.example.com ls --top 20
spacetrace --remote https://nas.example.com pull --root /var   # keep a local copy
```

Remotes can be saved so the URL and token do not have to be typed. In
`~/.config/spacetrace/remotes.toml` (macOS: `~/Library/Application Support/spacetrace/`):

```toml
[remotes.nas]
url = "https://nas.example.com"
token = "..."
```

Then `spacetrace --remote nas diff --path /var`.

`scan`, `prune` and `rm` refuse to run with `--remote`: they act on local state,
and the agent deletes nothing.

## HTTP API

All routes except `/health` require `Authorization: Bearer <token>`.

| Method | Route | Purpose |
|--------|-------|---------|
| GET | `/health` | Liveness and version. No token, and deliberately reveals nothing else |
| GET | `/status` | Host, uptime, configured roots, scans in flight, snapshot count |
| GET | `/scans` | Every snapshot's metadata, newest first |
| GET | `/scans/{id}` | One snapshot's metadata |
| GET | `/scans/{id}/download` | The snapshot itself, as a standalone SQLite file |
| POST | `/scans` | Start a scan. Body: `{"root": "/var", "label": "manual"}` |
| POST | `/snapshots` | Accept a snapshot pushed by another agent |

`GET /scans/{id}/download` returns the raw SQLite file, `Content-Type:
application/vnd.sqlite3`. Send `Accept-Encoding: zstd` to get it compressed —
worth it, since it typically shrinks the body about three-fold.

The body is a snapshot database, so anything that opens a local snapshot opens
this one too:

```bash
curl -H "Authorization: Bearer $SPACETRACE_TOKEN" \
  https://nas.example.com/scans/7/download -o snap.sqlite
spacetrace --db snap.sqlite scans
```

`POST /scans` answers **202** immediately and scans in the background: a large
root takes minutes, well past any sensible HTTP timeout. Watch `GET /scans` for
the new snapshot. A root already being scanned returns **409** rather than
starting a second walk over the same tree.

A path that is not in `[[roots]]` returns **403** unless
`server.allow_adhoc_scans` is on.

## Push

An agent can send a snapshot to another agent or a hub:

```bash
spacetrace-agent --config agent.toml push https://hub.example.com --root /var
```

The body is the same file `GET /scans/{id}/download` serves, so pushing and
pulling are two directions of one format. The receiver keeps the original host,
root and timestamp and assigns its own id, which is what makes a pushed snapshot
comparable with the rest of that target's history. Re-pushing is harmless: a
snapshot already present with the same host, root and start time is skipped.

## Security notes

- **Loopback by default.** Exposing a filesystem inventory to the network should
  be a deliberate edit. Put a reverse proxy with TLS in front of it.
- **The token is compared without an early exit**, so a wrong token takes the
  same time to reject regardless of how much of it was right.
- **`/health` is unauthenticated** so a container healthcheck or uptime monitor
  works without being given the token. It returns only `status` and `version` —
  no hostname, no roots.
- **Ad-hoc scans are off by default.** With them on, anyone holding the token can
  enumerate any directory the agent's user can read.
- **The agent never deletes anything** outside its own snapshot database, and
  retention only ever removes rows from that database.
