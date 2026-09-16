# Releases and distribution

How the three components (CLI + agent, desktop, hub) get built and become
downloadable. Everything this produces — release notes, the download page,
install instructions — is English, and since 16 September 2026 so is every
document describing it, this one included (K1).

## Why everything is published in this repository

This setup was established while the desktop and hub were private: a private
repository's release assets can't be downloaded without authentication, so the
assets had to live in a public repository.

**That constraint is live again.** The desktop and hub sat public for a while,
which is what this paragraph used to report as permanent; they were made
private again on 16 September 2026 (K2). A private repository's assets still
cannot be downloaded without a credential the download page has no way to
supply. The setup would stay even if that changed, because it has two further
reasons:

- The download page learns all three components' versions with **a single
  GitHub API call**. Splitting across three separate repositories would mean
  three calls and three failure paths.
- `install.sh` and the site's `src/data/releases.ts` file are tied to a single
  repository. Splitting would mean rewriting every working download link for
  no gain.

So each repository builds its own code in its own CI, and publishes the
output to **this repository's releases**.

## Channels

Tag names are fixed. The download page binds to them directly, so **renaming
them breaks the site.**

| Tag | Content | Trigger |
|--------|--------|------------|
| `continuous` | CLI + agent, latest `main` | push to this repository |
| `v*` | CLI + agent, stable | `v*` tag in this repository |
| `desktop-continuous` | Desktop installers | push to the desktop repository |
| `desktop-v*` | Desktop, stable | `v*` tag in the desktop repository |
| `hub-continuous` | Hub binaries | push to the hub repository |
| `hub-v*` | Hub, stable | `v*` tag in the hub repository |

`continuous` releases are **deleted and recreated**, not edited: this way the
tag tracks `main` and an asset dropped from the matrix doesn't stay behind as
a dead link on the download page. There's a brief 404 window; acceptable for
the continuous channel.

Asset names carry the version (`spacetrace-continuous-x86_64-apple-darwin.tar.gz`)
because `install.sh` builds the file name from the given version. So
`SPACETRACE_VERSION=continuous` works with no changes at all.

## Container images

| Image | What | Tags |
|------|-----|-------|
| `ghcr.io/unalcakir28/spacetrace` | agent + CLI | `main`, `edge`, `v*`, `latest` |
| `ghcr.io/unalcakir28/spacetrace-hub` | hub | `main`, `edge`, `v*`, `latest` |

Both are amd64 + arm64. Built **from pre-compiled musl binaries, not from
source** (`.github/docker/Dockerfile.release`): compiling Rust under QEMU on
an amd64 runner takes tens of minutes and routinely exhausts memory. The
`Dockerfile` at the repository root still builds from source — so that
`docker build .` works in a clone.

## Manual steps needed when going live

These are done once and can't be automated.

### 1. `RELEASE_TOKEN` (for the desktop and hub repositories)

A `GITHUB_TOKEN` in one repository can't write to **another** repository —
this holds even if the repository is public; it's not about privacy, it's the
token's scope. Since the desktop and hub publish their own output here, a
separate token is needed:

1. <https://github.com/settings/personal-access-tokens/new>
2. Repository access → **Only select repositories** → `unalcakir28/spacetrace`
3. Permissions → Repository permissions → **Contents: Read and write**
4. Pick an expiration, create it, and copy the token

Then add it as a secret to both repositories:

```bash
gh secret set RELEASE_TOKEN --repo unalcakir28/spacetrace-desktop
gh secret set RELEASE_TOKEN --repo unalcakir28/spacetrace-hub
```

If the secret is missing the workflow **doesn't fail**: it publishes the
assets in its own repository and prints a warning. So the first pushes aren't
wasted — but the assets won't be where the site expects them, so the download
links 404.

When the token expires, the publish step fails with a 403. Renew it and run
the same command again.

### 2. GHCR package visibility

GHCR packages **start private** even if the repository is public — package
visibility is separate from repository visibility. So that `docker pull`
doesn't ask for authentication, once for each new package:

Package page → **Package settings** → Change visibility → **Public**.

- <https://github.com/users/unalcakir28/packages/container/spacetrace/settings>
- <https://github.com/users/unalcakir28/packages/container/spacetrace-hub/settings>

The `spacetrace` image can already be public since it comes from this public
repository; check it after the first publish anyway.

### 3. GitHub Pages — **no longer in this repository**

The site moved to its own repository, so this step belongs there. There's
value in keeping the record here anyway, because the same trap has been hit
twice: `GITHUB_TOKEN` has no permission to create a Pages site that has never
existed ("Resource not accessible by integration"), so `configure-pages`
can't bootstrap it itself. A new Pages site has to be opened by hand, once:

```bash
gh api -X POST repos/unalcakir28/spacetrace-website/pages -f build_type=workflow
gh api -X PUT  repos/unalcakir28/spacetrace-website/pages -f cname=spacetrace.teknobakkall.com
```

The domain points to `unalcakir28.github.io` as a CNAME in Cloudflare and
**the proxy has to be off** (grey cloud): with the orange cloud in front,
GitHub can't verify the domain or issue the Let's Encrypt certificate.

## Cutting a stable release

For every release: first the changelog, then the version number, then the tag.

**1. Close the changelog.** `promote` moves that component's `unreleased`
entries into a new release and rewrites `changelog.json`. An empty
`unreleased` is rejected: a release whose notes say nothing is worse than a
release that was never cut — the reader can't tell whether the notes are
missing or the release was empty.

```bash
cargo run -p spacetrace-changelog -- promote --component cli --version 0.2.0
cargo run -p spacetrace-changelog -- markdown --component cli > CHANGELOG.md
```

**2. Bump the version number and tag.**

```bash
# CLI + agent (this repo): workspace.package.version in Cargo.toml
git tag v0.2.0 && git push origin v0.2.0

# desktop: package.json, src-tauri/Cargo.toml and src-tauri/tauri.conf.json
git tag v0.2.0 && git push origin v0.2.0

# hub: Cargo.toml
git tag v0.2.0 && git push origin v0.2.0
```

**For the desktop and hub the order can't be broken.** Their entries live in
this repository (K11), and the binary embeds the changelog from the core
version pinned by its own `Cargo.lock`:

1. commit and push the entry here
2. advance the core pin in `Cargo.lock` in the other repository
3. tag it there

If the second step is skipped, the app ships embedding a changelog older than
its own release notes — when the user opens the "What's new" window, they
won't find the version they just installed there.

As long as the tag is `v*`, the workflow publishes to the stable channel.
Desktop and hub tags appear in this repository as `desktop-v0.2.0` /
`hub-v0.2.0` — because three components share the same namespace.

Dry run: give a tag name via `workflow_dispatch`. The build runs, the publish
step is skipped — unless you check the `publish` box.

Manual publishing with `publish: true` is the way out of the case where the
`paths-ignore` filter skips a tag push (see below). The tag is created at the
tip of the default branch in the target repository.

### If the schema version is bumping: desktop and hub first, then CLI

The desktop and the CLI use **the same database file** — the
`default_database()` comment on the desktop side says it explicitly: "so the
app opens the same history". And `store::schema::migrate` deliberately
doesn't open a file newer than itself: it refuses rather than reading it
wrong.

Combine the two and this happens. The moment the new CLI opens the shared
database it upgrades the schema; from that point on the old desktop can do
**nothing**. On 10 September 2026 a v3 database was opened with the 0.4.1
binary:

```text
error: this database was written by a newer spacetrace (schema v3, this
build understands v2)
```

The scan can't be listed, the history can't be opened. If the user only
updated the CLI, the desktop app looks broken, and the reason isn't shown on
screen.

So in a release that bumps `SCHEMA_VERSION` the order is:

1. Advance the desktop's and hub's core pin, publish both.
2. **Then** publish the CLI.
3. Say it in the changelog: "older versions won't open this database, update
   the desktop too". The user doesn't know the order, you do.

The reverse is also true: the new desktop carries the old database without
issue, because `migrate_from` only ever moves forward. So publishing the
desktop first costs nothing, publishing it after does.

### "Latest" belongs to the CLI, it isn't determined by order

GitHub's `releases/latest` endpoint returns the **most recently published**
release, without distinguishing components. Since all three components'
downloads live in this repository, this means cutting a desktop or hub
release takes the CLI's place.

The damage is concrete: `spacetrace update` and the agent, **in versions up
to and including v0.4.0**, read that endpoint, and a tag like
`desktop-v0.4.0` doesn't correspond to any version in their parser — the
update check silently shuts off and stays that way. Measured on 9 September
2026 when `hub-v0.3.0` took that spot.

So the desktop and hub workflows publish the stable release with **`gh
release create --latest=false`**. If a new component repository is added, it
needs to do the same. If the spot somehow gets taken anyway, the fix is one
command:

```bash
gh release edit <cli tag> --repo unalcakir28/spacetrace --latest
```

v0.4.1 and later check the tag prefix (`is_ours`), so they're not affected by
this trap — the rule stays in place for those still running the old binary.

## Which commit triggers what

A five-target build isn't free, so only changes that concern the code trigger
the release workflow:

- `docs/**`, `website/**`, `*.md`, `tasks/**` → the release workflow **does
  not run**
- `website/**` → the Pages workflow runs (and only that)

The site and the binary build are deliberately separate: fixing a typo on the
download page shouldn't have to wait for a five-platform build, and a build
failure shouldn't block a doc fix from going live.

**Watch out:** on GitHub, path filters inside `on.push` are
branch-independent, so they also apply to tag pushes. If you tag a commit
that only changes `docs/` or `*.md`, the release workflow never runs. Not a
problem in practice — cutting a release requires changing `Cargo.toml` /
`package.json` / `tauri.conf.json`, and none of those are ignored. If it
happens anyway, the way out is `workflow_dispatch` + `publish: true`.

## Code signing — status and exit plan

**Nothing is signed.** On macOS the bundle only carries the ad-hoc signature
the linker forces for arm64 (`codesign` → `Signature=adhoc`,
`TeamIdentifier=not set`, `spctl` → `rejected, no usable signature`); the
Windows installer is completely unsigned.

The cost of this was measured on 10 September 2026: the downloaded `.dmg`
doesn't open on macOS 26.5.2, and because **Apple removed the "right-click →
Open" shortcut in macOS 15**, the instructions that had been on the site for
years were no longer valid. What's left is System Settings → Privacy &
Security → Open Anyway, and whether that button appears for an ad-hoc signed
app **has not been verified**.

As a stopgap, `install-desktop.sh` and `install-desktop.ps1` were added.
These don't fool Gatekeeper: the `com.apple.quarantine` attribute that
triggers the first-launch check (Mark-of-the-Web on Windows) is written by
the **browser**, not by curl or `Invoke-WebRequest`. Measured — a dmg
downloaded with curl has no extended attribute at all, and the app that comes
out of it opens without a single dialog. In exchange, instead of the
*identity* guarantee a signature gives, only the *integrity* guarantee
SHA256SUMS gives remains; the scripts say this explicitly.

Homebrew can't fill this gap: `--no-quarantine` was removed in Homebrew 4.7,
and casks that fail Gatekeeper became unsupported, **as of 1 September
2026**, even in your own tap.

The cost, if it's decided:

| | Fee | Outcome |
|---|---|---|
| Apple Developer Program | $99/year | Notarized `.dmg`, the macOS warning goes away entirely |
| Windows OV certificate | ~$220–400/year | SmartScreen doesn't clear instantly, reputation builds up |
| Windows EV certificate | ~$500–660/year | SmartScreen clean from day one |

Azure Artifact Signing ($9.99/month) is **closed to Turkey** — limited to the
US, Canada, the EU and the UK, so there's no cheap path for Windows.

### Unsigned ≠ identity-less: TCC is a separate problem, and it's solved

Gatekeeper and TCC (permissions) aren't the same thing, and the second one
**didn't cost money**.

macOS ties Full Disk Access and folder permissions to the app's *designated
requirement*. An ad-hoc signature doesn't have one, so the system falls back
to the binary's cdhash — and the cdhash changes on every build. Result: every
release is a different app to macOS, the permission the user granted shows
**as on in System Settings but isn't applied**, and after every update the
scan asks folder by folder again. Reported by a user on 10 September 2026,
with the toggle on.

The fix is a self-signed certificate. It's no use to Gatekeeper, but it pins
the identity. Measured: two packages with different contents (cdhash
`53007fdd…` / `de0072e5…`) share a single requirement:

```text
identifier "com.spacetrace.desktop" and certificate root = H"940f909c…"
```

It's `root`, not `leaf`: codesign writes whatever it sees in the chain, and
it comes out as `root` because the certificate is a trusted root on the build
machine. The same certificate wrote `leaf` while it was untrusted — both are
stable across builds, but **different strings**, and TCC compares the
requirement as written. So a build that skips the trust step produces a valid
signature and still drops everyone's permission. That's why the release
workflow validates the exact `root` form.

Four traps in the setup, all four of them hit:

- **macOS can't read OpenSSL 3's default p12.** It writes a SHA-256 MAC;
  `security import` says "MAC verification failed (wrong password?)" and
  sends you chasing the password. `openssl pkcs12 -export -legacy` is needed.
- **The certificate must be trusted on the build machine.** Tauri looks up
  the identity with `security find-identity -v`, which only lists valid
  identities; a self-signed certificate isn't counted as valid until it's
  made a trustRoot. Runners are single-use, so trusting it there doesn't
  reach any other machine.
- **Changing the certificate resets everyone's permission.** The requirement
  is written with the leaf's fingerprint. It expires in 2036; renewing it
  means a new certificate, so users will grant the permission once more.
- **Tauri also stamps the identity onto the .dmg, and this shuts the download
  off completely.** A disk image signed with a certificate macOS doesn't
  trust is rejected while *mounting* — the warning appears before the app
  ever runs, while the file is being opened. An unsigned image mounts and
  leaves the question to the app; so on this point **unsigned beats badly
  signed**. Measured on 26.5.2: v0.4.0 `source=no usable signature` → opens,
  v0.4.1 `origin=spacetrace` → doesn't open. `codesign --remove-signature`
  doesn't work on a disk image ("operation inapplicable"), so the workflow
  rebuilds the image with `hdiutil convert`; the signed .app inside it isn't
  touched.

Secrets: `APPLE_CERTIFICATE` (the p12's base64) and
`APPLE_CERTIFICATE_PASSWORD`, in the desktop repository. The private key is
at `~/.spacetrace/macos-signing.p12`, not inside any repository. If the
secret is missing the build falls back to ad-hoc and prints a warning; the
release workflow also validates the package's requirement, so it doesn't fall
back silently.

**Things to do once signing arrives** (this list is the reason this section
exists):

1. `install-desktop.sh` and `install-desktop.ps1` **get deleted** — they're
   not files to maintain, they're a patch for a gap.
2. The `installAltTitle` / `installAltBody` / `installAltNote` keys in the
   site repository and the alternative box in `Download.astro` are removed.
3. `installMacBody` goes back to "double-click, it opens".
4. The "Not code-signed" release-note paragraph in the desktop's
   `release.yml` is deleted.
5. The "Unsign the disk image" step in the desktop's `release.yml` is deleted
   — a `.dmg` signed with a real certificate is the correct thing,
   notarization already expects that.

## Site

The site is no longer in this repository:
**[unalcakir28/spacetrace-website](https://github.com/unalcakir28/spacetrace-website)**
— Astro, five languages, 36 pages, live at `spacetrace.teknobakkall.com`. How
it works and where it breaks easily is in that repository's `CLAUDE.md`.

It split off from here because the only thing keeping it here was the
address: GitHub Pages serves a project site under `/<repo-name>/`, so the
repository name was part of the URL, and splitting it out would have broken
the address. Its own domain cut that tie; the site already had no code tie to
`crates/` anyway.

**The only remaining tie in this repository is the download contract below.**
The site's `src/data/releases.ts` file binds one-to-one to the tag and asset
names produced by the three release workflows here. If you change a name in
the table above, update the site repository the same day too, or every
download link silently breaks — and because they're now two separate
repositories, no single CI step catches it.
