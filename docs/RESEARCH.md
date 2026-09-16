# Research summary (September 2026)

The project's decisions rest on market and technical research done on 6
September 2026. This document keeps the **decision-driving** part of that
research in the repository; for product rationale see [WHY.md](WHY.md), for
design see [ARCHITECTURE.md](ARCHITECTURE.md).

**Revised on 9 September 2026.** A competitor-focused follow-up, backed by
measurements taken on our own machine and with every claim tagged with
evidence, is in [COMPETITORS.md](COMPETITORS.md). Below, the spots that
review **corrected** are marked explicitly — the old version wasn't deleted,
because if which decision was made with which information is lost, it becomes
impossible to re-evaluate the decision.

> Prices, versions and store policies change fast. If a decision rests on one
> of these, re-check the source before using it.

## 1. Competitors and price anchors

| Product | Platform | Price | Note |
|------|----------|-------|-----|
| TreeSize Free / Personal / Pro | Windows | $0 / $50 lifetime / $49.20/year | SSH-UNC-cloud scanning in Pro; **the lifetime license only remains on Personal**. Correction: **even Free reads the MFT if run as administrator** |
| SpaceObServer | Windows + server | $600/instance/year + user | Database-backed, enterprise. **Incremental scanning via USN Journal** — the closest architectural competitor |
| WizTree | Windows | Personal $0, business $25–1,800 | Reads the NTFS MFT; remote scanning is **not** available |
| WinDirStat 2.x | Windows | GPL | Correction: the 2024 speed gain was **many threads per drive**; MFT only arrived in **v2.5.0 (January 2026)** and is optional |
| DaisyDisk | macOS | $9.99 lifetime, 5 Macs | An emotional price anchor on Mac. **Deduplicates APFS clones** (4.34) — we don't have this |
| FreeSize | Win/mac/Linux | $0, Pro CHF 29/year | **.NET + Photino.Blazor** (binary examined); Pro has "Portal & history, multiple devices centrally" |
| Diskaroo | Win/mac/Linux | $19.99 lifetime | Separate native codebases (Swift + WPF) |
| DiskRaptor | Win/mac/Linux | MIT | Rust + Tauri |
| **dua-cli** | cross-platform | MIT | **v2.44.0 (30 Aug 2026): `--export` + `dua diff`** — history comparison now free in a CLI |
| ncdu 2 / gdu / dust | Linux/TUI | free | The de facto standard on servers |

**Two points that matter for the decision.**

**(a) The desktop treemap category filled up with three new products in
2025–26, one of them free** — entering as a fourth there is indefensible.
This still holds.

**(b)** The original version said: *"Remote machine + scan history only exist
in SpaceObServer, and it's $600+/year; there's nothing beneath it."* **This
sentence was falsified on 9 September 2026** — two developments happened in
the same month
([COMPETITORS.md §4.3](COMPETITORS.md)):

- `dua-cli` v2.44.0 (30 August 2026) gave history comparison for free under
  the MIT license with `--export` + `dua diff`.
- FreeSize Pro (CHF 29/year) launched a portal that says *"all your devices
  at a glance: history, trends"*.

So the gap isn't "nobody", it's **"open source + self-host + fleet history
together, nobody"**. The remaining differentiator is the intersection of this
trio; not, on its own, "compare two points in time". The price hypothesis has
to account for this narrowing too ([WHY.md](WHY.md) → Positioning).

**License window:** JAM Software moved TreeSize to a subscription in 2025,
and in July 2026 cut off updates for lifetime license holders. The backlash
was large; offering a lifetime license option is, on its own, an acquisition
argument.

## 2. Desktop framework comparison

After dropping mobile scope (see §5), the ranking:

| Framework | Win/mac/Linux | Install size | 100k rectangles | Note |
|-----------|---------------|----------------|-----------------|-----|
| **Tauri v2 + Rust** | 5 / 5 / 4 | **3–15 MB** | Canvas/WebGL, WebView differences | Calls Rust in-process, no FFI bridge. **Selected.** |
| Avalonia 12 | 5 / 5 / 5 | ~medium (NativeAOT) | Skia, pixel-identical | End-to-end C#; **fallback plan** |
| Flutter + FRB | 5 / 5 / 5 | 20–80 MB | Impeller, batched canvas | Its selling point was "five platforms, one codebase"; lost its rationale once mobile dropped |
| Qt/QML | 5 / 5 / 5 | medium | Best | License $618+/year, no C++ team |
| Electron | 5 / 5 / 5 | 50–150 MB | medium | Tauri gives the same UI in a 10× smaller package |
| .NET MAUI | 4 / 3 / **1** | — | — | Linux is officially unsupported |

Tauri's known risk is **WebKitGTK** on Linux: the treemap has to be tested on
all three WebViews (Phase 3 exit criterion).

## 3. Fast scanning techniques (for Phase 3)

There is currently only one portable backend (`read_dir` +
`symlink_metadata`). The speed expectation is set by the MFT-reading WizTree;
shipping on Windows without a fast path means losing.

### Windows
- **Direct MFT reading:** reads the NTFS Master File Table raw from disk,
  bypassing the OS. User report: WinDirStat 18 min → WizTree ~14 s. Requires
  administrator rights; ReFS has no MFT; network/FAT drives fall back to
  normal enumeration.
  Crate: [`usn-journal-rs`](https://github.com/wangfu91/usn-journal-rs)
  (MFT enumeration + **incremental** rescanning via the USN change journal),
  [`ntfs-reader`](https://lib.rs/crates/ntfs-reader).
- **The unprivileged path:** `NtQueryDirectoryFileEx`, a 64 KB+ buffer,
  `FileIdBothDirectoryInformation` — size and file id arrive in a single
  call, no extra per-file syscall needed for hardlink dedup.
  `FindFirstFileEx` + `FIND_FIRST_EX_LARGE_FETCH` cuts wall-clock time by
  ~2× in measurements
  ([measurement](https://blog.s-schoener.com/2024-06-09-find-first-large-fetch/)).

### macOS
- `getattrlistbulk`: markedly faster than `readdir + lstat` when size/date
  are needed. `dumac` is 6.39× faster than `du` with it
  ([measurement](https://healeycodes.com/maybe-the-fastest-disk-usage-program-on-macos)).
  Crate: [`getattrlistbulk-rs`](https://github.com/quivent/getattrlistbulk-rs).
- **The APFS trap:** clones share blocks, Finder can be off by tens of GB.
  DaisyDisk counts only the first appearance of a clone. Snapshots are
  invisible to a scan; `tmutil listlocalsnapshots` is needed.
- **Getting onto the Mac App Store.** Even if a sandboxed app is granted Full
  Disk Access, it doesn't bypass App Sandbox controls (Apple DTS). DaisyDisk's
  MAS version has no "scan as administrator". Distribute outside the store
  with Developer ID + notarization.

### Linux
- `getdents64` + `statx`, DFS per thread. `dut` is 6.87× faster than `du` on
  a warm cache, 2.8–3.75× faster than dust/dua/gdu
  ([dut](https://codeberg.org/201984/dut)).
- io_uring has no `getdents`; only useful for batched `statx`.
- **btrfs/ZFS:** snapshots, reflinks, compression and dedup make a tree walk
  wrong. The correct approach needs sampling, like
  [`btdu`](https://github.com/CyberShadow/btdu).

### General
Work-stealing parallel DFS on SSD, 1–2 threads on HDD/network. Memory target
on the order of ncdu 2's: **~25 B/file** (3.8M files = 162 MB).

**Measurement (9 September 2026), both sides off target:** real peak memory
is **276–437 B/entry**, i.e. 11–17× the target (extrapolated to ~2.8 GB at
10M files). **But the ~25 B target was also set wrong:** ncdu 2 doesn't keep
`own_size`, `own_alloc`, `files`, `dirs` per node — we do — so with the most
aggressive shrinking the floor is ~93 B/entry. The realistic target is
dua-cli's **64 B**.
The parallelism side holds — **5.2×** between 1 and 8 threads — but **there's
a regression at 16 threads**, so next to the "1–2 threads on HDD/network"
rule we need to add "not core-count-many on SSD either". Detail and method
in [COMPETITORS.md §1](COMPETITORS.md).

## 4. Treemap rendering (Phase 3)

- **Squarified treemap** (Bruls, Huizing, van Wijk 2000): sort children
  descending, keep adding to a row while the worst aspect ratio improves,
  otherwise fix the row and repeat with what's left.
  [PDF](https://vanwijk.win.tue.nl/stm.pdf). Write it yourself in ~300 lines
  of Rust; don't tie it to a charting library.
- **Cushion treemap** (van Wijk 1999, SequoiaView → WinDirStat): a parabolic
  bump per rectangle, surface coefficients accumulating through the
  hierarchy, a fixed light vector.
  [PDF](https://vanwijk.win.tue.nl/ctm.pdf). Fits a GPU fragment shader
  one-to-one; a cheap approximation is a radial gradient overlay.
- **For 100k+ rectangles:** compute the layout once per zoom (Rust, flat
  array) → **LOD**: stop subdividing below ~4–6 px² (at any zoom, the number
  of visible rectangles in a real tree is bounded to the thousands) →
  **culling** with a quadtree → **batched drawing** (WebGL instanced quad).
  Do hit-testing with the same spatial index, not the widget tree.

## 5. Why mobile is out of scope

- **iOS:** the app only sees its own sandbox; anything else is limited to the
  subtree of a folder picked from Files (security-scoped bookmark). There's
  no public API for the Settings → iPhone Storage per-app accounting.
- **Android:** a full scan needs `MANAGE_EXTERNAL_STORAGE`; the [Play
  policy's](https://support.google.com/googleplay/android-developer/answer/10467955)
  allow-list (file manager, backup, antivirus, document management, search,
  encryption, device transfer) **has no disk analyzer**. Even with the
  permission granted, `Android/data` and `Android/obb` stay closed. That's
  exactly why DiskUsage was pulled from Play.

## 6. Distribution and signing costs

| Channel | Cost | Note |
|-------|---------|-----|
| Apple Developer Program | $99/year | Notarization required; getting onto MAS (§3) |
| Windows: Azure Artifact Signing | $9.99/month | **Not available to individuals from Turkey** (US/Canada individuals, US/CA/EU/UK organizations) |
| Windows: OV certificate | $150–300/year | The realistic path. EV no longer bypasses SmartScreen |
| Microsoft Store | Individual $0 | Microsoft re-signs the MSIX: no SmartScreen warning. Needs `runFullTrust` for MFT |
| Linux: AppImage / AUR / deb / rpm | $0 | Unrestricted, suits a disk analyzer |
| Linux: Flatpak / Snap | $0 | Sandbox trouble: Filelight's Flatpak was reported "useless". Needs `--filesystem=host` + justification |

## 7. Projects worth studying

Reference while reading code: [dua-cli](https://github.com/Byron/dua-cli)
(jwalk, TUI; also a **64 B arena node + shared name store** and the
SHA-256-integrity `DUASNAP` snapshot format — a ready reference for TODO's B1
and A5),
[dut](https://codeberg.org/201984/dut) (the fastest Linux walker),
[gdu](https://github.com/dundee/gdu) (Go, JSON export),
[ncdu 2](https://dev.yorhel.nl/doc/ncdu2) (memory model),
[QDirStat](https://github.com/shundhammer/qdirstat) (C++ treemap reference),
[Czkawka](https://github.com/qarmin/czkawka) (single core + many UIs pattern,
duplicate-finding pipeline),
[SquirrelDisk](https://github.com/adileo/squirreldisk) (Tauri; dead but
readable).
