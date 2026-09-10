# spacetrace-changelog

`changelog.json` is the single source for what changed in all three components.
`CHANGELOG.md` in each repo, the GitHub release notes, the website's
`/changelog` page and the **What's new** dialog in the desktop app are all
rendered from it. Edit here, never there.

JSON has no comments, so the rules live in this file instead.

## Writing an entry

Add it to the component's `unreleased` list. Cutting a release moves the whole
list into `releases`.

**`unreleased` is never shown to a user.** The website's changelog page, the
desktop app's What's new panel and the hub's About card all render `releases`
only, and each has a test that fails if that changes. Those entries describe
code that exists on `main` and in nobody's download, so listing them tells a
reader about a change they cannot get. They become visible the moment the
release containing them is cut.

The exceptions are tooling, and deliberate: `CHANGELOG.md` carries an
**Unreleased** section, and the release workflow slices it for the notes on a
continuous build — which is the one artifact that really does contain the work.

```json
{
  "kind": "fixed",
  "text": {
    "en": "APFS clones are counted once.",
    "tr": "APFS clone'ları bir kez sayılıyor.",
    "it": "…", "fr": "…", "de": "…"
  }
}
```

`kind` is one of `added`, `changed`, `performance`, `fixed`, `removed`,
`security`. Rendered output groups by kind in that order, not in file order.

**All five locales are required.** A missing one fails `cargo test`, the same
way a missing key fails `yarn typecheck` on the website. Half a translation
never reaches a reader.

**Commands, flags, file names and tags are not translated.** `--no-clone-dedupe`
and `spacetrace diff` stay as they are in every language; only the prose around
them changes. A translated command is false information.

Write for someone who uses the thing, not for someone who maintains it. The
commit already says what was done to the code — in Turkish, for the next
maintainer. This says what changed for the reader. That is why no generator
derives one from the other, and why entries are written by hand.

## `published: false`

A development milestone that was never tagged and has no downloadable files.
Recorded because the work happened; marked because claiming a release nobody
can install would be a lie. Defaults to `true`.

## Commands

```sh
cargo run -p spacetrace-changelog -- check                  # validate
cargo run -p spacetrace-changelog -- fmt                    # canonical form
cargo run -p spacetrace-changelog -- markdown --component cli
cargo run -p spacetrace-changelog -- notes --component cli --version 0.2.0
cargo run -p spacetrace-changelog -- unreleased --component cli
cargo run -p spacetrace-changelog -- promote --component cli --version 0.2.0
```

`promote` moves `unreleased` into a new release and rewrites `changelog.json`.
Run `fmt` before committing anything hand-edited, so the next `promote` produces
a small diff instead of reordering the file.

## Releasing a component that lives in another repo

Desktop and hub entries live here, in the core repo, because the website reads
one public place and two of the three repos are private. That makes the order
matter:

1. commit the entry here and push
2. bump the core pin in the other repo's `Cargo.lock`
3. tag and release there

Skipping step 2 ships a binary whose embedded changelog predates its own
release notes.
