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

mod events;
#[cfg(target_os = "linux")]
mod inotify;
mod links;
mod model;

use std::collections::HashSet;
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use spacetrace_scan_core::{capacity_of, ScanProgress};

use crate::args::WatchArgs;
use crate::fmt;
use events::{Change, Events, How};
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
    let (tx, rx) = mpsc::sync_channel::<Change>(EVENT_QUEUE);
    let overflowed = Arc::new(AtomicBool::new(false));
    let mut events = Events::start(&root, tx, Arc::clone(&overflowed))?;
    let prewatched = match events.per_dir() {
        true => events.watch_tree(&root, &opts)?,
        false => HashSet::new(),
    };

    let started_scan = Instant::now();
    let progress = Arc::new(ScanProgress::default());
    let ticker = crate::Ticker::start(Arc::clone(&progress), !json);
    let first = Scanned::of(&root, opts.clone(), progress)
        .with_context(|| format!("cannot scan: {}", root.display()))?;
    drop(ticker);
    let model = Model::new(&first, opts, events.per_dir());
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
        watcher_failed: None,
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
        if !session.events.per_dir() || prewatched.contains(&path) {
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
    /// Directories listed in the last tick that listed any; see
    /// `Session::doubt_links`.
    recent: HashSet<DirId>,
}

impl Pending {
    /// Follow every directory held by id to its new id after a compaction;
    /// one dropped is a directory gone. The only place ids move: a full
    /// rescan keeps every record's id, and starts `Pending` afresh anyway.
    fn remap(&mut self, remap: &[DirId]) {
        for ids in [&mut self.dirty, &mut self.subtrees, &mut self.recent] {
            *ids = ids
                .iter()
                .filter_map(|&id| remap.get(id as usize).copied())
                .filter(|&id| id != DirId::MAX)
                .collect();
        }
    }
}

struct Note {
    at: Instant,
    text: String,
}

struct Session {
    model: Model,
    events: Events,
    rx: Receiver<Change>,
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
    /// The last error the watcher reported, said again if it then stops.
    watcher_failed: Option<String>,
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
                Err(RecvTimeoutError::Disconnected) => match &self.watcher_failed {
                    Some(why) => anyhow::bail!(
                        "the file watcher stopped ({why}); changes can no longer be seen"
                    ),
                    None => {
                        anyhow::bail!("the file watcher stopped; changes can no longer be seen")
                    }
                },
            }
        }
    }

    /// Turn one event into work for the next tick. Cheap on purpose: a busy
    /// disk sends thousands a second, and all this does is name a directory.
    fn handle(&mut self, change: Change) -> Result<()> {
        self.events_seen += 1;
        let (paths, how) = match change {
            Change::At { paths, how } => (paths, how),
            #[cfg(not(target_os = "linux"))]
            Change::Exhausted => {
                let tracked = self.model.tracked();
                return Err(events::exhausted(self.model.root(), Some(tracked)));
            }
            Change::Failed(err) => {
                self.pending.resync = Some(format!("the watcher reported an error: {err}"));
                // Kept for the message if the watcher stops after it.
                self.watcher_failed = Some(err);
                return Ok(());
            }
            Change::Lost(paths) => {
                // A loss with no path is a loss anywhere.
                if paths.is_empty() {
                    self.pending.resync = Some("events were dropped".to_string());
                }
                for path in &paths {
                    if let Some(id) = self.model.locate(path, true) {
                        self.pending.subtrees.insert(id);
                    }
                }
                return Ok(());
            }
        };
        // A folder created, or renamed into place, under a name the model
        // already tracks is not the folder the model knows: deleted and made
        // again, swapped for another (`npm install`, most deploys), or renamed
        // away and back. Its parent's listing sees the same name and cannot
        // tell; and on inotify its watch, and every watch below it, went with
        // the old one. So the whole subtree is read again and watched again.
        let replaced = how == How::Made;
        // A new hardlink changes its original's link count, and the original
        // has to be listed for the ledger to know both names (`links.rs`).
        // FSEvents names the folder the original is in, as that folder's
        // metadata, so that folder is listed too. inotify does not report it
        // at all; `doubt_links` covers it.
        let metadata = how == How::Metadata;
        for path in &paths {
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

        let mut listed = HashSet::new();
        if self.pending.resync.is_some() && since_full >= self.resync_gap() {
            let reason = self.pending.resync.clone().unwrap_or_default();
            self.resync(Some(reason))?;
        } else if since_full >= self.verify_every() {
            // Due whatever is pending: a full rescan covers it, and a disk
            // that is written to every tick would otherwise never be checked
            // — the losses nobody flagged would stay wrong all session.
            self.resync(None)?;
        } else {
            listed = self.apply_pending()?;
            if self.pending.resync.is_some() && since_full >= self.resync_gap() {
                let reason = self.pending.resync.clone().unwrap_or_default();
                self.resync(Some(reason))?;
                listed.clear();
            }
        }

        self.doubt_links(listed);
        self.model.aggregate();
        self.model.roll_marks(Instant::now());
        for (id, path) in self.model.take_new_dirs() {
            let tracked = self.model.tracked();
            self.events.watch_dir(&path, tracked)?;
            // Watched only now: whatever landed in it before is in no event.
            if self.events.per_dir() {
                self.pending.dirty.insert(id);
            }
        }
        // Also when work is pending, for the same reason as the check: under
        // steady writes the folders that come and go would pile up otherwise.
        // What is pending is held by id, and ids move.
        if let Some(remap) = self.model.compact_if_worth_it() {
            self.pending.remap(&remap);
        }
        Ok(())
    }

    /// A hardlinked file with names no listing has met is counted under none
    /// of them (`links.rs`), and gets until the end of the next tick before a
    /// full scan is asked to find them.
    ///
    /// Most often its other name is a file just written — cargo writes an
    /// object into `deps` and links it into an incremental session moments
    /// later — whose folder a tick listed in between, while it had one link,
    /// and counted as an ordinary file. FSEvents names that folder when the
    /// link is made; inotify does not, because a link count is the file's
    /// metadata and only a watch on the file itself hears of it. So on
    /// inotify the folders listed this tick and in the last tick that listed
    /// any are listed again next tick: one of them is usually where the other
    /// name is. Idle ticks in between do not count, or a link made a few quiet
    /// seconds after its file would always cost a full scan.
    fn doubt_links(&mut self, listed: HashSet<DirId>) {
        let (fresh, long) = self.model.doubted_links();
        if !self.events.hears_link_originals() {
            if fresh {
                let again = self.pending.recent.iter().chain(&listed);
                self.pending.dirty.extend(again);
            }
            if !listed.is_empty() {
                self.pending.recent = listed;
            }
        }
        let Some(rel) = long else {
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
    /// before anything tries to list it. Returns the directories listed, the
    /// tops of the subtrees rescanned among them.
    fn apply_pending(&mut self) -> Result<HashSet<DirId>> {
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
        done.extend(rescanned);
        Ok(done)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// After a compaction every id the session holds names the same folder
    /// as before — the folders kept for the hardlink relisting too, or that
    /// relisting would list folders that are not the ones it remembered.
    #[test]
    fn a_compaction_moves_every_id_the_session_holds() {
        let mut pending = Pending {
            dirty: HashSet::from([1, 4]),
            subtrees: HashSet::from([4]),
            recent: HashSet::from([2, 4]),
            ..Pending::default()
        };
        // 0 stays, 1 and 3 were dropped, 2 and 4 move down.
        pending.remap(&[0, DirId::MAX, 1, DirId::MAX, 2]);

        assert_eq!(pending.dirty, HashSet::from([2]));
        assert_eq!(pending.subtrees, HashSet::from([2]));
        assert_eq!(pending.recent, HashSet::from([1, 2]));
    }
}
