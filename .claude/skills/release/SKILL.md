---
name: release
description: Cut a stable spacetrace release — CLI, desktop or hub — end to end: close the changelog, bump the version files, regenerate CHANGELOG.md, commit, tag, push, then verify the published binary rather than trusting CI. Use whenever the user asks to publish, cut, ship or tag a version, including Turkish phrasings like "versiyon yayınla", "sürüm kes", "sürüm çıkar", "release çıkar", "0.7.1 yayınla", "yeni sürüm yayınlayalım", or asks whether a version has been released yet. Also use when the user asks what is waiting to be released, or wants the unreleased changelog entries reviewed before a release.
---

# Cutting a spacetrace release

Three components ship from three repos but all publish their downloadable files
into **this** repo's releases, and all three changelogs live here. That is what
makes the order matter; most of this skill is order.

The authoritative rationale is [docs/RELEASING.md](../../../docs/RELEASING.md)
and [crates/changelog/README.md](../../../crates/changelog/README.md). Read the
relevant section when something below surprises you — this file is the
procedure, those explain why it is shaped this way.

## Before anything: what is actually waiting

```bash
cargo run -q -p spacetrace-changelog -- unreleased --component cli
cargo run -q -p spacetrace-changelog -- unreleased --component desktop
cargo run -q -p spacetrace-changelog -- unreleased --component hub
```

Only cut a component that has entries. `promote` refuses an empty `unreleased`
on purpose: a release whose notes say nothing is worse than no release, because
the reader cannot tell whether the notes are missing or the version was empty.

If a user-visible change has landed with no changelog entry, write the entry
first (five locales, rules in the changelog README). A release is the last
moment that omission is cheap to fix.

Report what you found before doing anything else.

### The version number is the one judgement the docs do not make for you

Everything else in this procedure is written down somewhere. This is not, and
it is the part a user is most likely to get wrong in passing — including by
naming a number in their request.

Read the entry **kinds** before accepting a number:

- any `added` → minor (0.7.0 → 0.8.0)
- only `fixed` / `performance` / `changed` → patch (0.7.0 → 0.7.1)
- `removed` or a breaking `changed` → say so loudly; this project has users
  running `spacetrace update` against these numbers

If the user names a number that understates what is in the list — asking for
0.7.1 when an `added` entry is sitting there — **say so and let them decide.**
They may still want the patch number, and that is fine; what is not fine is
cutting it silently. A version number is a promise to people who already
installed the previous one.

Use AskUserQuestion when more than one component is in play, or when your
reading of the kinds disagrees with the number you were given.

## The two orderings, and which one applies

Most releases only need the second. Check the first anyway; it is the one that
breaks users' machines.

**1. Did `SCHEMA_VERSION` go up?**

```bash
grep -n "pub const SCHEMA_VERSION" crates/store/src/schema.rs
git show $(git describe --tags --abbrev=0 --match 'v*'):crates/store/src/schema.rs | grep "pub const SCHEMA_VERSION"
```

If the number is higher than it was at the last `v*` tag, then **desktop and
hub are released first and the CLI last**, and the changelog has to say "older
builds will not open this database, update the desktop app too". The CLI
upgrades the shared database the moment it opens it, and `migrate` deliberately
refuses a file newer than itself — so shipping the CLI first leaves every
desktop user with an app that cannot list a single scan and does not say why.

If the number is unchanged, this ordering does not apply. Say so explicitly in
the commit message, with the reason; 0.7.0 did exactly that.

**2. The changelog is closed here before the other repos advance.**

Always, regardless of schema. Desktop and hub embed their changelog from the
core commit their own `Cargo.lock` pins, so tagging there before this repo is
pushed ships a binary whose embedded release notes predate its own release.

1. promote + commit + push **here**
2. advance the core pin in the other repo's `Cargo.lock`
3. tag there

## Cutting it

### Step 1 — close the changelog for every component in this release

```bash
cargo run -p spacetrace-changelog -- promote --component cli --version 0.7.1
cargo run -p spacetrace-changelog -- markdown --component cli > CHANGELOG.md
cargo run -q -p spacetrace-changelog -- check
```

`CHANGELOG.md` is generated. Never hand-edit it — CI regenerates it and fails
the build when the file differs.

### Step 2 — bump the version files

**This repo (CLI + agent):** `Cargo.toml` carries the version in *eight*
places, and missing one gives a workspace that still resolves to the old
version without any error.

- `[workspace.package] version`
- the seven `[workspace.dependencies]` path pins
  (`spacetrace-scan-core`, `-store`, `-diff`, `-dupes`, `-treemap`,
  `-changelog`, `-buildinfo`)

Then refresh the lock file, which is part of the release commit:

```bash
cargo check --workspace
git diff --stat   # expect Cargo.toml, Cargo.lock, CHANGELOG.md, changelog.json
```

`Cargo.lock` is not optional here. The release workflow builds with `--locked`,
so a lock file that still says the old version fails the build rather than
quietly resolving. Running `cargo check` is enough — the workspace members
refresh themselves; this is not `cargo update` and it must not touch external
dependencies.

**The tag is checked against the manifest.** On a real tag push the `meta` job
compares the tag to `workspace.package.version` and fails with
`tag v0.7.1 does not match workspace.package.version …` if they differ. So a
forgotten bump costs a failed run, not a mislabelled binary — but note the
check runs on tag pushes only, and a `workflow_dispatch` bypasses it entirely.

**Desktop:** `package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`
— all three, or the installer and the app report different versions.

**Hub:** `Cargo.toml`.

### Step 3 — commit and push here

English commit message, body says **why** (repo convention). State which
components were closed and whether the schema ordering applied. Commit and push
are pre-authorised in this project; tagging is not — confirm the tag with the
user if the version was not already agreed.

### Step 4 — tag

```bash
git tag v0.7.1 && git push origin v0.7.1
```

Desktop and hub tag `v*` **in their own repos**; they land here as
`desktop-v0.7.1` / `hub-v0.5.1`. Their workflows publish with
`gh release create --latest=false`, because `releases/latest` returns whichever
release was published last regardless of component, and a desktop tag taking
that spot silently disables update checks in CLI builds up to v0.4.0.

Before tagging desktop or hub, advance their core pin to the commit you just
pushed:

```bash
cargo update -p spacetrace-scan-core -p spacetrace-store -p spacetrace-changelog
```

### Step 5 — verify the artifact, not the workflow

A green workflow means it built. It does not mean the right thing was
published, and this is where releases actually go wrong.

```bash
gh run list --workflow release.yml --limit 3
gh release view v0.7.1 --json tagName,assets,publishedAt
gh release view --json tagName    # releases/latest — must be the CLI tag
```

Then download the published binary and run it:

```bash
./spacetrace --version   # version, commit, and channel=release
docker pull ghcr.io/unalcakir28/spacetrace:v0.7.1
```

Check the commit matches the tag, that `SHA256SUMS` matches the files, and that
the container image is pullable without authentication — GHCR packages start
private even when the repo is public, so a brand-new package needs its
visibility set once by hand.

The CLI workflow claims `latest` deliberately; it is the desktop and hub
workflows that pass `--latest=false`. If one of them took the spot anyway, put
it back:

```bash
gh release edit v0.7.1 --repo unalcakir28/spacetrace --latest
```

## Traps that have actually cost time here

- **`paths-ignore` applies to tag pushes.** Tagging a commit that only touched
  `docs/` or `*.md` runs no release workflow at all. A real release commit
  always touches `Cargo.toml` / `package.json` / `tauri.conf.json`, so this
  only bites when retagging. Way out: `workflow_dispatch` with `publish: true`.
- **`RELEASE_TOKEN` expiry fails the publish step with 403**, in the desktop and
  hub repos only. Re-issue and `gh secret set RELEASE_TOKEN --repo …`.
- **A green CI in desktop or hub proves nothing about this repo's changelog.**
  Their `Cargo.lock` pins a core commit, so the staleness guard there is looking
  at whatever that commit contained. Hub's `CHANGELOG.md` sat stale for three
  days behind exactly this.
- **Asking `gh release view --json` for a field that does not exist** (such as
  `isLatest`) errors in a way that reads like "there are no releases". Check the
  field name before concluding anything about what is published.
- **Renaming a tag or asset breaks the website silently.** `install.sh` and the
  website repo's `src/data/releases.ts` are bound to these exact names, and the
  website is a separate repo, so no single CI step catches it.

## When the user asks "did you publish a version?"

Pushing to `main` is not a release. It produces a `continuous` build, which is a
different channel with different assets. Answer with the tag state, not the
branch state:

```bash
git tag --sort=-creatordate | head -5
git log --oneline $(git describe --tags --abbrev=0 --match 'v*')..HEAD
```

If unreleased entries have accumulated, say what they are and whether any of
them closes a defect that is live in the published version — that is the fact
that decides whether a patch release is worth cutting, and it is the user's
call, not yours.
