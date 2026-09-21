# spacetrace — project notes for Claude

Tool scan disk, write SQLite snapshot, compare two snapshot, say **what grew**. Rust workspace.

Read for thing code no show: [docs/WHY.md](docs/WHY.md) why product, [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) why design, [docs/DECISIONS.md](docs/DECISIONS.md) closed decision, [docs/ROADMAP.md](docs/ROADMAP.md) phase, [TODO.md](TODO.md) what next, [docs/AGENT.md](docs/AGENT.md) install agent (systemd unit: `deploy/systemd/spacetrace-agent.service`), [docs/COMPETITORS.md](docs/COMPETITORS.md) measured answer to "how tool X do this". Check WHY.md **out of scope** list before weigh feature, DECISIONS.md before reopen design decision.

## Commands

```bash
cargo test --workspace                   # 413 tests, all must pass
cargo clippy --workspace --all-targets   # must be warning-free
cargo fmt --all
cargo build --release                    # binary: target/release/spacetrace
cargo check -p spacetrace-scan-core --target x86_64-pc-windows-msvc --all-targets
cargo run -q -p spacetrace-changelog -- markdown --component cli > CHANGELOG.md
```

**`--all-targets` mandatory.** Without it test code never compile, so platform-dependent error under `#[cfg(test)]` only show in CI. Happen 11 September 2026: `parse_mountinfo` use `std::os::unix`, compile everywhere in test.

Windows type check only work for `scan-core`. `agent` and `cli` touch C code through zstd, no msvc cross compiler on macOS. Their Windows behaviour only visible in CI — **no say "work on Windows" before CI say so.**

Rust 1.85+. Test use **real file system** in temp directory, hardlink, symlink, permission error included. No mock.

## Invariants that must not break

These break silent, go unnoticed outside test.

0. **Open database must not take write lock.** `store::schema::migrate` run no DDL, no persistent pragma when schema current. Agent and hub open one connection per request, so unconditional `CREATE TABLE IF NOT EXISTS` or `PRAGMA journal_mode` let read knock over in-flight write with SQLITE_BUSY. Test: `roundtrip.rs`.
1. **Size semantics.** `size` = file byte only. `alloc` = **what disk hold**, directory block included. Directory own inode size stay out of logical total, so `size` no match `du -sb` — GNU `--apparent-size` add it.

   **`alloc` not `du`, `alloc` ≈ `df`.** Without shared block both identical, test enforce. With sharing `du` over-count and **difference exactly the shared block**, also tested. `du` dedupe hardlink but **not APFS clone**: clone have own inode and `nlink == 1` while block sit on disk once. Measured: 3 clone × 100 MB cost 0 MB free space.

   Test: `crates/scan-core/tests/du_equivalence.rs` (9 September 2026; before that, check by hand). Oracle for `alloc` is external `du`; oracle for `size` is naive serial walk in same file, because `du` cannot report logical size (BSD `-A` round to block, GNU add directory inode). Extend that file when scan behaviour change.
2. **Arena layout: two property, only two.** Child contiguous (`children_start .. +children_len`), and child index **greater** than parent. `TreeBuilder::aggregate` aggregate in one reverse pass — that what second property buy — `store` persist layout as is, `Tree::check` check exactly these two.

   **Not BFS, since 14 September 2026.** B1-K remove intermediate tree, so each directory enter arena soon as listed: **layout is order directory finish**. Both property hold by construction, since parent must be in arena to be nameable. `TreeBuilder::push_block` only way to add child.

   **So two scan no make same layout.** Same answer — `the_thread_count_does_not_change_the_answer` compare path by path — different id. `diff` match by name, desktop tie id to generation. **Write nothing new that lean on id order; that bug ship once already.** `dupes` sort by id in six place, each comment "so two run agree". Wrong since B1-K: released 0.7.0 give three order in three run on fixed fixture (measured). D6 move all six to path; scanner clone dedupe already sort by `(depth, path)`. Comment that lean on id order is not evidence — measure.

   Test it hard. `the_order_is_the_same_every_run` cannot catch it: it call `find` five time on **one** tree, where id identical anyway. New test build two layout with `from_nested`. Build them via thread count give one layout on small fixture and lose bet.
3. **Symlink not followed**; they count with own size. **Hardlink counted once** (`(dev, ino)`): both name appear, one give 0 byte. **Which name carry byte is undefined.** Walk parallel, first thread to claim inode win, vary by platform — macOS count root copy, Linux inner one, caught in CI. Guarantee is "once", not "first path", so assert over pair.
4. **`Tree::remove_subtree` zero the node, no drop it.** Cut entry out of middle renumber everything after and force every client hold id (desktop) forget everything. Entry stay addressable at 0 byte, `children_len = 0` block descent. Returned id list is what caller must stop listing.
5. **Cancelled scan return no tree.** After `ScanProgress::cancel`, `scan()` return `ErrorKind::Interrupted`. Partial tree look complete and report wrong total; write it beside real snapshot is worst outcome. **In test**, cancel *before scan start* and assert `progress.files == 0`. Cancel from side thread bet walk slower than timer — it lose on macOS CI.
6. **Measure thing sorted or drawn by is parameter, not default.** `SizeBasis` (`Logical` | `OnDisk`) run through `children_by`, `Node::measure`, `LayoutOptions.basis`. Sparse file report up to 50× what it hold — Docker.raw claim 1 TiB hold 19 GiB — and those the *biggest* entry on real disk, so logical measure most wrong where matter most. Sort and number beside it must come from same measure. Desktop default `OnDisk`, CLI `Logical`, both explicit at call site.
7. **Error not swallowed.** Unreadable path counted and sampled; scan continue. **Mount that no answer is read error too.** `entry.metadata()` never return on dead mount, cannot interrupt, so mount point (`mounts.rs`, read at scan start) approached on thread that can be abandoned. Past deadline path count as unreadable, walk move on. Read table with `MNT_NOWAIT`; `MNT_WAIT` block on dead mount and turn precaution into bug.
8. **Every long-run phase need counter that move.** Watcher answer "is it stuck" from counter alone — CLI warn after 10 second of still — so phase without one look hung while work. Clone probe do that: 1193 ms of 1989 ms scan on `~/github`, no counter move (measured 10 September 2026). Hence `clones_probed`. **Save is phase too** (14 September 2026): 412,983 entry take 753 ms to walk, 571 ms to write, so ~14 second at 10M. `Phase::Saving`, `Phase::Checksumming` (273 ms write, 208 ms digest) and `ScanProgress::rows_done`/`rows_total` exist for this, CLI progress line cover save. **`StallWatch` read counter off `ScanProgress`**, so new counter reach every watcher unchanged.
9. **Agent delete nothing.** Decision, so software installed on server can earn trust. Not missing feature.

## Code and repository habits

- **Everything here English**: code, comment, user-visible string, `--help` text, error message, all doc, this file and `docs/` included. No i18n layer for CLI, agent or hub, none planned — translated command is wrong information (K1). Commit message English going forward; Turkish history not rewritten.
- **One exception, two place: desktop GUI and changelog text in five locale** (`en tr it fr de`). That narrow K1, no repeal it; see DECISIONS K10. Terminal and server surface stay English.
- Comment explain **why done this way**, not what code do.
- Be stingy with dependency: agent must install on NAS as one static binary. Cron parser and calendar math hand-written instead of chrono (~200 line), because only need was "next matching minute". axum + tokio deliberate exception (DECISIONS K3).
- New dependency versioned in `[workspace.dependencies]`; crate take them with `foo.workspace = true`.
- Commit body explain **why**. See `git log`; older entry Turkish.
- **User-visible change need changelog entry**: `crates/changelog/changelog.json`, component `unreleased` list, five locale. No replace commit message — commit tell code what done, changelog tell user what changed (K11). `CHANGELOG.md` generated; hand edit break CI. Rule: [crates/changelog/README.md](crates/changelog/README.md).
- Leave no clippy warning; CI run `-D warnings`.
- **Two list, one temporary.** `TODO.md` at root is real one. `tasks/` (`todo.md`, `lessons.md`) gitignored session note, no authority. Close task in `TODO.md`.

## Claude tooling that lives in the repo

Some rule here enforce themselves under `.claude/` (rationale: `9f09332`), and it all come with clone.

| Tool | When |
| ---- | ---- |
| `invariant-guard` (agent) | Diff touch invariant above: scan-core, store, diff, dupes, treemap |
| `downstream-api-guard` (agent) | Public API changed, going to `main`; CI here no see desktop or hub |
| `security-reviewer` (agent) | Agent and CLI against threat model: near-root service, its token, two path that open byte off network |
| `code-reviewer` (agent) | Ordinary review, before commit |
| `test-writer` (agent) | New test, in repo style |
| `changelog-translation-reviewer` (agent) | Five locale of entry disagree: dropped negation, translated flag, drifted number. Test only prove locale **present** |
| `changelog-entry` (skill) | User-visible change: five locale, then regenerate `CHANGELOG.md` |
| `release` (skill) | Cut release; full sequence in [docs/RELEASING.md](docs/RELEASING.md) |
| `preflight` (skill) | Everything CI run, before push, cheapest first |

**Skill trigger themselves** since 16 September 2026; nobody type `/preflight`. Irreversible step of `release` — commit, tag, push — gated on approval in skill body.

Two hook run via `.claude/settings.json`: Edit/Write on generated `CHANGELOG.md` blocked (Bash redirection stay allowed, release procedure use it), and at end of session you asked once if `crates/*/src` changed while `changelog.json` no change. `.claude/hooks/rustfmt-on-edit.sh` still on disk but **nothing reference it**: plugin hook do same job in every repo and back off silent without `rustfmt`, so local wiring left `settings.local.json` on 16 September 2026, where both had run per edit.

**Cross-repo tool live in `spacetrace-tools` plugin** (`spacetrace-tooling`, next door, private), under `spacetrace-tools:` prefix. Relevant here: `doc-drift-auditor` (what diff turn false in doc), `workspace-audit` (same across five repo), `release-landed` (after release: binary downloadable, `desktop-latest` moved, site fallback bumped, site rebuilt — four post-condition no single repo can see) and `doc-number-guard` hook, which at Stop compare documented count against tree. **It no check "399 tests" above**: only exact count be `cargo test --workspace -- --list`, which need full compile and no belong in Stop hook, and static attribute count be 405 because feature-gated crate no build by default. It watch that number move against `HEAD` instead, and say so when `CLAUDE.md` no move with it. `code-reviewer` and `test-writer` exist there too, but **same-named agent here sharper and stay**; prefix stop collision. `security-reviewer` share name without being same agent — plugin one read hub credential and dashboard, this one near-root service on somebody else server. **Full list stay in plugin README**, because inventory in four place make the drift this file exist to hunt. Plugin `CHANGELOG.md` hook deliberately silent here; local one already cover it.

Vendored `web-design-guidelines` skill left with site in `4d90a05`. That commit left broken symlink under `.claude/skills/` and root `skills-lock.json`, so skill never load at all; both deleted 16 September 2026.

## Crates

| Crate | Responsibility |
| ---- | ---- |
| `scan-core` | Scan, tree model, platform metadata. Depend on nothing. |
| `store` | SQLite snapshot store, ncdu and CSV export, hash cache |
| `diff` | Compare two snapshot, spot "culprit folder" |
| `dupes` | Identical content: size → pre-hash → blake3, cache trait |
| `cli` | `spacetrace` binary, remote source included |
| `agent` | `spacetrace-agent` binary: scheduler + HTTP service |
| `treemap` | Squarified layout + LOD + hierarchical hit-test, used by desktop |
| `changelog` | Three component changelog in five locale; generator own binary |
| `buildinfo` | Stamp commit, build date, channel into binary (`build.rs`) |

Dependency direction one-way: agent depend on `scan-core`, `store`, `buildinfo`, never on `cli` or `diff`.

**`store` `dupes` feature off by default** (`dupes = ["dep:spacetrace-dupes"]`). Its hash cache pull in BLAKE3, and agent — which must stay one static binary — build from `store` too. CLI turn it on. Ask same question of next heavy dependency in `store`.

## Repositories

All four phase work. Per K2 code live in three repo:

| Repository | Contents | Visibility |
| ---- | ---- | ---- |
| this repo | all nine crate above | public, Apache-2.0 |
| [spacetrace-desktop](https://github.com/unalcakir28/spacetrace-desktop) | Tauri v2 + React desktop | private, commercial |
| [spacetrace-hub](https://github.com/unalcakir28/spacetrace-hub) | Fleet dashboard, trend, alert | private, commercial |

Both take this repo as **git dependency**, not path one. Change that break public API here break them silent. Think on that before push to `main`.

## Releases and the website

[docs/RELEASING.md](docs/RELEASING.md) own procedure: channel, container image, `RELEASE_TOKEN`, `--latest=false`, signing, version bump. What not in it, and bite:

- **Nothing build on push.** Since 19 September 2026 every release workflow — here, desktop, hub — trigger on `v*` tag and nothing else. Rolling `…continuous` pre-release gone. Their 90-day artifact had fill account 0.5 GB Actions storage and fail real release: three platform built, nothing uploaded, `publish` skipped. Hence **`retention-days: 1` on every `upload-artifact`** — bundle reach `publish` in same run, release asset are durable copy.
- **No `paths-ignore` in release workflow, deliberate.** It only apply to push. With `main` gone, its one remaining effect would be skip *tag* push whose commit touch only doc, publish nothing, silent.
- **Release not finished until website rebuilt.** Site copy changelog in at build time and stop polling 19 September 2026. Run `gh workflow run pages.yml` in `spacetrace-website`, or push there. Skip it and site keep describing previous version, no warning.
- **All three component downloadable file live in this repo release.** Desktop and hub build in own CI and publish here with `RELEASE_TOKEN` secret, because `GITHUB_TOKEN` in one repo cannot write to another, even public one. Tag: `v*` (CLI), `desktop-v*`, `hub-v*`.
- **Tag and asset name are contract whose other end in another repo.** `src/data/releases.ts` in website repo bind to them verbatim, and `install.sh` build file name from version it given. Site static link now name real version tag, so must move with each release. No single CI see both end; take two commit on same day.
- **`Cross.toml` mandatory for musl target.** Stamp (`SPACETRACE_GIT_SHA`, `SPACETRACE_BUILD_DATE`, `SPACETRACE_CHANNEL`) reach container only through its passthrough list. Incomplete list build unstamped binary, and `--version` cannot tell you.
- **`desktop-latest` not a build.** It hold one `latest.json`, which every installed desktop app read to find update. `--prerelease` keep it out of `releases/latest`, which belong to CLI. Delete it switch off update check for everyone.
- Three install script — `install.sh`, `install-desktop.sh`, `install-desktop.ps1` — read asset name from same contract.
- **Website not in this repo.** [unalcakir28/spacetrace-website](https://github.com/unalcakir28/spacetrace-website), Astro, five locale, `spacetrace.teknobakkall.com`. How it work in that repo `CLAUDE.md`; what matter here is contract above. Why split: docs/RELEASING.md → Site.

## What is next

Remaining work need real hardware or real time (full list in TODO.md): agent on real server with week of data, Windows MFT fast path, macOS Full Disk Access onboarding, treemap performance on WebKitGTK. Decision between phase closed — read [docs/DECISIONS.md](docs/DECISIONS.md) before reopen one.

Thing to watch in code:

- Snapshot on wire is **raw SQLite**. `Store::export_snapshot` copy one scan into separate file with ATTACH. `import_snapshot` reassign id but keep host/root/`started_at` triple, which duplicate check need.
- **Tree from outside go through `Tree::from_nested`.** ncdu import, and every format after, must not build own arena layout: that path call walk `TreeBuilder` + `aggregate`, or invariant 2 and aggregation get second implementation. **Directory own `asize` discarded** (invariant 1). Real ncdu write it, our exporter no, so round trip against our own output cannot see bug. **Run test through store too.** Root parent must be `NO_PARENT`; `0` give tree that look flawless in memory, fail `save` → `load` with `RootHasParent`, and send `remove_subtree` into infinite loop. In-memory compare miss exactly this 11 September 2026 and broken `import` ship.
- **`store::load` is trust boundary** — snapshot downloaded from remote arrive here, which why `TreeAssembler::finish` run `Tree::check`. Never add path that skip validation: corrupt `children_start` is index panic, backward child pointer infinite loop.
- **List on macOS have two path, both must agree.** `bulk.rs` take name and metadata in one `getattrlistbulk` call (measured 2.3–2.4× end to end). `read_dir` + `lstat` is fallback, and only path for directory containing mount, since D1 per-entry protection no work in bulk call. Second metadata source that diverge silent surface years later as "snapshot corrupt", so `assert_same_answer_as_lstat` compare the two field by field — extend it when you add field. Directory `nlink` especially: `ATTR_DIR_LINKCOUNT` is 1 on APFS, `st_nlink` is 2+subdirectory, and one `lstat` per directory reconcile them (measured: free).
- **Structural validation not value validation; second one is `content_hash`.** Bit flip in `size` field leave flawless tree with wrong number, invisible to `Tree::check`. Since schema v3 every scan carry SHA-256 of its logical content (`crates/store/src/digest.rs`): `import_snapshot` recompute it and import **nothing** on mismatch, `export_snapshot` refuse to send data it know corrupt, `spacetrace verify` check on request. `NULL` mean "pre-v3, no digest", not "corrupt". **It not authentication** — whoever can change body can recompute digest. Threat model is corruption.
- **New `scans` column go at end of both `create_tables` and `migrate_from`, same order.** `export_snapshot` run `INSERT INTO snap.scans SELECT * FROM main.scans`, and `ALTER TABLE` can only append; diverge order write every value into wrong column, silent. Test: `crates/store/tests/integrity.rs`.
- **Path built on way down, not by walk up (D6).** `rel_path` right for one entry and wrong for every entry: cost is **sum of depth**, and depth come from input, not from us — snapshot from another machine can be arbitrarily deep, so loop over it quadratic on input we no choose. Use `Tree::for_each_path`: descend append segment to buffer, ascend truncate, no allocation per node. Measured on 412,983 entry: 61 → 4.5 ms, CSV export 449 → 400 ms, ncdu export (build no path) 182 → 183, the control. It use stack, not recursion, same reason as `from_nested`; its test descend 50,000 level.
- **CSV row order strict DFS**: folder immediately followed by its content. Sibling used to stay together with content page below; D6 change it and changelog say so.
- Root scanned once at a time (`Runner::try_claim`, 409 over HTTP).
- Scheduler use UTC plus fixed offset. No time zone database.
- Capacity is **free/total**, not "% full" (K6), exposed as `capacity_of`, not `capacity::of`.
- **`ScanProgress` is cancellation switch as well as counter.** Walk check it once per directory. Per entry put shared atomic read in hottest loop, and abandon directory already read gain nothing.

## Known gaps

Known debt, not bug. **TODO.md hold measurement**; these the shape, so you recognise one when you hit it.

- **Windows `alloc` and hardlink dedupe written but verified only in CI** (9 September 2026). One handle per entry carry both query, at measured ~+36%; B4 remove it. **No use `GetCompressedFileSizeW`** — on file neither compressed nor sparse it return logical size, which CI disprove. Detail: TODO.md, B5 and B4.
- **btrfs and ZFS report real usage wrong**, because of reflink and compression. **APFS clone deduplicated** (A3, `fcntl(F_LOG2PHYS_EXT)`, on by default) — this line say opposite until 11 September 2026 and contradict invariant 1 in same file.
- **Scan hold whole tree in memory**: 96 byte/entry, linear from 100k to 10M, same on both platform (10M = 916 MiB). Peak above that platform-dependent and **still pile up on macOS**, because libmalloc no return fragmented span; not leak, agent unaffected, desktop Rescan pass `expected_entries` hint. Number and argument: TODO.md D4. Tool: `cargo run --release -p spacetrace-scan-core --example memprobe -- scan <root>` and `scripts/bench-walk.sh`.
- **Agent serve own TLS since D2** (`tls.rs`, 21 September 2026): `server.tls_cert_file` + `server.tls_key_file`, both or neither, refused at config load if half. Operator supply certificate; agent generate nothing, renew nothing, and restart pick up renewal. Client side `ca_file` in `remotes.toml`, which for self-signed certificate **be** pinning. Reverse proxy stay recommendation where domain name exist. **No new crate**: rustls, tokio-rustls already come through reqwest, hyper through axum — graph byte-identical before and after, measured. **Handshake off accept path**: `axum::serve::Listener::accept` await one connection at time, so handshake inside it serialise every new connection behind slowest; `TlsListener` queue finished handshake over mpsc instead. **`PeerAddr` be local type on purpose** — rate limit key on peer address, axum give `Connected` only for own `TcpListener`, and orphan rule forbid impl for `SocketAddr`. Test `the_rate_limiter_still_sees_the_peer_address_over_tls` be what catch that break.
- **Rate limit exist** (`ratelimit.rs`): token bucket per client address, **before** auth, so it cover `/health` and wrong token — exactly the traffic that fall outside valid token. Behind proxy every request carry proxy address and limit collapse into one shared bucket; `X-Forwarded-For` deliberately not read.
- **No mTLS, no ACME.** TLS authenticate server only; token stay sole client credential. ACME need public DNS name, and box this feature serve have none.

## Context outside this repo

Project start in Cowork (claude.ai). That session memory no carry into Claude Code — separate system — and everything worth carry was written into document here. September 2026 market and technical research in [docs/RESEARCH.md](docs/RESEARCH.md).
