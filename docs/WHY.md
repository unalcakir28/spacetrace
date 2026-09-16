# Why spacetrace?

This document describes the project's starting point, which gap it fills,
and what it will **not** do. The rationale for decisions made before writing
code is here; when a feature proposal comes in, the question "is this our
job?" gets answered from here.

## Starting point

The idea at the start was "a TreeSize clone that runs on every platform".
Market and technical research done in September 2026 showed two things
clearly:

**1. The desktop treemap market is no longer empty.** In 2025–2026 three
products shipped matching exactly this description:

| Product | Platform | Price | Note |
|------|----------|-------|-----|
| FreeSize | Win/mac/Linux | Free, Pro CHF 29/year | Markets itself as a "TreeSize alternative" |
| Diskaroo | Win/mac/Linux | $19.99 one-time | Separate native codebases (Swift + WPF) |
| DiskRaptor | Win/mac/Linux | Free, MIT | Rust + Tauri |

Putting a fourth desktop treemap on top of these means a product that
amounts to nothing but the claim "we did it better". With one of the
competitors free, that claim is indefensible: whatever feature you think is
missing gets added, and the price stays zero.

**2. The real gap is in remote machines and on the time axis.** Almost every
tool in the market answers a single question: *"what's on the disk in front
of me right now?"* Three products answer the second one — *"what changed
since last week, and on which machine?"* — and **all three have a cost**:
Windows + MSSQL + $600, or the vendor's cloud, or no concept of a remote
machine at all.

| Capability | FreeSize | dua-cli | Diskaroo | WizTree | TreeSize Pro | SpaceObServer | ncdu/gdu |
|---|---|---|---|---|---|---|---|
| Local treemap | ✅ | TUI | ✅ | Win only | Win only | Win only | TUI |
| Remote server/NAS, in a GUI | — | — | — | — | SSH/UNC, from Win | ✅ | manual, via SSH |
| Container / Docker volume | — | — | — | — | — | — | manual |
| Scan history, diff | **In the Pro portal** | **✅ v2.44.0** | — | — | Partially in Pro | ✅ | — |
| Single view of multiple machines | **In the Pro portal** | — | — | — | — | ✅ | — |
| **Self-hostable** | — (Swiss cloud) | ✅ local file | — | — | ✅ | ✅ | ✅ |
| Price | 0 / CHF 29/year | 0 (MIT) | $19.99 | 0 / $25+ | $49/year | **$600+/year** | 0 |

**Correction (9 September 2026).** The first version of this table said
"history only exists in SpaceObServer, a huge gap underneath it", and that
claim was falsified in September 2026
([COMPETITORS.md §4.3](COMPETITORS.md)): `dua-cli` v2.44.0 gave diff away for
free, and FreeSize opened a portal for CHF 29/year that says *"all your
devices at a glance: history, trends"*. The decisive row in the table
changed because of this — it is no longer "history", it is
**"self-hostable"**.

The remaining gap is the **intersection** of three things: history +
multiple machines + on your own server. The target user — homelab and small
IT teams — wants exactly this trio, and today SSHes in and runs `ncdu`,
unable to save the output, unable to compare it with two weeks ago.

## Target user

In order of priority:

1. **Homelab / VPS-owning developer.** A few Hetzner servers, a Proxmox
   host, a NAS, dozens of Docker volumes. When disk fills up, hunts down the
   culprit by navigating the tree with `du -sh *`. Does the same thing again
   a week later.
2. **Small IT team managing 5–50 servers.** Finds SpaceObServer expensive;
   the Zabbix disk alert says "85% full" but doesn't say *what* grew.
3. **Desktop user.** Anyone who wants to clean up their local disk. This
   group is the product's entry point and community source; the money
   doesn't come from here.

## Product definition

> See the disk usage on your computer, your servers, your NAS and your
> containers in one place, track it over time, find out what grew.

Local analysis will be free and good — that's the entry point. The
defensible part is the agent, the history and the multi-machine view.

**The first version of this paragraph said "for FreeSize to copy this
they'd have to write a server side, design a storage format and change their
target user" — FreeSize wrote it in September 2026** (the Pro portal). So the
expensive thing to copy was not server code. What's expensive is this:
**the agent being open source, the data staying with the user, and the
central service being self-hostable.** For a vendor with a hosted portal,
imitating this isn't a technical task, it's a decision to give up their own
revenue model. That's where we lean our differentiation.

## Why no mobile

The original idea included iOS and Android too. Technical research showed
this isn't possible:

- **iOS:** The app can only see its own sandbox. Access to anywhere else is
  limited to the subtree of the folder the user picked from Files (a
  security-scoped bookmark). There is no public API for the per-app
  accounting shown in Settings → iPhone Storage. No app on the App Store can
  show system-wide, folder-level usage.
- **Android:** A full tree scan requires `MANAGE_EXTERNAL_STORAGE`. Google
  Play grants this permission only to listed categories (file manager,
  backup, antivirus, document management, on-device search, disk
  encryption, device migration), and **disk analyzer is not on that list**.
  Even if the permission is granted, `Android/data` and `Android/obb` are
  closed off. The DiskUsage app was pulled from Play for exactly this
  reason.

If disk analysis can't be done on mobile, there's no rationale for a mobile
app either. If viewing agent data from a phone is wanted later, a web view
served by the central service is enough; there's no need to write native
mobile code.

## Out of scope (non-goals)

These will deliberately not be done. If a feature request falls into this
list, the answer is no:

- **Not a cleaner.** No "junk cleaner", "clear cache", "speed up with one
  click". In its first release the agent deletes nothing, it only reads and
  reports — the easiest way for software installed on a server to earn
  trust.
- **Not a file manager.** No copy, move, preview, archive.
- **Not an antivirus / security scanner.**
- **Not a backup tool.** Here "snapshot" means "a photograph of disk usage",
  not a copy of the data.
- **No mobile app** (rationale above).
- **No mandatory cloud.** The central service will be optional and
  self-hostable; it must be usable fully functionally without any data ever
  leaving.

## Positioning and pricing hypothesis

Pricing anchors from the research: DaisyDisk $9.99 one-time, Diskaroo
$19.99, TreeSize Personal $50, TreeSize Pro $49/year, SpaceObServer
$600+/year.

**Anchor added on 9 September 2026: FreeSize Pro, CHF 29/year.** What
matters isn't the price itself but what it delivers: background monitoring +
portal history + multiple devices — that is, the promise of both the
**Pro *and* Team tiers below**. Pricing Team per-server per-month looks
expensive to a user who can pay CHF 29/year to a single vendor. The
distinction can't be built on price; it has to be built on **self-host,
unlimited agent and open source**.

| Tier | Scope | Price idea |
|--------|--------|-------------|
| Free | CLI, **agent** (unlimited), local history, single machine via SSH | $0 |
| Pro | Desktop app: treemap, remote source browser, diff/timeline, duplicate | $29–39 lifetime or $19/year |
| Team | Central service, fleet dashboard, alerts | $5–8/server/month |

**Why is the agent in Free?** In the first draft the Pro tier was "unlimited
agent". Because the agent is Apache-2.0 open source (see
[DECISIONS.md](DECISIONS.md) K2), such a limit can't be enforced — anyone
can build it and run as many copies as they want. Making the limit
enforceable would require closing the agent, and that would directly
trigger the "the agent cannot earn trust" losing scenario below. The money
comes from what is distributed as a binary and that the user cannot easily
rewrite: the desktop and the central service.

These are unvalidated hypotheses; to be tested with the first 100 users. One
note: JAM Software switched TreeSize to a subscription in 2025 and stopped
giving updates to lifetime-license holders in July 2026. The reaction on
forums shows that **offering a lifetime license option** is an acquisition
channel in its own right.

## How we win / how we lose

**Winning conditions**
- A developer can install the agent on their server with a single command
  and, a week later, gets the answer "that folder grew by 40 GB".
- Local scanning is as fast and accurate as WizTree/FreeSize; nobody says
  "slow". Measured state (9 September 2026): **7.2× ahead** against
  FreeSize's architecture, **behind** WizTree on Windows without MFT — which
  is why speed isn't made the headline
  ([COMPETITORS.md §1.1 and §5](COMPETITORS.md)).
- We're referred to on r/selfhosted and HN as "the thing that compares ncdu
  over time".

**Losing scenarios**
- We ship on Windows without reading the MFT, get put side by side with
  WizTree, and stay slow.
- Scope bloats: the agent + central service + desktop are all attempted at
  once, and none of them gets finished. (Antidote: no central service in
  the MVP, the desktop connects directly to the agent.)
- The agent cannot earn trust. (Antidote: core and agent are open source, no
  delete permission, no telemetry.)
- Positioning drifts and we turn back into a fourth desktop treemap.

## Accuracy pledge

The one thing a disk tool cannot sell is a wrong number. Therefore:

- `size` (logical) and `alloc` (on disk) are reported separately, never
  conflated.
- Hardlinks are counted once, symlinks are not followed.
- Totals must match `du` exactly; this is a test condition, not a wish.
- Unreadable paths are not silently skipped, they are counted and reported.
- Filesystem capacity is reported as **free / total**, not as "% full". On
  APFS, btrfs and thin LVM, space is shared between volumes, so
  `total - free` also includes sibling volumes' usage and contradicts `df`
  for the same mount. This difference was caught by measurement during
  development: for the same disk the tool said "70% full" while `df` said
  "6%" (see DECISIONS.md K6).
- A forecast is **not stated** if its basis is weak. The central service's
  "full in N days" calculation is left blank if there aren't enough
  samples, enough time span, and a good enough fit (r² ≥ 0.5). A confidently
  wrong date is worse than no date at all.

## Sources

The price, date and policy information above is from research done on
6 September 2026; the part that drove the decision is kept in
[RESEARCH.md](RESEARCH.md). Competitor stacks, measurements taken on our own
machine, and each claim's evidence tag are in
[COMPETITORS.md](COMPETITORS.md) — the 9 September 2026 corrections in this
document come from there. Figures change quickly; re-verify before deciding.
