# Accepted behaviours

Decided to be correct. Do not report these again; if one looks wrong, argue with
the reason recorded here rather than filing it fresh.

## The snapshot digest covers logical content, not file bytes

Flipping a byte in unused SQLite page space is **not** detected, and that is
right. `crates/store/src/digest.rs` explains it: `export_snapshot` builds a new
file, so a byte hash would never match what was sent, and page layout, `VACUUM`
or a different SQLite build all move the file without moving a value. "A digest
that moves on its own is worse than no digest." Tampering inside real row data
**is** caught — verified 3/3 on 17 September 2026.

It is also explicitly **not authentication**: anyone who can alter the body can
recompute the digest. The threat modelled is a flipped bit.

## CSV export defuses formula injection

`crates/store/src/csv.rs` prefixes `'` to a name beginning with `= + - @` or a
tab. Intentional, doc-commented, unit-tested
(`a_name_that_would_be_a_formula_is_defused`). The ncdu export and the stored
snapshot keep the exact byte sequence, so a CSV-only comparison will show a
difference that is not a round-trip bug.

## A single file is a valid scan root for the library

`scan()` on a regular file returns a one-node tree and succeeds.
`scanning_a_single_file_yields_a_one_node_tree` specifies it, and `du` behaves
the same way. The **agent** separately refuses it with 400 at the HTTP layer
(`serve.rs`, `not a directory`) because a scheduled scan of one file is
meaningless — the two layers differ on purpose. Reported as a divergence on
17 September 2026 and withdrawn; a change here breaks a passing test.

## `help --help` exits 2

`help` is clap's generated meta-command, not one of the 13 product subcommands.
All 13 pass. A case asserting "every subcommand" over-reached once already
(CORE-011, 17 September 2026).
