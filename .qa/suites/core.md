# Suite: spacetrace core — CLI, scanner, store, agent

**ID prefix:** `CORE`
**Level:** L2 · Standart · report-only
**Run:** 2026-09-17 · workspace 0.7.0 · macOS/APFS

## Tier plan

Tier A is **empty**: the last commit touching product code is 14 September
(`0.7.0`); everything since is documentation and tooling. So there is no diff to
test, and this run is a state-of-the-project sweep.

| Tier | Content |
|---|---|
| A — the change | **empty**, stated above |
| B — blast radius | n/a without a diff |
| C — critical flows | the CLI pipeline (scan → store → diff → export/import → verify) and the agent's HTTP surface |
| D — rotation | **the agent** — most security-sensitive component (near-root, one bearer token), and D2/TLS is still open. First rotation, so nothing is owed from a previous run. |

## Statuses

`PASS` · `FAIL` · `BLOCKED` · `NOT RUN`. A case is only `PASS` with observed
output attached.

| ID | Tier | Cat | Scenario | Basis | Expected | Status | Evidence |
|----|------|-----|----------|-------|----------|--------|----------|
| CORE-001 | C | 1 | `scan` a fixture tree, print summary | README Usage | totals reported, exit 0 | PASS | CORE-001.txt |
| CORE-002 | C | 1 | `scan --store` then `scans` lists it | README Usage | snapshot appears with label | PASS | CORE-002.txt |
| CORE-003 | C | 1 | `diff` two snapshots, culprit folder named | README Usage; crate `diff` | growth attributed to the right folder | PASS | CORE-003.txt |
| CORE-004 | C | 1 | `export` ncdu → `import` → totals identical | README Usage; store roundtrip | byte-identical totals | PASS | CORE-004.txt |
| CORE-005 | C | 1 | `ls` folders by size | README Usage | descending by size | PASS | CORE-005.txt |
| CORE-006 | C | 1 | `age` report on a tree with mixed mtimes | README Usage | buckets sum to the total | PASS | CORE-006.txt |
| CORE-007 | C | 1 | `dupes` finds identical contents | README Usage | the duplicate pair, reclaimable bytes correct | PASS | CORE-007.txt |
| CORE-008 | C | 1 | `verify` passes on a fresh snapshot | README Usage | exit 0, digest ok | PASS | CORE-008.txt |
| CORE-009 | C | 1 | agent `/health` without a token | AGENT.md HTTP API | 200, body is status+version only | PASS | CORE-009.txt |
| CORE-010 | C | 1 | agent POST `/scans` → GET `/scans/{id}` → `/download` | AGENT.md HTTP API | 200s, downloaded file opens as SQLite | PASS | CORE-010.txt |
| CORE-011 | C | 2 | every subcommand `--help` | clap contract | exit 0, names itself, no panic | PASS | CORE-011.txt — 13/13 product subcommands; clap's `help` meta-command out of scope, see report |
| CORE-012 | D | 2 | `prune` keeps newest N per target | README Usage | older ones gone, newest N intact | PASS | CORE-012.txt |
| CORE-013 | D | 2 | `rm` one snapshot | README Usage | only that one gone | PASS | CORE-013.txt |
| CORE-014 | D | 2 | CSV export | README Usage | parses as CSV, header present | PASS | CORE-014.txt |
| CORE-015 | D | 2 | `pull` from a second local agent | AGENT.md "Use it from the CLI" | snapshot lands locally, comparable | PASS | CORE-015.txt |
| CORE-016 | C | 3 | `scan` a path that does not exist | oracle: claims | non-zero exit, clear message, **no panic** | PASS | CORE-016.txt |
| CORE-017 | C | 3 | `scan` a regular file, not a directory | agent test names this a client error | clean error, no panic | PASS (reclassified) | not a bug — specified by `scanning_a_single_file_yields_a_one_node_tree`; see accepted-behaviours |
| CORE-018 | D | 3 | `diff` with a bad / missing snapshot id | oracle: claims | clean error, non-zero exit | PASS | CORE-018.txt |
| CORE-019 | D | 3 | `import` malformed / truncated JSON | store ncdu_import | clean error, DB untouched | PASS | CORE-019.txt |
| CORE-020 | D | 3 | agent: wrong bearer token | AGENT.md "All routes except /health" | 401 | PASS | CORE-020.txt |
| CORE-021 | D | 3 | agent: no Authorization header | AGENT.md | 401 | PASS | CORE-021.txt |
| CORE-022 | D | 3 | agent: POST `/scans` malformed body | AGENT.md | 4xx, **never 500**, no panic | PASS | CORE-022.txt |
| CORE-023 | C | 4 | empty directory (zero entries) | oracle: boundary | totals 0, no divide-by-zero, no crash | PASS | CORE-023.md |
| CORE-024 | D | 4 | very deep nesting (>1000 levels) | oracle: boundary | no stack overflow, completes or errors cleanly | **FAIL → fixed** | CORE-024.md + agent repro; regression test added |
| CORE-025 | D | 4 | sparse file, logical size ≫ allocated | README size semantics | `size` ≫ `alloc`, both correct | PASS | CORE-025.md |
| CORE-026 | D | 4 | `ls --top 0`, huge value | oracle: boundary | sane behaviour, no panic | PASS | CORE-026.md |
| CORE-027 | D | 4 | `prune` N=0 and N=1 | oracle: boundary | N=0 handled deliberately, not by accident | PASS | CORE-027.md |
| CORE-028 | D | 7 | unreadable directory inside the tree | README "permission-error scenarios" | counted as an error, scan completes | PASS | CORE-028.md |
| CORE-029 | D | 7 | agent: scan a path not in `[[roots]]` | AGENT.md rate-limiting §, last line | **403** with `allow_adhoc_scans` off | PASS | CORE-029.txt |
| CORE-030 | D | 7 | agent: `/health` reveals nothing else | AGENT.md security notes | no hostname, no roots, no paths | **FAIL → fixed in docs** | CORE-030.txt; AGENT.md corrected, field set now pinned by the test |
| CORE-031 | D | 7 | token comparison has no early exit | AGENT.md security notes | timing does not vary with prefix length | PASS | CORE-031.txt (t=0.04-0.15, noise floor 17us) |
| CORE-032 | D | 7 | `X-Forwarded-For` does not grant a fresh allowance | AGENT.md rate limiting | header ignored; limit still applies | PASS | CORE-032.txt |
| CORE-033 | D | 8 | re-push an identical snapshot | AGENT.md Push | second push skipped, not duplicated | PASS | CORE-033.txt |
| CORE-034 | D | 8 | tamper with a stored snapshot, then `verify` | README Usage; store integrity | verify **fails**, names the snapshot | PASS | CORE-034.md — see open question on digest scope |
| CORE-035 | D | 8 | `rm` a snapshot, then `diff` against it | oracle: claims | clean error, not a panic or a silent wrong answer | PASS | CORE-035.md |
| CORE-036 | D | 9 | two scans of the same root concurrently | oracle: concurrency | both complete, no DB corruption | PASS | CORE-036.md |
| CORE-037 | D | 9 | 20 parallel `/status` requests | oracle: concurrency | all answered consistently | PASS | CORE-037.txt |
| CORE-038 | D | 9 | rate limit: burst then over-limit | AGENT.md: 120/min, burst 60 | 429 **with `Retry-After` in seconds** | PASS | CORE-038.txt (Retry-After: 1) |
| CORE-039 | C | 10 | `alloc` vs `du` on a real tree | README: "a test condition, not an aspiration" | exact match (BSD du flags, not README's GNU ones) | PASS | CORE-039.md |
| CORE-040 | C | 10 | hardlink counted once; `--no-dedupe` flips it | README size semantics | totals differ by exactly the shared file | PASS | CORE-040.md |
| CORE-041 | C | 10 | symlink not followed, counted at own size | README size semantics | target's bytes not included | PASS | CORE-041.md |
| CORE-042 | D | 10 | capacity reported as free-of-total vs `df` | README size semantics | matches `df` Avail | PASS | CORE-042.md |
| CORE-043 | D | 11 | Turkish characters, emoji, RTL in filenames | oracle: edge data | names round-trip through scan → store → export intact | PASS | CORE-043.md |
| CORE-044 | D | 11 | filename with newline, quote, backslash | oracle: edge data | not misparsed; CSV/ncdu export stays valid | PASS | CORE-044.md |
| CORE-045 | D | 12 | snapshot DB path unwritable | oracle: claims | clean error, no partial/corrupt DB | PASS | CORE-045.md |
| CORE-046 | D | 13 | path traversal / injection via `{id}` and `--label` | oracle: claims | parameterised, no injection, no traversal | PASS | CORE-046.md |
| CORE-047 | D | 13/20 | token never written to logs or error bodies | AGENT.md security notes | absent from stdout, stderr and any 4xx body | PASS | CORE-047.txt |
| CORE-048 | D | 14 | full chain: scan → push → pull → diff | AGENT.md Push | end-to-end consistent | PASS | CORE-048.txt |
| CORE-049 | D | 19 | agent: invalid / missing config | agent `config.rs` tests | clean error naming the field | PASS | CORE-049.txt |
| CORE-050 | D | 18 | changelog present and consistent in all five locales | CLAUDE.md contract 3 | `en tr it fr de` all present, same entry set | PASS | CORE-050.md |
| CORE-051* | D | 19 | `install.sh` / `install-desktop.sh` POSIX + busybox claim | TODO Phase 2 Distribution | `sh -n` clean, no bashisms | PASS | main session, inline |

## Categories judged out, with the reason

| Cat | Why not |
|---|---|
| 5 Equivalence classes | folded into 3, 4 and 11 rather than listed separately — the inputs here are paths and numbers, and a separate pass would re-run the same cases |
| 6 Combinations | no multi-condition business rule in the core; the flag matrix is exercised through 10 and 2 |
| 16 Performance | the measured claims (B1-K 231 B/entry, B5 2.3–2.4×) belong to `invariant-guard` and want a stable machine; an L2 sweep would produce noise, not a number |
| 17 Compatibility / a11y | **no UI in this repo** — the desktop app is a separate repository and out of the chosen scope |

## Blocked at design time — see `environment.md`

Windows metadata path, MFT (B4), USN journal (B7), `getdents64` (B6),
btrfs/ZFS reflinks (A6), HDD/NFS mode (B3), >1M-entry corpus (B2's open
question). All need a machine this run does not have.
