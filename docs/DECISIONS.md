# Decisions

Decisions between phases that are expensive to reverse, and their rationale.
If a decision is written here, the discussion is closed; reopening it
requires new information.

Product rationale [WHY.md](WHY.md), design [ARCHITECTURE.md](ARCHITECTURE.md),
plan [ROADMAP.md](ROADMAP.md).

---

## K1 — Interface language English · 7 September 2026

CLI strings, `--help` text, error messages, README and ARCHITECTURE are
English.

**16 September 2026 update: the documentation exception was removed.** This
decision originally left WHY / ROADMAP / TODO / RESEARCH / DECISIONS and
CLAUDE.md in Turkish; now **everything is English**, rationale documents
included. Two reasons: Turkish prose noticeably costs more tokens and this
repo loads CLAUDE.md every session; and this repo is public, so Turkish
rationale documents narrow who can contribute. The CLAUDE.md files were
translated that day, `docs/` and both runbooks the next, and TODO.md on
17 September 2026 — nothing is left. Commit messages are English from here
on, the existing history is not rewritten.

**Why.** The target channels (HN, r/selfhosted) and the target user are
English. The migration cost only grows: when this decision was made the
surface to translate was 71 lines across two files; it would multiply several
times over once the agent's HTTP error messages, TOML comments, systemd unit
and install script were added. The repo hadn't been published yet, so there
were no external links to break.

**Result.** No i18n framework is added; strings stay English and embedded in
the code. "No i18n" is not technical debt, it is a decision declared out of
scope.

---

## K2 — Core and agent Apache-2.0, desktop in a separate repository · 7 September 2026

| Component | License | Repository |
|---------|--------|------|
| `scan-core`, `store`, `diff`, `cli`, `agent` | Apache-2.0 | this monorepo, public |
| Desktop (Phase 3) | commercial, to be settled in Phase 3 | separate repository |
| Central service (Phase 4) | commercial / source-available | separate repository |

**Why.** The agent runs with near-root privileges on the user's own server.
The only antidote to the "the agent cannot earn trust" losing scenario in
[WHY.md](WHY.md) is that the code is readable. Closing the agent's source does
not protect the defensible part of the product — it only lowers the install
rate.

**The correction this forces.** In WHY.md's first pricing hypothesis, the Pro
tier was "unlimited agent". If the agent is open source this limit cannot be
enforced: anyone can build it and run as many copies as they want. The Pro
tier was redefined as **desktop + remote sources + timeline**; the money comes
from what is distributed as a binary and that the user cannot easily rewrite.

---

## K3 — Agent protocol HTTP + JSON · 7 September 2026

Metadata endpoints are JSON, the snapshot body is `application/octet-stream`.
The server framework is **axum**.

**Why not gRPC.**

- The target user is a homelab developer: they put the agent behind
  Caddy/Traefik, poke at it with `curl`, open `/health` from a browser.
  Putting gRPC behind a reverse proxy (h2c) and then consuming it from a
  browser (grpc-web) is friction for this audience.
- The Phase 3 desktop is a WebView, Phase 4 is already axum. JSON is
  consumed directly by all three.
- Dependency budget: `tonic + prost` plus build-time `protoc` is heavy next to
  `axum + serde`. serde is already in the workspace.
- gRPC's two strong suits aren't needed here: there are ~6 endpoints, and the
  snapshot transfer is a single large blob that a plain HTTP body already
  streams just fine.

**Accepted cost — measured.** In Phase 1 the workspace was 26 crates; after
axum, tokio, reqwest and zstd, **162**. That is a big jump against
CLAUDE.md's dependency-frugality rule, and far above the (~60) figure
estimated at decision time. Accepted anyway: a single HTTP stack will be used
in both the agent and Phase 4, and the alternatives didn't really save
this — `tiny_http` (~10 crates) loses async streaming and the Phase 4
sharing, and `tonic` is even heavier.

Most of that count comes from the network layer; `scan-core` still depends
only on rayon and the core scan path is unaffected by this bloat. In
exchange, the cron parser, calendar arithmetic and the constant-time token
comparison were written by hand — chrono, `cron` and `subtle` were not added.

**To watch:** binary size and the musl build. Expectation is 5–8 MB; to be
verified the first time the release workflow runs.

---

## K4 — Snapshot on the wire is raw SQLite · 7 September 2026

The agent sends the SQLite file as-is. There is no intermediate
serialization format. Before transfer a clean, single-scan copy of the file
is produced; the body is compressed with zstd.

**Implementation note (7 September 2026).** The first draft called for
`VACUUM INTO`. In the implementation, ATTACH + `INSERT ... SELECT` was
preferred (`Store::export_snapshot`), because `VACUUM INTO` copies the entire
database; ATTACH takes only the requested scan and the new file does not
inherit the WAL — by the time the call returns, the file is a single,
self-contained piece. The WAL trap (below) is thereby already solved.

**Why.**

- The arena layout is already the shape that travels on the wire. The
  `entries` table is a flat array; putting a format in between means
  unpacking and repacking the same rows.
- Version negotiation is already there: `PRAGMA user_version` refuses to
  open a newer one (`store/src/schema.rs`). That is exactly the check needed
  between machines.
- The SQLite file format is endian-independent and backward-stable.
- **The biggest win:** the code that opens a remote snapshot is the same code
  that opens a local snapshot. `store::load` does not know where the file
  came from.

**Measurement (7 September 2026, `/usr/share`, 20 180 entries).**

| | Raw | gzip -9 | zstd -19 |
|---|---|---|---|
| Total | 999 KB | 280 KB | 250 KB |
| Per entry | 49.5 B | 13.9 B | **12.4 B** |

A 1M-file root ≈ 50 MB raw, ~12 MB zstd. Acceptable for a nightly transfer;
the "SQLite is too big" worry is closed by measurement.

**Escape hatch.** If size becomes a problem later, adding a compact format
via `Accept-Encoding` does not break existing clients.

**Trap.** The schema is in WAL mode (`schema.rs`). Sending the live DB file
directly carries a risk of missing data — that is why the raw file is never
sent as-is, it always goes through `export_snapshot`.

---

## K5 — The repository is public · 7 September 2026

The monorepo is public on GitHub, Apache-2.0.

**Why.** Trust is built with a track record, not a flag flipped on launch
day. An agent repository that has been open for six months with a real
commit history reads differently from one opened a week before the HN post.
Competitive risk is low: the moat isn't the scan code (competitors already
have that), it's the agent + history + fleet view — copying it requires
changing who your target users are. Also, GitHub Actions is free on a public
repository.

**Order.** K1 (the English migration) first, then push. The first impression
must not be a Turkish README.

---

## K6 — Capacity is reported as "free / total" · 7 September 2026

Filesystem capacity is shown to the user as **free space and total**; not as
"% full". `Capacity::unavailable()` still exists but what it is has been
documented, and it is not used in the interface.

**Why — found by measurement.** After capacity was added for Phase 4, the
CLI printed "filesystem 70% full". For the same mount, `df` said "6%". Both
see the same `total` and `available` values; the difference is in the
definition: on APFS (and in btrfs subvolumes, thin LVM pools) space is shared
between volumes, so `total - available` is the container's usage, not this
volume's. `df` on macOS reports the volume's own bytes.

The answer to "which one is 'correct'" depends on the question: for "will I
run out of space" it's our number, for "what have I put here" it's `df`'s
number. But since the one thing a disk tool cannot sell is a wrong number,
showing a percentage that contradicts `df` is unacceptable. `available` is
the same number on both sides, so the interface shows that.

**Result.** A bullet was added to WHY.md's accuracy pledge.

---

## K7 — A forecast is not stated if its basis is weak · 7 September 2026

The central service's "full in N days" forecast is shown only when **all**
of the following conditions hold: ≥3 snapshots, ≥1 day of span, linear fit
r² ≥ 0.5, measured capacity, and the answer being within 10 years.

**Why.** Disk usage is often not linear. A log rotation or a one-off restore
produces a line whose slope means nothing. Three samples spread over an hour
say nothing about next week. A confidently wrong date is worse than no date
at all — and for a warning system, a false alarm leads to the whole system
being shut off.

If the conditions are not met the column stays empty and the target page
states why ("not extrapolated: 2 samples over 0.3 days, fit 0.12"). The
thresholds sit explicitly in `spacetrace-hub/src/trend.rs` as `MIN_SAMPLES`,
`MIN_SPAN_DAYS`, `MIN_R2`.

---

## K8 — The three components' downloadable files are also in the public repository · 8 September 2026

The desktop and hub are built in their own (private) repositories, but the
resulting installer files are published to **this repository's** releases:
under the `desktop-v*` and `hub-v*` tags. (There were `desktop-continuous`
and `hub-continuous` tags too until K9 was reversed; see below.)

**Why.** A private repository's release assets cannot be downloaded without
authentication. Per K2 the desktop and hub are private, but a marketing
site's download button cannot ask a visitor for a token. The source needed
to stay closed while the distribution stayed open; the only way was to move
the assets to a public repository.

There is a second benefit to having them in the same place: the download
page learns all three components' versions with a single GitHub API call.

**Cost.** For the private repositories' CI to write to the public repository
requires a fine-grained PAT (`RELEASE_TOKEN`) — `GITHUB_TOKEN` cannot write
outside its own repository. If the secret is missing the workflow does not
fail, it publishes the assets in its own repository and prints a warning; so
a missing secret is not a silent failure.

**Result.** Tag and asset names are now a contract: `website/download.html`
links to them directly, and `install.sh` builds the file name from the given
version. Renaming them breaks the download page. Details:
[RELEASING.md](RELEASING.md).

---

## K9 — Every push publishes a "continuous" release · 8 September 2026

Every push to `main` deletes and recreates a pre-release with the fixed
`continuous` tag. Stable releases are cut separately with the `v*` tag.

**Why.** The "latest build" has to have a fixed URL: the download page
contains real links that work even without JavaScript, and `install.sh`
builds the file name from the version. Deleting and recreating the tag
instead of moving it ensures both that the tag tracks `main` and that an
asset dropped from the matrix does not linger as a dead link. The cost is a
few-second 404 window per release — acceptable for a continuous channel.

Because the `releases/latest` endpoint does not return a pre-release,
`install.sh` falls back to `continuous` when there is no stable release, and
prints that it did. Otherwise the documented one-line install command
wouldn't work until the first stable tag.

**19 September 2026: reversed. The channel is gone in all three
repositories.** A `v*` tag is now the only thing that triggers a release
workflow anywhere; a push to `main` publishes nothing.

Three things it got wrong. It cost a full multi-platform build on every push,
and on the private desktop repository — where macOS runner minutes bill at
10x — that was the single most expensive job in the account, for a channel
the download page only ever *fell back* to. Its `upload-artifact` copies sat
for the default 90 days and filled the account's 0.5 GB of Actions storage,
which then failed a real release: all three platforms built, nothing
uploaded, `publish` skipped. And the fixed URL the decision was built
around turned out to be the cheap half of the problem — a rolling tag never
goes stale because it is republished constantly, so nothing ever forced the
site's static links to be checked.

What replaced it: the site's static download links name a real version tag
(`CHANNELS[…].fallback` in the website repository), so they now have to move
with each release or they 404; `retention-days: 1` on every
`upload-artifact`, because the durable copy is the release asset; and no
`paths-ignore` in any of the three release workflows, since with `main` gone
its one remaining effect would be to skip a *tag* push whose commit touched
only documentation.

The `install.sh` fallback above is also gone; it kept the comment explaining
why, so nobody adds it back for the same reason.

---

## K10 — Localisation only for the GUI, the site and the changelog · 9 September 2026

K1 said "no i18n framework is added, strings stay English and embedded" and
described this as a decision declared out of scope. This decision **does not
repeal K1, it narrows its scope**:

| Surface | Language |
|-------|-----|
| Desktop app interface | `en tr it fr de` |
| Site | `en tr it fr de` (already was) |
| Changelog text | `en tr it fr de` |
| CLI output, `--help`, error messages | English |
| Agent and hub HTTP messages, `--help` | English |
| `install.sh`, systemd unit, TOML comments | English |
| Code, code comments, commit messages | unchanged |

**Why K1 was not reopened entirely.** Everything K1's rationale counted as
the surface to translate — `--help`, HTTP error messages, TOML comments, the
systemd unit, the install script — is terminal and server surface. K1 never
considered a GUI, because there was no GUI when the decision was made. The
terminal-side rationale still holds: **a translated command is wrong
information**, and it works in the user's favor for `spacetrace diff` output
to be in English when they paste it into a search engine.

**Why it does not hold on the GUI side.** The desktop is a commercial
product and its target audience is not HN. The site is already five
languages: the user reads about the product in Turkish and downloads it,
then opens the app in English. K1 had not foreseen this inconsistency.

**Result.** The languages are kept in the same set as the site's — a reader
who picks Italian on the site should not fall back to English in the app. In
changelog entries, commands, flags, file names and tags are **not
translated**; only the surrounding sentence is.

---

## K11 — The changelog is a single, hand-written data file · 9 September 2026

`crates/changelog/changelog.json` is the single source for all three
components' changelog. Each repository's `CHANGELOG.md`, the GitHub release
notes, the site's `/changelog` page and the desktop's "What's new" window
are all generated from it.

**Why it is not generated from commits.** A commit says "what I did to the
code", for the next maintainer. The changelog says "what changed for you".
Different texts, different readers. It's not technically possible either:
this repository deliberately uses prose commits — the measured history is
Turkish, as in `APFS clone'larını bir kez say`, and English since
16 September 2026 (K1) — and it was measured: git-cliff,
release-please and cocogitto classify **100%** of the history as "other".

**Why all three components are in a single file.** The desktop and hub
already put their downloads into the core repository's releases, and the
site reads from a single public place. Splitting it across three
repositories would have meant either a sync job or handing the site a token.
The cost: an entry for a change made in the desktop must be committed here,
and the core pin must be advanced before the desktop is tagged — otherwise
the binary's embedded changelog is older than its own release notes. The
order is in [RELEASING.md](RELEASING.md).

**Why embedded, not downloaded.** The desktop shows "what changed" right
after updating itself — exactly the moment it might not have a network. An
empty "what's new" window is worse than not having one at all.

**`published: false`.** An untagged development milestone with no
downloadable file. It is recorded because the work was done, and marked
because showing it as a version nobody can install would be a lie.
