# Competitor analysis (September 2026)

Record of the competitor research done on 9 September 2026 and of
**measurements taken on our own machine**. [RESEARCH.md](RESEARCH.md) holds
the market research from early September 2026; this document is its
competitor-focused, measurement-backed continuation.

> Competitor versions and prices change fast. If a decision rests on one of
> these, recheck the source before using it.

## Evidence tags

Every claim in this document is tagged with one of the following. Keeping
them separate matters: a vendor's own speed claim and an API name read from
its binary do not carry the same weight.

| Tag | Meaning |
|--------|--------|
| `[measured]` | Measured by us on this machine, reproducible with the method below |
| `[binary]` | Read from the competitor's binary/source code |
| `[vendor]` | The vendor's own statement |
| `[user]` | Third-party user report, not verified |
| `[not found]` | Searched, no source found — **no guess was made** |

---

## 1. Our own measurements

**Environment.** Mac15.9 (Apple Silicon, 16 cores), 48 GB RAM, macOS 26.5.2, APFS.
Target `/Applications` = 412,233 entries (33,661 directories + 378,572 files;
verified bit-for-bit with `find`). Warm filesystem cache, best of 5 repeats.
Cold cache not measured (`purge` requires root).

### 1.1 Shares of the slowdown

To isolate the two mechanisms that slow down a disk scanner, four
configurations were measured on the same machine `[measured]`:

| Configuration | Duration |
|---------------|------|
| Single thread + extra `stat` per entry (Python) | 8.03 s |
| Single thread, no extra syscall (Python) | 5.17 s |
| `du -s` (single thread, C) | 1.19 s |
| spacetrace, `RAYON_NUM_THREADS=1` | 5.79 s |
| **spacetrace, 8 threads** | **1.11 s** |

The two shares extracted:

- **Parallelism: 5.2×** (5.79 s → 1.11 s, same binary, thread count the only
  variable)
- **Extra `stat` per entry: +36%** (5.17 s → 8.03 s, same language, same
  single thread)

Combined, **7.2×**. 7 seconds on 400k entries; on a disk with 4M files, 10
seconds against 70.

**An important and counterintuitive result:** when spacetrace is throttled
down to a single thread (5.79 s) it is **slower** than Python (5.17 s). This
work is not CPU-bound, it is **syscall-bound** — the time goes into doing
`stat` in the kernel. So a disk tool's speed comes **not from the choice of
language, but from parallelism and the number of syscalls per entry**. "Fast
because it's Rust" is not a defensible claim; "fast because it walks in
parallel" is a measured one.

**And the natural consequence of that inference was measured (11 September
2026, B5).** If the syscall count is decisive, cutting it is the biggest win.
On macOS `getattrlistbulk` returns a directory's names *and* metadata in a
single call, which removes per-entry `lstat` entirely:

| Tree | `readdir` + `lstat` per entry | `getattrlistbulk` | Gain |
|------|--------------------------------|-------------------|--------|
| `~/github` (297,695 entries) | 1293 ms | 556 ms | **2.33×** |
| `/Applications` (412k entries) | 1554 ms | 646 ms | **2.41×** |

*Method:* two real binaries, alternating with the order changed each round,
7–9 rounds, median. The distributions do not overlap at all (`~/github`: new
max 598 ms, old min 1225 ms). If only the listing layer is measured
single-threaded it's 3.3×; end to end it's 2.3–2.4×, because tree building
and the clone probe did not change. On all three roots the two binaries'
output is bit-for-bit identical.

**This is not pulling ahead, it's catching up.** The table in §2 notes that
DiskRaptor already uses `getattrlistbulk` on macOS; the note next to B5 in
TODO.md saying "none of the competitors do this on macOS" was **wrong**
and has been corrected. The Windows (B4) and Linux (B6) counterparts are
still open, and those machines aren't on hand.

**The same inference, taken one step further (22 September 2026).** After B5
the remaining per-entry syscall was the clone probe: an `open` plus an
`fcntl(F_LOG2PHYS_EXT)` for every file whose size collided with another's, in
a phase of its own after the walk. Ablation put it at 23 ms of 74 on `/usr`,
90 of 596 on `/Applications` and 551 of 2400 on `~/Desktop/Projects` — **15%
to 31% of the scan** `[measured]`.

APFS will name the clone family inside the bulk record itself:
`ATTR_CMNEXT_CLONEID` and `ATTR_CMNEXT_EXT_FLAGS`, in `forkattr`, under
`FSOPT_ATTR_CMN_EXTENDED`. The family is keyed `(device, clone id)` and
gated on `EF_MAY_SHARE_BLOCKS` — without that gate every file would join a
family, because APFS hands every file a clone id whether or not it shares
anything. Charging happens in the walk, beside the hardlink claim, so the
phase is gone rather than faster. **dua-cli 2.45.0 does exactly this**, which
is how it got the same answer for 2–18% where we paid 15–31%.

Head to head against the previous build and against dua, interleaved in a
random order each round, warm cache, median `[measured]`:

| Corpus | before | after | dua 2.45.0 |
|--------|--------|-------|------------|
| `/usr`, 50,189 entries | 94 ms | **78 ms** | 77 ms |
| `/Applications`, 415,503 | 485 ms | **412 ms** | 448 ms |
| `~/Desktop/Projects`, 1,064,452 | 2133 ms | **1431 ms** | 1566 ms |

Part of the gain is the thread default, which moved for the same reason
(§1.2). `--no-clone-dedupe` now buys nothing measurable (398 vs 412 ms on
`/Applications`), where it used to buy 15%: correctness on APFS has stopped
costing anything at all.

**Worth keeping in view when reading that table:** `dua <path>` prints an
aggregate and keeps nothing — 24 MiB peak against our 200 — while these runs
build a full queryable arena that can be saved, diffed and served. The two
tools are not doing the same amount of work, and we are now faster anyway on
both large corpora.

### 1.2 Thread scaling

| Threads | Duration | Speedup |
|--------|------|----------|
| 1 | 5.79 s | 1.0× |
| 2 | 2.37 s | 2.4× |
| 4 | 1.58 s | 3.7× |
| 8 | **1.11 s** | **5.2×** |
| 16 | 1.27 s | 4.6× ← **regression** |

The regression at 16 threads is real: at 412k entries the per-work-item cost
shrinks enough that rayon's work-stealing coordination starts to dominate.

**10 September 2026, second measurement — and there's no such thing as the
best thread count.** The table above was taken on a single corpus (412k
entries). Repeated on two corpora, the optimum was seen to *shift* with the
size of the tree. M3 Max (12 performance + 4 efficiency cores), interleaved
runs, median of 9 samples `[measured]`:

| Corpus | 6 | 8 | 10 | 12 | 16 |
|--------|---|---|----|----|----|
| `/usr`, 50k entries | **71 ms** | 92 | 106 | 139 | 151 |
| `/Applications`, 412k entries | 1541 | 1407 | 1426 | **1233 ms** | 1581 |

6 wins on the small tree, 12 on the large one; 16 comes last on both.
**The default is now `min(cores, 8)`** — best on neither corpus (30%
behind on the small one, 14% on the large one) but beats the old default on
both (by 39% and 11%). A fixed number was never going to be best on both;
`--threads` is there for the user who knows to use it.

**Method note.** In the first attempt the settings were measured in sequence
and, because of a scan left running in the background, the same setting gave
89 ms and 170 ms in two runs — that data was discarded. Since the machine
would not go quiet, the measurement was anchored to interleaving rather than
silence: every round runs all settings in random order, so drift is spread
evenly across all of them. Round totals stayed within ±5%.

**And an experiment that didn't pan out.** A synthetic tree was generated to
push the range to 1.2M entries, and the result went unused: the generator
chained directories underneath one another, producing a deep and narrow tree
that doesn't parallelize either. Files per directory was matched, but what
actually matters for parallelism — **branching** — was missed. Above 412k is
still unmeasured.

**22 September 2026, third measurement — the optimum moved because the walk
got cheaper.** `~/Desktop/Projects` (1,064,452 entries) closes the "above 412k
unmeasured" gap with a real tree rather than a synthetic one, and the same
sweep was repeated after the clone probe moved into the bulk listing (§1.1).
Best of 4–5 runs per setting `[measured]`:

| Corpus | 4 | 5 | 6 | 7 | 8 | 10 |
|--------|---|---|---|---|---|----|
| `/usr`, 50,189 | 84 | 77 | **74 ms** | 76 | 75 | 81 |
| `/Applications`, 415,503 | 450 | 407 | 424 | **389 ms** | 403 | 457 |
| `~/Desktop/Projects`, 1,064,452 | 1553 | 1376 | **1342 ms** | 1408 | 1647 | 2009 |

**The default is now `min(cores, 6)`.** Two things changed since the 10
September table. The curve *sharpened*: with the per-file clone probe still in
the walk it was almost flat on `/usr` (92/89/96/99/93/98 ms across 4–10), so a
wrong cap cost little; it now costs 22% on the largest corpus. And the
optimum *fell*, because less work per entry means the arena lock and the
work-stealing coordination are a larger share of the run. 6 is at or within
noise of the best on all three, which is the first time one setting has been
able to say that.

### 1.3 Accuracy

`/usr/share` (19,288 files, 892 directories, 0 errors) `[measured]`:

```
du -s          → 265,641,984 bytes
total_alloc    → 265,641,984 bytes  ← bit-for-bit identical
total_size     → 524,146,086 bytes
```

**Correction (9 September 2026):** this measurement had been taken **by
hand**, and its first version said "169 tests pass, this is live proof of
invariant #1" — that was wrong. None of those 169 tests called `du`;
`totals_match_the_files_on_disk` compares against constants the test wrote
itself, and its only claim for `alloc` is `>= 4096`. The automated comparison
**was written the same day**: `crates/scan-core/tests/du_equivalence.rs` (6
tests, running in CI on three platforms, A1/A2 pending on Windows). It was
verified with mutation testing: equating `alloc` to the logical size fails 3
tests, breaking dedupe fails 3 tests, adding a directory's inode size into
the logical total fails 1 test. So the claim is now proven — but **it wasn't
until 9 September.**

### 1.4 Memory — far above target

`Node` = **104 bytes** (measured with `size_of::<Node>()`), alignment 8.

| Target | Entries | Peak RSS | Per entry (baseline subtracted) |
|-------|-------|----------|--------------------------------|
| baseline (small directory) | — | 8.1 MB | — |
| /usr | 50,132 | 26 MB | ~359 B |
| ~/github | 120,065 | 61 MB | ~437 B |
| /Applications | 412,233 | 122 MB | ~276 B |

Two costs pile on top of the 104 bytes: the `name: String` that goes to a
separate heap allocation per node (24-byte body + separate allocation +
malloc header), and the arena `Vec` growing by doubling (at the moment of
realloc the old and new buffers live side by side).

[RESEARCH.md §3](RESEARCH.md) set the target at **~25 B/file** (ncdu 2's
order of magnitude). Measured 276–437 B/entry, i.e. **11–17 times** the
target. Extrapolated to 10M files: **~2.8 GB peak memory.**

**Correction (9 September 2026):** that target is **not reachable** with our
field set, and the comparison is apples to oranges — ncdu 2 does not keep
`own_size`, `own_alloc`, `files`, `dirs` per node, we do. The breakdown taken
with a phase-by-phase RSS probe `[measured]`:

| Item | B/entry |
|-------|---------|
| `RawEntry` intermediate tree (walk phase) | 88 |
| arena `Node` | 104 |
| name `String`s | ~32 |
| fragmentation + malloc headers | ~46 |
| baseline | — |
| **total** | **290** |

When the walk finishes RSS is 77 MB, when flatten finishes it's 119 MB: **the
intermediate tree and the arena are alive at the same time**, and the freed
`RawEntry` memory does not go back to the operating system. So 192 of the 290
is double storage — that's where the real work is. Even with the most
aggressive field-shrinking, the baseline `Node` is 72 B + name ~21 B =
**~93 B/entry**. The realistic target is therefore the order of magnitude of
**dua-cli's 64 B**, not ncdu's 25 B.

Comparison points:

| Tool | Node/entry | Source |
|------|--------------------|--------|
| ncdu 2.0 | file **25 B**, dir **56 B** | `[vendor]` verified |
| ncdu 1.16 | file 78 B, dir 78 B | `[vendor]` |
| dua-cli 2.44.0 | **64 B** arena node | `[vendor]` |
| **spacetrace** (9 Sep, before) | ~276–437 B | `[measured]` |
| **spacetrace** (9 Sep, after) | **231 B** (`Node` 72 B) | `[measured]` |

`dua-cli` 2.44.0 wrote its solution up explicitly: **a 64-byte arena node +
shared filename store + dense directory ids**, peak RSS down 49% (525 MB →
268 MB). Our 104-byte + per-node `String` design is exactly the design they
abandoned.

**Re-measured 22 September 2026, and most of the table above is now stale.**
The intermediate tree went in B1-K (14 September): the walk writes each
directory straight into the arena, so the two are never alive at once. On
1,064,452 real entries `[measured]`:

| | before 22 Sep | after |
|--|--------------|-------|
| peak RSS, whole process | 299.2 MiB | **199.7 MiB** |
| peak, `memprobe` | 276.5 MiB (272 B/entry) | **182.7 MiB (178 B/entry)** |
| the tree itself | 108.8 MiB (106 B/entry) | 108.8 MiB (106 B/entry) |
| everything else | 169.0 MiB (61% of peak) | **73.9 MiB (40%)** |

The live tree never moved — `Node` is 72 B plus the name, as it has been
since 9 September. What fell is what the *walk* held on the way there: it
used to build a `PathBuf` for every entry it listed, and now builds one only
for the directories it descends into, which on a real disk is a tenth of
them. So the honest per-entry figure to compare against dua-cli's 64 B is
**106 B**, not 178: the rest is allocator retention, not structure.

Two things were tried and did not help, recorded so they are not tried again:
passing `expected_entries` so the arena never doubles made the peak **worse**
(191.3 vs 182.7 MiB), which rules out arena growth as the cause; and reusing
one bulk-listing buffer per thread instead of allocating 256 KiB per
directory did not move the wall time at all. The residue is libmalloc
declining to return fragmented spans — RSS does not fall when the tree is
dropped — which is TODO D4 and is not a leak.

For scale against the tools that keep nothing: `dua` peaks at 24.1 MiB and
`ncdu -o` at 2.5 MiB on the same tree, because neither retains a queryable
structure afterwards. `dust`, which does, peaks at 773 MiB.

---

## 2. Competitor stacks

| Product | Language / Stack | Enumeration | Parallelism |
|------|-------------|----------------|------------|
| TreeSize Free/Pro | **Delphi/VCL** `[vendor]` | **MFT** if administrator; normal otherwise `[vendor]` | 2 threads (32 in Pro), based on CPU load `[vendor]` |
| WizTree | `[not found]` | **Reads the MFT raw off disk** `[vendor]` | `[not found]` |
| WinDirStat 2.x | C++ `[binary]` | `NtQueryDirectoryFile`; **MFT only from v2.5.0 (January 2026), and optional** `[binary]` | Multiple threads per drive (v2.0.1) `[binary]` |
| SpaceObServer | Delphi + MSSQL `[vendor]` | Incremental via **USN Journal** `[vendor]` | Configurable `[vendor]` |
| SpaceSniffer | `[not found]` | `[not found]` | `[not found]` |
| DaisyDisk | `[not found]` | `[not found]` | `[not found]` |
| GrandPerspective | Objective-C `[binary]` | `[not found]` | `[not found]` |
| QDirStat | C++/Qt6 `[binary]` | `readdir` + `fstatat`, stat **sorted by inode** `[binary]` | Single thread, timer-based work queue `[binary]` |
| Filelight | C++/Qt `[binary]` | `[not found]` | `[not found]` |
| Baobab | **Vala**/GTK `[binary]` | GIO `enumerate_children_async` `[binary]` | Event loop, not a thread pool |
| btdu | **D** `[binary]` | **Not** a tree walk — Monte Carlo sampling `[vendor]` | Multi-process, an io_uring variant exists |
| ncdu 2 | **Zig** `[vendor]` | `openat` family `[vendor]` | **Single thread** (multi-thread on the roadmap) |
| ncdu 1.x | C | `chdir` + `opendir` | Single thread |
| gdu | Go `[binary]` | goroutines in parallel; **GC off** during analysis `[binary]` | `--max-cores`, `--sequential` `[binary]` |
| dust | Rust + rayon `[binary]` | Standard Rust FS API `[binary]` | rayon work-stealing |
| dua-cli | Rust `[binary]` | `[not found]` | "Parallel by default" `[vendor]` |
| dut | C `[binary]` | DFS + binary heap `[binary]` | `[not found]` |
| diskus | Rust `[binary]` | custom walker on top of rayon `[binary]` | rayon |
| erdtree | Rust `[binary]` | `[not found]` | **Empirically 3 threads** (instead of 1:1 with cores) `[binary]` |
| **FreeSize** | **.NET + Photino.Blazor** `[binary]` | **`DirectoryInfo.GetFiles`/`GetDirectories`** `[binary]` | **No trace of it** `[binary]` |
| Diskaroo | Swift (mac) + WPF/.NET 8 (Win); Linux `[not found]` `[vendor]` | **Itself admits it does not read the MFT** `[vendor]` | `[not found]` |
| DiskRaptor | Rust + Tauri 2 `[binary]` | jwalk (mac) / walkdir (Win, Linux) + `getattrlistbulk`, `FindFirstFileW` `[binary]` | rayon, jwalk |
| **spacetrace** | **Rust** | `read_dir` + `symlink_metadata` | **rayon, measured 5.2×** `[measured]` |

### 2.1 Feature matrix

| Product | History + diff | Remote agent | Platform | License / price |
|------|---------------|-----------|----------|----------------|
| **spacetrace** | **✓** | **✓** | Win/mac/Linux | Apache-2.0 core |
| SpaceObServer | ✓ full | ✓ Windows service | Windows | ~$283+ `[user]` |
| dua-cli | **✓ since v2.44.0** | ✗ | cross-platform | MIT |
| FreeSize | ◐ Pro "Portal" `[vendor]` | ◐ unclear | Win/mac/Linux | CHF 0 / **29/year** |
| TreeSize Pro | ◐ saved index comparison `[vendor]` | ◐ UNC/SSH | Windows | paid |
| gdu | ◐ save+load to SQLite, **no diff** `[binary]` | ✗ | cross-platform | MIT |
| QDirStat | ◐ cache file (not a diff) `[binary]` | ✗ | Linux | GPL |
| WizTree | ✗ | ✗ | Windows | $0 personal / $25+ business |
| WinDirStat 2.x | ✗ | ✗ | Windows | GPL-2.0 |
| DaisyDisk | ✗ | ✗ | macOS | $9.99 |
| Diskaroo | ✗ (its own table: "Time-based Comparison: No") `[vendor]` | ✗ | Win/mac/Linux | $19.99 lifetime |
| DiskRaptor | ✗ (no snapshot store in the source code) `[binary]` | ✗ | Win/mac/Linux | MIT |
| ncdu / dust / dut / diskus | ✗ | ✗ | cross-platform | open source |
| Baobab / Filelight | ✗ | mount-based | Linux | GPL |

---

## 3. FreeSize — binary forensics

FreeSize is closed-source and its technology stack is written nowhere public
`[not found]`. Its macOS `.pkg` (35,766,335 bytes) was downloaded and opened
with `pkgutil --expand-full`, and `FreeSize.app` was inspected directly.

### 3.1 Stack `[binary]`

```
FreeSize.app/Contents/MacOS/
  Photino.Native.dylib, Photino.NET.dll, Photino.Blazor.dll
  libcoreclr.dylib                              ← embedded .NET runtime
  Microsoft.AspNetCore.Components.WebView.dll   ← Blazor
  libSystem.Native.dylib, FreeSize.runtimeconfig.json
  wwwroot/{index.html, css/app.css, js/app.js}
  … 203 DLLs, 87 MB total
```

Version 0.3.1.0, `com.FreeSize.FreeSize`, signed `Developer ID Application:
LightNet (88S5ATC4M6)`, arm64-only. The Windows side is the same family:
`shared\Microsoft.NETCore.App`, `Microsoft.AspNetCore.App`,
`WebView2Loader.dll`, `Microsoft Edge WebView2 Runtime`.

**FreeSize = .NET + [Photino.Blazor](https://www.tryphotino.io/)** — the
interface is written in C# with Blazor, rendered in the operating system's
own webview.

Ruled-out alternatives `[binary]`: not Electron, not Qt, not Java — searches
for `electron`, `libffmpeg`, `node_modules`, `Qt5|Qt6`, `libjvm`,
`v8_context` all came back zero. The reason for the 74 MB Windows install is
not Chromium, it's the embedded CoreCLR.

**Note:** its architectural family is **the same** as ours — a native shell +
OS webview, exactly what Tauri does. The difference isn't in the frame, it's
in what's done inside it.

### 3.2 Why it's slow — three mechanisms

`FreeSize.Core.dll` (85 KB, the entire scan core) does metadata scanning
`[binary]`:

| API searched for | Found |
|------------|---------|
| `GetFiles`, `GetDirectories`, `DirectoryInfo` | ✓ |
| `EnumerateFiles`, `EnumerateFileSystemInfos` | **0** |
| `Parallel`, `MaxDegreeOfParallelism` | **0** |
| `ConcurrentQueue` / `ConcurrentBag` / `ConcurrentDictionary` | **0** |
| `SemaphoreSlim`, `ThreadPool` | **0** |
| `EnumerationOptions`, `RecurseSubdirectories` | **0** |

Referenced assemblies: `System.IO`, `System.IO.FileSystem.DriveInfo`,
`System.Linq`, `System.Threading`, `System.Threading.Tasks`,
`System.Threading.Thread` — **no `System.Collections.Concurrent`.**

**① No parallelism.** A parallel directory walk cannot be written without a
thread-safe work queue; there is no concurrent collection reference at all.
The presence of `Thread`/`Task` is entirely consistent with a **single**
background scan thread meant not to block the Blazor UI. The vendor never
claims multi-threading anywhere either; on the contrary it itself admits
WizTree's MFT advantage `[vendor]`.
→ Penalty we measured: **5.2×**.

**② `Get*` instead of `Enumerate*`.** In .NET, `GetFiles()` materializes the
directory's entire `FileInfo[]` array before returning; `EnumerateFiles()`
streams lazily. Microsoft's own documentation recommends `Enumerate*` for
performance. Every `FileInfo` is a heap object + a full path string: at 400k
entries, 400k objects + 400k strings → heavy GC pressure. If
`FileInfo.Length` isn't cached, a separate `stat` per file. → Penalty we
measured: **+36%**.

**③ The treemap is drawn to the DOM — probably the biggest one.** There is no
canvas in `wwwroot`: searches for `getContext`, `canvas`,
`requestAnimationFrame`, `d3`, `OffscreenCanvas` all come back **zero**
`[binary]`. `app.js` is 5,242 bytes total, and its one job is clear from its
own comment:

```js
// Measure an element's content box (for the squarified treemap layout).
measure: function (el) { ... getBoundingClientRect() ... }
```

So **the squarified layout is computed in C#, and every rectangle is created
by Blazor as a DOM element** — JS only measures the box. On top of this comes
the headline feature: *"the tree grows live during the scan"* `[vendor]`. The
result: for a tree growing while the scan runs, a constant Blazor
render-tree diff and DOM mutation crossing the .NET → WebView interop
boundary.

### 3.3 Comparison with our side

| | FreeSize | spacetrace |
|---|---|---|
| Shell | Photino.NET + OS webview | Tauri v2 + OS webview |
| Core language | C# / .NET (embedded CoreCLR) | Rust |
| Enumeration | `GetFiles` (eager array) | `read_dir` + `symlink_metadata` |
| Parallelism | no trace of it | rayon, **5.2×** `[measured]` |
| Layout | in C# | in Rust (`crates/treemap`, 21 tests) |
| Drawing | Blazor → **DOM** | **Canvas2D**, only the visible rectangles |
| Drawing during scan | yes (headline feature) | no |

### 3.4 Limits of honesty

These are **architectural evidence + a mechanism measured on our own
machine**, but FreeSize itself was not measured. The absence of an API in
the metadata is strong evidence, not mathematical certainty (obfuscation or
generic instantiation could hide names — unlikely for these APIs, but not
impossible). For an exact number the binary would have to be run and thread
count measured with `sample <pid>` and syscalls per entry with `fs_usage`.

Also: FreeSize v0.3.1, last updated June 2026, **not a single user review was
found anywhere** `[not found]` (Reddit, HN, AlternativeTo, G2, Capterra,
Softpedia each searched separately). So the speed problem is most likely part
of an "immature product" story, not a permanent architectural choice — they
may fix it in a later version.

### 3.5 Vendor and positioning

lightnet multimedia GmbH, Graben/Switzerland (UID CHE-435,553,655). The
product is one of four. Positioning: "Swiss made", "no tracking".

| Tier | Price | Scope `[vendor]` |
|--------|-------|-------------------|
| Free | CHF 0 | Unlimited scanning, treemap + sunburst + heatmap |
| Pro | **CHF 29/year** | + background monitoring, **"FreeSize Portal & history"**, **"Multiple devices, centrally"** |

The Portal text, verbatim: *"All your devices at a glance: history, trends
and usage of your volumes — hosted in Switzerland."* Because the Portal sits
behind registration, its actual depth (a rough usage graph, or a real
snapshot diff) `[not found]`. **But the marketing text overlaps directly
with our own positioning**, and hosting is on the Swiss cloud — not
self-hosted.

---

## 4. Where we're good, where we're bad

### 4.1 Real advantages

**① CLI + desktop + fleet dashboard in one product, with the same snapshot
format.** Nobody in the table does all three. The technical basis:
`store::load` doesn't know where the snapshot came from (K4).

**② The combination of open source + self-host + fleet history.** The only
other product doing this is SpaceObServer: Windows-only, MSSQL, ~$283+.
FreeSize's portal is the Swiss cloud. For the homelab/self-hosted audience
this distinction is decisive.

**③ Measurement honesty — a subject nobody else in the category talks
about.** `SizeBasis` (invariant #6), K6 (free/total, not percentage), K7 (no
guess is stated when the basis is weak). None of the competitors'
documentation has these distinctions `[not found]`. By comparison, WizTree's
own FAQ admits its total is "almost always a little under" Windows's own
figure `[vendor]`.

**④ Parallel walk on macOS/Linux.** ncdu 2 is still single-threaded. Measured
5.2×.

### 4.2 Real weaknesses

| Weakness | Who does better |
|----------|--------------|
| **No Windows MFT fast path** | WizTree, TreeSize (admin), WinDirStat 2.5.0 |
| ~~Windows `alloc` wrong + hardlink dedupe off~~ → **written on 9 September 2026, awaiting CI confirmation.** Remaining gap: on Windows, directory blocks don't enter `alloc` | TreeSize, WizTree, WinDirStat 2.5.0 |
| ~~No APFS clone deduplication~~ → **closed on 9 September 2026** (`F_LOG2PHYS_EXT`, on by default) | **DaisyDisk 4.34** (counts a clone's first appearance and gives the rest 0 bytes) |
| **Memory is 11–17× the target** | dua-cli (64 B), ncdu 2 (25 B) |
| ~~No snapshot integrity check~~ → **closed on 10 September 2026** (schema v3, content SHA-256; import rejects it, `spacetrace verify` audits it) | dua-cli (SHA-256) |
| **No incremental rescan** | SpaceObServer (USN Journal) |
| **No mode for HDD/network** | gdu (`--sequential`), QDirStat (inode ordering) |
| **The thread count does not adapt** | erdtree (empirically 3), TreeSize (based on CPU load) |
| **No code signing** | FreeSize, Diskaroo, TreeSize, WizTree — all signed |
| **Zero users, zero distribution presence** | all of them |
| **No sunburst/heatmap view** | FreeSize, Filelight |
| **No JSON import** (export exists) | ncdu |

### 4.3 Two differentiators lost in September 2026

These are the most important output of this research, because **they make
the positioning in the existing documents wrong.**

**① `dua-cli` v2.44.0 (30 August 2026) caught up on diff.**
`dua interactive --export before.dua`, then `dua diff OLD NEW` —
"additions, removals, and signed size changes as a compact, colored context
tree". Format `DUASNAP\0`, zlib stream compression, SHA-256 integrity.
Verified from the release page. **"Compare two points in time" is no longer
a differentiator on its own**; it's free in an MIT-licensed CLI. What's left
for us is **remote agent + fleet**.

**② FreeSize Pro builds exactly our sentence**, CHF 29/year.

The following sentence in [WHY.md](WHY.md) is **now wrong**: *"Remote machine +
scan history only exists in SpaceObServer, and it is $600+/year; there is
nothing underneath it."*

---

## 5. Positioning conclusions

**The most critical decision: don't headline speed.** A speed headline puts
us into an MFT fight with WizTree on Windows, and that fight is currently
being lost (§4.2). Also, as §1.1 shows, "fast because it's Rust" isn't backed
by measurement.

Headline order:

1. **"Don't wait for the disk to fill up — we tell you what's growing."** All
   the competitors answer the question "what's here now"; our answer is
   "what's changed since last week". Concretely: *"Your server will be full
   in 4 days and the culprit is `/var/log/nginx`."*
2. **"Your servers are included. The agent is open source, your data stays
   with you."** FreeSize's counterpart is the Swiss cloud; SpaceObServer's
   counterpart is $283 + MSSQL + Windows.
3. **"The numbers are correct — and we prove it."** Showing an exact match
   with `du` (a side-by-side screenshot) is a trust move nobody in the
   category makes. K6 goes alongside it.
4. **"CLI on the server, treemap on the laptop, dashboard for the team — the
   same snapshot."**

**What not to use:** "fastest" (can't be proven), "works on Windows" (risky
until MFT and `alloc` are fixed), "free" (blurs K2).

---

## 6. Sources

Measurements were taken on this machine and are reproducible with the method
in §1. Competitor sources:

- **FreeSize:** [freesize.ch](https://freesize.ch/en/),
  [treesize-alternative](https://freesize.ch/en/treesize-alternative.html),
  [wiztree-alternative](https://freesize.ch/en/wiztree-alternative.html),
  imprint; binaries downloaded via `get.freesize.ch/dl.php`
- **dua-cli:** [v2.44.0 release](https://github.com/Byron/dua-cli/releases/tag/v2.44.0)
- **ncdu 2:** [dev.yorhel.nl/doc/ncdu2](https://dev.yorhel.nl/doc/ncdu2)
- **TreeSize:** [features](https://www.jam-software.com/treesize/features.shtml),
  [NTFS notes](https://manuals.jam-software.com/treesize/EN/notesonntfs.html),
  [scan options](https://manuals.jam-software.de/treesize/EN/scan_options.html);
  Delphi confirmation from the [Embarcadero blog](https://blogs.embarcadero.com/powerful-file-and-disk-space-manager-software-for-windows-is-built-in-delphi/)
- **WizTree:** [about](https://www.diskanalyzer.com/about), [faq](https://diskanalyzer.com/faq)
- **WinDirStat:** [discussions/218](https://github.com/windirstat/windirstat/discussions/218) (maintainer's explanation), GitHub release API
- **SpaceObServer:** [agent](https://www.jam-software.com/spaceobserver/spaceobserveragent.shtml),
  [scan options](https://manuals.jam-software.com/spaceobserver/EN/scan_options.html)
- **DaisyDisk APFS clone:** [4.34 release notes](https://daisydiskapp.com/blog/daisydisk-4-34-released)
- **QDirStat:** [DirReadJob.cpp](https://github.com/shundhammer/qdirstat/blob/master/src/DirReadJob.cpp)
- **gdu:** [README](https://github.com/dundee/gdu/blob/master/README.md)
- **dut:** [codeberg](https://codeberg.org/201984/dut)
- **erdtree:** [CHANGELOG](https://github.com/solidiquis/erdtree/blob/master/CHANGELOG.md)
- **btdu:** [README](https://github.com/CyberShadow/btdu/blob/master/README.md)
- **Diskaroo:** [bravely.dev/diskaroo](https://bravely.dev/diskaroo), the vs/wiztree and vs/treesize comparison pages
- **DiskRaptor:** [github.com/SunMe1977/DiskRaptor](https://github.com/SunMe1977/DiskRaptor)
