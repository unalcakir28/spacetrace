# Regression log

| Date | Level | Tiers | Rotated area | Cases | Verdict |
|---|---|---|---|---|---|
| 2026-09-17 | L2 | C + D (A empty — no diff) | the agent | 51 → 48 PASS / 3 FAIL, all resolved | **GO WITH RISK** (S1 fixed, macOS only) |

## 2026-09-17 — first run, so the memory starts here

Tier A was empty: the last product commit is 14 September, so this was a
state-of-the-project sweep rather than a change review.

The S1 found here was fixed in the same session at the user's request, and the
verdict moved from NO-GO to GO WITH RISK — the risk being that the fix has only
ever run on macOS.

**Owed to the next run:**

- **CORE-024 on Windows and on Linux — this is the first thing to do.** The
  fix is verified on macOS only. The default thread stack is a per-platform
  number, and `MAX_WALK_DEPTH` cannot even be reached on macOS because
  `ENAMETOOLONG` stops the walk near 475 levels. Windows has the smallest
  default stack and is the next machine in use.
- `windows_metadata.rs` compiles to **zero** tests on macOS. Whatever it
  asserts has never been exercised in a run recorded here.
- Blocked, each waiting on a machine: B4 (MFT), B7 (USN journal) — Windows;
  B6 (`getdents64`) — Linux; A6 (btrfs/ZFS) — Linux; B3 (HDD/NFS) — hardware;
  B2's >412k-entry question — a real corpus.
- Next rotation area (tier D): the **store** — `prune`, retention, schema v3
  forward compatibility, ncdu/gdu import edge cases. The agent has now had its
  pass.
