/**
 * The source dictionary. Every other language is typed against this shape, so
 * a key added here and forgotten elsewhere is a compile error rather than a
 * blank spot on a live page.
 *
 * Commands, flags, file names and CLI output are NOT in here: the tool itself
 * is English (see docs/DECISIONS.md K1) and a translated command would be
 * false information.
 */
export const en = {
  locale: {
    label: "Language",
    pick: "Choose a language",
  },

  nav: {
    desktop: "Desktop app",
    cli: "Command line",
    hub: "Hub",
    download: "Download",
    guide: "Guide",
    github: "GitHub",
    menu: "Menu",
    skip: "Skip to content",
  },

  common: {
    copy: "Copy",
    copied: "Copied",
    copyCommand: "Copy command",
    replay: "Replay",
    download: "Download",
    free: "Free",
    size: "Size",
    file: "File",
    platform: "Platform",
    notes: "Notes",
    version: "Version",
    latestBuild: "latest build",
    stableRelease: "stable release",
    notARelease: "built from main, not a release",
    checksums: "Checksums",
    learnMore: "Learn more",
    sourceOnGitHub: "Source on GitHub",
    apacheCore: "Apache-2.0",
    proprietary: "Proprietary",
    loading: "Loading…",
  },

  home: {
    title: "spacetrace — see what fills your disk, and what changed",
    description:
      "A desktop app that maps what is filling your disks, keeps snapshots, and tells you which folder actually grew. With a command line tool and a self-hosted fleet dashboard for servers.",
    kicker: "macOS · Windows · Linux",
    headlineGiven: "Every disk analyser maps what is on the disk.",
    headlineSecond: "This one also shows what changed.",
    lede: "Scan a drive and browse it as a live treemap. Keep the result as a snapshot, come back next week, and get the one folder that actually grew — not the root it happens to sit under.",
    ctaPrimary: "Download the app",
    ctaSecondary: "Or use the command line",
    noAccount: "No account, no telemetry, no upload of anything.",

    demoTitle: "This is the app. Take it for a drive.",
    demoLede:
      "The real thing renders tens of thousands of tiles on a canvas. This one runs the same squarified layout on a smaller tree, right here in the page.",

    diffTitle: "One scan is a photograph. Two are an answer.",
    diffLede:
      "Save a scan as a snapshot, then compare. Folders that merely pass a change through get skipped, and the first level where growth genuinely spreads out is the one reported.",
    diffPoint:
      "It names the folder you can act on, not its ancestors.",

    partsTitle: "One core, three ways in",
    partsLede:
      "The scanner, the snapshot store, the diff and the treemap layout are one Rust library. A snapshot written by any of these is readable by the other two.",

    trustTitle: "Numbers you can check",
    trustLede:
      "A disk tool that prints a figure you cannot reconcile with your own shell has lost the argument. These are test conditions in the repository, not promises.",

    ctaTitle: "Find out what grew while you were not looking.",
    ctaLede:
      "The desktop app is free to download. The scanner, the command line tool and the server agent are open source and always will be.",
  },

  demo: {
    zoomHint: "Double-click a folder to zoom in",
    backHint: "Go up",
    rootLabel: "Home",
    basisLabel: "Measure",
    onDisk: "On disk",
    logical: "Logical",
    basisHelp:
      "On disk is the blocks actually allocated. Logical is the length each file claims. They disagree, and both are right.",
    sparseCallout:
      "Switch to Logical and watch the disk image swallow the map. It claims a terabyte and holds nineteen gigabytes — that is why the app defaults to on disk.",
    selected: "Selected",
    share: "Share of this scan",
    kind: "Kind",
    sparse: "sparse",
    empty: "Nothing selected",
    emptyHelp: "Click a tile to inspect it.",
    entries: "entries",
    tapHint: "Tap a folder twice to zoom in",
  },

  categories: {
    image: "image",
    video: "video",
    audio: "audio",
    document: "document",
    archive: "archive",
    code: "code",
    binary: "binary",
    cache: "cache",
    other: "other",
    directory: "folder",
  },

  parts: {
    desktopName: "Desktop app",
    desktopRole: "macOS · Windows · Linux",
    desktopPitch:
      "A zoomable treemap coloured by file type, with the folder list, the map and the inspector all agreeing on which measure they show.",
    desktopPoints: [
      "Browse a live scan, a stored snapshot, or a server over HTTP",
      "Compare two snapshots and see the folder that grew",
      "Move entries to the Trash without losing your place",
      "The window stays usable while a scan runs",
    ],
    desktopCta: "What the app does",

    cliName: "Command line",
    cliRole: "one static binary · plus a server agent",
    cliPitch:
      "The same core as a command you can script, and as a service that scans on a schedule and answers over HTTP.",
    cliPoints: [
      "Scan, snapshot, diff, list, export to ncdu",
      "Point any read-only command at a remote agent",
      "Runs on a NAS, in a container, on elderly glibc",
      "The agent reads. It never deletes anything.",
    ],
    cliCta: "Commands and setup",

    hubName: "Hub",
    hubRole: "self-hosted · one binary, one SQLite file",
    hubPitch:
      "Agents push their snapshots to it. It keeps the history, works out what is growing, and tells you before a disk fills up.",
    hubPoints: [
      "Fleet dashboard ordered by urgency, not by hostname",
      "Growth rate and a “fills in N days” forecast",
      "Threshold rules delivered by webhook",
      "Never touches the machines it watches",
    ],
    hubCta: "Run the hub",
  },

  trust: {
    duTitle: "Totals match your shell exactly",
    duBody:
      "Logical is file bytes only and matches du -sb. On disk is the blocks actually allocated, directory blocks included, and matches du -s --block-size=1. Verified on /usr with 141k files.",
    capacityTitle: "Free of total, never “% used”",
    capacityBody:
      "Capacity is reported the way df reports it. On an APFS container, a btrfs subvolume or thin LVM, a used figure includes the siblings and disagrees with df about the same mount — so it is not printed.",
    forecastTitle: "The forecast is withheld more often than shown",
    forecastBody:
      "“Fills in N days” needs at least three snapshots spanning a day, a linear fit of r² ≥ 0.5, a capacity that was actually measured, and an answer inside ten years. A confident wrong date is worse than no date.",
    snapshotTitle: "A snapshot is a plain SQLite file",
    snapshotBody:
      "Not an export format, not a proprietary blob. What the agent stores is what travels over the wire and what lands on your laptop, so a remote snapshot is browsed by the identical code as a local one.",
    errorsTitle: "Errors are counted, not swallowed",
    errorsBody:
      "An unreadable path is reported and sampled; the scan carries on. A cancelled scan returns no tree at all, because a partial tree looks complete and reports a total that was never true of any disk.",
    deleteTitle: "The agent has no delete",
    deleteBody:
      "Software you install on a server earns trust by not being able to do the frightening thing. The desktop app can move entries to the Trash — only on a live scan of the machine you are sitting at, and only after asking.",
  },

  desktop: {
    title: "Desktop app — spacetrace",
    description:
      "A zoomable treemap of what is filling your disks, coloured by file type, with snapshots, remote agents and diffs. macOS, Windows and Linux.",
    kicker: "Tauri and Rust · a small window over a native scanner",
    headlineGiven: "A map of your disk.",
    headlineSecond: "Then a map of what changed on it.",
    lede: "Scan a folder and browse it as a treemap coloured by file type. Or open a snapshot from last week, or one sitting on a server, and put the two side by side.",
    ctaPrimary: "Download the app",
    ctaSecondary: "See every file",

    windowCaption:
      "Drawn from the application's own stylesheet rather than screenshotted, so it stays true as long as the palette does.",

    featuresTitle: "What it is like to use",
    featuresLede:
      "The app is a window onto the same Rust core the command line uses. Its own job is to stay out of the way.",

    f1Title: "The window never goes away while it works",
    f1Body:
      "A scan reports itself in a strip, not behind a curtain. Whatever was open stays open and stays usable, and is only replaced when the new scan finishes. Stop it, or let it fail, and the previous scan is still there.",
    f2Title: "Zoom in without losing your place",
    f2Body:
      "Double-click a folder in the map, pick one from the breadcrumb, or use a row's context menu. Backspace goes back up. A click in the folder list never moves the map, so browsing cannot lose the view you were looking at.",
    f3Title: "The progress bar admits when it is guessing",
    f3Body:
      "A filesystem walk has no denominator. The one honest estimate is the last time this app scanned that same folder with the same options, so that is what it uses — and the strip says where the number came from. A first scan shows counters and motion but no percentage.",
    f4Title: "Deleting does not reload anything",
    f4Body:
      "Moving entries to the Trash edits the open tree in place and corrects the totals above it, so expanded folders stay expanded, the map stays put and the scroll position does not move. A hundred entries is the same call as one, with a real “3 of 11”.",
    f5Title: "Moving to Trash frees no space, and the app says so",
    f5Body:
      "On every supported platform the Trash is on the same filesystem, so the folder gets smaller and the disk does not. The confirmation says it, the notice afterwards says it, and the free-space readout can be clicked to re-measure once you have emptied it.",
    f6Title: "Sizes always say which measure they are",
    f6Body:
      "On disk by default, with a labelled switch in the toolbar rather than a buried setting. The ordering, the figure beside it and the row's colour all come from the same measure, so “biggest first” means the same thing as the number printed on the row.",

    sourcesTitle: "Three sources, one window",
    sourcesLede:
      "The app does not care where a tree came from, because a snapshot is the same SQLite file wherever it was written.",
    s1Title: "This machine",
    s1Body:
      "A live scan. The only source where acting on files is allowed, because it is the only one where the paths still mean what they say.",
    s2Title: "A stored snapshot",
    s2Body:
      "From the same database the command line writes. Save the open scan and it shows up in spacetrace scans, and the other way round.",
    s3Title: "A remote agent",
    s3Body:
      "Point the app at an agent's URL and token and read its snapshots over HTTP — the server never has to be logged in to.",
    s4Title: "Two of them at once",
    s4Body:
      "Compare any two snapshots of the same root and see which folder actually grew, with the same culprit-folder rule the command line uses.",

    safetyTitle: "What it will not do",
    safetyBody1:
      "The app reads. The single exception is Move to Trash, and it is gated four ways: the backend refuses unless the open tree is a live scan of this machine, it refuses the scan root itself, it re-checks that every path still exists, and the window asks first — listing what is about to go when there is more than one.",
    safetyBody2:
      "Nothing is ever deleted outright. It goes to the system Trash, where you can put it back.",
    safetyBody3:
      "Acting on files is disabled whenever a snapshot is open. A snapshot is a photograph of the past, or of another machine, and a path in it may no longer mean what it says.",

    notYetTitle: "Not done yet",
    notYetLede: "Stated rather than discovered:",
    notYet: [
      "The Windows MFT fast path — scans there are a normal directory walk",
      "A macOS Full Disk Access onboarding screen",
      "A timeline view of one target's whole history, rather than two snapshots at a time",
      "Treemap performance on WebKitGTK, the Linux renderer, has not been tuned",
    ],
  },

  cli: {
    title: "Command line — spacetrace",
    description:
      "The spacetrace command line tool and server agent: scan, snapshot, diff, and read a remote machine over HTTP. One static binary, Apache-2.0.",
    kicker: "One static binary · no runtime to install · Apache-2.0",
    headlineGiven: "The same core, without a window.",
    headlineSecond: "Scriptable, and at home on a server.",
    lede: "Everything the app does, as a command you can put in a cron job — plus an agent that scans on a schedule and answers over HTTP, so a machine you never log in to can still tell you what happened to its disk.",
    installTitle: "Install",
    installNote:
      "Installs spacetrace and spacetrace-agent into /usr/local/bin. Deliberately boring POSIX sh, because it also has to run on NAS firmware whose shell is busybox.",
    windowsNote:
      "On Windows, unzip the archive and put spacetrace.exe on your PATH. There is no installer for the command line tool.",

    toursTitle: "The four commands you will actually use",
    tour1Title: "Look at a folder",
    tour1Body: "Walks it and prints the totals. Nothing is stored, nothing is changed.",
    tour2Title: "Keep the result",
    tour2Body: "Stores the scan as a snapshot in a plain SQLite file, with a label if you want one.",
    tour3Title: "Ask what changed",
    tour3Body: "Compares the last two snapshots of a root, or the newest one against the disk right now.",
    tour4Title: "Read another machine",
    tour4Body: "Any read-only command takes --remote. Same subcommands, pointed elsewhere.",

    agentTitle: "The agent",
    agentLede:
      "spacetrace-agent is the same code as a service. It scans the roots you configure on a schedule, keeps the snapshots, and serves them over HTTP. One static binary — and it only ever reads.",
    agentPoints: [
      "Five-field cron schedules per root, with retention by count",
      "Bearer token required; serve refuses to start without one",
      "Binds loopback by default — exposing an inventory should be a deliberate edit",
      "/health needs no token so a container healthcheck works, and reveals nothing else",
      "Unknown config keys are rejected at startup rather than silently ignored",
    ],
    agentCta: "Full agent setup",

    dockerTitle: "In a container",
    dockerNote:
      "Mount the host read-only and scan that. The image carries both amd64 and arm64.",

    openTitle: "Open source, and staying that way",
    openBody:
      "The scanner, the snapshot store, the diff and both binaries are Apache-2.0. The agent runs on your servers, so you should be able to read it. The desktop app and the hub are the commercial part.",
  },

  hub: {
    title: "Hub — spacetrace",
    description:
      "The self-hosted spacetrace hub: agents push snapshots, the hub keeps the history, works out what is growing, and tells you before a disk fills up.",
    kicker: "Self-hosted · one binary, one SQLite file · no telemetry",
    headlineGiven: "Forty machines, forty disks.",
    headlineSecond: "One page, worst first.",
    lede: "Agents scan their own machines and push the snapshots here. The hub keeps the history, works out what is growing, and tells you before a disk fills up. It never touches the machines it watches.",
    ctaBinaries: "Binaries",
    dashCaption:
      "Drawn rather than screenshotted. The real dashboard is server-rendered HTML — a self-hosted tool that needs an npm install before it will show you a page is a worse tool.",
    withheldNote:
      "A blank forecast is a refusal to guess, not a missing number. The target's own page says which condition failed.",

    setupTitle: "Set it up",
    setupLede:
      "Two credentials, one config file, and one line added to each agent. There is nothing else to stand up.",
    step1Title: "Make an admin token and a config",
    step1Body:
      "Save it as hub.toml, then docker compose up -d — or run the binary directly and validate the config before serving.",
    step2Title: "Sign in and create an agent token",
    step2Body:
      "Open the hub, sign in with the contents of admin-token, and create a token on the Agents page. Or do it from the command line.",
    step3Title: "Point each agent at it",
    step3Body:
      "Add that to the agent's schedule and the fleet fills itself in. Re-pushing is harmless: a snapshot already present with the same host, root and start time is skipped.",
    step4Title: "Put TLS in front of it",
    step4Body:
      "Before exposing the hub beyond your own network, terminate TLS in a reverse proxy. The session cookie is HttpOnly and SameSite=Strict; the Secure flag comes from serving it over HTTPS.",

    credsTitle: "Two credentials, on purpose",
    credsBody1:
      "A token sitting in a config file on a NAS should not be a key to the whole fleet's inventory. So an agent token is not a dashboard credential — and the reverse holds too.",
    credsBody2:
      "Agent tokens are stored as SHA-256 hashes, so a copy of the database is not a set of working credentials. Revoking keeps the record and stops the token working immediately.",
    credsTested: "Both directions are covered by tests.",
    credCol: "Credential",
    credCan: "Can do",
    credCannot: "Cannot do",
    agentToken: "Agent token",
    adminToken: "Admin token",
    agentCan: "Push a snapshot",
    agentCannot: "Read the dashboard or the API",
    adminCan: "Read everything, manage tokens and rules",
    adminCannot: "Push snapshots",

    forecastTitle: "The forecast is the feature held back hardest",
    forecastLede:
      "It is the one most likely to be wrong, so it is the one that refuses to answer most often. “Fills in N days” appears only when all of these hold.",
    fc1Title: "At least three snapshots, spanning at least a day",
    fc1Body:
      "Two points always make a perfect line. A day is the shortest span in which a daily pattern can show up at all.",
    fc2Title: "A linear fit of r² ≥ 0.5",
    fc2Body:
      "Disk usage is frequently not linear. A log rotation or a one-off restore will happily produce a fitted line whose slope means nothing.",
    fc3Title: "A capacity that was actually measured",
    fc3Body:
      "Without the filesystem's real size there is nothing to fill. The hub does not guess it from the largest total it has seen.",
    fc4Title: "An answer inside ten years",
    fc4Body: "“Fills in 4,000 days” is arithmetic, not information.",
    fc5Title: "Growth and the forecast are different numbers",
    fc5Body:
      "Growth is measured on the scanned folder. The forecast projects the filesystem's remaining space at that rate. The pages say which is which rather than blending them into one reassuring figure.",

    alertsTitle: "Alerts",
    alertsLede:
      "Threshold rules, delivered by webhook. Three kinds, each with a cooldown so a filling disk does not become a stream of identical messages:",
    alerts: [
      "Free space below a percentage",
      "Growth above a rate",
      "A forecast landing inside a horizon",
    ],
    alertsMissing:
      "Email delivery is not built yet, and there is one admin credential rather than per-person logins.",

    apiTitle: "HTTP API",
    noRollupTitle: "Why there is no rollup table",
    noRollupBody1:
      "Snapshots are kept in the same store the command line and the agent use, unchanged. The hub keeps no summary table of its own, so there is nothing that can drift out of step with the snapshots it came from.",
    noRollupBody2:
      "What an agent pushes is the same file it stores locally, so nothing is converted on either side, and the diff on a target's page is computed by the same code that runs on your laptop.",

    fleetHost: "Host",
    fleetRoot: "Root",
    fleetFree: "Free",
    fleetGrowth: "Growth",
    fleetTrend: "Last 14 days",
    fleetFills: "Fills in",
    fleetDays: "days",
    fleetTargets: "targets",
    fleetReporting: "reporting",
    fleetSilent: "silent for 3 days",
    fleetNoCapacity: "capacity not recorded",
    fleetTwoSnapshots: "2 snapshots",
  },

  download: {
    title: "Download spacetrace",
    description:
      "Downloads and install instructions for the spacetrace desktop app, the command line tool and agent, and the self-hosted hub. macOS, Windows, Linux, NAS and Docker.",
    headline: "Download",
    lede: "Three things you can install, all built from the same core. Take the one you need; a snapshot written by any of them is readable by the others.",

    detectMac: "macOS",
    detectMacSub: "Universal — Apple silicon and Intel",
    detectWindows: "Windows",
    detectWindowsSub: "64-bit",
    detectLinux: "Linux",
    detectLinuxSub: "Static binaries — no runtime to install",
    detectUnknown: "Pick a platform",
    detectUnknownSub:
      "This browser did not say which machine it is on. The tables below list every file.",
    goToFiles: "Go to the file lists",
    alsoCli: "Command line tool and agent",
    cliAlongside:
      "The app and the command line tool share one snapshot database, so they can be used together.",

    continuousTitle: "What “continuous” means",
    continuousBody:
      "Until the first tagged release, every download here is the newest build of main: it has passed CI and nothing else. When a stable release exists this page switches to it automatically.",

    desktopSection: "Desktop app",
    desktopSectionLede:
      "The treemap window. Free to download while it is in phase 3.",
    desktopCaption:
      "The macOS build is universal, so there is one file for Apple silicon and Intel.",

    cliSection: "Command line and agent",
    cliSectionLede:
      "One archive holds both binaries: spacetrace for the command line and spacetrace-agent for the service that scans on a schedule and answers over HTTP.",
    cliCaption:
      "Linux builds are static musl, so they run on old glibc and inside a scratch container. Every archive also holds the README and the licence.",

    hubSection: "Hub",
    hubSectionLede:
      "The fleet view. One binary, one SQLite file, no database server and no frontend build.",
    hubCaption:
      "The container image carries both architectures, so docker pull gets the right one without a tag suffix.",

    unsignedTitle: "Nothing here is code-signed",
    unsignedBody:
      "Signing certificates cost money per year and cannot live in a public repository, so macOS and Windows will both stop you the first time. That is worth being suspicious about, which is why a checksum is published beside every file and the build is a workflow you can read.",

    installMacTitle: "macOS: “spacetrace can’t be opened”",
    installMacBody:
      "Open the .dmg, drag the app to Applications, then right-click it and choose Open — that gives the one-time permission a double-click does not offer. Or clear the quarantine flag directly:",
    installMacFda:
      "Scanning outside your home folder needs Full Disk Access for the app, in System Settings → Privacy & Security.",
    installWinTitle: "Windows: SmartScreen",
    installWinBody:
      "“Windows protected your PC” → More info → Run anyway. The installer is per-user and needs no administrator rights.",
    installLinuxTitle: "Linux",
    installLinuxBody:
      "The AppImage needs no install at all: mark it executable and run it. On Wayland, if the window comes up blank, run it with WEBKIT_DISABLE_DMABUF_RENDERER=1 — a WebKitGTK issue rather than an app one.",

    verifyTitle: "Check what you downloaded",
    verifyBody:
      "Every release carries a SHA256SUMS file covering its own assets. Verify before you install, especially since none of the binaries are signed.",
    verifyMacNote: "macOS has no sha256sum; use shasum -a 256 -c instead.",

    buildTitle: "Or build it yourself",
    buildBody:
      "The core, the command line tool and the agent are Apache-2.0 and need only Rust 1.85 or newer.",
    buildNote:
      "The desktop app and the hub are not open source, so they cannot be built from source — that is the trade for the core staying permissively licensed.",

    platMacArm: "macOS, Apple silicon",
    platMacIntel: "macOS, Intel",
    platMacAny: "macOS 11+",
    platWin: "Windows 10 / 11",
    platLinuxX64: "Linux, x86_64",
    platLinuxArm: "Linux, aarch64",
    platDebian: "Debian, Ubuntu",
    platFedora: "Fedora, RHEL",
    platAnyLinux: "Any Linux",
    noteUniversal: "Universal binary",
    noteNsis: "NSIS installer, per-user",
    noteDeb: "Needs libwebkit2gtk-4.1",
    noteRpm: "Needs webkit2gtk4.1",
    noteAppImage: "Mark executable and run; nothing is installed",
    noteStatic: "Static; servers and containers",
    noteNas: "Raspberry Pi, ARM servers, most NAS boxes",
    noteM1: "M1 and later",
    noteMac11: "macOS 11+",
    noteWinAlloc: "On-disk size equals logical size here; see known limits",
    noteAllArchives: "One file covering every archive above",
    noteHubServer: "Static; the usual choice for a server",
    noteHubTry: "For trying it on your own machine first",
  },

  guide: {
    title: "Guide — spacetrace",
    description:
      "How to use spacetrace: scan, keep snapshots, diff two of them, set up the agent on a server, read a remote machine, and which size measure means what.",
    headline: "Guide",
    lede: "Everything the command line tool does, in the order you are likely to need it. --help on any command prints the same thing without a browser.",
    onThisPage: "On this page",

    startTitle: "First scan",
    startBody:
      "A scan walks a folder and prints what it found. Nothing is stored, nothing is changed.",
    startUnreadable:
      "Unreadable paths are counted and sampled rather than stopping the walk. On macOS, reaching outside your home folder needs Full Disk Access for your terminal; without it the scan still completes and tells you what it missed.",
    startNarrowTitle: "Making it faster and narrower",
    startExclude:
      "--exclude takes a directory name, not a path, and repeats. Excluded folders are never descended into, so excluding node_modules on a developer machine is usually the difference between two seconds and thirty.",

    snapTitle: "Snapshots and diff",
    snapBody:
      "This is the part other analysers do not have. Save a scan, save another one later, and ask what changed between them.",
    snapThen: "Then compare. With no arguments it takes the last two.",
    culpritTitle: "Why it names the deepest folder",
    culpritBody:
      "A folder whose growth comes entirely from one child tells you nothing you did not already know, so it is skipped. The report names the first level where the change genuinely spreads out — the culprit, not its ancestors.",
    snapSinceTitle: "Against the disk as it is now",
    snapSinceBody:
      "You do not need two snapshots. Compare the newest one against a live scan:",
    snapFilterTitle: "Choosing what appears",
    snapPruneTitle: "Keeping the database from growing forever",

    browseTitle: "Browsing a scan",
    browseBody:
      "ls lists folders by size, either from a live scan or from a stored snapshot.",
    browseExport:
      "To hand a snapshot to something else, export writes ncdu's own format:",
    browseJson:
      "Every command also takes --json, which is the supported way to script against it. The human-readable columns are free to change; the JSON shape is not.",

    agentTitle: "The agent on a server",
    agentBody:
      "spacetrace-agent is the same code as a service: it scans the roots you configure on a schedule, keeps the snapshots, and answers over HTTP. It is one static binary, and it only ever reads — there is no code path in it that deletes anything outside its own snapshot database.",
    agentStep1: "Write a config and a token",
    agentStep1Body:
      "serve refuses to start without a token. An unauthenticated agent hands its whole filesystem inventory to anyone who can reach the port.",
    agentStep2: "Say what to scan and when",
    agentStep2Body:
      "Five cron fields, with *, a-b, */n and lists. No seconds, no @daily. Unknown config keys are rejected at startup, because a typo that silently does nothing on a machine nobody watches is worse than a refusal to boot.",
    agentStep3: "Check before you start it",
    agentStep3Body:
      "A schedule that can never fire reports never rather than failing quietly.",
    agentStep4: "Run it",
    agentStep4Body:
      "The bundled systemd unit runs the agent as its own unprivileged user under ProtectSystem=strict, at Nice=10 with idle I/O priority. A scan should never get in the way of what the machine is actually for.",
    agentDocker: "In Docker, mount the host read-only and scan that:",
    agentSecTitle: "Before you expose it",
    agentSec: [
      "It binds loopback by default. Publishing a filesystem inventory to a network should be a deliberate edit.",
      "There is no TLS in the agent. Put a reverse proxy in front of it.",
      "The token is compared without an early exit, so a wrong token takes the same time to reject however much of it was right.",
      "/health needs no token so a container healthcheck works, and returns only status and version — no hostname, no roots.",
      "Ad-hoc scans are off. With them on, anyone holding the token can enumerate any directory the agent's user can read.",
    ],

    remoteTitle: "Reading another machine",
    remoteBody:
      "Any read-only command takes --remote. Nothing new has to be learned: the same subcommands, pointed elsewhere.",
    remoteSaveBody:
      "Save a remote so the URL and token stop being typed — in ~/.config/spacetrace/remotes.toml:",
    remoteNotDifferentTitle: "A remote snapshot is not a different kind of thing",
    remoteNotDifferentBody:
      "What comes down the wire is the same standalone SQLite file the agent stores, so listing, browsing and diffing it run the identical code as a local snapshot. You can skip the tool entirely and still get a file you can open:",
    remoteRefuse:
      "scan, prune and rm refuse to run with --remote: they act on local state, and the agent deletes nothing.",

    sizeTitle: "Which size, and why",
    sizeBody: "Every entry carries two numbers, and neither is an estimate of the other.",
    sizeMeasure: "Measure",
    sizeWhat: "What it is",
    sizeMatches: "Matches",
    sizeLogical: "logical",
    sizeLogicalWhat: "The length each file reports",
    sizeOnDisk: "on disk",
    sizeOnDiskWhat: "Blocks actually allocated, directory blocks included",
    sizeCapacity: "capacity",
    sizeCapacityWhat: "Free space out of total, on the scanned filesystem",
    sizeCapacityMatches: "df's Avail column, exactly",
    sizeDiverge:
      "They diverge in both directions and both are right. A one-byte file allocates a whole block, so it is bigger on disk than its length. A sparse file reports a length it never allocated: a disk image can claim a terabyte and hold nineteen gigabytes.",
    sizeWhyOnDisk:
      "Sparse files are why the app defaults to on disk. Virtual machine images, database files and core dumps are among the largest entries on any real disk, so the logical measure is most wrong about exactly the entries that matter most. The command line defaults to logical, and both say which one they are showing.",
    sizeCapacityWhy:
      "Capacity is reported as free of total, never as “% used”. On a filesystem whose space is shared between volumes — an APFS container, btrfs subvolumes, thin LVM — a used figure would include the siblings and disagree with df about the same mount.",
    sizeLinks:
      "Hardlinks are counted once by default; the second copy appears in the tree contributing zero bytes. Symlinks are never followed and count at their own size.",

    refTitle: "Command reference",
    refOptionsTitle: "Options that work on most commands",

    storageTitle: "Where things are kept",
    storagePlatform: "Platform",
    storageDb: "Snapshot database",
    storageAnywhere: "Anywhere",
    storageOverride:
      "SPACETRACE_HOME overrides it; --db overrides that for one command",
    storageShared:
      "The desktop app writes the same database, so a snapshot saved in the app appears in spacetrace scans and the other way round.",

    limitsTitle: "Known limits",
    limitsLede:
      "Published rather than discovered. If you hit one of these it is known debt, not a surprise.",
    limits: [
      "On Windows, on-disk size equals logical size and hardlink de-duplication is off. Real figures need GetFileInformationByHandleEx and FileIdInfo.",
      "APFS clones are not de-duplicated, and on btrfs or ZFS reflinks and compression mean a tree walk cannot report true usage. No tree walker can; the numbers are the files' own.",
      "A scan holds the whole tree in memory. The memory profile above ten million files has not been measured.",
      "The agent has no TLS and no rate limiting. Use a reverse proxy; the token is already required.",
      "The scheduler has no timezone database — UTC plus a fixed offset, so daylight-saving shifts are yours to think about.",
    ],
  },

  footer: {
    licence:
      "The scanner, snapshot store, command line tool and agent are Apache-2.0. The desktop app and the hub are proprietary.",
    home: "Home",
    allReleases: "All releases",
    builtWith: "Built with Astro. No trackers, no cookies.",
  },
} as const;

/**
 * The same shape as `en`, but with literal types widened to `string`. Without
 * this, `as const` would force every translation to repeat the English text
 * verbatim to satisfy the type.
 */
type Widen<T> = T extends readonly (infer U)[]
  ? readonly Widen<U>[]
  : T extends string
    ? string
    : T extends object
      ? { [K in keyof T]: Widen<T[K]> }
      : T;

export type Dictionary = Widen<typeof en>;
