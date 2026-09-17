# QA memory — spacetrace core

Started 17 September 2026. Read this first; it is the index.

| File | What it holds |
|---|---|
| `environment.md` | the target, its oracles, what is real vs absent, how to authenticate |
| `critical-flows.md` | smoke these every run regardless of the diff |
| `known-issues.md` | confirmed bugs still open |
| `accepted-behaviours.md` | decided-correct behaviour — do not re-report |
| `regression-log.md` | one row per run, plus what the next run owes |
| `metrics.md` | cases, findings, false alarms, escapes |
| `suites/` | the case lists; re-run and extend, do not start fresh |
| `reports/` | one report per run |
| `contracts/` | snapshots of public contracts, diffed at the start of a run |
| `evidence/` | raw output per case — **gitignored**, may contain tokens |

## Suites

| Suite | Prefix | Cases | Last run | Verdict |
|---|---|---|---|---|
| `suites/core.md` | `CORE` | 51 | 2026-09-17 | GO WITH RISK — S1 fixed, verified on macOS only |

## Not covered by any suite yet

The **desktop app** and the **hub** have no QA memory at all — they are separate
repositories and were outside the scope chosen for the first run. The
**website** has none either.

Within the core, the rotation has covered the **agent**. The **store** is next:
prune and retention, schema v3 forward compatibility, ncdu/gdu import edge
cases.
