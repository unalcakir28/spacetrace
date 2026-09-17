# Known issues

Confirmed bugs. A row leaves this file when its permanent test exists and passes.

## Open

Nothing product-breaking is open. One **wording question** remains, which is a
decision rather than a defect: the workspace `CLAUDE.md` says `unreleased` is
never shown on a user-facing surface, while `render.rs:47` deliberately writes
`## Unreleased` into the generated public `CHANGELOG.md`.

## Fixed 17 September 2026

### CORE-024 — a ~210-level-deep directory aborted the process (was S1)

Pre-existing in 0.7.0. 209 levels completed, 210 aborted with
`fatal runtime error: stack overflow` (SIGABRT). In the agent the **whole
process** died: port closed, scheduler gone, no snapshot written.

Cause: `walk()` recurses per level on the rayon pool and `walk_pool()` set no
`.stack_size()`. Fixed with `WALK_STACK_BYTES` (16 MiB) plus `MAX_WALK_DEPTH`
(1024), which records an error through `note_error` instead of descending.

Test: `a_deeply_nested_tree_does_not_abort_the_process`.

**Not closed until it is re-run on Windows and Linux.** The threshold was a
per-platform default stack, and `MAX_WALK_DEPTH` is unreachable on macOS because
`ENAMETOOLONG` stops the walk near 475 levels first.

### CORE-030 — `/health` and its documentation disagreed (was S3)

The body carries `commit` and `channel`; `docs/AGENT.md` claimed `status` and
`version` only. **Fixed in the document.** The code has an explicit reason for
`commit` and the real invariant — no roots, no hostname — was never violated.
The test now pins the exact field set.

### README — two stale claims (was S4)

`# 150 tests` against a measured 402, and a `crates/` tree listing 7 of 9. The
count was **removed** rather than corrected: it was 403 by the end of the same
run.

## Withdrawn

### CORE-017 — the CLI accepts a file as a scan root

Reported as a divergence from the agent's 400. **Not a bug.**
`scanning_a_single_file_yields_a_one_node_tree` specifies the library's
behaviour, and the attempted fix broke it. The library is permissive like `du`;
the agent is strict because a scheduled scan of one file is meaningless. Moved
to `accepted-behaviours.md`.
