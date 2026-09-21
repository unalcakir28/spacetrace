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
mount_timeout = 60           # seconds a dead network share may cost
keep = 14                    # snapshots of this root to retain
```

`threads` is per root rather than per agent because it is really a property of
the disk: an NVMe root and a spinning-disk root on the same machine want
different numbers. The default is `min(cores, 8)` — not one per core, which
measured slower on every corpus tried (docs/COMPETITORS.md §1.2).

`mount_timeout` is per root for the same reason, and it matters most here: a
server is the machine most likely to have a network share whose far end has
gone away. The first `lstat` into such a share never returns and cannot be
interrupted, so before this the whole nightly scan wedged — and because a
directory is listed on one thread, one dead share took every sibling with it.
The agent now reads the mount table at the start of a scan, approaches a mount
point through a thread it is willing to abandon, and after the timeout records
it as an unreadable path and carries on. The scan is short by that filesystem
and says so in its error count.

The default of 60 seconds is deliberately generous: waiting too long makes a
scan slow, while giving up too early drops an entire volume out of a total
that claims to be complete. Waiting is no longer silent either — `/status`
reports the stall and the directory it is waiting on.

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
| `server.rate_limit_per_minute` | `120` | Requests per minute one client address may make; `0` switches it off |
| `server.rate_limit_burst` | `60` | Requests allowed at once before the rate applies |
| `server.tls_cert_file` | — | PEM certificate chain, leaf first; set with `tls_key_file` to serve HTTPS |
| `server.tls_key_file` | — | PEM private key for that certificate |
| `roots[].path` | required | Absolute path to scan |
| `roots[].schedule` | — | Five-field cron; omit to scan only on request |
| `roots[].label` | — | Free text stored with the snapshot |
| `roots[].exclude` | `[]` | Directory names never descended into |
| `roots[].one_file_system` | `false` | Do not cross mount points (`du -x`) |
| `roots[].depth` | — | Stop descending below this depth |
| `roots[].dedupe_hardlinks` | `true` | Count hardlinked files once |
| `roots[].threads` | `min(cores, 8)` | Threads to walk this root with |
| `roots[].mount_timeout` | `60` | Seconds a mounted filesystem under this root gets to answer before it is recorded as unreadable; `0` waits forever |
| `roots[].keep` | — | Snapshots of this root to retain; unset keeps all |

The token is resolved in this order: `server.token`, `server.token_file`, then
the `SPACETRACE_TOKEN` environment variable. `serve` refuses to start without
one — an unauthenticated agent hands its whole filesystem inventory to anyone
who can reach the port.

Unknown keys are rejected at startup. A typo that silently does nothing on a
machine nobody watches is worse than a failure to start.

Setting one of `tls_cert_file` and `tls_key_file` without the other is refused
at startup too. Serving plaintext on a port its operator has decided is HTTPS
is the one failure this feature must not have.

### TLS

Nothing is required here: the agent listens on loopback by default, and for
anything with a domain name a reverse proxy in front remains the better answer
— Caddy and Traefik renew certificates and the agent does not.

What this is for is the case with no proxy and no public name, a NAS or a home
server reached across a LAN. Point the agent at a certificate and a key and it
serves HTTPS instead of HTTP:

```toml
[server]
listen = "0.0.0.0:7878"
tls_cert_file = "/etc/spacetrace/agent.pem"
tls_key_file = "/etc/spacetrace/agent.key"
```

A self-signed certificate is a chain of one and works. Generate it naming the
address clients will use — **as a subject alternative name**, because no
current TLS client falls back to the common name, and a certificate without a
matching SAN is rejected by every one of them:

```bash
openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -keyout /etc/spacetrace/agent.key -out /etc/spacetrace/agent.pem -subj "/CN=nas.lan" -addext "subjectAltName=DNS:nas.lan,IP:192.168.1.10"
```

Keep the key unreadable to anyone else (`chmod 600`), and remember the agent
reads both files at startup: a renewed certificate takes a restart.

Then tell the client to trust it, in `remotes.toml`:

```toml
[remotes.nas]
url = "https://nas.lan:7878"
token = "..."
ca_file = "/home/you/.config/spacetrace/nas.pem"
```

`ca_file` is the agent's certificate, copied to the client. For a self-signed
certificate naming it here *is* pinning: it is trusted for this remote and for
nothing else, and no public authority can issue a certificate this client would
accept in its place. A certificate from a public authority needs no `ca_file`
at all.

`ca_file` is only read for a remote named in `remotes.toml`. A bare
`--remote https://…` URL carries no trust configuration, so an agent with its
own certificate has to be given an entry in that file.

The agent neither generates nor renews certificates, and there is no ACME
client: on a box with no public DNS name there is nothing for ACME to prove.

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
# ca_file = "/path/to/agent.pem"   # only for an agent serving its own TLS
```

Then `spacetrace --remote nas diff --path /var`.

`ca_file` is explained under [TLS](#tls); it is needed only when the agent's
certificate was not issued by a public authority.

`scan`, `prune` and `rm` refuse to run with `--remote`: they act on local state,
and the agent deletes nothing.

## HTTP API

All routes except `/health` require `Authorization: Bearer <token>`.

| Method | Route | Purpose |
|--------|-------|---------|
| GET | `/health` | Liveness and which build. No token, and nothing about the machine or what it scans |
| GET | `/status` | Host, uptime, configured roots, running scans with their counters, snapshot count |
| GET | `/scans` | Every snapshot's metadata, newest first |
| GET | `/scans/{id}` | One snapshot's metadata |
| GET | `/scans/{id}/download` | The snapshot itself, as a standalone SQLite file |
| POST | `/scans` | Start a scan. Body: `{"root": "/var", "label": "manual"}` |
| POST | `/snapshots` | Accept a snapshot pushed by another agent |

### Is a scan working, or wedged?

`scanning` in `/status` carries one object per running scan, not just a path —
because a path alone cannot answer the question the endpoint is opened for:

```json
{
  "scanning": [
    {
      "root": "/mnt/backup",
      "elapsed_ms": 94120,
      "files": 812004, "dirs": 51233, "bytes": 419923884032, "errors": 3,
      "clones_probed": 0,
      "phase": "walking",
      "stalled_ms": 61000,
      "waiting_on": ["/mnt/backup/nfs-archive"]
    }
  ]
}
```

Three fields are worth knowing:

- **`phase`** is `walking` or `finishing`. After the walk ends only
  `clones_probed` moves, so a reader who watches `files` alone reads a healthy
  scan as a stuck one — on one measured tree that was 1193 ms out of 1989.
- **`stalled_ms`** is absent while the scan is moving, and otherwise says how
  long *every* counter has stood still. It is measured by a watcher inside the
  agent, not from one request to the next, so polling `/status` rarely does not
  inflate it.
- **`waiting_on`** is sent only alongside a stall: the directories being listed
  at that moment. During a healthy scan the answer is already out of date by
  the time it is read.

A scan that never moves at all is still listed, with every counter at zero —
that is the shape of an agent wedged on its own root.

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

### Rate limiting

Every request is counted against the address it came from, and one that is over
the limit gets **429** with a `Retry-After` in seconds. The count happens
**before** the token is checked, which is the only position that bounds the two
things the token cannot: `/health` needs no token by design, and a wrong token
still costs the agent a reply.

The default — 120 a minute with 60 allowed at once — is far above anything
ordinary use produces; a CLI command makes a handful of requests and the hub
polls on a schedule. What it stops is a client stuck in a retry loop, which on
a NAS means a process spending the one disk this agent exists to measure.

**Behind a reverse proxy every request arrives from the proxy's address**, so
the per-client limit becomes one limit shared by everyone. It still stops a
single runaway client from saturating the machine, but it no longer keeps
clients from affecting each other. Where that matters, set
`rate_limit_per_minute = 0` and limit in the proxy, which is the only component
that can still tell the callers apart. The agent deliberately does **not** read
`X-Forwarded-For`: a header anybody can set would be a way to get a fresh
allowance by typing one.

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
  be a deliberate edit. Either put a reverse proxy with TLS in front of it, or
  give the agent a certificate of its own — see [TLS](#tls). Plaintext across a
  network puts both the bearer token and the inventory on the wire.
- **The token is the only credential.** TLS here encrypts and authenticates the
  *server*; it does not authenticate clients. There is no mTLS, and a valid
  token from any address is accepted.
- **The token is compared without an early exit**, so a wrong token takes the
  same time to reject regardless of how much of it was right.
- **`/health` is unauthenticated** so a container healthcheck or uptime monitor
  works without being given the token. It returns `status`, `version`, `commit`
  and `channel` — which build is running, and nothing else. No hostname, no
  roots, no paths, nothing about what the machine holds. The commit is there on
  purpose: every build between two tags shares a version number, so it is the
  only way to identify a box without handing out the token.
- **Ad-hoc scans are off by default.** With them on, anyone holding the token can
  enumerate any directory the agent's user can read.
- **The agent never deletes anything** outside its own snapshot database, and
  retention only ever removes rows from that database.
