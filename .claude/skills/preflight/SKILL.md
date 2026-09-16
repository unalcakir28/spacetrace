---
name: preflight
description: Runs everything CI runs for the core workspace, in the order that fails cheapest first. Use before any push to main, after finishing a change and before committing it, when a public API in the consumed crates changed, and whenever someone asks whether CI will pass — including Turkish phrasings like "push etmeden önce kontrol et", "her şey yeşil mi", "CI geçer mi".
---

# Preflight

CI runs a three-platform test matrix and a separate lint job. Everything below
except the Windows step is something CI will run anyway; the point of running it
here is that a red CI costs a round trip and a push to `main` that breaks the two
downstream repos costs more than that.

Run the steps **in order** and stop at the first failure — each is slower than
the one before it.

## 1. Formatting

```bash
cargo fmt --all --check
```

Seconds. Fails its own CI job. If it fails, run `cargo fmt --all` and continue.

## 2. Clippy

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

**`--all-targets` is not optional.** Without it test code is never compiled, and a
platform-specific error under `#[cfg(test)]` first appears in CI — which is
exactly what happened on 11 September 2026 with `parse_mountinfo`.

CI uses `-D warnings`, so a warning here is a failure there.

## 3. Tests

```bash
cargo test --workspace
```

All of them must pass. These use real temp-directory filesystems, so a failure
can be environmental (disk full, permissions) — read the failure before assuming
the code is wrong.

## 4. Windows type check

```bash
cargo check -p spacetrace-scan-core --target x86_64-pc-windows-msvc --all-targets
```

`scan-core` only. `agent` and `cli` link C code through zstd and there is no msvc
cross-compiler on macOS, so their Windows behaviour is visible only in CI.

**Do not report "works on Windows" on the strength of this step.** It is a type
check on one crate. If the change touches Windows behaviour, say it needs CI.

## 5. Changelog

```bash
cargo run -q -p spacetrace-changelog -- check
cargo run -q -p spacetrace-changelog -- markdown --component cli > /tmp/CHANGELOG.md
diff -u CHANGELOG.md /tmp/CHANGELOG.md
```

An empty diff is the pass. If it differs, regenerate into the real file:

```bash
cargo run -q -p spacetrace-changelog -- markdown --component cli > CHANGELOG.md
```

Then ask whether the change needs an entry of its own — see the `changelog-entry`
skill. CI checks that the file is current, not that it is complete.

## 6. Downstream

Not a command. If the diff changed a public item in `scan-core`, `store`, `diff`,
`treemap`, `changelog` or `buildinfo`, `spacetrace-desktop` and
`spacetrace-hub` consume them over a git dependency and **no CI in this repo
compiles them**. Run the `downstream-api-guard` agent before pushing.

## Report

State each step's result plainly, with the failing output when something is red.
A step that was skipped is reported as skipped, not as passed. Finish with a
one-line verdict: safe to push, or what is blocking.
