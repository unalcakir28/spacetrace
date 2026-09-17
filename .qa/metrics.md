# QA metrics

`escaped` starts at 0 and is filled in later by a postmortem, never at run time.
It is the only honest measure of whether the rest of this is working.

| Date | Level | Cases | S1 | S2 | S3 | S4 | Findings/case | False alarms killed | Harness-caused fake FAILs | Blocked | Escaped |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 2026-09-17 | L2 | 51 | 1 | 0 | 1 | 1 | 0.06 | 2 | 1 | 6 areas | 0 |

**False alarms killed: 2.** CORE-017 (the CLI accepting a file as a scan root)
survived into the written report and was only caught when the fix broke
`scanning_a_single_file_yields_a_one_node_tree`. The question that would have
killed it in Phase 3 — *is there already a passing test asserting this?* — was
never asked. That is the most expensive miss of this run.

CORE-011 (`help --help` exits 2) — the case text said
"every subcommand", but `help` is clap's meta-command, not a product subcommand.
Caught in Phase 3 by asking what anchored the expectation.

**Harness-caused fake FAIL:** a subagent's first CORE-011 pass looped over an
unquoted `$CMDS` in zsh, which does not word-split, so every subcommand looked
unrecognised. Caught by the group's own rule-7 check before it reached the
report.

**One measurement of mine was wrong mid-run** and is worth the same honesty: I
read an exit code through a pipe (`cmd | head`), so `$?` was `head`'s status,
not the program's. Re-measured with the output captured first.

**Two measurement errors of my own, both the same shape:** an exit code read
through a pipe (`cmd | head`), so `$?` belonged to `head`; and a conclusion that
deep-tree truncation was silent, drawn from output cut off by `head -6` — the
scan was in fact naming the path and the reason three lines further down. Both
were caught by re-measuring. Truncating output and then reasoning about what is
missing is the failure mode to watch here.
