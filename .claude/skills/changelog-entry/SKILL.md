---
name: changelog-entry
description: Write a changelog entry for a user-visible change in any of the three components, in all five locales, and regenerate CHANGELOG.md. Use whenever a change lands that a user can observe — a new command or flag, changed output or numbers, a fixed bug, a removed feature — and whenever the user asks for a changelog entry, "changelog girdisi", "changelog yaz" or asks why CI says CHANGELOG.md is stale.
---

# Writing a changelog entry

One file, [crates/changelog/changelog.json](../../../crates/changelog/changelog.json),
is the source for all three components. From it are rendered: `CHANGELOG.md` in
each repo, the GitHub release notes, the website's `/changelog` page, and the
desktop app's **What's new** dialog. Nothing is written in those places by hand.

The authority on the rules is
[crates/changelog/README.md](../../../crates/changelog/README.md). Read it when
something below surprises you. This file is the procedure.

## 1. Decide whether there is an entry to write

An entry describes something a **user of the thing** can observe: a command, a
flag, output, a number, a failure mode, a default. A refactor that changes no
observable behaviour gets no entry — the commit message already records it, in
Turkish, for the next maintainer.

If unsure: could someone who only downloads binaries notice? If no, stop here and
say so.

## 2. Pick the component and the kind

Component is `cli`, `desktop` or `hub`. All three live in this file even though
two of them ship from other repos — the website reads one public place.

`kind` is one of `added`, `changed`, `performance`, `fixed`, `removed`,
`security`. Output groups by kind in that order, so file order does not matter.

## 3. Write it in English first, then translate

Add to the component's `unreleased` list:

```json
{
  "kind": "fixed",
  "text": {
    "de": "…",
    "en": "…",
    "fr": "…",
    "it": "…",
    "tr": "…"
  }
}
```

**All five locales are required** — `en tr it fr de`. A missing one fails
`cargo test`. Half a translation never reaches a reader.

**Commands, flags, file names and tags are never translated.**
`--no-clone-dedupe` and `spacetrace diff` stay identical in all five; only the
prose around them changes. A translated command is false information.

**Write for someone who uses it.** Say what changed for them and, where it is not
obvious, what it was before — the existing entries do this, and they are the model
to imitate. Numbers that were measured belong in the entry; numbers that were
guessed do not.

Note that `unreleased` is never shown to a user anywhere. It becomes visible the
moment a release containing it is cut.

## 4. Validate and normalise

```bash
cargo run -q -p spacetrace-changelog -- check
cargo run -q -p spacetrace-changelog -- fmt
```

`fmt` rewrites the file in canonical form. Run it before committing anything
hand-edited, so the next `promote` produces a small diff instead of reordering
the whole file.

## 5. Regenerate CHANGELOG.md — cli only

```bash
cargo run -q -p spacetrace-changelog -- markdown --component cli > CHANGELOG.md
```

CI diffs this file and fails on a stale copy. Only the `cli` component: this repo
does not hold the other two `CHANGELOG.md` files, and desktop and hub check their
own in their own CI.

Editing `CHANGELOG.md` directly is blocked by a hook. The redirect above is the
only way it changes.

## 6. If the entry is for desktop or hub

Order matters, and skipping the middle step ships a binary whose embedded
changelog predates its own release notes:

1. commit the entry here and push
2. bump the core pin in the other repo's `Cargo.lock`
3. tag and release there

Cutting an actual release is a different job — that is the `release` skill.

## Review before finishing

- five locales present, none of them an untranslated copy of the English
- commands and flags identical across all five
- `kind` matches what actually happened
- `check` passes, `fmt` run, `CHANGELOG.md` regenerated if the component is `cli`
