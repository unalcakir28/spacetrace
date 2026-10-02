use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rayon::prelude::*;

use crate::capacity::Capacity;
use crate::clones::Families;
use crate::extents::{Claims, Deferred, FsKind, Mapped, Volume, Volumes};
use crate::meta::{display_name, EntryKind, FileIdentity, NamedMeta, RawMeta};
use crate::mounts::Mounts;
use crate::partial::PartialTree;
use crate::tree::{NewNode, NodeId, Tree, TreeBuilder};

/// How many failing paths we keep for the report before we only count them.
const MAX_REPORTED_ERRORS: usize = 64;

/// Stack for each walk thread.
///
/// The walk recurses once per directory level, so the stack is what bounds the
/// depth it survives — and rayon hands its workers the ordinary thread default,
/// which a real tree can exhaust. Nesting about 210 levels used to abort the
/// process, taking a running agent down with it. This is deliberately far more
/// than `MAX_WALK_DEPTH` can spend, so the guard below is what ends a descent
/// and the stack never is.
const WALK_STACK_BYTES: usize = 16 * 1024 * 1024;

/// How deep the walk will go, whatever the caller asked for.
///
/// Not a reporting choice — `ScanOptions::max_depth` is that one, and it
/// defaults to unlimited. This is the bound that keeps a pathological tree from
/// becoming a crash: past it the directory is recorded as an error, exactly
/// like one that could not be read, and the scan carries on. A tree this deep
/// is already past what the system can name — 1024 levels exceeds `PATH_MAX` on
/// macOS before a single component is longer than one character.
const MAX_WALK_DEPTH: usize = 1024;

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
    /// Count blocks shared between files only once, the way the disk holds
    /// them: APFS clones on macOS, reflinked and snapshotted extents on btrfs
    /// and XFS. On btrfs it also charges compressed extents at their
    /// compressed size, where the process may read it (`CAP_SYS_ADMIN`).
    ///
    /// One switch for both because both answer the same question — what the
    /// disk holds rather than what each name reports — and switching it off
    /// gives back exactly what `du` says.
    pub dedupe_clones: bool,
    /// How many threads to walk with. `None` (and `Some(0)`) take the default
    /// below, which is measured rather than inherited from the core count.
    pub threads: Option<usize>,
    /// How long a mounted filesystem gets to answer the walk's first question
    /// about it before the walk gives up and records it as unreadable.
    ///
    /// `None` switches the protection off: every mount point is entered the
    /// way an ordinary directory is, and one that has stopped answering wedges
    /// the scan the way it always did.
    ///
    /// The default is generous on purpose. The two mistakes are not
    /// symmetrical: waiting too long makes a scan slow, while giving up too
    /// early omits an entire volume from a total that claims to be complete.
    /// A slow scan is also no longer silent — the CLI, the window and the
    /// agent all say which directory is being waited on.
    pub mount_timeout: Option<Duration>,
    /// Roughly how many entries the walk expects to find, if the caller has a
    /// way to know — the previous scan of the same root is the obvious one.
    ///
    /// Purely a memory hint; a wrong answer costs nothing but a reallocation
    /// and changes no result. It exists because the arena is now filled during
    /// the walk, so its final size is not known when it is created, and a
    /// `Vec` that doubles its way to N holds the old and the new buffer at once
    /// during the last move. At 412k entries that transient is 57 MB; at a
    /// size just past a power of two it is twice the arena. Handing the figure
    /// over turns the whole thing into one allocation.
    ///
    /// Left `None` by a first scan, which has nothing to go on. A scheduled
    /// agent and the desktop's "Rescan" — the two cases where this costs
    /// something — always have a previous scan to ask.
    pub expected_entries: Option<usize>,
}

/// How long a mount point gets by default.
pub const MOUNT_TIMEOUT: Duration = Duration::from_secs(60);

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions {
            exclude_names: Vec::new(),
            one_filesystem: false,
            max_depth: None,
            dedupe_hardlinks: true,
            dedupe_clones: true,
            threads: None,
            mount_timeout: Some(MOUNT_TIMEOUT),
            expected_entries: None,
        }
    }
}

/// How many nodes to reserve before the walk starts.
///
/// The eighth on top of the hint absorbs a disk that grew a little since the
/// last scan without a reallocation; being over is virtual address space that
/// never becomes resident, being under costs one copy. Without a hint this is
/// a small opening guess and the `Vec` doubles from there, exactly as it did
/// before the arena was pre-sized.
///
/// Clamped, because the hint comes from the previous snapshot of the root and
/// a snapshot can arrive over the network: one claiming 2^40 files would ask
/// for 80 TB here and abort every scheduled scan of that root.
fn arena_capacity(hint: Option<usize>) -> usize {
    let Some(entries) = hint else {
        return 4096;
    };
    let entries = entries.min(MAX_CAPACITY_HINT);
    entries + entries / 8 + 1
}

/// The most entries any capacity hint may reserve up front.
///
/// Every hint in this workspace — the walk's, the store's on load and on
/// import — is a number read back from a snapshot, and a snapshot may have
/// crossed a network. Sixteen million entries is past any root measured here;
/// a larger real one costs reallocations, never correctness.
pub const MAX_CAPACITY_HINT: usize = 1 << 24;

/// The stages of a scan, in order.
///
/// Named because a progress line has to stay true: after the walk finishes,
/// "scanning…" is no longer what is happening and the file counter has stopped
/// for good. Invariant 8 is the rule these serve — a phase without a moving
/// counter looks hung while it is working.
///
/// The last two belong to `store`, not to the walk. A scan a person asked for
/// is not over when the tree is built: on 412,983 entries the walk takes
/// 753 ms and writing that tree to the database takes another 571 ms, which is
/// most of a second with nothing on screen. Extrapolated to ten million
/// entries it is about fourteen seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Reading directories.
    Walking,
    /// Walk done; charging the blocks files share (btrfs and XFS extents,
    /// APFS clone families) in a fixed order, then building the tree. The
    /// first part counts its files in `rows_done` of `rows_total`, because on
    /// a tree with its snapshots inside it that is every file.
    Finishing,
    /// Writing the tree into a snapshot database, one row per entry.
    Saving,
    /// Reading those rows back to compute the snapshot's content hash.
    ///
    /// A second pass over the same rows, and a deliberate one: the stored
    /// digest has to come from the same function every reader uses, or it
    /// eventually disagrees with them about some field and a healthy snapshot
    /// reports itself corrupt.
    Checksumming,
}

impl Phase {
    fn from_u8(value: u8) -> Phase {
        match value {
            1 => Phase::Finishing,
            2 => Phase::Saving,
            3 => Phase::Checksumming,
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
    counters: [u64; 6],
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
            counters: [0; 6],
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
            progress.rows_done.load(Ordering::Relaxed),
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
    /// Files whose clone family had to be asked for one at a time.
    ///
    /// **Zero on an ordinary macOS scan.** The platform's bulk listing carries
    /// the family inside the record it was already fetching, so nothing is
    /// probed; this counts only the entries that could not go that way — a
    /// directory holding a mount point, and filesystems with no bulk listing
    /// at all.
    ///
    /// **On Linux it is every regular file**, because there is no bulk
    /// listing. On btrfs and XFS each of them is a real question — an `open`
    /// and a FIEMAP, see `extents.rs` — and this is the counter that moves
    /// while a directory of a hundred thousand of them is being asked, since
    /// `files` is published once per directory.
    ///
    /// It used to count a phase of its own that ran after the walk, and it was
    /// 1193 ms of a 1989 ms scan on `~/github` with no other counter moving.
    /// That phase is gone; the counter stays because it is on the agent's wire
    /// and because the slow path still has to be visible while it runs.
    pub clones_probed: AtomicU64,
    /// Rows written or checked in the phase that is running, if it counts rows.
    ///
    /// Reset when a row-counting phase begins, because a bar that ran to the
    /// end and then started again from there would be reporting one number for
    /// two different jobs. Read together with [`ScanProgress::rows_total`].
    pub rows_done: AtomicU64,
    /// How many rows that phase will get through, or 0 when it is not running.
    pub rows_total: AtomicU64,
    /// Which phase the scan is in, for a caller that wants to say so. Written
    /// once per phase; read as [`ScanProgress::phase`].
    phase: AtomicU8,
    /// Set by [`ScanProgress::cancel`] and read once per directory.
    cancelled: AtomicBool,
    /// The arena the walk is filling, so a caller can read the tree as it
    /// stands rather than waiting for the whole disk.
    ///
    /// On `ScanProgress` rather than returned by `scan`, because the whole
    /// point is to be readable *while* the scan is running, and the progress
    /// object is the one thing a caller already holds during that time. What it
    /// hands back is an ordinary [`Tree`](crate::Tree), so a caller already able
    /// to draw a finished scan needs nothing new to draw a running one.
    pub partial: PartialTree,
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

    /// Announce a phase that works through a known number of rows.
    ///
    /// Public because the phase that needs it lives in `store`: a scan is not
    /// over for the person who asked for it when the tree exists, and the
    /// database write is long enough to need a counter of its own (invariant
    /// 8). Resets the counter, so each phase reports its own progress rather
    /// than continuing the last one's.
    pub fn begin_rows(&self, phase: Phase, total: u64) {
        self.rows_done.store(0, Ordering::Relaxed);
        self.rows_total.store(total, Ordering::Relaxed);
        self.enter_phase(phase);
    }

    /// One more row done.
    ///
    /// Per row rather than batched, unlike the walk's counters: this runs on
    /// the single thread doing the writing, so the atomic is uncontended and
    /// costs nothing next to the `INSERT` beside it. The walk batches because
    /// eight threads share its counters.
    pub fn row_done(&self) {
        self.rows_done.fetch_add(1, Ordering::Relaxed);
    }

    /// How far through a row-counting phase, as done and total.
    ///
    /// `None` when no such phase is running, which is what a caller should
    /// show rather than "0 of 0".
    pub fn rows(&self) -> Option<(u64, u64)> {
        let total = self.rows_total.load(Ordering::Relaxed);
        (total > 0).then(|| (self.rows_done.load(Ordering::Relaxed), total))
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
    ///
    /// On btrfs and XFS a file can share only part of itself, so there it is
    /// the files at least one of whose shared bytes had already been charged.
    pub clones_deduped: u64,
    /// Bytes `du` counts that `alloc` does not, because another name already
    /// holds them — the clones above, measured in bytes.
    ///
    /// Together with `compressed_bytes_saved` this is the whole difference
    /// between `du` and the on-disk total, which is how the tests check it.
    pub shared_bytes_deduped: u64,
    /// How much less than `du` the compressed files on btrfs were charged,
    /// because they were charged at their on-disk size.
    ///
    /// **Signed.** A compressed extent stays whole on disk until nothing
    /// references any of it, so a file that overwrote most of one can cost
    /// more than `du` says — `compsize` agrees — and then this is negative.
    pub compressed_bytes_saved: i64,
    /// Files on btrfs with compressed extents whose on-disk size could not be
    /// read — it takes `CAP_SYS_ADMIN` — and which therefore count at their
    /// uncompressed size, once per name, as `du` counts them. When this is not
    /// zero the on-disk total is too high, and by how much is unknown.
    pub compressed_files_inexact: u64,
    /// Files on btrfs or XFS that could not be opened to ask which of their
    /// extents are shared (no read permission, usually). They count in full,
    /// as `du` counts them, so the total errs high by whatever they share.
    ///
    /// Not an error: the entry itself was read, and the scan is complete.
    pub files_unmapped: u64,
    /// The scan crossed a filesystem whose shared blocks no walk can see —
    /// ZFS, where block clones and deduplicated blocks are kept in pool-wide
    /// tables. Each name counts them in full there, so the total can exceed
    /// what the pool holds.
    pub unseen_sharing: bool,
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

/// How the walk asks a mount point whether it is alive.
///
/// A function pointer rather than a direct call so the tests below can make a
/// probe time out on demand. There is no way to create a filesystem that hangs
/// from a test — that needs a server to kill — so without this seam the entire
/// skip path would ship unexercised, which for a precaution is the same as
/// shipping it broken.
type Probe = fn(&Path, Duration) -> Option<std::io::Result<std::fs::Metadata>>;

/// The real one: `lstat`, on a thread we are prepared to abandon.
///
/// Public so that anything else that has to step onto a mounted filesystem
/// outside a walk — `spacetrace pkgs` reading package databases — approaches
/// it the same way, rather than with a second deadline of its own. `None`
/// means it did not answer within `limit`.
pub fn probe_mount(path: &Path, limit: Duration) -> Option<std::io::Result<std::fs::Metadata>> {
    let owned = path.to_path_buf();
    crate::timeout::with_deadline(limit, move || std::fs::symlink_metadata(&owned))
}

struct Ctx {
    opts: ScanOptions,
    mounts: Mounts,
    probe: Probe,
    root_dev: u64,
    seen_inodes: Mutex<HashSet<(u64, u64)>>,
    hardlinks_deduped: AtomicU64,
    /// Families of copy-on-write clones whose blocks some name has already
    /// been charged for, keyed `(device, clone id)`.
    shared_blocks: Mutex<HashSet<(u64, u64)>>,
    /// Every member of those families, so that after the walk the charge can
    /// move to the one first in `(depth, path)` order (`clones.rs`).
    families: Mutex<Families>,
    clones_deduped: AtomicU64,
    shared_bytes_deduped: AtomicU64,
    /// The filesystems met so far, and which of them can share blocks.
    volumes: Volumes,
    /// Files whose shared extents are charged after the walk, in `(depth,
    /// path)` order, so the same name carries them in every scan.
    deferred: Mutex<Deferred>,
    /// Set by the first btrfs tree search refused for want of privilege, so
    /// the rest of the scan does not ask again.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    compressed_denied: AtomicBool,
    compressed_bytes_saved: AtomicI64,
    compressed_files_inexact: AtomicU64,
    files_unmapped: AtomicU64,
    unseen_sharing: AtomicBool,
    errors: Mutex<Vec<(PathBuf, String)>>,
    progress: Arc<ScanProgress>,
}

impl Ctx {
    /// The arena, written to directly by whichever thread finished listing a
    /// directory.
    ///
    /// The walk used to build a second tree of its own and copy it in at the
    /// end, which meant both were alive at once and cost more than the arena
    /// itself (B1-K). Writing straight in removes that, at the price of one
    /// short critical section per directory — one for every `readdir`, next to
    /// the two this walk already takes per directory for `reading`.
    ///
    /// It lives on the progress object rather than here so that the caller can
    /// read the tree while it is being built; a reader takes this same lock for
    /// the length of one memcpy and no longer (see [`PartialTree`]).
    ///
    /// **Nothing that can block goes inside this lock.** No rayon call, no
    /// live-view publish, no `claim_inode`: the section is a memcpy of one
    /// directory's names and a run of node writes, and it stays that way.
    fn builder(&self) -> std::sync::MutexGuard<'_, TreeBuilder> {
        self.progress.partial.lock()
    }

    /// Ask a mount point for its metadata, with the configured patience.
    ///
    /// `None` means it did not answer. Only called when `mount_timeout` is
    /// set, so the `unwrap_or` below is unreachable in practice; it is there
    /// rather than an `expect` because a panic in the walk would lose a whole
    /// scan over a bookkeeping slip.
    fn probe_mount(&self, path: &Path) -> Option<std::io::Result<std::fs::Metadata>> {
        let limit = self.opts.mount_timeout.unwrap_or(MOUNT_TIMEOUT);
        (self.probe)(path, limit)
    }

    /// A mount that stopped answering is an unreadable path, not a reason to
    /// stop (invariant #7). The entry is dropped the same way an entry whose
    /// metadata could not be read is dropped, and the walk carries on with its
    /// siblings — which is the whole point, since today one dead mount takes
    /// the rest of its directory down with it.
    fn note_unreachable_mount(&self, path: &Path) {
        let limit = self.opts.mount_timeout.unwrap_or(MOUNT_TIMEOUT);
        self.note_error(
            path,
            &std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "mounted filesystem did not respond within {}s; skipped",
                    limit.as_secs()
                ),
            ),
        );
    }

    fn note_error(&self, path: &Path, err: &std::io::Error) {
        self.progress.errors.fetch_add(1, Ordering::Relaxed);
        let mut guard = self.errors.lock().unwrap();
        if guard.len() < MAX_REPORTED_ERRORS {
            guard.push((path.to_path_buf(), err.to_string()));
        }
    }

    /// Which of a directory's entries are charged for their own blocks, or
    /// `None` when all of them are.
    ///
    /// Two deduplications resolved in one pass, because they interact. A
    /// hardlinked name met a second time carries no bytes; so does a name
    /// whose copy-on-write clone family has already been charged — a clone
    /// has its own inode and `nlink == 1`, so hardlink deduplication cannot
    /// see it, yet the disk holds those blocks once. That is why `du`
    /// over-counts a cloned tree and `alloc` deliberately does not
    /// (invariant #1); Cargo alone produces clones by the thousand, copying
    /// build artifacts out of `target/debug/deps`.
    ///
    /// **Order matters.** A name dropped as a repeat hardlink must not also
    /// consume its clone family's claim, or the family's blocks end up
    /// charged to nobody at all and the total quietly comes out short.
    ///
    /// **One lock per set per directory, taken only when there is something to
    /// claim.** On a tree of build artifacts nearly every file is a clone, and
    /// a process-wide mutex taken per entry would be the walk's slowest
    /// point — the same reason the arena is written a directory at a time
    /// rather than an entry at a time.
    ///
    /// Which name keeps a hardlink's bytes is undefined (invariant #3): the
    /// first thread to claim wins, and the guarantee is "once", not "this
    /// path". A clone family is charged here to the first member too, which
    /// keeps the running total right, and moved after the walk to the member
    /// first in `(depth, path)` order (`Ctx::settle_clones`): a hardlink's
    /// names are one file, but clones are files of their own, and a diff
    /// would see the bytes moving between them.
    ///
    /// A third kind follows on Linux, by physical extent rather than by
    /// family (`extents.rs`), and the same order holds for it: a repeat
    /// hardlink is not charged for any of it. Its shared extents are not
    /// claimed here at all but after the walk (`Deferred`); here the file is
    /// charged only what it holds alone.
    fn claim(&self, pending: &[Pending]) -> Option<Vec<Charge>> {
        let hardlinks =
            self.opts.dedupe_hardlinks && pending.iter().any(|p| p.meta.is_hardlinked());
        let clones = self.opts.dedupe_clones && pending.iter().any(|p| p.share.is_some());
        // Only ever mapped with `dedupe_clones` on; see `map_extents`.
        let extents = pending.iter().any(|p| p.extents.is_some());
        if !hardlinks && !clones && !extents {
            return None;
        }

        let mut counted = vec![Charge::Full; pending.len()];
        if hardlinks {
            let mut deduped = 0u64;
            let mut seen = self.seen_inodes.lock().unwrap();
            for (slot, p) in counted.iter_mut().zip(pending) {
                if !p.meta.is_hardlinked() || seen.insert((p.meta.dev, p.meta.ino)) {
                    continue;
                }
                *slot = Charge::Nothing;
                deduped += 1;
            }
            drop(seen);
            self.hardlinks_deduped.fetch_add(deduped, Ordering::Relaxed);
        }
        if clones {
            let (mut deduped, mut bytes) = (0u64, 0u64);
            let mut seen = self.shared_blocks.lock().unwrap();
            for (slot, p) in counted.iter_mut().zip(pending) {
                if *slot == Charge::Nothing {
                    continue;
                }
                let Some(key) = p.share else {
                    continue;
                };
                if seen.insert((p.meta.dev, key)) {
                    continue;
                }
                *slot = Charge::Cloned;
                deduped += 1;
                bytes += p.meta.alloc;
            }
            drop(seen);
            self.clones_deduped.fetch_add(deduped, Ordering::Relaxed);
            self.shared_bytes_deduped
                .fetch_add(bytes, Ordering::Relaxed);
        }
        if extents {
            self.charge_own_extents(pending, &mut counted);
        }
        Some(counted)
    }

    /// Charge the mapped entries what they hold alone. Touches no shared
    /// state but the statistics: what they share waits for the walk to end.
    fn charge_own_extents(&self, pending: &[Pending], counted: &mut [Charge]) {
        let (mut saved, mut inexact) = (0i64, 0u64);
        for (slot, p) in counted.iter_mut().zip(pending) {
            // A repeat hardlink is not charged at all, so it must not claim
            // what the name that *is* charged needs.
            if !slot.counts() {
                continue;
            }
            let Some(mapped) = p.extents.as_deref() else {
                continue;
            };
            inexact += u64::from(mapped.compressed_inexact);
            saved += mapped.compressed_saved;
            *slot = Charge::Alloc(mapped.own_alloc(p.meta.alloc));
        }
        if saved != 0 {
            self.compressed_bytes_saved
                .fetch_add(saved, Ordering::Relaxed);
        }
        if inexact > 0 {
            self.compressed_files_inexact
                .fetch_add(inexact, Ordering::Relaxed);
        }
    }

    /// The volume directory `dir`, on device `dev`, sits on; `parent` is the
    /// one its parent directory was on, `None` for the root. `None` back
    /// means the filesystem did not answer in time.
    ///
    /// Only a change of device asks anything, and not every change:
    ///
    /// * **A mount point** — or the root, which may be one — is asked
    ///   through the same deadline the walk gives any mount point, because a
    ///   `statfs` on a mount whose server has gone does not return.
    /// * **A new device where no mount is, under btrfs,** is a subvolume or a
    ///   snapshot: the same filesystem, so the same domain, and nothing to
    ///   ask. Asking would be a `statfs` and an `open` per snapshot.
    /// * Anywhere else it is a mount the table did not list — mounted after
    ///   the scan started — and is asked like a mount point.
    ///
    /// Nothing is asked with clone deduplication off: that scan counts what
    /// every name reports.
    fn volume_of(&self, dev: u64, dir: &Path, parent: Option<Volume>) -> Option<Volume> {
        if !self.opts.dedupe_clones {
            return Some(Volume {
                dev,
                kind: FsKind::Plain,
            });
        }
        if let Some(parent) = parent {
            if parent.dev == dev {
                return Some(parent);
            }
            let subvolume = matches!(parent.kind, FsKind::Reflink { btrfs: true, .. })
                && !self.mounts.contains(dir);
            if subvolume {
                return Some(Volume {
                    dev,
                    kind: parent.kind,
                });
            }
        }
        let volume = self.volumes.lookup(dev, dir, self.opts.mount_timeout)?;
        if volume.kind == FsKind::Opaque {
            self.unseen_sharing.store(true, Ordering::Relaxed);
        }
        Some(volume)
    }

    /// Charge what the walk deferred into `builder`, in `(depth, path)`
    /// order. `Err` only for a cancelled scan.
    ///
    /// A phase of its own, and on a tree with its snapshots inside it one
    /// that touches every file, so it counts them in `rows_done` (invariant
    /// 8) and checks for cancellation as it goes (invariant 5).
    fn settle_shared(&self, builder: &mut TreeBuilder) -> std::io::Result<()> {
        let deferred = std::mem::take(
            &mut *self
                .deferred
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        if deferred.is_empty() {
            return Ok(());
        }
        let progress = &self.progress;
        progress.begin_rows(Phase::Finishing, deferred.len() as u64);
        let mut done = 0u64;
        let settled = deferred.settle(builder, &mut Claims::default(), |charged| {
            progress.row_done();
            if charged > 0 {
                progress.bytes.fetch_add(charged, Ordering::Relaxed);
            }
            done += 1;
            // Every few thousand files is soon enough to notice a cancel, and
            // keeps a shared atomic read out of the per-file step.
            done % 4096 != 0 || !progress.is_cancelled()
        });
        let Some(settled) = settled else {
            return Err(cancelled());
        };
        self.clones_deduped
            .fetch_add(settled.files_sharing, Ordering::Relaxed);
        self.shared_bytes_deduped
            .fetch_add(settled.shared_bytes, Ordering::Relaxed);
        Ok(())
    }

    /// Move each clone family's charge to its member first in `(depth,
    /// path)` order. `Err` only for a cancelled scan.
    ///
    /// Counted in `rows_done` (invariant 8) and cancellable (invariant 5) like
    /// `settle_shared`: on a tree of build artifacts most files are clones.
    fn settle_clones(&self, builder: &mut TreeBuilder) -> std::io::Result<()> {
        let families = std::mem::take(
            &mut *self
                .families
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        if families.is_empty() {
            return Ok(());
        }
        let progress = &self.progress;
        progress.begin_rows(Phase::Finishing, families.len() as u64);
        let mut done = 0u64;
        let moved = families.settle(builder, || {
            progress.row_done();
            done += 1;
            // As in `settle_shared`: soon enough, and no shared atomic read
            // per member.
            done % 4096 != 0 || !progress.is_cancelled()
        });
        let Some(moved) = moved else {
            return Err(cancelled());
        };
        // The total charged moves by the opposite amount, which the live
        // counter should follow.
        match moved.shared_bytes {
            d if d > 0 => {
                self.shared_bytes_deduped
                    .fetch_add(d as u64, Ordering::Relaxed);
                progress.bytes.fetch_sub(d as u64, Ordering::Relaxed);
            }
            d if d < 0 => {
                self.shared_bytes_deduped
                    .fetch_sub(d.unsigned_abs(), Ordering::Relaxed);
                progress
                    .bytes
                    .fetch_add(d.unsigned_abs(), Ordering::Relaxed);
            }
            _ => {}
        }
        Ok(())
    }

    /// Which of a file's extents are shared or compressed, where the
    /// filesystem can say. `None` means "charge what `st_blocks` says".
    #[cfg(target_os = "linux")]
    fn map_extents(&self, volume: Volume, path: &Path, meta: &RawMeta) -> Option<Box<Mapped>> {
        let FsKind::Reflink { domain, btrfs } = volume.kind else {
            return None;
        };
        // No blocks, nothing to share. A file on another device than its
        // directory is a bind mount onto a filesystem this volume does not
        // describe, and its addresses would be charged in the wrong space.
        if meta.alloc == 0 || meta.dev != volume.dev {
            return None;
        }
        let cancelled = || self.progress.is_cancelled();
        match crate::extents::linux::map(
            path,
            meta,
            domain,
            btrfs,
            &self.compressed_denied,
            &cancelled,
        ) {
            Ok(mapped) => mapped.map(Box::new),
            Err(_) => {
                self.files_unmapped.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    /// Nowhere else says which extents are shared.
    #[cfg(not(target_os = "linux"))]
    fn map_extents(&self, _volume: Volume, _path: &Path, _meta: &RawMeta) -> Option<Box<Mapped>> {
        None
    }
}

/// What one entry is charged for its own blocks, decided for its whole
/// directory at once (see [`Ctx::claim`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Charge {
    /// What the platform reported.
    Full,
    /// Nothing: a repeat hardlink.
    Nothing,
    /// Nothing for now: a clone whose family is already charged. Which member
    /// keeps the charge is settled after the walk.
    Cloned,
    /// This much for what it holds alone — compressed extents at their
    /// on-disk size, shared extents left out. What it shares is added after
    /// the walk.
    Alloc(u64),
}

impl Charge {
    /// Whether the entry is charged anything during the walk.
    fn counts(self) -> bool {
        !matches!(self, Charge::Nothing | Charge::Cloned)
    }
}

/// One listed entry, with everything needed to account for it.
///
/// The name is carried rather than a full path, because a `PathBuf` per entry
/// is an allocation per entry and only the subdirectories the walk descends
/// into ever need one — a tenth of the entries on a real disk. The raw
/// `OsString` and not a slice of the shared name buffer: that buffer holds
/// `to_string_lossy` output, and rebuilding a path from a lossily converted
/// name would name a file that does not exist.
struct Pending {
    name: std::ffi::OsString,
    name_off: u32,
    name_len: u16,
    meta: RawMeta,
    /// See [`NamedMeta::share`].
    share: Option<u64>,
    /// Its shared and compressed extents, on a filesystem that says (Linux).
    /// Boxed because almost every entry has none.
    extents: Option<Box<Mapped>>,
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
const THREAD_CAP: usize = 6;

/// How many threads to walk with when the caller does not choose.
///
/// One per logical core — rayon's default, and what this used to do — is the
/// worst measured setting on every corpus tried. Walking is syscall-bound, so
/// past a point the threads are queueing in the kernel rather than working,
/// and the coordination is pure loss.
///
/// There is no best fixed number: the optimum moves with the shape of the
/// tree. It also moves when the walk itself gets cheaper, which is what
/// happened on 22 September 2026 when the clone accounting stopped being a
/// phase of its own — less work per entry means coordination is a larger share
/// of the run, and the curve sharpened. Re-measured on the same M3 Max
/// (12 performance + 4 efficiency cores), best of 4–5 runs per setting:
///
/// ```text
///   threads               4      5      6      7      8     10
///   /usr, 50k            84     77     74     76     75     81 ms
///   /Applications, 415k 450    407    424    389    403    457 ms
///   /Projects, 1.06M   1553   1376   1342   1408   1647   2009 ms
/// ```
///
/// 6 is the first setting that is at or near the best on all three rather
/// than winning one and losing another, and 8 — what this was until that
/// day — now costs 22% on the largest of them. Above it the threads are
/// queueing in the kernel and contending for the arena rather than working.
/// A caller who knows their disk can still say `--threads`.
///
/// The previous numbers, for comparison: with the per-file clone probe still
/// in the walk the curve was flat (92/89/96/99/93/98 ms on `/usr`), which is
/// why a wrong cap cost little then and costs a fifth of the run now.
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
        .stack_size(WALK_STACK_BYTES)
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
    // The table is read here rather than in `ScanOptions::default` so that
    // building options never makes a syscall, and so a scan that has switched
    // the protection off does not pay for a table it will not consult.
    let mounts = match opts.mount_timeout {
        Some(_) => Mounts::read(),
        None => Mounts::none(),
    };
    scan_with(root, opts, progress, mounts, probe_mount)
}

/// `scan`, with the mount table and the probe supplied.
///
/// Private: the two extra arguments exist so the tests can build a filesystem
/// boundary that is not one and a probe that never answers.
fn scan_with(
    root: impl AsRef<Path>,
    opts: ScanOptions,
    progress: Arc<ScanProgress>,
    mounts: Mounts,
    probe: Probe,
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

    // The arena exists before the walk does, because the walk writes into it.
    // Its capacity is a guess unless the caller has one: see
    // `ScanOptions::expected_entries`.
    let mut builder = TreeBuilder::with_capacity(arena_capacity(opts.expected_entries));
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

    // Handed over before the walk starts, because from here on the arena is
    // shared: the walk writes into it and the caller reads copies of it.
    progress.partial.install(builder, root_path.clone());

    let ctx = Ctx {
        root_dev: root_meta.dev,
        opts,
        mounts,
        probe,
        seen_inodes: Mutex::new(HashSet::new()),
        hardlinks_deduped: AtomicU64::new(0),
        shared_blocks: Mutex::new(HashSet::new()),
        families: Mutex::new(Families::default()),
        clones_deduped: AtomicU64::new(0),
        shared_bytes_deduped: AtomicU64::new(0),
        volumes: Volumes::default(),
        deferred: Mutex::new(Deferred::default()),
        compressed_denied: AtomicBool::new(false),
        compressed_bytes_saved: AtomicI64::new(0),
        compressed_files_inexact: AtomicU64::new(0),
        files_unmapped: AtomicU64::new(0),
        unseen_sharing: AtomicBool::new(false),
        errors: Mutex::new(Vec::new()),
        progress: Arc::clone(&progress),
    };
    if let Some(e) = root_failure {
        ctx.note_error(&root_path, &e);
    }

    if root_meta.kind == EntryKind::Dir {
        progress.dirs.fetch_add(1, Ordering::Relaxed);
        pool.install(|| walk(&root_path, root_id, 1, root_meta.dev, None, &ctx));
    } else {
        progress.files.fetch_add(1, Ordering::Relaxed);
        progress.bytes.fetch_add(root_meta.alloc, Ordering::Relaxed);
    }

    // Bail before finishing anything: a cancelled walk leaves directories it
    // never listed, so the tree would look complete while silently missing
    // most of the disk (invariant #5).
    if progress.is_cancelled() {
        return Err(cancelled());
    }

    // Taken back rather than copied: the finished tree is built from these
    // exact nodes, and once they are gone a snapshot answers `None` — which is
    // what a refresh still in flight has to be told.
    let mut builder = progress.partial.take();
    progress.enter_phase(Phase::Finishing);
    // Before `finish`, whose aggregation then carries what it adds up the
    // tree like any other leaf value.
    ctx.settle_shared(&mut builder)?;
    ctx.settle_clones(&mut builder)?;
    let tree = builder.finish(root_path);

    let stats = ScanStats {
        files: progress.files.load(Ordering::Relaxed),
        dirs: progress.dirs.load(Ordering::Relaxed),
        errors: progress.errors.load(Ordering::Relaxed),
        hardlinks_deduped: ctx.hardlinks_deduped.load(Ordering::Relaxed),
        clones_deduped: ctx.clones_deduped.load(Ordering::Relaxed),
        shared_bytes_deduped: ctx.shared_bytes_deduped.load(Ordering::Relaxed),
        compressed_bytes_saved: ctx.compressed_bytes_saved.load(Ordering::Relaxed),
        compressed_files_inexact: ctx.compressed_files_inexact.load(Ordering::Relaxed),
        files_unmapped: ctx.files_unmapped.load(Ordering::Relaxed),
        unseen_sharing: ctx.unseen_sharing.load(Ordering::Relaxed),
        error_samples: ctx.errors.into_inner().unwrap(),
        duration_ms: started.elapsed().as_millis() as u64,
        // Asked once, after the walk: it describes the mount, not the tree,
        // and a failure here must not fail the scan.
        capacity: crate::capacity::capacity_of(tree.root_path()),
    };
    Ok((tree, stats))
}

/// What a cancelled scan returns instead of a tree (invariant 5).
fn cancelled() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Interrupted, "scan cancelled")
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

/// Read one directory, write its children into the arena, and recurse into the
/// subdirectories among them.
///
/// `parent_id` is the entry this directory already occupies in the arena — the
/// caller put it there, which is why the children it adds are guaranteed to sit
/// at a higher index. Nothing is returned: the tree is the shared arena, not
/// something assembled from what the recursion hands back.
///
/// `dev` is this directory's own device and `parent` the volume its parent was
/// on, `None` for the root; see `Ctx::volume_of` for what a change costs.
fn walk(dir: &Path, parent_id: NodeId, depth: usize, dev: u64, parent: Option<Volume>, ctx: &Ctx) {
    // Checked before the syscall, so a cancelled scan stops issuing I/O
    // immediately instead of draining whatever rayon had already queued.
    if ctx.progress.is_cancelled() {
        return;
    }
    // Guarded from here to the end of the listing loop, and no further: this
    // is the stretch that blocks on a mount that has stopped answering, and
    // the recursion below runs on the pool where it would only add ancestors
    // to the list. See `ScanProgress::reading_now`.
    let listing = ctx.progress.listing(dir);
    // Inside the guard: a new device can mean a `statfs`, which blocks on a
    // dead mount exactly as the `read_dir` after it would, and a watcher must
    // be able to name the directory either way. One that does not answer in
    // time is unreadable, like any mount past its deadline (invariant 7).
    let Some(volume) = ctx.volume_of(dev, dir, parent) else {
        ctx.note_unreachable_mount(dir);
        return;
    };
    // One lookup for the whole directory. Almost every directory on a real
    // disk holds no mount point, and those pay nothing per entry.
    let guarded = ctx.opts.mount_timeout.is_some() && ctx.mounts.holds_a_boundary(dir);

    // Read the whole directory first, then fan out. Doing the syscalls for one
    // directory on a single thread keeps readdir sequential (which is what the
    // kernel is fastest at) while different directories still run in parallel.
    // Names are collected here, on this one thread, so that the parallel phase
    // below only has to carry offsets into a buffer nobody else writes to.
    let mut names = String::new();
    let mut pending: Vec<Pending> = Vec::new();

    // macOS can answer names and metadata in one call. Not for a directory
    // holding a mount point, though: the whole directory arrives at once, so
    // there is no per-entry moment left at which a filesystem that stopped
    // answering could be given a deadline, and the protection below is worth
    // more than the speed.
    if !guarded {
        if let Some(entries) = bulk_list(dir) {
            for item in entries {
                let (name_off, name_len) = push_name(&mut names, &item.name.to_string_lossy());
                pending.push(Pending {
                    name: item.name,
                    name_off,
                    name_len,
                    meta: item.meta,
                    share: item.share,
                    extents: None,
                });
            }
            drop(listing);
            return place(dir, parent_id, depth, volume, ctx, &names, pending);
        }
    }

    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => {
            ctx.note_error(dir, &e);
            return;
        }
    };

    for entry in rd {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                ctx.note_error(dir, &e);
                continue;
            }
        };
        let path = entry.path();
        // `DirEntry::metadata` does not follow symlinks, which is what we
        // want: a symlink is counted as itself, never as its target. It is
        // also the call that never returns on a mount whose server has gone,
        // which is why a boundary is approached through `probe` instead.
        let md = if guarded && ctx.mounts.contains(&path) {
            match ctx.probe_mount(&path) {
                Some(Ok(md)) => md,
                Some(Err(e)) => {
                    ctx.note_error(&path, &e);
                    continue;
                }
                None => {
                    ctx.note_unreachable_mount(&path);
                    continue;
                }
            }
        } else {
            match entry.metadata() {
                Ok(md) => md,
                Err(e) => {
                    ctx.note_error(&path, &e);
                    continue;
                }
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
        // The clone family the bulk listing would have handed over for free.
        // This path is reached for a directory holding a mount point and for
        // filesystems with no bulk listing at all, so it is asked one entry at
        // a time — the only place left that still pays per file for it.
        //
        // Linux has no clone family; there the same question is which of the
        // file's extents are shared, asked on the filesystems that can answer.
        let (share, extents) = if ctx.opts.dedupe_clones && meta.kind == EntryKind::File {
            ctx.progress.clones_probed.fetch_add(1, Ordering::Relaxed);
            (clone_key(&path), ctx.map_extents(volume, &path, &meta))
        } else {
            (None, None)
        };
        let raw_name = entry.file_name();
        let (name_off, name_len) = push_name(&mut names, &raw_name.to_string_lossy());
        pending.push(Pending {
            name: raw_name,
            name_off,
            name_len,
            meta,
            share,
            extents,
        });
    }

    // Listing done; the recursion that follows is not what hangs.
    drop(listing);

    place(dir, parent_id, depth, volume, ctx, &names, pending)
}

/// Account for one directory's entries, write them into the arena as a block,
/// and fan out over the subdirectories among them.
///
/// Shared by both listing paths so that whichever produced the metadata, what
/// happens to it afterwards is the same code.
///
/// **The accounting runs on the thread that did the listing.** It used to run
/// one entry per rayon task, which bought nothing — none of it touches the
/// disk, and the one part that could contend (`claim_inode`) is behind a
/// process-wide mutex either way. Now only the subdirectories become tasks,
/// which is a tenth as many on a real disk.
fn place(
    dir: &Path,
    parent_id: NodeId,
    depth: usize,
    volume: Volume,
    ctx: &Ctx,
    names: &str,
    pending: Vec<Pending>,
) {
    let mut children: Vec<NewNode<'_>> = Vec::with_capacity(pending.len());
    let mut subdirs: Vec<(NodeId, PathBuf, u64)> = Vec::new();
    // Counted up here and published once per directory rather than once per
    // entry. The watcher only needs the numbers to be moving (`StallWatch`),
    // and they move thousands of times a second either way.
    let (mut files, mut dirs, mut bytes) = (0u64, 0u64, 0u64);
    // Resolved for the whole directory before anything is charged, so each
    // process-wide lock is taken once. See `Ctx::claim`.
    let claimed = ctx.claim(&pending);
    // Files whose shared extents wait for the walk to end, by index in this
    // block; they get their node ids once the block is in the arena. Until
    // then such a file is in the arena at what it holds alone, so a live view
    // of a running scan shows the shared part only once `Finishing` adds it —
    // low rather than counted twice.
    let mut deferred: Vec<(NodeId, u64, Box<Mapped>)> = Vec::new();
    // Clone-family members, charged or not, by index in this block as above.
    let mut members = Families::default();

    for (index, entry) in pending.into_iter().enumerate() {
        let Pending {
            name,
            name_off,
            name_len,
            meta,
            share,
            extents,
        } = entry;
        let is_dir = meta.kind == EntryKind::Dir;
        if is_dir {
            dirs += 1;
        } else {
            files += 1;
        }

        // A hardlinked file already counted elsewhere stays visible in the tree
        // but contributes no bytes, so a directory's total never double-counts
        // it. A copy-on-write clone whose family has been charged is treated
        // the same way, and for the same reason: the blocks exist once. A
        // directory's own `len()` is its inode size, not user data: real
        // disk usage, so it counts towards `alloc`, but adding it to the
        // logical size would make totals disagree with "sum of the files in
        // here" (invariant #1).
        //
        // A file only *part* of whose extents are shared elsewhere keeps its
        // logical size and is charged for the rest (`Charge::Alloc`): its
        // bytes are its own as far as `ls` is concerned, and a partly shared
        // file has no meaningful partial length.
        let charge = claimed
            .as_ref()
            .map_or(Charge::Full, |charges| charges[index]);
        let (size, alloc) = match (charge, is_dir) {
            (Charge::Nothing | Charge::Cloned, _) => (0, 0),
            (_, true) => (0, meta.alloc),
            (Charge::Full, false) => (meta.size, meta.alloc),
            (Charge::Alloc(alloc), false) => (meta.size, alloc),
        };
        if charge.counts() && !is_dir {
            bytes += alloc;
        }
        if let Some(mapped) = extents {
            if charge.counts() && mapped.claims_anything() {
                deferred.push((index as NodeId, meta.ino, mapped));
            }
        }
        // A repeat hardlink is not a member: the name of it that is charged
        // already is, and two would make one file two members.
        if let Some(clone_id) = share {
            if ctx.opts.dedupe_clones && matches!(charge, Charge::Full | Charge::Cloned) {
                members.push(&meta, clone_id, index as NodeId, charge == Charge::Full);
            }
        }

        // Kept separate from the caller's own limit, and checked first: this one
        // is not a preference, and a tree that trips it has to leave a trace
        // rather than quietly stopping short of its own contents.
        let too_deep = is_dir && depth >= MAX_WALK_DEPTH;
        if too_deep {
            ctx.note_error(
                &dir.join(&name),
                &std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("nested deeper than {MAX_WALK_DEPTH} levels; not descended"),
                ),
            );
        }
        let descend = is_dir
            && !too_deep
            && ctx.opts.max_depth.is_none_or(|max| depth < max)
            && (!ctx.opts.one_filesystem || meta.dev == ctx.root_dev)
            && !is_excluded(&name, &ctx.opts.exclude_names);
        // The only entries that get a path of their own: the walk needs one to
        // recurse with, and building one for every entry was an allocation per
        // entry for the nine in ten that are not directories.
        if descend {
            subdirs.push((index as NodeId, dir.join(&name), meta.dev));
        }

        let from = name_off as usize;
        children.push(NewNode {
            name: names
                .get(from..from + name_len as usize)
                .unwrap_or_default(),
            kind: meta.kind,
            size,
            alloc,
            mtime: meta.mtime,
            // Narrower than the platform's `nlink` on purpose: the arena
            // stores a `u32`, and a link count that overflowed one would be a
            // filesystem bug rather than something to carry eight bytes for.
            nlink: meta.nlink.min(u64::from(u32::MAX)) as u32,
        });
    }

    if files > 0 {
        ctx.progress.files.fetch_add(files, Ordering::Relaxed);
    }
    if dirs > 0 {
        ctx.progress.dirs.fetch_add(dirs, Ordering::Relaxed);
    }
    if bytes > 0 {
        ctx.progress.bytes.fetch_add(bytes, Ordering::Relaxed);
    }
    // The one critical section: this directory's names and its run of nodes.
    // Everything that could block has already happened.
    let start = ctx.builder().push_block(parent_id, children.into_iter());
    // A lock of its own, taken once per directory and only by one that has
    // something shared; never inside the arena's.
    if !deferred.is_empty() {
        let mut store = ctx
            .deferred
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for (index, ino, mapped) in &deferred {
            store.push(start + index, *ino, mapped);
        }
    }
    if !members.is_empty() {
        let mut families = ctx
            .families
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        families.append_block(members, start);
    }

    if subdirs.is_empty() {
        return;
    }
    subdirs.into_par_iter().for_each(|(index, path, dev)| {
        walk(&path, start + index, depth + 1, dev, Some(volume), ctx);
    });
}

/// The platform's one-call directory listing, where there is one.
#[cfg(target_os = "macos")]
fn bulk_list(dir: &Path) -> Option<Vec<NamedMeta>> {
    crate::bulk::list(dir)
}

/// Everywhere else the ordinary walk is the only walk.
#[cfg(not(target_os = "macos"))]
fn bulk_list(_dir: &Path) -> Option<Vec<NamedMeta>> {
    None
}

/// Whether this directory's name is on the skip list.
fn is_excluded(name: &std::ffi::OsStr, excluded: &[String]) -> bool {
    if excluded.is_empty() {
        return false;
    }
    let name = name.to_string_lossy();
    excluded.iter().any(|x| x.as_str() == name)
}

/// The clone family of one file, where the platform has one.
///
/// Only for the listing path that reads a directory an entry at a time; the
/// bulk listing answers it inside the record it was already fetching.
#[cfg(target_os = "macos")]
fn clone_key(path: &Path) -> Option<u64> {
    crate::bulk::clone_key(path)
}

/// Nowhere else has copy-on-write clones this scanner can see.
#[cfg(not(target_os = "macos"))]
fn clone_key(_path: &Path) -> Option<u64> {
    None
}

#[cfg(test)]
mod capacity_tests {
    use super::*;

    /// The hint is the previous snapshot's entry count, which a pushed
    /// snapshot can set to anything. 2^40 must neither overflow nor reserve
    /// beyond the clamp.
    #[test]
    fn an_absurd_hint_reserves_no_more_than_the_clamp() {
        let most = MAX_CAPACITY_HINT + MAX_CAPACITY_HINT / 8 + 1;
        assert_eq!(arena_capacity(Some(usize::MAX)), most);
        assert_eq!(arena_capacity(Some(1 << 40)), most);
        assert_eq!(arena_capacity(Some(800)), 901, "a real hint is unchanged");
        assert_eq!(arena_capacity(None), 4096);
    }

    /// And a scan handed that hint still runs: before the clamp it asked for
    /// 80 TB and aborted before reading a directory.
    #[test]
    fn a_scan_with_an_absurd_hint_still_completes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f"), b"12345").unwrap();
        let opts = ScanOptions {
            expected_entries: Some(1 << 40),
            ..ScanOptions::default()
        };
        let (tree, _) = scan(dir.path(), opts, Arc::new(ScanProgress::default())).unwrap();
        assert_eq!(tree.total_size(), 5);
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

    /// The literal 6 is deliberate. Asserting against `THREAD_CAP` would
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
            n <= 6,
            "the default grew past the measured cap: {n}. \
             `default_threads` carries the numbers this was chosen from"
        );
    }

    // ------------------------------------------------ unresponsive mounts

    /// A probe that never answers, standing in for a filesystem whose server
    /// has gone away. There is no way to build one of those in a test — it
    /// needs a second machine to kill — so this is the seam that lets the
    /// skip path be exercised at all.
    fn never_answers(_: &Path, _: Duration) -> Option<std::io::Result<std::fs::Metadata>> {
        None
    }

    /// The real prober, reached through the same seam, so the tests below can
    /// prove that the careful path still produces the ordinary answer.
    fn answers_normally(
        path: &Path,
        limit: Duration,
    ) -> Option<std::io::Result<std::fs::Metadata>> {
        super::probe_mount(path, limit)
    }

    fn corpus() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        // A "mount point" with something under it, and two ordinary siblings
        // either side of it in the listing.
        std::fs::create_dir(dir.path().join("aaa")).unwrap();
        std::fs::write(dir.path().join("aaa/file.bin"), vec![0u8; 1000]).unwrap();
        std::fs::create_dir(dir.path().join("mnt")).unwrap();
        std::fs::write(dir.path().join("mnt/hidden.bin"), vec![0u8; 5000]).unwrap();
        std::fs::create_dir(dir.path().join("zzz")).unwrap();
        std::fs::write(dir.path().join("zzz/file.bin"), vec![0u8; 2000]).unwrap();
        dir
    }

    fn names_in(tree: &Tree) -> Vec<String> {
        tree.iter()
            .map(|id| tree.name(id).to_string())
            .collect::<Vec<_>>()
    }

    /// The fix, stated as a test: one dead mount costs its own subtree and
    /// nothing else. Today it costs the rest of the directory, because the
    /// listing loop is single-threaded and never gets past the entry it is
    /// stuck on.
    #[test]
    fn a_mount_that_never_answers_is_skipped_and_its_siblings_are_still_scanned() {
        let dir = corpus();
        let root = dir.path().canonicalize().unwrap();
        let mounts = Mounts::from_paths([root.join("mnt")]);

        let (tree, stats) = scan_with(
            &root,
            ScanOptions::default(),
            Arc::new(ScanProgress::default()),
            mounts,
            never_answers,
        )
        .unwrap();

        let names = names_in(&tree);
        assert!(!names.iter().any(|n| n == "mnt"), "got {names:?}");
        assert!(!names.iter().any(|n| n == "hidden.bin"), "got {names:?}");
        assert!(
            names.iter().any(|n| n == "aaa") && names.iter().any(|n| n == "zzz"),
            "the siblings either side of the dead mount must survive: {names:?}"
        );

        // Counted and sampled, not swallowed (invariant #7).
        assert_eq!(stats.errors, 1);
        assert!(
            stats.error_samples[0].1.contains("did not respond"),
            "got {:?}",
            stats.error_samples[0]
        );
    }

    /// The bytes behind a skipped mount must not be invented. A total that
    /// silently included them would be the failure this whole mechanism
    /// exists to avoid.
    #[test]
    fn a_skipped_mount_contributes_nothing_to_the_total() {
        let dir = corpus();
        let root = dir.path().canonicalize().unwrap();

        let (skipped, _) = scan_with(
            &root,
            ScanOptions::default(),
            Arc::new(ScanProgress::default()),
            Mounts::from_paths([root.join("mnt")]),
            never_answers,
        )
        .unwrap();
        let (whole, _) = scan_with(
            &root,
            ScanOptions::default(),
            Arc::new(ScanProgress::default()),
            Mounts::none(),
            never_answers,
        )
        .unwrap();

        assert!(
            whole.total_size() > skipped.total_size(),
            "the skipped subtree held 5000 bytes"
        );
        assert_eq!(whole.total_size() - skipped.total_size(), 5000);
    }

    /// A healthy mount point is scanned exactly like an ordinary directory.
    /// Nothing about this protection may change the answer on a machine where
    /// every filesystem is answering, which is every machine most of the time.
    #[test]
    fn a_mount_that_answers_is_scanned_normally() {
        let dir = corpus();
        let root = dir.path().canonicalize().unwrap();

        let (guarded, stats) = scan_with(
            &root,
            ScanOptions::default(),
            Arc::new(ScanProgress::default()),
            Mounts::from_paths([root.join("mnt")]),
            answers_normally,
        )
        .unwrap();
        let (plain, _) = scan_with(
            &root,
            ScanOptions::default(),
            Arc::new(ScanProgress::default()),
            Mounts::none(),
            never_answers,
        )
        .unwrap();

        assert_eq!(guarded.total_size(), plain.total_size());
        assert_eq!(guarded.len(), plain.len());
        assert_eq!(stats.errors, 0);
    }

    /// With the protection off, a mount point is not treated specially at all
    /// — so a probe that never answers is never reached.
    #[test]
    fn switching_the_timeout_off_stops_probing_altogether() {
        let dir = corpus();
        let root = dir.path().canonicalize().unwrap();
        let opts = ScanOptions {
            mount_timeout: None,
            ..ScanOptions::default()
        };

        let (tree, stats) = scan_with(
            &root,
            opts,
            Arc::new(ScanProgress::default()),
            Mounts::from_paths([root.join("mnt")]),
            never_answers,
        )
        .unwrap();

        assert!(names_in(&tree).iter().any(|n| n == "hidden.bin"));
        assert_eq!(stats.errors, 0);
    }
}
