//! `spacetrace watch`: which folders are growing, live.
//!
//! The product's question is "what grew", and `diff` answers it between two
//! snapshots. This answers it for the stretch of time someone is looking: the
//! disk is filling *now*, and the useful thing is where. So the baseline is a
//! fresh scan taken when the watch starts — not the last stored snapshot,
//! whose distance from now is already `diff --since-last`'s job — and every
//! frame says what changed since then, and how fast, biggest first.
//!
//! **Events say where to look; scans say how much.** A filesystem event only
//! marks the directory it happened in. Once per `--interval` each marked
//! directory is listed again by the scanner, so a burst of a thousand writes to
//! one file costs one listing. Nothing here estimates a size from an event.
//!
//! **Lost events are said out loud.** Every backend can drop events — inotify
//! overflows its queue, FSEvents asks for a subtree rescan, and on Windows the
//! backend can lose an overflow without telling anyone — so: a flagged loss
//! rescans what it names (the whole root when it names nothing), and a full
//! rescan also runs on its own every minute or so, cheaply, to catch the losses
//! nobody flagged. Either way the frame says it happened.
//!
//! **Ctrl-C needs no handler.** Nothing here puts the terminal in a state that
//! has to be undone — no raw mode, no alternate screen, no hidden cursor — and
//! each frame is one write. The default signal leaves the last frame standing,
//! which is the picture worth keeping.

mod links;
mod model;

use std::collections::HashSet;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use notify::event::ModifyKind;
use notify::{EventKind, RecursiveMode, Watcher, WatcherKind};
use spacetrace_scan_core::{capacity_of, ScanOptions, ScanProgress};

use crate::args::WatchArgs;
use crate::fmt;
use model::{DirId, Model, Mover, MoverKind, Refusal, Scanned, ROOT};

/// How often a full rescan checks the model when nothing asked for one, at
/// the least. Scaled up for a root that takes long to scan; see `verify_every`.
const VERIFY_FLOOR: Duration = Duration::from_secs(60);

/// How many recent notes the screen keeps under the table.
const NOTES_SHOWN: usize = 3;

/// How many events may wait between the watcher and this loop. Bounded,
/// because the kernel's own queue overflowing is the backends' only way of
/// saying "too fast", and an unbounded channel in between swallows that
/// signal and grows instead — through a long full rescan with a build writing
/// beside it, without limit. Past this the events are dropped and counted as
/// a loss, which a full rescan puts right. A few hundred bytes each, so this
/// is a few megabytes at the most.
const EVENT_QUEUE: usize = 1 << 16;

type EventResult = notify::Result<notify::Event>;

pub(crate) fn cmd_watch(a: &WatchArgs, json: bool) -> Result<()> {
    let root = a
        .path
        .canonicalize()
        .with_context(|| format!("path not found: {}", a.path.display()))?;
    anyhow::ensure!(
        root.is_dir(),
        "{} is not a folder; watch needs a folder to watch",
        root.display()
    );
    let opts = model::watch_options(a.walk.to_options());

    // The watcher starts before the first scan, so that whatever changes while
    // the scan runs is reported rather than missed. It is reconciled with the
    // scan's result straight after.
    let (tx, rx) = mpsc::sync_channel::<EventResult>(EVENT_QUEUE);
    let overflowed = Arc::new(AtomicBool::new(false));
    let mut events = Events::start(&root, tx, Arc::clone(&overflowed))?;
    let prewatched = match events.per_dir {
        true => events.watch_tree(&root, &opts)?,
        false => HashSet::new(),
    };

    let started_scan = Instant::now();
    let progress = Arc::new(ScanProgress::default());
    let ticker = crate::Ticker::start(Arc::clone(&progress), !json);
    let first = Scanned::of(&root, opts.clone(), progress)
        .with_context(|| format!("cannot scan: {}", root.display()))?;
    drop(ticker);
    let model = Model::new(&first, opts, events.per_dir);
    let files = first.stats.files;
    drop(first);

    let mut session = Session {
        model,
        events,
        rx,
        overflowed,
        pending: Pending::default(),
        interval: a.interval,
        min: a.min.max(1),
        top: a.top,
        started: Instant::now(),
        last_full: started_scan.elapsed(),
        last_full_end: Instant::now(),
        last_verified: None,
        events_seen: 0,
        notes: Vec::new(),
        fresh_notes: Vec::new(),
        reread: 0,
        fs_start: capacity_of(&root).map(|c| c.available),
        output: Output::pick(json),
    };
    // Directories that appeared while the scan ran were not in the walk that
    // set up the watches; they get one now, and a listing, since anything
    // written into them before that is in no event.
    for (id, path) in session.model.take_new_dirs() {
        if !session.events.per_dir || prewatched.contains(&path) {
            continue;
        }
        let tracked = session.model.tracked();
        session.events.watch_dir(&path, tracked)?;
        session.pending.dirty.insert(id);
    }
    drop(prewatched);
    session.intro(files)?;
    session.run()
}

// ------------------------------------------------------------------ events

/// The platform's change notification, set up the way this command needs it.
struct Events {
    watcher: notify::RecommendedWatcher,
    /// inotify watches one directory per watch, and notify's recursive mode
    /// walks everything to set them up — excluded folders included, symlinks
    /// followed, unreadable ones dropped without a word. So on inotify the
    /// watch adds its own, exactly where the scanner descends: an excluded
    /// `node_modules` costs no watches. FSEvents and Windows watch a whole
    /// tree with one handle, and the events outside the scan are filtered.
    per_dir: bool,
    /// Directories that could not be watched, so changes in them are only
    /// seen by the periodic rescan. Counted on screen, never dropped silently.
    unwatched: u64,
}

impl Events {
    fn start(
        root: &Path,
        tx: SyncSender<EventResult>,
        overflowed: Arc<AtomicBool>,
    ) -> Result<Events> {
        let handler = move |event: EventResult| {
            // Disconnected is the session ending; nothing left to tell.
            if let Err(TrySendError::Full(_)) = tx.try_send(event) {
                overflowed.store(true, Ordering::Relaxed);
            }
        };
        let watcher = notify::recommended_watcher(handler).map_err(|e| explain(e, root, None))?;
        let per_dir = <notify::RecommendedWatcher as Watcher>::kind() == WatcherKind::Inotify;
        let mut events = Events {
            watcher,
            per_dir,
            unwatched: 0,
        };
        if !per_dir {
            events
                .watcher
                .watch(root, RecursiveMode::Recursive)
                .map_err(|e| explain(e, root, None))?;
        }
        Ok(events)
    }

    /// Watch every directory the scanner will descend into, before it does.
    ///
    /// Watch first and scan second, so there is no moment at which a
    /// directory is already read and not yet watched. Returns what was
    /// watched, to compare against what the scan then found.
    ///
    /// It runs before the guarded first scan, so it approaches a mount point
    /// the way the scanner does (invariant 7): through the mount table, on a
    /// thread that can be abandoned. One that does not answer is neither
    /// watched nor entered — `inotify_add_watch` looks the path up, and that
    /// lookup is what hangs — and the scan reports it with the rest.
    fn watch_tree(&mut self, root: &Path, opts: &ScanOptions) -> Result<HashSet<PathBuf>> {
        let mounts = model::read_mounts(opts);
        let root_dev = model::device_through(opts, &mounts, root);
        let mut watched = HashSet::new();
        let mut stack = vec![(root.to_path_buf(), 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            self.watch_dir(&dir, watched.len() + stack.len() + 1)?;
            // An unreadable directory is the scan's to report, with the rest.
            let Ok(entries) = std::fs::read_dir(&dir) else {
                watched.insert(dir);
                continue;
            };
            for entry in entries.flatten() {
                // `file_type` does not follow symlinks; neither does the scan.
                if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                    continue;
                }
                let name = entry.file_name();
                if !model::descends(opts, depth + 1, &name.to_string_lossy()) {
                    continue;
                }
                let path = entry.path();
                if mounts.contains(&path) && model::probe(opts, &mounts, &path).is_none() {
                    continue;
                }
                if opts.one_filesystem && model::device_through(opts, &mounts, &path) != root_dev {
                    continue;
                }
                stack.push((path, depth + 1));
            }
            watched.insert(dir);
        }
        Ok(watched)
    }

    /// Watch one directory, where the backend wants that. A directory that
    /// cannot be watched is counted; running out of watches ends the command,
    /// because carrying on would be reporting on a tree it can no longer see.
    fn watch_dir(&mut self, path: &Path, dirs: usize) -> Result<()> {
        if !self.per_dir {
            return Ok(());
        }
        let Err(err) = self.watcher.watch(path, RecursiveMode::NonRecursive) else {
            return Ok(());
        };
        match &err.kind {
            notify::ErrorKind::MaxFilesWatch => Err(explain(err, path, Some(dirs))),
            // Gone already; the event about that is on its way.
            notify::ErrorKind::PathNotFound => Ok(()),
            notify::ErrorKind::Io(io) if io.kind() == std::io::ErrorKind::NotFound => Ok(()),
            _ => {
                self.unwatched += 1;
                Ok(())
            }
        }
    }
}

/// A watcher error, in words that say what to do about it.
fn explain(err: notify::Error, path: &Path, dirs: Option<usize>) -> anyhow::Error {
    match &err.kind {
        notify::ErrorKind::MaxFilesWatch => {
            let limit = std::fs::read_to_string("/proc/sys/fs/inotify/max_user_watches")
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| "unknown".to_string());
            let needed = dirs.map_or(String::new(), |n| {
                format!(
                    " This tree needs one per folder, at least {}.",
                    fmt::count(n as u64)
                )
            });
            anyhow::anyhow!(
                "cannot watch {}: the inotify watch limit is used up \
                 (fs.inotify.max_user_watches = {limit}).{needed} Raise it with \
                 `sudo sysctl fs.inotify.max_user_watches=524288` (and the same setting in \
                 /etc/sysctl.d/ to keep it), or watch less with --exclude or --depth",
                path.display()
            )
        }
        // EMFILE from inotify_init: the per-user instance limit, or the
        // process's descriptor limit — the kernel gives both the same number.
        notify::ErrorKind::Io(io) if cfg!(target_os = "linux") && io.raw_os_error() == Some(24) => {
            let limit = std::fs::read_to_string("/proc/sys/fs/inotify/max_user_instances")
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| "unknown".to_string());
            anyhow::anyhow!(
                "cannot start watching: no inotify instance left \
                 (fs.inotify.max_user_instances = {limit}), or this process is out of file \
                 descriptors. Raise it with `sudo sysctl fs.inotify.max_user_instances=1024`"
            )
        }
        _ => anyhow::Error::new(err).context(format!("cannot watch {}", path.display())),
    }
}

// ----------------------------------------------------------------- session

/// What the events asked for since the last tick.
#[derive(Default)]
struct Pending {
    /// Directories whose listing may be out of date.
    dirty: HashSet<DirId>,
    /// Subtrees the event stream said it lost events below.
    subtrees: HashSet<DirId>,
    /// Why only a full rescan will do, when that is the case.
    resync: Option<String>,
}

struct Note {
    at: Instant,
    text: String,
}

struct Session {
    model: Model,
    events: Events,
    rx: Receiver<EventResult>,
    /// Set by the watcher when [`EVENT_QUEUE`] was full and an event dropped.
    overflowed: Arc<AtomicBool>,
    pending: Pending,
    interval: Duration,
    min: u64,
    top: usize,
    started: Instant,
    /// How long the last full scan took, which is what paces the next one.
    last_full: Duration,
    last_full_end: Instant,
    last_verified: Option<Instant>,
    events_seen: u64,
    notes: Vec<Note>,
    /// Notes from this tick, for the outputs that print each one once.
    fresh_notes: Vec<String>,
    /// Directories listed again in the last tick.
    reread: usize,
    fs_start: Option<u64>,
    output: Output,
}

impl Session {
    fn run(&mut self) -> Result<()> {
        let mut next_tick = Instant::now();
        loop {
            let now = Instant::now();
            if now >= next_tick {
                self.tick()?;
                if !self.render()? {
                    // The reader went away (a closed pipe); nobody to tell.
                    return Ok(());
                }
                next_tick = Instant::now() + self.interval;
                continue;
            }
            match self.rx.recv_timeout(next_tick - now) {
                Ok(event) => self.handle(event)?,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    anyhow::bail!("the file watcher stopped; changes can no longer be seen")
                }
            }
        }
    }

    /// Turn one event into work for the next tick. Cheap on purpose: a busy
    /// disk sends thousands a second, and all this does is name a directory.
    fn handle(&mut self, event: EventResult) -> Result<()> {
        // Reads are no change, and the listings this command makes would
        // otherwise report themselves: inotify sends an open per `opendir`.
        let read =
            matches!(&event, Ok(e) if matches!(e.kind, EventKind::Access(_)) && !e.need_rescan());
        if read {
            return Ok(());
        }
        self.events_seen += 1;
        let event = match event {
            Ok(event) => event,
            Err(err) if matches!(err.kind, notify::ErrorKind::MaxFilesWatch) => {
                let tracked = self.model.tracked();
                return Err(explain(err, self.model.root(), Some(tracked)));
            }
            Err(err) => {
                self.pending.resync = Some(format!("the watcher reported an error: {err}"));
                return Ok(());
            }
        };
        if event.need_rescan() {
            // A loss with no path is a loss anywhere.
            if event.paths.is_empty() {
                self.pending.resync = Some("events were dropped".to_string());
            }
            for path in &event.paths {
                if let Some(id) = self.model.locate(path, true) {
                    self.pending.subtrees.insert(id);
                }
            }
            return Ok(());
        }
        // A folder created, or renamed into place, under a name the model
        // already tracks is not the folder the model knows: deleted and made
        // again, swapped for another (`npm install`, most deploys), or renamed
        // away and back. Its parent's listing sees the same name and cannot
        // tell; and on inotify its watch, and every watch below it, went with
        // the old one. So the whole subtree is read again and watched again.
        let replaced = matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(_))
        );
        // A new hardlink changes its original's link count, and the original
        // has to be listed for the ledger to know both names (`links.rs`).
        // FSEvents names the folder the original is in, as that folder's
        // metadata, so that folder is listed too. inotify does not report it
        // at all; the doubt below ends in a full rescan instead.
        let metadata = matches!(event.kind, EventKind::Modify(ModifyKind::Metadata(_)));
        for path in &event.paths {
            let Some((holder, at)) = self.model.event_dirs(path) else {
                continue;
            };
            self.pending.dirty.insert(holder);
            let Some(id) = at else {
                continue;
            };
            if replaced && id != ROOT {
                self.pending.subtrees.insert(id);
            }
            if metadata {
                self.pending.dirty.insert(id);
            }
        }
        Ok(())
    }

    /// Full rescans asked for by a refusal or a loss get at most a tenth of
    /// the time; until the next one may run, the frame says one is waiting.
    ///
    /// A tenth, not more, because whatever asks may ask constantly. A build
    /// tree did, before hardlinks were settled by the ledger: cargo's output
    /// is hardlinked, and on a 1.26M-entry root with builds running beside it
    /// a gap of four scans spent a quarter of a core on rescans (about 10 s of
    /// CPU each, measured) and spiked RSS to 1.1 GB.
    fn resync_gap(&self) -> Duration {
        self.interval.max(self.last_full * 10)
    }

    /// A full rescan nobody asked for, as a check: at most every minute, and
    /// never more than a thirtieth of the time on a root that is slow to scan.
    fn verify_every(&self) -> Duration {
        VERIFY_FLOOR.max(self.last_full * 30)
    }

    fn tick(&mut self) -> Result<()> {
        self.fresh_notes.clear();
        self.reread = 0;
        let now = Instant::now();
        let since_full = now.duration_since(self.last_full_end);
        if self.overflowed.swap(false, Ordering::Relaxed) {
            self.pending.resync =
                Some("events arrived faster than they could be read, and some were dropped".into());
        }

        if self.pending.resync.is_some() && since_full >= self.resync_gap() {
            let reason = self.pending.resync.clone().unwrap_or_default();
            self.resync(Some(reason))?;
        } else if since_full >= self.verify_every() {
            // Due whatever is pending: a full rescan covers it, and a disk
            // that is written to every tick would otherwise never be checked
            // — the losses nobody flagged would stay wrong all session.
            self.resync(None)?;
        } else {
            self.apply_pending()?;
            if self.pending.resync.is_some() && since_full >= self.resync_gap() {
                let reason = self.pending.resync.clone().unwrap_or_default();
                self.resync(Some(reason))?;
            }
        }

        self.doubt_links();
        self.model.aggregate();
        self.model.roll_marks(Instant::now());
        for (id, path) in self.model.take_new_dirs() {
            let tracked = self.model.tracked();
            self.events.watch_dir(&path, tracked)?;
            // Watched only now: whatever landed in it before is in no event.
            if self.events.per_dir {
                self.pending.dirty.insert(id);
            }
        }
        // Also when work is pending, for the same reason as the check: under
        // steady writes the folders that come and go would pile up otherwise.
        // What is pending is held by id, and ids move.
        if let Some(remap) = self.model.compact_if_worth_it() {
            let moved = |ids: &mut HashSet<DirId>| {
                *ids = ids
                    .iter()
                    .filter_map(|&id| remap.get(id as usize).copied())
                    .filter(|&id| id != DirId::MAX)
                    .collect();
            };
            moved(&mut self.pending.dirty);
            moved(&mut self.pending.subtrees);
        }
        Ok(())
    }

    /// A hardlinked file with names no listing has met is counted under none
    /// of them (`links.rs`). The usual reason is a folder whose event is still
    /// to come — on FSEvents a link names both folders — so it gets until the
    /// end of the next tick; after that, only a full scan finds the names.
    fn doubt_links(&mut self) {
        let Some(rel) = self.model.doubted_link() else {
            return;
        };
        if self.pending.resync.is_some() {
            return;
        }
        self.pending.resync = Some(format!(
            "a hardlinked file in {} has names no listing has met: only a full scan finds them",
            shown(&rel)
        ));
    }

    /// Lost subtrees first, then every directory an event named, shallowest
    /// first — so a directory that went away is found missing by its parent
    /// before anything tries to list it.
    fn apply_pending(&mut self) -> Result<()> {
        let mut subtrees: Vec<DirId> = self.pending.subtrees.drain().collect();
        subtrees.sort_by_key(|&id| self.model.depth(id));
        let mut rescanned: Vec<DirId> = Vec::new();
        for id in subtrees {
            if !self.model.is_present(id) || rescanned.iter().any(|&r| self.model.is_within(id, r))
            {
                continue;
            }
            let rel = self.model.rel_path(id);
            match self.model.rescan(id) {
                Ok(()) => {
                    rescanned.push(id);
                    // Watched again from the top: on inotify a replaced
                    // folder's watches went with the folder it replaced.
                    self.model.announce_subtree(id);
                    self.note(format!("rescanned {}", shown(&rel)));
                }
                // Gone since the loss was flagged: its parent's listing,
                // next, is what says so.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && id != ROOT => {
                    if let Some(parent) = self.model.parent(id) {
                        self.pending.dirty.insert(parent);
                    }
                }
                Err(e) => self.relist_failed(id, e)?,
            }
        }

        let mut work: Vec<DirId> = self.pending.dirty.drain().collect();
        work.sort_by_key(|&id| std::cmp::Reverse(self.model.depth(id)));
        let mut done: HashSet<DirId> = HashSet::new();
        while let Some(id) = work.pop() {
            if !done.insert(id) || !self.model.is_present(id) {
                continue;
            }
            if rescanned.iter().any(|&r| self.model.is_within(id, r)) {
                continue;
            }
            match self.model.relist(id) {
                Ok(update) => {
                    self.reread += 1;
                    if let Some(refusal) = update.refused {
                        self.pending.resync = Some(refusal_text(&refusal, self.model.root()));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Gone before it could be listed: its parent says how.
                    let Some(parent) = self.model.parent(id) else {
                        anyhow::bail!(
                            "{} is gone; nothing left to watch",
                            self.model.root().display()
                        );
                    };
                    work.push(parent);
                }
                Err(e) => self.relist_failed(id, e)?,
            }
        }
        Ok(())
    }

    fn relist_failed(&mut self, id: DirId, err: std::io::Error) -> Result<()> {
        if id == ROOT && err.kind() == std::io::ErrorKind::NotFound {
            anyhow::bail!(
                "{} is gone; nothing left to watch",
                self.model.root().display()
            );
        }
        let rel = self.model.rel_path(id);
        self.note(format!("cannot read {}: {err}", shown(&rel)));
        Ok(())
    }

    /// Scan the whole root and take it as the truth about now.
    ///
    /// `reason` is `None` for the periodic check, which says something only
    /// when it found the model wrong.
    fn resync(&mut self, reason: Option<String>) -> Result<()> {
        self.model.aggregate();
        let before = self.model.totals();
        let seen = self.events_seen;
        // Said before it starts, the check included: on a large root this
        // is seconds in which the frame does not move (invariant 8).
        let notice = match &reason {
            Some(why) => format!("rescanning everything: {why}…"),
            None => "checking against a full rescan…".to_string(),
        };
        self.output.announce(&notice)?;

        let started = Instant::now();
        let root = self.model.root().to_path_buf();
        let scanned = Scanned::of(&root, self.model.options().clone(), Arc::default())
            .with_context(|| format!("cannot scan: {}", root.display()))?;
        self.model.resync(&scanned);
        drop(scanned);
        self.last_full = started.elapsed();
        self.last_full_end = Instant::now();
        self.last_verified = Some(self.last_full_end);
        // Everything that was waiting is covered by what was just read. What
        // arrived during the scan is still in the channel and comes next.
        self.pending = Pending::default();
        self.model.aggregate();

        let took = fmt::duration(self.last_full.as_millis() as u64);
        match reason {
            Some(why) => self.note(format!("rescanned everything in {took}: {why}")),
            None => {
                // Drain first: only a check that saw no event while it ran
                // can call a difference drift rather than news.
                while let Ok(event) = self.rx.try_recv() {
                    self.handle(event)?;
                }
                let after = self.model.totals();
                let drift = after.size as i64 - before.size as i64;
                if drift != 0 && self.events_seen == seen {
                    self.note(format!(
                        "a full rescan found {} that no event had reported; corrected",
                        fmt::delta(drift)
                    ));
                }
            }
        }
        Ok(())
    }

    fn note(&mut self, text: String) {
        self.fresh_notes.push(text.clone());
        self.notes.push(Note {
            at: Instant::now(),
            text,
        });
        if self.notes.len() > NOTES_SHOWN {
            self.notes.remove(0);
        }
    }

    // ------------------------------------------------------------- output

    /// What every output says once, before the first tick.
    fn intro(&mut self, files: u64) -> Result<()> {
        if !matches!(self.output, Output::Lines { .. }) {
            return Ok(());
        }
        let totals = self.model.totals();
        let line = format!(
            "watching {} — {} in {} files, {} folders (first scan {}). Changes from here on.\n",
            self.model.root().display(),
            fmt::size(totals.size),
            fmt::count(files),
            fmt::count(self.model.tracked() as u64),
            fmt::duration(self.last_full.as_millis() as u64),
        );
        self.output.write(&line).map(|_| ())
    }

    /// Draw this tick. `false` when the reader has gone.
    fn render(&mut self) -> Result<bool> {
        let now = Instant::now();
        let frame = self.frame(now);
        let text = match self.output {
            Output::Json => self.json(&frame, now)?,
            Output::Screen { .. } => self.screen(&frame, now),
            Output::Lines { .. } => {
                // A log wants a block when something changed, not every tick;
                // the rates move every tick, so they are not part of "changed".
                let rows: Vec<_> = frame.rows.iter().map(|r| (&r.path, r.delta)).collect();
                let signature = format!("{}|{rows:?}", frame.totals.delta());
                let unchanged =
                    matches!(&self.output, Output::Lines { last: Some(l) } if *l == signature);
                if unchanged && self.fresh_notes.is_empty() {
                    return Ok(true);
                }
                let text = self.lines(&frame);
                self.output = Output::Lines {
                    last: Some(signature),
                };
                text
            }
        };
        self.output.write(&text)
    }

    fn frame(&self, now: Instant) -> Frame {
        let movers = self.model.movers(self.min);
        let more = movers.len().saturating_sub(self.top);
        let mut rows: Vec<Row> = movers
            .into_iter()
            .take(self.top)
            .map(|m| Row::of(&m, self.model.rate(m.id, now)))
            .collect();
        let files = self.model.root_files_delta();
        if files.unsigned_abs() >= self.min {
            rows.push(Row {
                path: ".".to_string(),
                kind: if files >= 0 {
                    MoverKind::Grown
                } else {
                    MoverKind::Shrunk
                },
                old_size: 0,
                new_size: 0,
                old_alloc: 0,
                new_alloc: 0,
                delta: files,
                rate: self.model.root_files_rate(now),
                root_files: true,
            });
            rows.sort_by_key(|r| std::cmp::Reverse(r.delta.abs()));
        }
        let (errors, _) = self.model.errors();
        Frame {
            totals: self.model.totals(),
            rate: self.model.rate(ROOT, now),
            fs_available: capacity_of(self.model.root()).map(|c| c.available),
            rows,
            more,
            errors,
        }
    }

    fn screen(&mut self, f: &Frame, now: Instant) -> String {
        let elapsed = now.duration_since(self.started);
        let mut head = vec![
            format!("spacetrace watch  {}", self.model.root().display()),
            format!(
                "{} so far · every {} · {} folders",
                fmt::duration(elapsed.as_millis() as u64),
                fmt::duration(self.interval.as_millis() as u64),
                fmt::count(self.model.tracked() as u64),
            ),
            String::new(),
            format!(
                "total  {} → {}   {}  ({}/s)",
                fmt::size(f.totals.base_size),
                fmt::size(f.totals.size),
                fmt::delta(f.totals.delta()),
                fmt::delta(f.rate as i64),
            ),
            format!(
                "on disk  {} ({} since start)",
                fmt::size(f.totals.alloc),
                fmt::delta(f.totals.alloc as i64 - f.totals.base_alloc as i64),
            ),
        ];
        if let (Some(now_free), Some(start)) = (f.fs_available, self.fs_start) {
            // The filesystem's own count, which no event can miss: the one
            // number here that is not ours.
            head.push(format!(
                "filesystem  {} free ({} since start)",
                fmt::size(now_free),
                fmt::delta(now_free as i64 - start as i64),
            ));
        }
        head.push(String::new());
        let mut tail = vec![String::new()];
        for note in &self.notes {
            let ago = now.duration_since(note.at).as_secs();
            tail.push(format!("{} ({ago} s ago)", note.text));
        }
        tail.extend(self.status_lines(f));
        if let Some(at) = self.last_verified {
            tail.push(format!(
                "checked against a full rescan {} s ago",
                now.duration_since(at).as_secs()
            ));
        }
        if let Some(note) = CLONE_NOTE {
            tail.push(note.to_string());
        }
        tail.push("Ctrl-C to stop".to_string());

        // Fitted to the terminal: a frame taller than the screen scrolls on
        // every tick instead of redrawing in place, and a line wider than it
        // wraps and pushes the rest down. The table gives way first — its
        // header and the "… and N more" line stay, the rows shrink.
        let (rows, cols) = terminal_size().unwrap_or((usize::MAX, usize::MAX));
        let room = rows.saturating_sub(head.len() + tail.len() + 1);
        let mut lines = head;
        self.table(f, &mut lines, room);
        lines.extend(tail);
        lines.truncate(rows.saturating_sub(1).max(1));

        let first = matches!(self.output, Output::Screen { drawn: false });
        if let Output::Screen { drawn } = &mut self.output {
            *drawn = true;
        }
        // Home, overwrite each line and clear what is left of it, then clear
        // below: no blank frame between two, so no flicker, and a line that
        // wrapped is put right by the next frame instead of piling up.
        let mut out = String::from(if first { "\x1b[2J\x1b[H" } else { "\x1b[H" });
        for line in lines {
            out.push_str(&clip(&line, cols.saturating_sub(1).max(1)));
            out.push_str("\x1b[K\n");
        }
        out.push_str("\x1b[J");
        out
    }

    /// The table, in at most `room` lines when there is a screen to fit.
    fn table(&self, f: &Frame, lines: &mut Vec<String>, room: usize) {
        if f.rows.is_empty() {
            lines.push(if self.min <= 1 {
                "No folder has changed since the watch began.".to_string()
            } else {
                format!(
                    "No folder has changed by {} or more since the watch began.",
                    fmt::size(self.min)
                )
            });
            return;
        }
        lines.push(format!(
            "{:>12}  {:>12}  {:<7}  {:>10}  PATH",
            "CHANGE", "RATE", "STATUS", "NOW"
        ));
        // The header, and a line for what did not fit, are part of the room.
        let fits = match f.rows.len() + usize::from(f.more > 0) < room {
            true => f.rows.len(),
            false => room.saturating_sub(2).max(1).min(f.rows.len()),
        };
        for row in &f.rows[..fits] {
            lines.push(row.line());
        }
        let more = f.more + (f.rows.len() - fits);
        if more > 0 {
            lines.push(format!("… and {more} more"));
        }
    }

    fn status_lines(&self, f: &Frame) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(why) = &self.pending.resync {
            lines.push(format!("waiting to rescan everything: {why}"));
        }
        if f.errors > 0 {
            lines.push(format!(
                "{} paths cannot be read and are not counted",
                fmt::count(f.errors)
            ));
        }
        if self.events.unwatched > 0 {
            lines.push(format!(
                "{} folders cannot be watched; only the periodic rescan sees them",
                fmt::count(self.events.unwatched)
            ));
        }
        lines
    }

    fn lines(&self, f: &Frame) -> String {
        let stamp = fmt::duration(self.started.elapsed().as_millis() as u64);
        let mut out = String::new();
        for note in &self.fresh_notes {
            out.push_str(&format!("[{stamp}] {note}\n"));
        }
        out.push_str(&format!(
            "[{stamp}] total {} ({}/s) → {}\n",
            fmt::delta(f.totals.delta()),
            fmt::delta(f.rate as i64),
            fmt::size(f.totals.size),
        ));
        for row in &f.rows {
            out.push_str("  ");
            out.push_str(&row.line());
            out.push('\n');
        }
        if f.more > 0 {
            out.push_str(&format!("  … and {} more\n", f.more));
        }
        for line in self.status_lines(f) {
            out.push_str(&format!("[{stamp}] {line}\n"));
        }
        out
    }

    fn json(&self, f: &Frame, now: Instant) -> Result<String> {
        let changes: Vec<_> = f
            .rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "path": r.path,
                    "kind": r.kind,
                    "old_size": r.old_size,
                    "new_size": r.new_size,
                    "old_alloc": r.old_alloc,
                    "new_alloc": r.new_alloc,
                    "delta": r.delta,
                    "rate": r.rate.round() as i64,
                    "root_files": r.root_files,
                })
            })
            .collect();
        let (_, samples) = self.model.errors();
        let payload = serde_json::json!({
            "elapsed_ms": now.duration_since(self.started).as_millis() as u64,
            "root": self.model.root().to_string_lossy(),
            "old_total": f.totals.base_size,
            "new_total": f.totals.size,
            "delta": f.totals.delta(),
            "rate": f.rate.round() as i64,
            "old_alloc": f.totals.base_alloc,
            "new_alloc": f.totals.alloc,
            "fs_available": f.fs_available,
            "fs_available_at_start": self.fs_start,
            "changes": changes,
            "more": f.more,
            "folders": self.model.tracked(),
            "dirs_reread": self.reread,
            "events": self.events_seen,
            "notes": self.fresh_notes,
            "pending_rescan": self.pending.resync,
            "errors": f.errors,
            "error_samples": samples
                .iter()
                .take(5)
                .map(|(p, e)| serde_json::json!({ "path": p.to_string_lossy(), "error": e }))
                .collect::<Vec<_>>(),
            "unwatched": self.events.unwatched,
            "clone_dedupe": false,
        });
        Ok(format!("{}\n", serde_json::to_string(&payload)?))
    }
}

/// A row as shown, a mover or the root's own files.
struct Row {
    path: String,
    kind: MoverKind,
    old_size: u64,
    new_size: u64,
    old_alloc: u64,
    new_alloc: u64,
    delta: i64,
    rate: f64,
    root_files: bool,
}

impl Row {
    fn of(m: &Mover, rate: f64) -> Row {
        Row {
            path: m.path.clone(),
            kind: m.kind,
            old_size: m.old_size,
            new_size: m.new_size,
            old_alloc: m.old_alloc,
            new_alloc: m.new_alloc,
            delta: m.delta(),
            rate,
            root_files: false,
        }
    }

    fn line(&self) -> String {
        let status = match self.kind {
            MoverKind::Grown => "grew",
            MoverKind::Shrunk => "shrank",
            MoverKind::Added => "added",
            MoverKind::Removed => "removed",
        };
        let (now, path) = match self.root_files {
            true => (String::new(), "./ (files directly in the root)".to_string()),
            false => (
                fmt::size(self.new_size),
                format!("{}/", fmt::ellipsize(&self.path, 60)),
            ),
        };
        format!(
            "{:>12}  {:>12}  {:<7}  {:>10}  {path}",
            fmt::delta(self.delta),
            format!("{}/s", fmt::delta(self.rate as i64)),
            status,
            now,
        )
    }
}

struct Frame {
    totals: model::Totals,
    rate: f64,
    fs_available: Option<u64>,
    rows: Vec<Row>,
    more: usize,
    errors: u64,
}

/// Where frames go, decided once.
enum Output {
    /// A terminal: one frame, redrawn in place.
    Screen { drawn: bool },
    /// A pipe or a file: a block appended when something changed.
    Lines { last: Option<String> },
    /// `--json`: one object per tick, one per line.
    Json,
}

impl Output {
    fn pick(json: bool) -> Output {
        if json {
            return Output::Json;
        }
        if !std::io::stdout().is_terminal() {
            return Output::Lines { last: None };
        }
        // The old Windows console shows escape sequences as text; Windows
        // Terminal and anything that sets TERM draws them.
        let plain_console = cfg!(windows)
            && std::env::var_os("WT_SESSION").is_none()
            && std::env::var_os("TERM").is_none();
        if plain_console {
            return Output::Lines { last: None };
        }
        Output::Screen { drawn: false }
    }

    /// One write per frame. `false` when the reader has gone away.
    fn write(&self, text: &str) -> Result<bool> {
        let stdout = std::io::stdout();
        let mut lock = stdout.lock();
        let result = lock.write_all(text.as_bytes()).and_then(|_| lock.flush());
        match result {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(false),
            Err(e) => Err(e).context("writing to stdout"),
        }
    }

    /// A notice under the frame before something slow, so a screen that stops
    /// moving says why. Only on a screen: the other two print the rescan as a
    /// note in the next block, and a line in between would break their shape.
    fn announce(&self, text: &str) -> Result<()> {
        match self {
            Output::Screen { .. } => self.write(&format!("\x1b[J{text}\r")).map(|_| ()),
            Output::Lines { .. } | Output::Json => Ok(()),
        }
    }
}

/// What the watch does not count the way `scan` does, said where it applies.
///
/// It runs with clone deduplication off (see `model::watch_options`): which
/// name a shared block belongs to is settled by a full walk, never by one
/// listing. On macOS that is APFS clones; on Linux, since the scanner charges
/// btrfs and XFS shared and compressed extents once, it is those.
const CLONE_NOTE: Option<&str> = if cfg!(target_os = "macos") {
    Some("clones are counted at their full size, as --no-clone-dedupe")
} else if cfg!(target_os = "linux") {
    Some("on btrfs and XFS shared and compressed blocks count in full, as --no-clone-dedupe")
} else {
    None
};

/// The terminal's rows and columns, when stdout is one that says.
#[cfg(unix)]
fn terminal_size() -> Option<(usize, usize)> {
    // SAFETY: TIOCGWINSZ writes one `winsize` into the struct it is given,
    // and fails without touching it when stdout is not a terminal.
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) } == 0;
    (ok && size.ws_row > 0 && size.ws_col > 0)
        .then(|| (usize::from(size.ws_row), usize::from(size.ws_col)))
}

/// Windows Terminal has no ioctl to ask; a frame that is too tall scrolls
/// there, as it did everywhere before.
#[cfg(not(unix))]
fn terminal_size() -> Option<(usize, usize)> {
    None
}

/// `line` cut to `width` characters, with an ellipsis where it was cut.
fn clip(line: &str, width: usize) -> std::borrow::Cow<'_, str> {
    if line.chars().count() <= width {
        return line.into();
    }
    let mut cut: String = line.chars().take(width.saturating_sub(1)).collect();
    cut.push('…');
    cut.into()
}

fn shown(rel: &str) -> String {
    match rel.is_empty() {
        true => "the root".to_string(),
        false => format!("{rel}/"),
    }
}

fn refusal_text(refusal: &Refusal, root: &Path) -> String {
    match refusal {
        Refusal::Unnamed(at) => {
            let rel = at.strip_prefix(root).unwrap_or(at).to_string_lossy();
            format!(
                "{rel} has a name that is not valid UTF-8 and more than one real name \
                 fits it: only a full scan counts it"
            )
        }
    }
}
