use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use rayon::prelude::*;

use crate::capacity::Capacity;
use crate::meta::{display_name, EntryKind, FileIdentity, RawMeta};
use crate::tree::{NewNode, NodeId, Tree, TreeBuilder};

/// How many failing paths we keep for the report before we only count them.
const MAX_REPORTED_ERRORS: usize = 64;

/// Files smaller than this are never probed for being clones.
///
/// Every probe is an open and an `fcntl`, and the small end of a tree is where
/// the file count is: on one real disk, dropping below this would have tripled
/// the number of probes to recover bytes that round to nothing in any total a
/// user reads.
const CLONE_MIN_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Directory names to skip entirely (e.g. `node_modules`, `.git`).
    pub exclude_names: Vec<String>,
    /// Do not cross filesystem boundaries (like `du -x`).
    pub one_filesystem: bool,
    /// Stop descending below this depth. `None` means unlimited.
    pub max_depth: Option<usize>,
    /// Count a hardlinked file only the first time it is met.
    pub dedupe_hardlinks: bool,
    /// Count copy-on-write clones only once, the same way (macOS/APFS).
    pub dedupe_clones: bool,
    /// How many threads to walk with. `None` (and `Some(0)`) take the default
    /// below, which is measured rather than inherited from the core count.
    pub threads: Option<usize>,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions {
            exclude_names: Vec::new(),
            one_filesystem: false,
            max_depth: None,
            dedupe_hardlinks: true,
            dedupe_clones: true,
            threads: None,
        }
    }
}

/// The stages of a scan, in order.
///
/// Only two do enough work to be worth naming, and the reason to name them is
/// honesty in a progress line: after the walk finishes, "scanning…" is no
/// longer true and the file counter has stopped for good.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Reading directories.
    Walking,
    /// Walk done; deduplicating copy-on-write clones and building the tree.
    Finishing,
}

impl Phase {
    fn from_u8(value: u8) -> Phase {
        match value {
            1 => Phase::Finishing,
            // Anything else is the initial zero, which is where a scan starts.
            _ => Phase::Walking,
        }
    }
}

/// How long the counters may stand still before a watcher should say so.
///
/// Long enough not to nag a slow network share on its first directory, short
/// enough to answer "is this stuck" before the reader gives up and kills it.
pub const STALL_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

/// Decides when a scan has stopped moving.
///
/// The counters are the only evidence available: a mount that stopped
/// answering blocks a thread in the kernel, and nothing in this process can
/// see that directly. So a watcher polls, and this says whether anything has
/// changed.
///
/// It reads the counters off [`ScanProgress`] rather than taking them as an
/// argument, which is what keeps invariant 8 workable: when a new phase adds a
/// counter, every watcher starts including it without being changed. A caller
/// that assembled its own tuple would go on ignoring the new one and report a
/// healthy phase as a stall.
pub struct StallWatch {
    counters: [u64; 5],
    /// When these values were **first** seen — not the second sighting. The
    /// counters stopped somewhere between the two, and the first is the
    /// earliest moment they are known to have been still already.
    since: std::time::Instant,
    grace: std::time::Duration,
}

impl StallWatch {
    /// Starts counting from `now`, so a scan that produces nothing at all —
    /// blocked on its own root — is still reported.
    pub fn new(now: std::time::Instant, grace: std::time::Duration) -> Self {
        StallWatch {
            counters: [0; 5],
            since: now,
            grace,
        }
    }

    /// How long the scan has been stalled, or `None` while it is moving or has
    /// not been still for longer than the grace period.
    pub fn observe(
        &mut self,
        progress: &ScanProgress,
        now: std::time::Instant,
    ) -> Option<std::time::Duration> {
        let counters = [
            progress.files.load(Ordering::Relaxed),
            progress.dirs.load(Ordering::Relaxed),
            progress.bytes.load(Ordering::Relaxed),
            progress.errors.load(Ordering::Relaxed),
            progress.clones_probed.load(Ordering::Relaxed),
        ];
        if counters != self.counters {
            self.counters = counters;
            self.since = now;
            return None;
        }
        // `checked_duration_since`, not `-`: the caller passes the clock, and
        // one handed an earlier instant should get "not stalled", not a panic.
        let waited = now.checked_duration_since(self.since)?;
        (waited >= self.grace).then_some(waited)
    }
}

/// Live counters a UI can poll while a scan runs, and the switch that stops it.
#[derive(Debug, Default)]
pub struct ScanProgress {
    pub files: AtomicU64,
    pub dirs: AtomicU64,
    pub bytes: AtomicU64,
    pub errors: AtomicU64,
    /// Clone candidates probed, during the phase after the walk.
    ///
    /// Its own counter because that phase moves none of the others, and a
    /// caller watching for a stall has to be able to tell "still working" from
    /// "stuck". Measured on `~/github`: probing was 1193 ms of a 1989 ms scan,
    /// so a phase with no counter would look frozen for most of the run — and
    /// on a disk ten times the size it would trip any stall warning.
    pub clones_probed: AtomicU64,
    /// Which phase the scan is in, for a caller that wants to say so. Written
    /// once per phase; read as [`ScanProgress::phase`].
    phase: AtomicU8,
    /// Set by [`ScanProgress::cancel`] and read once per directory.
    cancelled: AtomicBool,
    /// Directories whose listing has started and not finished.
    ///
    /// This exists for one failure: a mount that stops answering. The walk
    /// then blocks inside `read_dir` or inside the `metadata` of one entry,
    /// every counter freezes, and without this there is nothing to tell the
    /// difference between "stuck" and "slow" — let alone *where*. A caller
    /// that sees the counters stand still can read this and name the path.
    ///
    /// Scoped to the listing loop, not to the recursion, so it holds one path
    /// per worker rather than every ancestor: the blocked directory is the
    /// leaf, and a list with its ancestors in it buries the answer.
    reading: Mutex<HashSet<PathBuf>>,
}

impl ScanProgress {
    /// Ask a running scan to stop.
    ///
    /// The walk notices between directories rather than between entries: a
    /// check per entry would be a shared atomic read in the hottest loop, and
    /// abandoning a directory already read gains nothing. In practice the
    /// difference is invisible, because every thread stops descending at once.
    ///
    /// The scan then fails with [`std::io::ErrorKind::Interrupted`]. It does not
    /// return the partial tree: a tree missing an unknowable part of itself
    /// would report totals that are simply wrong, and nothing good comes of
    /// storing that next to real snapshots.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// What the scan is doing now.
    pub fn phase(&self) -> Phase {
        Phase::from_u8(self.phase.load(Ordering::Relaxed))
    }

    fn enter_phase(&self, phase: Phase) {
        self.phase.store(phase as u8, Ordering::Relaxed);
    }

    /// Directories being listed right now, in no particular order.
    ///
    /// Read this when the counters have stopped moving. A hung mount leaves
    /// its directory here for as long as the kernel keeps the thread, so what
    /// comes back is the answer to "what is it waiting on".
    pub fn reading_now(&self) -> Vec<PathBuf> {
        let guard = self
            .reading
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut paths: Vec<PathBuf> = guard.iter().cloned().collect();
        paths.sort();
        paths
    }

    /// Mark `dir` as being listed until the returned guard is dropped.
    ///
    /// A guard rather than a pair of calls because the listing loop returns
    /// early on cancellation and on an unreadable directory, and a path left
    /// behind on one of those paths would be reported forever as the thing
    /// the scan is stuck on.
    fn listing(&self, dir: &Path) -> Listing<'_> {
        self.reading
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(dir.to_path_buf());
        Listing {
            progress: self,
            dir: dir.to_path_buf(),
        }
    }
}

/// Removes a directory from [`ScanProgress::reading_now`] however the listing
/// ends.
struct Listing<'a> {
    progress: &'a ScanProgress,
    dir: PathBuf,
}

impl Drop for Listing<'_> {
    fn drop(&mut self) {
        self.progress
            .reading
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.dir);
    }
}

/// What the scan found, beyond the tree itself.
#[derive(Debug, Clone, Default)]
pub struct ScanStats {
    pub files: u64,
    pub dirs: u64,
    pub errors: u64,
    /// Hardlinked files met more than once and counted only the first time.
    pub hardlinks_deduped: u64,
    /// Clones whose blocks were already counted under another name.
    pub clones_deduped: u64,
    /// Up to `MAX_REPORTED_ERRORS` paths that could not be read.
    pub error_samples: Vec<(PathBuf, String)>,
    pub duration_ms: u64,
    /// Capacity of the filesystem the root sits on, when the OS can say.
    ///
    /// A scan measures what a folder uses; this is the other half of "when
    /// does it fill up". `None` means the platform could not answer, which is
    /// not an error.
    pub capacity: Option<Capacity>,
}

struct Ctx {
    opts: ScanOptions,
    root_dev: u64,
    seen_inodes: Mutex<HashSet<(u64, u64)>>,
    hardlinks_deduped: AtomicU64,
    errors: Mutex<Vec<(PathBuf, String)>>,
    progress: Arc<ScanProgress>,
}

impl Ctx {
    fn note_error(&self, path: &Path, err: &std::io::Error) {
        self.progress.errors.fetch_add(1, Ordering::Relaxed);
        let mut guard = self.errors.lock().unwrap();
        if guard.len() < MAX_REPORTED_ERRORS {
            guard.push((path.to_path_buf(), err.to_string()));
        }
    }

    /// Returns false when this (dev, ino) pair was already counted.
    fn claim_inode(&self, meta: &RawMeta) -> bool {
        if !self.opts.dedupe_hardlinks || !meta.is_hardlinked() {
            return true;
        }
        let fresh = self
            .seen_inodes
            .lock()
            .unwrap()
            .insert((meta.dev, meta.ino));
        if !fresh {
            self.hardlinks_deduped.fetch_add(1, Ordering::Relaxed);
        }
        fresh
    }
}

/// One directory's children: every name in a single buffer, and the entries
/// that point into it.
///
/// A name per entry used to be a `String`, which is one heap allocation for
/// each of the 412k entries on a real disk. Names arrive from
/// `to_string_lossy`, which borrows when the name is already valid UTF-8 — so
/// concatenating them per directory turns those allocations into one per
/// directory instead, roughly a tenth as many.
#[derive(Default)]
struct Children {
    names: String,
    entries: Vec<RawEntry>,
}

/// One entry as returned by the recursive phase, before flattening.
struct RawEntry {
    /// Range inside the owning [`Children::names`].
    name_off: u32,
    name_len: u16,
    kind: EntryKind,
    size: u64,
    alloc: u64,
    mtime: i64,
    /// Narrower than the platform's `nlink` on purpose: the arena stores a
    /// `u32`, and a link count that overflowed one would be a filesystem bug
    /// rather than something to carry eight bytes for.
    nlink: u32,
    /// Boxed so a file — nine entries in ten — carries a pointer rather than a
    /// `String` and a `Vec` inline. That alone took this struct from 88 bytes
    /// to 48, and the whole walk holds one of these per entry.
    children: Option<Box<Children>>,
}

/// Append `name` and return the range that addresses it, truncating on a
/// character boundary at the length an offset pair can describe. No filesystem
/// produces a name that long; losing the scan over one would be the worse
/// answer.
fn push_name(buf: &mut String, name: &str) -> (u32, u16) {
    let mut name = name;
    if name.len() > u16::MAX as usize {
        let mut end = u16::MAX as usize;
        while end > 0 && !name.is_char_boundary(end) {
            end -= 1;
        }
        name = &name[..end];
    }
    let off = buf.len() as u32;
    buf.push_str(name);
    (off, name.len() as u16)
}

/// The most threads the default will use.
///
/// Not optimal anywhere — it is the setting that is never bad. See
/// `default_threads`.
const THREAD_CAP: usize = 8;

/// How many threads to walk with when the caller does not choose.
///
/// One per logical core — rayon's default, and what this used to do — is the
/// worst measured setting on every corpus tried. Walking is syscall-bound, so
/// past a point the threads are queueing in the kernel rather than working,
/// and the coordination is pure loss.
///
/// There is no best fixed number: the optimum moves with the shape of the
/// tree. Measured on an M3 Max (12 performance + 4 efficiency cores),
/// interleaved runs, median of 9 `[ölçüm]`:
///
/// ```text
///                        best        8        16 (the old default)
///   /usr, 50k entries      6 →  71    92 ms    151 ms
///   /Applications, 412k   12 → 1233  1407 ms   1581 ms
/// ```
///
/// So 8 is a compromise and not an optimum: it loses 30% to the best setting
/// on the small tree and 14% on the large one. It is chosen because it beats
/// the old default on both — by 39% and 11% — and because a caller who knows
/// their disk can say `--threads`. Picking the winner for one corpus would
/// have made the other markedly worse.
fn default_threads() -> usize {
    std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(THREAD_CAP)
}

/// The pool the walk runs on.
///
/// A pool of its own rather than rayon's global one. The global pool is
/// process-wide and can only be configured once, so a library that reached for
/// it would be deciding on behalf of whatever application linked it — and the
/// desktop app runs a scan next to its own work.
fn walk_pool(threads: Option<usize>) -> std::io::Result<rayon::ThreadPool> {
    let count = threads.filter(|n| *n > 0).unwrap_or_else(default_threads);
    rayon::ThreadPoolBuilder::new()
        .num_threads(count)
        .thread_name(|i| format!("spacetrace-walk-{i}"))
        .build()
        .map_err(std::io::Error::other)
}

/// Walk `root` and build a tree. The traversal is a parallel DFS: each
/// directory's subdirectories are recursed into on the rayon pool, which keeps
/// SSDs busy without the memory blow-up of a breadth-first queue.
pub fn scan(
    root: impl AsRef<Path>,
    opts: ScanOptions,
    progress: Arc<ScanProgress>,
) -> std::io::Result<(Tree, ScanStats)> {
    let started = std::time::Instant::now();
    let root = root.as_ref();
    let root_path = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

    // Built before `opts` moves into the context below.
    let pool = walk_pool(opts.threads)?;

    let root_md = std::fs::symlink_metadata(&root_path)?;
    let (root_meta, root_failure) = RawMeta::for_path(
        &root_path,
        &root_md,
        identity_needed(&opts, root_md.is_dir()),
    );

    let ctx = Ctx {
        root_dev: root_meta.dev,
        opts,
        seen_inodes: Mutex::new(HashSet::new()),
        hardlinks_deduped: AtomicU64::new(0),
        errors: Mutex::new(Vec::new()),
        progress: Arc::clone(&progress),
    };
    if let Some(e) = root_failure {
        ctx.note_error(&root_path, &e);
    }

    let children = if root_meta.kind == EntryKind::Dir {
        progress.dirs.fetch_add(1, Ordering::Relaxed);
        Some(pool.install(|| read_dir_parallel(&root_path, 1, &ctx)))
    } else {
        progress.files.fetch_add(1, Ordering::Relaxed);
        progress.bytes.fetch_add(root_meta.alloc, Ordering::Relaxed);
        None
    };

    // Bail before building anything: a cancelled walk returns empty directories,
    // so the tree would look complete while silently missing most of the disk.
    if progress.is_cancelled() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "scan cancelled",
        ));
    }

    // The exact node count is already known here — the walk counted every entry
    // it visited — so the arena is allocated once instead of doubling its way
    // up. That matters more than it looks: growing from 1024 to 412k nodes ends
    // at a capacity of 524288, and during the final reallocation the old and
    // new buffers are alive together. Reserving removes both the overshoot and
    // that transient, which measured as a third of peak memory on /Applications.
    //
    // `+ 1` is the root, which the counters below do not include for files.
    let expected = progress.files.load(Ordering::Relaxed) + progress.dirs.load(Ordering::Relaxed);
    let mut builder = TreeBuilder::with_capacity(expected as usize + 1);
    let root_name = display_name(&root_path);
    let root_id = builder.push_root(NewNode {
        name: &root_name,
        kind: root_meta.kind,
        size: if root_meta.kind == EntryKind::Dir {
            0
        } else {
            root_meta.size
        },
        alloc: root_meta.alloc,
        mtime: root_meta.mtime,
        nlink: 1,
    });
    if let Some(children) = children {
        flatten(&mut builder, root_id, children);
    }
    progress.enter_phase(Phase::Finishing);
    let clones_deduped = if ctx.opts.dedupe_clones {
        // On the same pool: this probes one file at a time over `fcntl`, so it
        // is the same kind of work as the walk and wants the same width.
        pool.install(|| dedupe_clones(&mut builder, &root_path, &progress))
    } else {
        0
    };
    let tree = builder.finish(root_path);

    let stats = ScanStats {
        files: progress.files.load(Ordering::Relaxed),
        dirs: progress.dirs.load(Ordering::Relaxed),
        errors: progress.errors.load(Ordering::Relaxed),
        hardlinks_deduped: ctx.hardlinks_deduped.load(Ordering::Relaxed),
        clones_deduped,
        error_samples: ctx.errors.into_inner().unwrap(),
        duration_ms: started.elapsed().as_millis() as u64,
        // Asked once, after the walk: it describes the mount, not the tree,
        // and a failure here must not fail the scan.
        capacity: crate::capacity::capacity_of(tree.root_path()),
    };
    Ok((tree, stats))
}

/// Charge copy-on-write clones once, the way hardlinks are charged once.
///
/// Runs after the walk and before aggregation, because it needs the whole tree
/// to work out which files are even worth asking about: only a file whose size
/// collides with another file's can be a clone, and finding those collisions
/// costs nothing next to an open per file. On one real tree that filter cut the
/// probes from every file to a tenth of them.
///
/// Unlike hardlink deduplication, *which* copy keeps the bytes is defined here:
/// the lowest node id, which is the entry nearest the top of the tree in BFS
/// order. It costs a sort and it means the answer does not depend on which
/// thread finished first.
fn dedupe_clones(builder: &mut TreeBuilder, root: &Path, progress: &ScanProgress) -> u64 {
    let mut by_size: std::collections::HashMap<u64, Vec<NodeId>> = std::collections::HashMap::new();
    for (index, node) in builder.nodes.iter().enumerate() {
        if node.kind == EntryKind::File && node.own_size >= CLONE_MIN_BYTES {
            by_size
                .entry(node.own_size)
                .or_default()
                .push(index as NodeId);
        }
    }
    let candidates: Vec<NodeId> = by_size
        .into_values()
        .filter(|group| group.len() > 1)
        .flatten()
        .collect();
    if candidates.is_empty() {
        return 0;
    }

    // Paths first, then probes, because the probe borrows nothing from the
    // builder and can therefore run on the pool.
    let paths: Vec<(NodeId, PathBuf)> = candidates
        .into_iter()
        .map(|id| (id, builder.path_of(id, root)))
        .collect();
    let mut probed: Vec<(NodeId, u64)> = paths
        .into_par_iter()
        .filter_map(|(id, path)| {
            let key = crate::meta::clone_key(&path);
            // Counted whether or not the file turned out to be a clone: the
            // point is to show the phase is moving, and a filesystem with no
            // clones at all would otherwise look stuck for the whole probe.
            progress.clones_probed.fetch_add(1, Ordering::Relaxed);
            key.map(|key| (id, key))
        })
        .collect();
    probed.sort_unstable();

    let mut charged: HashSet<u64> = HashSet::new();
    let mut deduped = 0;
    for (id, key) in probed {
        if charged.insert(key) {
            continue;
        }
        builder.charge_nothing(id);
        deduped += 1;
    }
    deduped
}

/// Whether this entry's identity will actually be read.
///
/// Deduplication reads the link count, which only a regular file can have above
/// one; `one_filesystem` reads the volume, which is only ever compared for a
/// directory the walk might descend into. Asking for neither costs nothing on
/// Unix and saves an open file handle per entry on Windows, where these fields
/// are not in the directory listing.
fn identity_needed(opts: &ScanOptions, is_dir: bool) -> FileIdentity {
    let wanted = if is_dir {
        opts.one_filesystem
    } else {
        opts.dedupe_hardlinks
    };
    if !wanted {
        return FileIdentity::Skipped;
    }
    FileIdentity::Needed
}

fn read_dir_parallel(dir: &Path, depth: usize, ctx: &Ctx) -> Children {
    // Checked before the syscall, so a cancelled scan stops issuing I/O
    // immediately instead of draining whatever rayon had already queued.
    if ctx.progress.is_cancelled() {
        return Children::default();
    }
    // Guarded from here to the end of the listing loop, and no further: this
    // is the stretch that blocks on a mount that has stopped answering, and
    // the recursion below runs on the pool where it would only add ancestors
    // to the list. See `ScanProgress::reading_now`.
    let listing = ctx.progress.listing(dir);

    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => {
            ctx.note_error(dir, &e);
            return Children::default();
        }
    };

    // Read the whole directory first, then fan out. Doing the syscalls for one
    // directory on a single thread keeps readdir sequential (which is what the
    // kernel is fastest at) while different directories still run in parallel.
    // Names are collected here, on this one thread, so that the parallel phase
    // below only has to carry offsets into a buffer nobody else writes to.
    let mut names = String::new();
    let mut pending: Vec<(PathBuf, u32, u16, RawMeta)> = Vec::new();
    for entry in rd {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                ctx.note_error(dir, &e);
                continue;
            }
        };
        let path = entry.path();
        let md = match entry.metadata() {
            // `DirEntry::metadata` does not follow symlinks, which is what we
            // want: a symlink is counted as itself, never as its target.
            Ok(md) => md,
            Err(e) => {
                ctx.note_error(&path, &e);
                continue;
            }
        };
        let (meta, failure) =
            RawMeta::for_path(&path, &md, identity_needed(&ctx.opts, md.is_dir()));
        // A metadata field the platform would not answer is counted like any
        // other read failure (invariant #7). The entry itself stays: it has a
        // name and a logical size, and dropping it would be the larger lie.
        if let Some(e) = failure {
            ctx.note_error(&path, &e);
        }
        let raw_name = entry.file_name();
        let (name_off, name_len) = push_name(&mut names, &raw_name.to_string_lossy());
        pending.push((path, name_off, name_len, meta));
    }

    // Listing done; the recursion that follows is not what hangs.
    drop(listing);

    let entries = pending
        .into_par_iter()
        .map(|(path, name_off, name_len, meta)| {
            build_entry(path, name_off, name_len, meta, depth, ctx)
        })
        .collect();
    Children { names, entries }
}

/// Whether this directory's name is on the skip list.
fn is_excluded(path: &Path, excluded: &[String]) -> bool {
    if excluded.is_empty() {
        return false;
    }
    let Some(name) = path.file_name() else {
        return false;
    };
    let name = name.to_string_lossy();
    excluded.iter().any(|x| x.as_str() == name)
}

fn build_entry(
    path: PathBuf,
    name_off: u32,
    name_len: u16,
    meta: RawMeta,
    depth: usize,
    ctx: &Ctx,
) -> RawEntry {
    let is_dir = meta.kind == EntryKind::Dir;

    // Ordered so the cheap tests run first: `is_excluded` has to recover the
    // name from the path, and there is no reason to pay for that on a file or
    // when nothing is excluded at all.
    let descend = is_dir
        && ctx.opts.max_depth.is_none_or(|max| depth < max)
        && (!ctx.opts.one_filesystem || meta.dev == ctx.root_dev)
        && !is_excluded(&path, &ctx.opts.exclude_names);

    if is_dir {
        ctx.progress.dirs.fetch_add(1, Ordering::Relaxed);
    } else {
        ctx.progress.files.fetch_add(1, Ordering::Relaxed);
    }

    // A hardlinked file already counted elsewhere stays visible in the tree but
    // contributes no bytes, so a directory's total never double-counts it.
    let counted = ctx.claim_inode(&meta);
    // A directory's own `len()` is its inode size, not user data. It is real
    // disk usage, so it counts towards `alloc`, but adding it to the logical
    // size would make totals disagree with "sum of the files in here".
    let (size, alloc) = match (counted, is_dir) {
        (false, _) => (0, 0),
        (true, true) => (0, meta.alloc),
        (true, false) => (meta.size, meta.alloc),
    };
    if counted && !is_dir {
        ctx.progress.bytes.fetch_add(alloc, Ordering::Relaxed);
    }

    let children = if descend {
        Some(Box::new(read_dir_parallel(&path, depth + 1, ctx)))
    } else {
        None
    };

    RawEntry {
        name_off,
        name_len,
        kind: meta.kind,
        size,
        alloc,
        mtime: meta.mtime,
        nlink: meta.nlink.min(u32::MAX as u64) as u32,
        children,
    }
}

/// Flatten the recursive result into the arena in BFS order, so that every
/// node's children end up in one contiguous index range.
fn flatten(builder: &mut TreeBuilder, root_id: NodeId, root_children: Children) {
    let mut queue: VecDeque<(NodeId, Children)> = VecDeque::new();
    queue.push_back((root_id, root_children));

    while let Some((parent_id, children)) = queue.pop_front() {
        let Children { names, entries } = children;
        if entries.is_empty() {
            continue;
        }
        let start = builder.nodes.len() as NodeId;
        let len = entries.len() as u32;

        let mut grandchildren: Vec<(NodeId, Children)> = Vec::new();
        for (i, entry) in entries.into_iter().enumerate() {
            let from = entry.name_off as usize;
            let name = names
                .get(from..from + entry.name_len as usize)
                .unwrap_or_default();
            let id = builder.push(
                parent_id,
                NewNode {
                    name,
                    kind: entry.kind,
                    size: entry.size,
                    alloc: entry.alloc,
                    mtime: entry.mtime,
                    nlink: entry.nlink,
                },
            );
            debug_assert_eq!(id, start + i as NodeId);
            if let Some(kids) = entry.children {
                if !kids.entries.is_empty() {
                    grandchildren.push((id, *kids));
                }
            }
        }

        let parent = &mut builder.nodes[parent_id as usize];
        parent.children_start = start;
        parent.children_len = len;

        queue.extend(grandchildren);
    }
}

#[cfg(test)]
mod thread_tests {
    use super::*;

    #[test]
    fn an_explicit_count_is_honoured() {
        let pool = walk_pool(Some(3)).unwrap();
        assert_eq!(pool.current_num_threads(), 3);
    }

    /// `Some(0)` reaches rayon as "pick for me", which would quietly restore
    /// the one-per-core default this exists to avoid.
    #[test]
    fn zero_is_treated_as_no_answer() {
        let pool = walk_pool(Some(0)).unwrap();
        assert_eq!(pool.current_num_threads(), default_threads());
    }

    /// The literal 8 is deliberate. Asserting against `THREAD_CAP` would
    /// compare the constant with itself, so raising the cap — or deleting it —
    /// would still pass. Changing the cap should have to change this line, and
    /// changing this line should mean re-reading the measurements behind it.
    /// The guard mechanics, deterministically. What this cannot reach is
    /// whether the *walk* still calls `listing` at all: observing that needs a
    /// look inside a running scan, and the only way to time one is to bet on
    /// the scheduler. That half was checked by hand instead — a real scan with
    /// 8 threads printed "(+7 more)", which is one directory per worker and no
    /// ancestors.
    #[test]
    fn a_listing_appears_while_it_runs_and_is_gone_after() {
        let progress = ScanProgress::default();
        assert!(progress.reading_now().is_empty());
        {
            let _outer = progress.listing(Path::new("/one"));
            let _inner = progress.listing(Path::new("/two"));
            assert_eq!(
                progress.reading_now(),
                vec![PathBuf::from("/one"), PathBuf::from("/two")],
                "both are being listed, and the answer is sorted"
            );
        }
        assert!(
            progress.reading_now().is_empty(),
            "the guards went out of scope, so nothing is being listed"
        );
    }

    #[test]
    fn the_default_is_capped_and_never_zero() {
        let n = default_threads();
        assert!(n >= 1, "a pool of no threads does no work");
        assert!(
            n <= 8,
            "the default grew past the measured cap: {n}. \
             docs/COMPETITORS.md §1.2 has the numbers this was chosen from"
        );
    }
}
