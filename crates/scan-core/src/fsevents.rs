//! FSEvents, the macOS change journal: where a rescan's cursor comes from.
//!
//! Hand-written against CoreServices rather than through a crate, for the
//! reason the rest of this crate is: it is a handful of calls, and the agent
//! has to stay one binary someone can audit. `examples/fsprobe.rs` is the
//! experiment these calls were first tried in.
//!
//! **The history is per volume and the ids are not.** FSEvents keeps each
//! volume's history in that volume's `/.fseventsd`, under a UUID that changes
//! whenever the history is discarded — a purge, an erase, a counter that
//! wrapped. The event ids come from one system-wide counter that only grows,
//! across reboots and across a disk moved from one Mac to another. So a
//! stored id means something only next to the UUID of the volume it was
//! taken on, and the two travel together in the cursor.

use std::collections::HashSet;
use std::ffi::{c_char, c_void};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::journal::{Change, ChangeKind, Journal, NoAnswer};
use crate::meta::RawMeta;
use crate::scan::ScanProgress;

/// `FSEventStreamEventId`.
type EventId = u64;

/// FSEvents, as the [`Journal`] a rescan asks.
pub(crate) struct FsEvents;

impl Journal for FsEvents {
    type Barrier = Marker;

    /// The digit is the cursor format's version.
    fn kind(&self) -> &'static str {
        "fsevents1"
    }

    fn volume(&self, root: &Path, root_meta: &RawMeta) -> Option<String> {
        journal_volume(root, root_meta.dev)
    }

    fn position(&self) -> u64 {
        current_event_id()
    }

    /// A [`Marker`] in the temporary directory, where that is on `root`'s
    /// volume.
    fn barrier(&self, root: &Path) -> Result<Marker, NoAnswer> {
        Marker::new(root)
    }

    fn replay(
        &self,
        root: &Path,
        since: u64,
        budget: Duration,
        barrier: Marker,
        progress: &ScanProgress,
    ) -> Result<Vec<Change>, NoAnswer> {
        replay(root, since, budget, MAX_CHANGES, barrier, progress)
    }
}

/// `FSEventStreamEventFlags`, the ones a rescan reads. Values from
/// `<CoreServices/FSEvents.h>`.
mod flag {
    pub const MUST_SCAN_SUB_DIRS: u32 = 0x0000_0001;
    pub const USER_DROPPED: u32 = 0x0000_0002;
    pub const KERNEL_DROPPED: u32 = 0x0000_0004;
    pub const EVENT_IDS_WRAPPED: u32 = 0x0000_0008;
    pub const HISTORY_DONE: u32 = 0x0000_0010;
    pub const ROOT_CHANGED: u32 = 0x0000_0020;
    pub const MOUNT: u32 = 0x0000_0040;
    pub const UNMOUNT: u32 = 0x0000_0080;
    pub const ITEM_CREATED: u32 = 0x0000_0100;
    pub const ITEM_REMOVED: u32 = 0x0000_0200;
    pub const ITEM_RENAMED: u32 = 0x0000_0800;
    pub const ITEM_IS_FILE: u32 = 0x0001_0000;
    pub const ITEM_IS_DIR: u32 = 0x0002_0000;
    pub const ITEM_IS_SYMLINK: u32 = 0x0004_0000;
}

/// `kFSEventStreamCreateFlagFileEvents`: one event per file rather than per
/// directory. Without it a file grown in place would still be reported, as
/// a change to its directory, but the event could not say what kind of
/// entry changed — and that is what decides whether a directory's contents
/// arrived without an event of their own.
const CREATE_FILE_EVENTS: u32 = 0x0000_0010;

#[repr(C)]
struct StreamContext {
    version: isize,
    info: *mut c_void,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
}

type Callback =
    extern "C" fn(*const c_void, *mut c_void, usize, *mut c_void, *const u32, *const EventId);

#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    fn FSEventsGetCurrentEventId() -> EventId;
    fn FSEventsCopyUUIDForDevice(dev: libc::dev_t) -> *const c_void;

    fn FSEventStreamCreate(
        allocator: *const c_void,
        callback: Callback,
        context: *const StreamContext,
        paths: *const c_void,
        since: EventId,
        latency: f64,
        flags: u32,
    ) -> *mut c_void;
    fn FSEventStreamSetDispatchQueue(stream: *mut c_void, queue: *mut c_void);
    fn FSEventStreamStart(stream: *mut c_void) -> u8;
    fn FSEventStreamStop(stream: *mut c_void);
    fn FSEventStreamInvalidate(stream: *mut c_void);
    fn FSEventStreamRelease(stream: *mut c_void);

    fn CFUUIDCreateString(allocator: *const c_void, uuid: *const c_void) -> *const c_void;
    fn CFStringGetCString(
        string: *const c_void,
        buffer: *mut c_char,
        size: isize,
        encoding: u32,
    ) -> u8;
    fn CFStringCreateWithBytes(
        allocator: *const c_void,
        bytes: *const u8,
        len: isize,
        encoding: u32,
        external: u8,
    ) -> *const c_void;
    fn CFArrayCreate(
        allocator: *const c_void,
        values: *const *const c_void,
        count: isize,
        callbacks: *const c_void,
    ) -> *const c_void;
    fn CFRelease(cf: *const c_void);
    static kCFTypeArrayCallBacks: c_void;
}

// libdispatch is part of libSystem, which every binary links.
extern "C" {
    fn dispatch_queue_create(label: *const c_char, attr: *const c_void) -> *mut c_void;
    fn dispatch_release(object: *mut c_void);
    fn dispatch_sync_f(queue: *mut c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
}

/// `kCFStringEncodingUTF8`.
const UTF8: u32 = 0x0800_0100;

/// The newest event id issued anywhere on the system.
///
/// Taken **before** a walk starts. Anything that changes after it gets a
/// larger id, so the next scan's replay from here sees it whether or not
/// this walk did; anything before it is in the tree this walk produces.
fn current_event_id() -> EventId {
    // SAFETY: takes nothing, returns a number.
    unsafe { FSEventsGetCurrentEventId() }
}

/// The identity of the FSEvents history of the volume `root` sits on, or
/// `None` where that history cannot be trusted to hold every change.
///
/// **APFS only.** A volume another operating system can write — FAT, exFAT,
/// NTFS, HFS+ under Linux — keeps its `/.fseventsd` and its UUID when it is
/// written elsewhere, and what was written there is in neither: the history
/// would be intact and silently incomplete. APFS is written by macOS alone,
/// and macOS records what it writes. Network filesystems record nothing a
/// remote writer does, and are not APFS either.
///
/// `None` also where FSEvents itself has no history for the device — a
/// read-only volume, one with logging switched off.
fn journal_volume(root: &Path, dev: u64) -> Option<String> {
    if !on_apfs(root) {
        return None;
    }
    // `as`, not `try_from`: `st_dev` is a signed 32-bit value that std widens
    // to `u64` by sign extension, and truncating gives back exactly the bits
    // the kernel reported.
    let dev = dev as libc::dev_t;
    // SAFETY: a plain lookup; a null answer means "no history".
    let uuid = unsafe { FSEventsCopyUUIDForDevice(dev) };
    if uuid.is_null() {
        return None;
    }
    // SAFETY: `uuid` is a live CFUUID we own (Copy rule) and is released
    // below whatever happens; the string likewise.
    unsafe {
        let text = CFUUIDCreateString(std::ptr::null(), uuid);
        CFRelease(uuid);
        if text.is_null() {
            return None;
        }
        // A UUID string is 36 characters.
        let mut buffer = [0 as c_char; 64];
        let ok = CFStringGetCString(text, buffer.as_mut_ptr(), buffer.len() as isize, UTF8);
        CFRelease(text);
        if ok == 0 {
            return None;
        }
        let uuid = std::ffi::CStr::from_ptr(buffer.as_ptr()).to_str().ok()?;
        (!uuid.is_empty()).then(|| uuid.to_string())
    }
}

/// Flags that mean FSEvents itself lost track for this stretch.
const LOST: u32 = flag::USER_DROPPED
    | flag::KERNEL_DROPPED
    | flag::EVENT_IDS_WRAPPED
    | flag::ROOT_CHANGED
    | flag::MOUNT
    | flag::UNMOUNT;

/// What kind of entry an event is about. With none of these it is a
/// directory-level change whose detail FSEvents did not keep.
const KINDS: u32 = flag::ITEM_IS_FILE | flag::ITEM_IS_DIR | flag::ITEM_IS_SYMLINK;

/// One FSEvents record as a journal-neutral [`Change`].
///
/// Coalescing can set several of `Created`, `Removed` and `Renamed` on one
/// record; the strongest claim wins (see [`ChangeKind`]). `MustScanSubDirs`,
/// or a record that does not say what kind of entry changed, is FSEvents
/// admitting it kept less than a rescan needs.
fn change(path: Vec<u8>, flags: u32) -> Change {
    let kind = if flags & LOST != 0 {
        ChangeKind::Lost
    } else if flags & flag::MUST_SCAN_SUB_DIRS != 0 || flags & KINDS == 0 {
        ChangeKind::SubtreeUnknown
    } else if flags & flag::ITEM_REMOVED != 0 {
        ChangeKind::Removed
    } else if flags & flag::ITEM_RENAMED != 0 {
        ChangeKind::Renamed
    } else if flags & flag::ITEM_CREATED != 0 {
        ChangeKind::Created
    } else {
        ChangeKind::Modified
    };
    Change {
        path,
        kind,
        is_dir: flags & flag::ITEM_IS_DIR != 0,
    }
}

/// Everything that changed under `root` after event `since`, or why there
/// is no answer.
///
/// **The cost grows with how far back `since` is, not with how much
/// changed** (measured, TODO.md B7): FSEvents reads its whole log from that
/// point and filters by path itself. An id old enough to send it through
/// all of `/.fseventsd` reports the condition by not finishing rather than
/// by a flag, which is what `budget` is for.
///
/// **The answer ends at `marker`, not at `HistoryDone`.** `HistoryDone`
/// marks the end of what fseventsd had numbered when the stream started, and
/// a change finished just before that can still be on its way to it. The
/// caller wrote `marker` before asking; the replay waits for both
/// `HistoryDone` and the marker, and whatever finished before the marker
/// was written is then in the answer (the marker's doc says why). What
/// arrives after it is extra, never missing: the next scan replays it again
/// from its own cursor. No flush is needed for the same reason — the marker
/// is what proves the records in flight have arrived.
///
/// **What the budget covers.** The history has `budget` from the moment the
/// stream is started; the marker has the same, but never less than
/// [`MARKER_FLOOR`], because its trip is fseventsd's own latency and says
/// nothing about how long the history is. Starting the stream is in neither
/// (see [`start`]) and has an allowance of its own: the same length, but
/// never under [`START_FLOOR`].
///
/// `progress.journal_ms` counts the wait — for the turn below, the start and
/// FSEvents — so a watcher sees a counter move while FSEvents works through a
/// log that holds nothing under this root (invariant 8). Cancellation is
/// checked as often, in the queue and during the start as well (invariant 5).
///
/// **One replay at a time per process, and the budget starts with this
/// one's turn.** Concurrent history reads queue behind each other: 22 run at
/// once — the incremental test suite — had replays that take tens of
/// milliseconds alone miss a one-second budget, and none missed it run one
/// after another. A budget that measured the queue would turn a busy agent's
/// rescans into full scans for no reason about the journal. Waiting for the
/// turn is bounded by the other replay's own budget.
///
/// At most `cap` distinct changes are held; past that there is no answer.
fn replay(
    root: &Path,
    since: EventId,
    budget: Duration,
    cap: usize,
    marker: Marker,
    progress: &ScanProgress,
) -> Result<Vec<Change>, NoAnswer> {
    use std::os::unix::ffi::OsStrExt;

    let asked = Instant::now();
    let _turn = take_turn(asked, progress)?;
    if HANGING.load(Ordering::SeqCst) {
        return Err(NoAnswer::Stuck);
    }
    // Read on this thread, which a test sets, and handed to the one that
    // starts the stream.
    #[cfg(test)]
    let slow_start = SLOW_START.get();
    #[cfg(not(test))]
    let slow_start = Duration::ZERO;

    let sink = Arc::new(Sink::new(cap, root, &marker));
    #[cfg(test)]
    LAST_REFS.set(Some(Arc::clone(&sink.refs)));
    let bytes = root.as_os_str().as_bytes();
    let marker_dir = marker.dir.as_os_str().as_bytes();

    // SAFETY: every CF object created here is released on every path out.
    // The callback reaches `sink` through the context's `info`, and the
    // stream owns a reference of its own to it (`retain_sink`), which
    // FSEvents gives back only when it will not call the callback again — so
    // a callback still queued when this gives up finds the sink alive
    // whatever this function has dropped by then. The barrier on the queue
    // when a `Stream` is dropped is the second guard: nothing of this stream
    // is running there afterwards.
    let stream = unsafe {
        let cf_path = CFStringCreateWithBytes(
            std::ptr::null(),
            bytes.as_ptr(),
            bytes.len() as isize,
            UTF8,
            0,
        );
        if cf_path.is_null() {
            return Err(NoAnswer::Failed);
        }
        let cf_marker = CFStringCreateWithBytes(
            std::ptr::null(),
            marker_dir.as_ptr(),
            marker_dir.len() as isize,
            UTF8,
            0,
        );
        if cf_marker.is_null() {
            CFRelease(cf_path);
            return Err(NoAnswer::Failed);
        }
        let values = [cf_path, cf_marker];
        let paths = CFArrayCreate(
            std::ptr::null(),
            values.as_ptr(),
            values.len() as isize,
            &kCFTypeArrayCallBacks as *const c_void,
        );
        CFRelease(cf_path);
        CFRelease(cf_marker);
        if paths.is_null() {
            return Err(NoAnswer::Failed);
        }
        let context = StreamContext {
            version: 0,
            info: Arc::as_ptr(&sink) as *mut c_void,
            retain: retain_sink as *const c_void,
            release: release_sink as *const c_void,
            copy_description: std::ptr::null(),
        };
        let stream = FSEventStreamCreate(
            std::ptr::null(),
            on_events,
            &context,
            paths,
            since,
            0.0,
            CREATE_FILE_EVENTS,
        );
        CFRelease(paths);
        if stream.is_null() {
            return Err(NoAnswer::Failed);
        }
        let queue = dispatch_queue_create(c"spacetrace.rescan".as_ptr(), std::ptr::null());
        if queue.is_null() {
            FSEventStreamRelease(stream);
            return Err(NoAnswer::Failed);
        }
        FSEventStreamSetDispatchQueue(stream, queue);
        Stream {
            stream,
            queue,
            started: false,
        }
    };

    let outcome =
        start(stream, asked, budget, slow_start, progress).and_then(|(stream, started)| {
            let answer = sink.wait(asked, started, budget, progress);
            drop(stream);
            answer
        });
    progress
        .journal_ms
        .store(asked.elapsed().as_millis() as u64, Ordering::Relaxed);
    outcome?;
    Ok(sink.take())
}

/// A created stream and the queue it delivers on, owned by one holder at a
/// time — the replay, or the thread starting it — and closed when that
/// holder drops it: once, by construction, whichever way it leaves.
struct Stream {
    stream: *mut c_void,
    queue: *mut c_void,
    /// Whether `FSEventStreamStart` succeeded, so that dropping stops it.
    started: bool,
}

// SAFETY: FSEvents and libdispatch objects may be used from any thread; the
// one rule, no two calls on a stream at once, is kept by moving the one
// `Stream` between the replay and the thread that starts it.
unsafe impl Send for Stream {}

impl Drop for Stream {
    /// Stop (if started), invalidate and release the stream, then wait out
    /// any callback still queued and release the queue.
    fn drop(&mut self) {
        // SAFETY: this value is the stream's and the queue's only owner, and
        // is dropped once.
        unsafe {
            if self.started {
                FSEventStreamStop(self.stream);
            }
            FSEventStreamInvalidate(self.stream);
            FSEventStreamRelease(self.stream);
            // A serial queue runs its blocks in order, so an empty one
            // submitted now returns only once every callback already queued
            // has finished.
            dispatch_sync_f(self.queue, std::ptr::null_mut(), barrier);
            dispatch_release(self.queue);
        }
    }
}

/// Where a stream start handed to its own thread has got to.
enum Starting {
    Running,
    /// Started; the stream comes back to the replay.
    Started(Stream),
    /// Did not start; the thread has closed it.
    Failed,
    /// The replay stopped waiting; the thread closes the stream when the
    /// start returns.
    Abandoned,
}

/// Whether an abandoned start has still not returned. While it has not,
/// every replay is refused (`NoAnswer::Stuck`): fseventsd is not answering,
/// and each new stream would only add a thread, a stream, a queue and a sink
/// waiting beside the first. Cleared by that thread once the start returns
/// and its stream is closed. One at a time, because none starts while it is
/// set.
static HANGING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Start `stream` on a thread of its own and wait for it as for anything
/// else FSEvents does: cancellable, `journal_ms` moving, and for at most
/// `budget` or [`START_FLOOR`], whichever is longer. The instant it started,
/// or why not.
///
/// **Why its own allowance and not the answer's budget.**
/// `FSEventStreamStart` took 0.45 to 1.9 s per call for a binary sitting in
/// a directory of 320,000 entries (a busy `target/debug/deps`), against
/// 0.3 ms anywhere else — measured, same binary, same minute. Counted in the
/// budget, it turned every rescan from there into a deadline fallback; the
/// budget's own floor is 500 ms.
///
/// **Why a thread.** The call blocks, and nothing can interrupt it. A start
/// that never returns would otherwise hold a scan nobody can stop, with
/// every counter still. Past its allowance or on cancel the replay walks
/// away, and the thread closes the stream whenever the call does return.
/// The turn is given up with it, and until the start returns every replay
/// is refused at once ([`HANGING`]).
///
/// The stream moves to the thread and back: the started stream and the
/// instant it started, or why not — in which case whoever held it last has
/// closed it.
fn start(
    stream: Stream,
    asked: Instant,
    budget: Duration,
    slow_start: Duration,
    progress: &ScanProgress,
) -> Result<(Stream, Instant), NoAnswer> {
    let shared = Arc::new((Mutex::new(Starting::Running), Condvar::new()));
    let theirs = Arc::clone(&shared);
    let spawned = std::thread::Builder::new()
        .name("spacetrace-fsevents-start".into())
        .spawn(move || {
            let mut stream = stream;
            std::thread::sleep(slow_start);
            // SAFETY: this thread owns the stream until it hands it back.
            stream.started = unsafe { FSEventStreamStart(stream.stream) } != 0;
            let (state, done) = &*theirs;
            let mut state = state.lock().unwrap_or_else(|p| p.into_inner());
            if matches!(*state, Starting::Abandoned) {
                drop(state);
                drop(stream);
                HANGING.store(false, Ordering::SeqCst);
                return;
            }
            *state = if stream.started {
                Starting::Started(stream)
            } else {
                Starting::Failed
            };
            done.notify_all();
        });
    // A thread that could not be made dropped the stream with its closure.
    if spawned.is_err() {
        return Err(NoAnswer::Failed);
    }

    let handed = Instant::now();
    let (state, done) = &*shared;
    let mut guard = state.lock().unwrap_or_else(|p| p.into_inner());
    loop {
        match std::mem::replace(&mut *guard, Starting::Running) {
            Starting::Started(stream) => return Ok((stream, Instant::now())),
            Starting::Failed => return Err(NoAnswer::Failed),
            Starting::Running | Starting::Abandoned => {}
        }
        progress
            .journal_ms
            .store(asked.elapsed().as_millis() as u64, Ordering::Relaxed);
        let why = if progress.is_cancelled() {
            Some(NoAnswer::Cancelled)
        } else if handed.elapsed() >= budget.max(START_FLOOR) {
            Some(NoAnswer::Deadline)
        } else {
            None
        };
        if let Some(why) = why {
            // Under the lock, so the thread cannot clear it first.
            HANGING.store(true, Ordering::SeqCst);
            *guard = Starting::Abandoned;
            return Err(why);
        }
        guard = done
            .wait_timeout(guard, TICK)
            .unwrap_or_else(|p| p.into_inner())
            .0;
    }
}

/// The least time the marker is waited for: its live trip through fseventsd
/// took 5 to 492 ms (median 287 ms) over 30 replays at a load average of
/// 4.5, measured, against a budget whose floor is 500 ms. Four times the
/// slowest, so that only an fseventsd that is stuck runs out of it.
const MARKER_FLOOR: Duration = Duration::from_secs(2);

/// The least time a stream start is waited for: well past the slowest start
/// measured (1.9 s), so that only a start that is stuck runs out of it.
const START_FLOOR: Duration = Duration::from_secs(5);

/// One replay at a time per process; see [`replay`].
static TURN: Mutex<()> = Mutex::new(());

#[cfg(test)]
thread_local! {
    /// Time added before `FSEventStreamStart` for the replays this thread
    /// asks for: how a test plays the slow start measured for a binary in a
    /// crowded directory, or one that hangs. Read by the asking thread and
    /// handed to the one that starts the stream.
    static SLOW_START: std::cell::Cell<Duration> = const { std::cell::Cell::new(Duration::ZERO) };

    /// The reference counts of the last sink a replay on this thread made.
    static LAST_REFS: std::cell::RefCell<Option<Arc<Refs>>> = const { std::cell::RefCell::new(None) };

    /// Where this thread's [`Marker::new`] makes its directory, in place of
    /// the temporary directory: how a test reaches a rescan whose marker
    /// cannot be written.
    pub(crate) static MARKER_BASE: std::cell::RefCell<Option<std::path::PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Wait for [`TURN`] without becoming a scan that cannot be stopped or that
/// looks stuck: polled, with the cancel switch read and `journal_ms` moved
/// on every tick. A blocking `lock` would hold a cancelled scan for as long
/// as another root's replay may take — its whole budget, which is that
/// root's full-scan time — with every counter still.
fn take_turn(
    asked: Instant,
    progress: &ScanProgress,
) -> Result<std::sync::MutexGuard<'static, ()>, NoAnswer> {
    loop {
        match TURN.try_lock() {
            Ok(turn) => return Ok(turn),
            // The lock guards nothing but the order of replays.
            Err(std::sync::TryLockError::Poisoned(poisoned)) => return Ok(poisoned.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
        if progress.is_cancelled() {
            return Err(NoAnswer::Cancelled);
        }
        progress
            .journal_ms
            .store(asked.elapsed().as_millis() as u64, Ordering::Relaxed);
        std::thread::sleep(TICK);
    }
}

/// How often a waiting replay looks at the clock and the cancel switch:
/// often enough to stop promptly and keep the counter moving, rarely enough
/// not to matter next to the replay itself.
const TICK: Duration = Duration::from_millis(20);

/// The references one stream took to its sink and gave back. Kept beside
/// the sink rather than in it, so a test can read them after the sink is
/// gone, and per sink, so other tests' streams do not count.
#[cfg(test)]
#[derive(Default)]
struct Refs {
    taken: std::sync::atomic::AtomicUsize,
    given: std::sync::atomic::AtomicUsize,
}

/// The context's `retain`: the stream takes a reference of its own to the
/// sink, so the sink outlives every callback FSEvents may still make.
extern "C" fn retain_sink(info: *const c_void) -> *const c_void {
    // SAFETY: `info` is `Arc::as_ptr` of a live `Arc<Sink>` (the replay's own
    // reference keeps it alive while the stream is created).
    unsafe {
        #[cfg(test)]
        {
            let sink = &*(info as *const Sink);
            sink.refs.taken.fetch_add(1, Ordering::SeqCst);
        }
        Arc::increment_strong_count(info as *const Sink);
    }
    info
}

/// The context's `release`: FSEvents gives back what `retain_sink` took.
extern "C" fn release_sink(info: *const c_void) {
    // SAFETY: balances exactly one `retain_sink` on the same pointer, which
    // is alive until this gives its reference back.
    unsafe {
        #[cfg(test)]
        {
            let sink = &*(info as *const Sink);
            sink.refs.given.fetch_add(1, Ordering::SeqCst);
        }
        Arc::decrement_strong_count(info as *const Sink);
    }
}

/// Nothing: what is waited for is that the queue reaches it.
extern "C" fn barrier(_: *mut c_void) {}

/// The most distinct changes a replay holds before it gives up.
///
/// Every change is a path held until the plan is made, and another node in
/// [`crate::rescan`]'s tree of changed paths: about 450 bytes between them,
/// measured end to end (200,000 new files cost 134 MiB of peak, against
/// 11 MiB for 1,000). Nothing else bounds it — a busy build directory can
/// write millions of files inside one budget — and the agent stays resident.
/// Past this many, reading the directories they are in is close to the cost
/// of reading everything anyway, so the scan reads everything instead, and
/// the list is dropped the moment it overflows rather than at the deadline.
const MAX_CHANGES: usize = 100_000;

/// Where the callback leaves what it is handed.
struct Sink {
    state: Mutex<SinkState>,
    done: Condvar,
    cap: usize,
    /// This replay's [`Marker`] file: seeing it is the barrier.
    marker: Vec<u8>,
    /// The marker's directory. Nothing in it is a change under the root.
    marker_dir: Vec<u8>,
    /// Whether the marker directory is outside the root, when its own
    /// records are not changes under the root either. Under it, they are:
    /// making and removing it changes its parent, which a full scan sees.
    marker_dir_outside: bool,
    #[cfg(test)]
    refs: Arc<Refs>,
}

#[derive(Default)]
struct SinkState {
    /// A set: FSEvents can deliver one record more than once for a path,
    /// and a rescan acts on each distinct one once.
    events: HashSet<Change>,
    history_done: bool,
    /// The marker has come back.
    synced: bool,
    /// FSEvents said it lost track of the marker's directory: whether the
    /// marker came back can no longer be told.
    barrier_lost: bool,
    overflowed: bool,
}

impl SinkState {
    /// Whether the answer is complete: the history is over and the marker
    /// is in, in either order — the marker can be numbered inside the
    /// history or after it (see [`Marker`]).
    fn answered(&self) -> bool {
        self.history_done && self.synced
    }
}

impl Sink {
    fn new(cap: usize, root: &Path, marker: &Marker) -> Sink {
        use std::os::unix::ffi::OsStrExt;
        Sink {
            state: Mutex::new(SinkState::default()),
            done: Condvar::new(),
            cap,
            marker: marker.path.as_os_str().as_bytes().to_vec(),
            marker_dir: marker.dir.as_os_str().as_bytes().to_vec(),
            // Both sides canonical: the marker's directory is, and a root
            // spelled through a symlink would otherwise never hold it.
            marker_dir_outside: !marker
                .dir
                .starts_with(root.canonicalize().as_deref().unwrap_or(root)),
            #[cfg(test)]
            refs: Arc::default(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SinkState> {
        // The callback must not panic across the FFI boundary, and a waiter
        // must not lose the events over a poisoned lock.
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// One record, as the callback hands it over.
    fn record(&self, state: &mut SinkState, path: &[u8], flags: u32) {
        if flags & flag::HISTORY_DONE != 0 {
            state.history_done = true;
            return;
        }
        let in_marker_dir = under(path, &self.marker_dir);
        if path == self.marker.as_slice() {
            state.synced = true;
        }
        // A record that says FSEvents lost track of what is under the marker
        // — dropped records, a directory to scan again — leaves "has it come
        // back?" without an answer, and the replay without its barrier. A
        // record that merely does not say what changed (a coalesced one, the
        // directory's metadata or `com.apple.provenance`) loses nothing:
        // the marker's own record still comes.
        let covers_marker = in_marker_dir || under(&self.marker_dir, path);
        if covers_marker && flags & (LOST | flag::MUST_SCAN_SUB_DIRS) != 0 {
            state.barrier_lost = true;
        }
        if in_marker_dir && (path != self.marker_dir.as_slice() || self.marker_dir_outside) {
            return;
        }
        if state.overflowed {
            return;
        }
        state.events.insert(change(path.to_vec(), flags));
        if state.events.len() > self.cap {
            state.overflowed = true;
            state.events = HashSet::new();
        }
    }

    /// Block until the answer is complete, its time — counted from
    /// `started`, when the stream started; see [`replay`] — is spent, or the
    /// scan is cancelled.
    /// `journal_ms` counts from `asked`, the queue included, so it never runs
    /// backwards.
    fn wait(
        &self,
        asked: Instant,
        started: Instant,
        budget: Duration,
        progress: &ScanProgress,
    ) -> Result<(), NoAnswer> {
        let mut guard = self.lock();
        loop {
            if guard.overflowed {
                return Err(NoAnswer::TooMany);
            }
            // The marker seen wins over a loss reported around it: it came
            // back, in order, and that is all the barrier asks.
            if guard.answered() {
                return Ok(());
            }
            if guard.barrier_lost && !guard.synced {
                return Err(NoAnswer::Lost);
            }
            let waited = started.elapsed();
            progress
                .journal_ms
                .store(asked.elapsed().as_millis() as u64, Ordering::Relaxed);
            if progress.is_cancelled() {
                return Err(NoAnswer::Cancelled);
            }
            let limit = if guard.history_done {
                budget.max(MARKER_FLOOR)
            } else {
                budget
            };
            let Some(left) = limit.checked_sub(waited).filter(|d| !d.is_zero()) else {
                return Err(NoAnswer::Deadline);
            };
            guard = self
                .done
                .wait_timeout(guard, left.min(TICK))
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
    }

    fn take(&self) -> Vec<Change> {
        std::mem::take(&mut self.lock().events)
            .into_iter()
            .collect()
    }
}

/// The stream's callback: copy the paths out before FSEvents frees them.
extern "C" fn on_events(
    _stream: *const c_void,
    info: *mut c_void,
    count: usize,
    paths: *mut c_void,
    flags: *const u32,
    _ids: *const EventId,
) {
    // SAFETY: `info` is the `Sink` the stream holds a reference to; without
    // `UseCFTypes` the paths are an array of `count` NUL-terminated C
    // strings, beside `count` flag words.
    let sink = unsafe { &*(info as *const Sink) };
    let paths = paths as *const *const c_char;
    let mut state = sink.lock();
    for i in 0..count {
        let flags = unsafe { *flags.add(i) };
        let path = unsafe { std::ffi::CStr::from_ptr(*paths.add(i)) }.to_bytes();
        sink.record(&mut state, path, flags);
    }
    sink.done.notify_all();
}

/// Whether `path` is `dir` or below it.
fn under(path: &[u8], dir: &[u8]) -> bool {
    path.strip_prefix(dir)
        .is_some_and(|rest| rest.is_empty() || rest[0] == b'/' || dir.ends_with(b"/"))
}

/// A file this process writes before it asks for a replay, and waits to see
/// come back: the barrier that makes a replay cover everything up to the
/// moment it was written.
///
/// **Why it is needed.** fseventsd numbers a record when it takes it from
/// the kernel, and `HistoryDone` means the end of what it has numbered — not
/// of what has happened. A change finished just before the replay can still
/// be on its way, and the replay ends without it: the rescan then copies
/// that directory from the base, and the snapshot is wrong. Measured with a
/// file written and replayed at once, twenty times: one build of the tests
/// saw every change, because its replays took about 130 ms; the same binary
/// copied out of its target directory replayed in about 25 ms and missed 17
/// of the 20. How long fseventsd takes to answer depends on the client, then,
/// and nothing about it can be relied on.
///
/// **Why writing it before the replay is enough.** The kernel hands
/// fseventsd its records through one queue, in the order the changes
/// happen, and fseventsd numbers them in that order. A change finished
/// before the marker was written is therefore numbered before it. The
/// stream reports every record numbered after the cursor: those numbered
/// before it started as history, ending with `HistoryDone`, and the rest
/// live, in order. Wherever the marker falls, then — in the history or after
/// it — every earlier change has been delivered once both the marker and
/// `HistoryDone` have.
///
/// **Why it is written before the base is loaded.** Its live trip through
/// fseventsd takes about 300 ms ([`MARKER_FLOOR`] has the numbers), which
/// on `/Applications` was most of what an incremental rescan cost. Written
/// 120 ms or more before the stream starts, it is already numbered, comes
/// back inside the history, and the replay took 4 to 27 ms (measured). The
/// rescan has that time anyway: loading its base takes 240 ms there.
///
/// **Where.** In a private directory of its own — mode 0700, a random name,
/// made fresh for each replay under the temporary directory, never in the
/// root: a scan writes nothing where it reads. The file is created with
/// `O_EXCL | O_NOFOLLOW`. A fresh directory also means its history since
/// any cursor holds nothing but this replay's own records. Both are removed
/// when the replay ends; a process killed mid-replay leaves one empty
/// directory in the per-user temporary directory, which macOS clears.
///
/// **Only on the root's volume.** The order the argument above rests on was
/// measured not to hold across volumes: with the marker on the Data volume
/// and the root on an APFS disk image, 1 to 3 of 20 changes written on the
/// image just before the replay were missing from it, in 3 of 5 runs; on one
/// volume, none in 200. A root anywhere else would need the marker written
/// on its own volume, which means into somebody's disk, so a rescan of it
/// falls back instead (`Fallback::MarkerVolume`) — every external disk,
/// until a marker that needs no write is found.
///
/// `/` is not one of them: on macOS 27 the sealed System volume reports the
/// Data volume's device (`stat -f %d / /usr/bin /Users` all 16777234,
/// measured), and nothing writes to the System volume while it is mounted.
/// The volumes mounted under `/` — `VM`, `Preboot` and the rest — are other
/// devices, and those subtrees are read in full every time anyway
/// (`rescan::DirPlan::next`), so the marker's order does not matter there.
/// Where a macOS reports the System volume as a device of its own, `/`
/// falls back like any other volume.
pub(crate) struct Marker {
    /// Removes the directory, the marker with it, when dropped.
    _owner: tempfile::TempDir,
    /// The directory, canonical, because FSEvents reports real paths.
    dir: std::path::PathBuf,
    path: std::path::PathBuf,
}

impl Marker {
    /// [`Marker::write_in`] the temporary directory, for a replay of
    /// `root` — which must be on the same volume.
    fn new(root: &Path) -> Result<Marker, NoAnswer> {
        use std::os::unix::fs::MetadataExt;
        #[cfg(test)]
        let base = MARKER_BASE
            .with_borrow(Clone::clone)
            .unwrap_or_else(std::env::temp_dir);
        #[cfg(not(test))]
        let base = std::env::temp_dir();
        let dev = |path: &Path| std::fs::metadata(path).map(|m| m.dev());
        let root_dev = dev(root).map_err(|_| NoAnswer::Failed)?;
        let base_dev = dev(&base).map_err(|_| NoAnswer::NoBarrier)?;
        if root_dev != base_dev {
            return Err(NoAnswer::MarkerVolume);
        }
        Marker::write_in(&base).ok_or(NoAnswer::NoBarrier)
    }

    /// A fresh private directory in `base`, with the marker written in it.
    fn write_in(base: &Path) -> Option<Marker> {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let owner = tempfile::Builder::new()
            .prefix("spacetrace-fsevents-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(base)
            .ok()?;
        let dir = owner.path().canonicalize().ok()?;
        let path = dir.join("sync");
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(libc::O_NOFOLLOW)
            .mode(0o600)
            .open(&path)
            .ok()?;
        Some(Marker {
            _owner: owner,
            dir,
            path,
        })
    }
}

/// Whether `path` is on an APFS volume.
fn on_apfs(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    let mut info = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: a valid C string and room for one `statfs`.
    if unsafe { libc::statfs(c_path.as_ptr(), info.as_mut_ptr()) } != 0 {
        return false;
    }
    // SAFETY: `statfs` returned 0, so it filled the struct.
    let info = unsafe { info.assume_init() };
    // SAFETY: the kernel NUL-terminates the name within its array.
    let name = unsafe { std::ffi::CStr::from_ptr(info.f_fstypename.as_ptr()) };
    name.to_bytes() == b"apfs"
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A replay as a rescan asks for one, its marker written at once — with
    /// no head start, the case where the marker's trip matters most.
    fn ask(
        root: &Path,
        since: EventId,
        budget: Duration,
        progress: &ScanProgress,
    ) -> Result<Vec<Change>, NoAnswer> {
        replay(
            root,
            since,
            budget,
            MAX_CHANGES,
            Marker::new(root)?,
            progress,
        )
    }

    /// The ids only grow: a cursor taken later is never behind one taken
    /// earlier, which is what makes "replay from here" mean "since then".
    #[test]
    fn event_ids_only_grow() {
        let before = current_event_id();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f"), b"x").unwrap();
        assert!(current_event_id() >= before);
        assert!(before > 0, "a running Mac has issued events");
    }

    /// A temporary directory is on the Data volume, which is APFS with a
    /// history; its identity is a UUID and is the same each time it is asked.
    #[test]
    fn the_data_volume_has_a_journal_identity() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let dev = std::fs::metadata(dir.path()).unwrap().dev();
        let first = journal_volume(dir.path(), dev).expect("the Data volume keeps a history");
        assert_eq!(first.len(), 36, "{first}");
        assert_eq!(journal_volume(dir.path(), dev), Some(first));
    }

    /// The replay sees a file grown in place — the case that rules out
    /// comparing directory mtimes, since writing into an existing file
    /// changes no directory — and stops at the end of the history rather
    /// than waiting out its budget.
    #[test]
    fn a_replay_sees_a_file_grown_in_place_and_ends_with_the_history() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(root.join("grown"), vec![1u8; 10]).unwrap();
        settle(&root);
        let since = current_event_id();
        std::fs::write(root.join("grown"), vec![1u8; 100_000]).unwrap();
        settle(&root);

        let progress = ScanProgress::default();
        let started = Instant::now();
        let events = ask(&root, since, Duration::from_secs(30), &progress).unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the history ended; the budget is not a wait"
        );
        let grown = root.join("grown");
        assert!(
            events
                .iter()
                .any(|e| e.path == grown.as_os_str().as_bytes() && !e.is_dir),
            "{events:?}"
        );
    }

    /// Starting the stream can be slow for reasons that have nothing to do
    /// with the journal — measured, 0.45 to 1.9 s per start for a binary in a
    /// directory of 320,000 entries, against 0.3 ms — and the budget is for
    /// the journal's answer, not for that. Played here with a start made
    /// 800 ms slow under a 500 ms budget.
    #[test]
    fn a_slow_stream_start_does_not_spend_the_budget() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let progress = ScanProgress::default();
        SLOW_START.set(Duration::from_millis(800));
        let got = ask(
            &root,
            current_event_id(),
            Duration::from_millis(500),
            &progress,
        );
        SLOW_START.set(Duration::ZERO);
        assert!(got.is_ok(), "{got:?}");
    }

    /// A change made the moment before the replay is in it. fseventsd numbers
    /// a record when it takes it from the kernel, which can be after a
    /// replay asked from before it has already been told the history is
    /// over — so `HistoryDone` alone is not "everything up to now". Twenty
    /// rounds, each writing a file and replaying at once, with no settling.
    ///
    /// **A probabilistic guard.** Whether the race shows depends on how fast
    /// fseventsd answers this client, which a test cannot set: without the
    /// marker, 17 of 20 rounds missed from one build and none from another.
    /// What the barrier does is held exactly by
    /// `the_answer_waits_for_the_marker_as_well_as_the_history`; this test is
    /// the real filesystem saying the barrier is enough.
    #[test]
    fn a_change_made_just_before_the_replay_is_in_it() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let progress = ScanProgress::default();
        let mut missed = Vec::new();
        for round in 0..20 {
            let since = current_event_id();
            let file = root.join(format!("just-now-{round}"));
            std::fs::write(&file, b"x").unwrap();
            let events = ask(&root, since, Duration::from_secs(10), &progress).unwrap();
            if !events.iter().any(|e| e.path == file.as_os_str().as_bytes()) {
                missed.push(round);
            }
        }
        assert!(
            missed.is_empty(),
            "rounds whose change the replay missed: {missed:?}"
        );
    }

    /// A sink for a marker at `marker_dir/sync`, without a stream: the
    /// records are handed to it by the test, exactly as FSEvents would.
    fn sink_for(marker_dir: &str, outside_root: bool) -> Sink {
        Sink {
            state: Mutex::new(SinkState::default()),
            done: Condvar::new(),
            cap: 1000,
            marker: format!("{marker_dir}/sync").into_bytes(),
            marker_dir: marker_dir.as_bytes().to_vec(),
            marker_dir_outside: outside_root,
            refs: Arc::default(),
        }
    }

    /// Hand `sink` one record and wait at most 50 ms for an answer.
    fn after(sink: &Sink, path: &str, flags: u32) -> Result<(), NoAnswer> {
        sink.record(&mut sink.lock(), path.as_bytes(), flags);
        let now = Instant::now();
        sink.wait(
            now,
            now,
            Duration::from_millis(50),
            &ScanProgress::default(),
        )
    }

    const CREATED_FILE: u32 = flag::ITEM_CREATED | flag::ITEM_IS_FILE;

    /// The answer is complete only once both the history and the marker are
    /// in, in either order: the marker can be numbered inside the history or
    /// after it. Exact, where the real-filesystem test is a probability.
    #[test]
    fn the_answer_waits_for_the_marker_as_well_as_the_history() {
        let sink = sink_for("/t/m", true);
        assert_eq!(
            after(&sink, "", flag::HISTORY_DONE),
            Err(NoAnswer::Deadline),
            "the history alone is not an answer"
        );
        assert_eq!(after(&sink, "/t/m/sync", CREATED_FILE), Ok(()));

        let sink = sink_for("/t/m", true);
        assert_eq!(
            after(&sink, "/t/m/sync", CREATED_FILE),
            Err(NoAnswer::Deadline),
            "the marker alone is not an answer"
        );
        assert_eq!(after(&sink, "", flag::HISTORY_DONE), Ok(()));
    }

    /// Once the history is over, the marker has [`MARKER_FLOOR`] however
    /// small the budget: its trip is fseventsd's latency, not the history's
    /// length. Played with a 50 ms budget and a marker 300 ms late.
    #[test]
    fn the_marker_is_waited_for_past_a_small_budget() {
        let sink = Arc::new(sink_for("/t/m", true));
        sink.record(&mut sink.lock(), b"", flag::HISTORY_DONE);
        let late = Arc::clone(&sink);
        let marker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            late.record(&mut late.lock(), b"/t/m/sync", CREATED_FILE);
            late.done.notify_all();
        });
        let now = Instant::now();
        let got = sink.wait(
            now,
            now,
            Duration::from_millis(50),
            &ScanProgress::default(),
        );
        marker.join().unwrap();
        assert_eq!(got, Ok(()));
    }

    /// A record that covers the marker without naming it — dropped events,
    /// a directory to rescan, a record that does not say what changed —
    /// means the marker may never be seen. That is a lost answer, said at
    /// once, not a deadline after the whole budget.
    #[test]
    fn a_record_that_loses_the_marker_is_a_lost_answer() {
        for (path, flags) in [
            ("/t/m", flag::MUST_SCAN_SUB_DIRS | flag::USER_DROPPED),
            ("/t/m", flag::MUST_SCAN_SUB_DIRS | flag::ITEM_IS_DIR),
            ("/", flag::MUST_SCAN_SUB_DIRS | flag::KERNEL_DROPPED),
        ] {
            let sink = sink_for("/t/m", true);
            assert_eq!(
                after(&sink, path, flags),
                Err(NoAnswer::Lost),
                "{path} {flags:#x}"
            );
        }
        // Next to it is not over it.
        let sink = sink_for("/t/m", true);
        assert_eq!(
            after(
                &sink,
                "/t/mm",
                flag::MUST_SCAN_SUB_DIRS | flag::USER_DROPPED
            ),
            Err(NoAnswer::Deadline)
        );
    }

    /// A record that only fails to say what changed — coalesced, or the
    /// directory's metadata and `com.apple.provenance` — is not a loss: the
    /// marker still comes. And once the marker is in, a loss reported around
    /// it changes nothing.
    #[test]
    fn noise_around_the_marker_is_not_a_loss() {
        const INODE_META: u32 = 0x0000_0400;
        const XATTR: u32 = 0x0000_8000;
        let sink = sink_for("/t/m", true);
        for (path, flags) in [
            ("/t/m", 0),
            ("/t/m", flag::ITEM_IS_DIR | INODE_META | XATTR),
            ("", flag::HISTORY_DONE),
        ] {
            assert_eq!(after(&sink, path, flags), Err(NoAnswer::Deadline), "{path}");
        }
        // A record naming the marker is the marker, whatever else it says.
        assert_eq!(after(&sink, "/t/m/sync", 0), Ok(()));

        let sink = sink_for("/t/m", true);
        after(&sink, "/t/m/sync", CREATED_FILE).unwrap_err();
        sink.record(&mut sink.lock(), b"", flag::HISTORY_DONE);
        assert_eq!(
            after(&sink, "/t/m", flag::MUST_SCAN_SUB_DIRS | flag::USER_DROPPED),
            Ok(()),
            "the marker came back before the loss"
        );
    }

    /// The marker's records are the barrier's, never changes under the root
    /// — not even where the temporary directory is under it. Its directory's
    /// own records are kept there, though: making and removing it changes
    /// its parent, which a full scan would see.
    #[test]
    fn the_marker_s_records_are_never_changes() {
        let paths = |sink: &Sink| -> Vec<String> {
            sink.lock()
                .events
                .iter()
                .map(|e| String::from_utf8(e.path.clone()).unwrap())
                .collect()
        };
        for outside in [true, false] {
            let sink = sink_for("/r/tmp/m", outside);
            let mut state = sink.lock();
            sink.record(
                &mut state,
                b"/r/tmp/m",
                flag::ITEM_CREATED | flag::ITEM_IS_DIR,
            );
            sink.record(&mut state, b"/r/tmp/m/sync", CREATED_FILE);
            sink.record(&mut state, b"/r/tmp/other", CREATED_FILE);
            drop(state);
            let mut got = paths(&sink);
            got.sort();
            let want: &[&str] = if outside {
                &["/r/tmp/other"]
            } else {
                &["/r/tmp/m", "/r/tmp/other"]
            };
            assert_eq!(got, want, "outside the root: {outside}");
        }
    }

    /// Whether the marker's directory is under the root is asked of both in
    /// their real spelling: the directory is canonical, and a root reached
    /// through a symlink holds it all the same.
    #[test]
    fn a_root_reached_through_a_symlink_holds_its_marker() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().canonicalize().unwrap().join("real");
        std::fs::create_dir_all(real.join("tmp")).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let marker = Marker::write_in(&link.join("tmp")).unwrap();
        assert!(!Sink::new(10, &link, &marker).marker_dir_outside);
        assert!(!Sink::new(10, &real, &marker).marker_dir_outside);
        let elsewhere = tempfile::tempdir().unwrap();
        assert!(Sink::new(10, elsewhere.path(), &marker).marker_dir_outside);
    }

    /// The same, through a real replay whose marker directory is made under
    /// the root.
    #[test]
    fn a_marker_made_under_the_root_is_not_in_the_answer() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let base = root.join("tmp");
        std::fs::create_dir(&base).unwrap();
        settle(&root);
        let progress = ScanProgress::default();
        let since = current_event_id();
        let events = replay(
            &root,
            since,
            Duration::from_secs(10),
            MAX_CHANGES,
            Marker::write_in(&base).unwrap(),
            &progress,
        )
        .unwrap();
        let marker = events
            .iter()
            .find(|e| e.path.ends_with(b"/sync"))
            .map(|e| String::from_utf8_lossy(&e.path).into_owned());
        assert_eq!(marker, None, "{events:?}");
    }

    /// Where the marker cannot be written, there is no barrier, and the
    /// reason is its own — not a journal that could not be read.
    #[test]
    fn a_barrier_that_cannot_be_written_says_so() {
        let dir = tempfile::tempdir().unwrap();
        MARKER_BASE.set(Some(dir.path().join("no-such-directory")));
        let got = FsEvents.barrier(dir.path());
        MARKER_BASE.set(None);
        assert_eq!(got.err(), Some(NoAnswer::NoBarrier));
    }

    /// A root on another volume than the temporary directory has no barrier:
    /// the marker's order against its changes was measured not to hold (see
    /// [`Marker`]). `/dev` is another volume everywhere.
    #[test]
    fn a_root_on_another_volume_than_the_marker_has_no_barrier() {
        let got = FsEvents.barrier(Path::new("/dev"));
        assert_eq!(got.err(), Some(NoAnswer::MarkerVolume));
    }

    /// The marker's directory is the replay's alone: mode 0700, made fresh,
    /// gone when the replay is.
    #[test]
    fn the_marker_directory_is_private_and_removed() {
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::tempdir().unwrap();
        let marker = Marker::write_in(base.path()).unwrap();
        let mode = std::fs::metadata(&marker.dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "{mode:o}");
        assert!(marker.path.is_file());
        let other = Marker::write_in(base.path()).unwrap();
        assert_ne!(marker.dir, other.dir, "one directory per replay");
        let dir = marker.dir.clone();
        drop(marker);
        assert!(!dir.exists());
    }

    /// The marker directory is compared as FSEvents spells it — the real
    /// path — even when the temporary directory is reached through a
    /// symlink, as `/tmp` and `/var` are on macOS.
    #[test]
    fn a_marker_base_reached_through_a_symlink_still_closes_the_replay() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap().join("root");
        let real = dir.path().canonicalize().unwrap().join("real");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let progress = ScanProgress::default();
        let got = replay(
            &root,
            current_event_id(),
            Duration::from_secs(10),
            MAX_CHANGES,
            Marker::write_in(&link).unwrap(),
            &progress,
        );
        assert!(got.is_ok(), "{got:?}");
    }

    /// Run `test`, one of this module's tests marked ignored, in a process
    /// of its own. A start it abandons hangs on in a thread, and while it
    /// does every replay in the process is refused (`HANGING`) — which would
    /// fail whatever else runs beside it.
    fn in_own_process(test: &str) {
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!("fsevents::tests::{test}"),
                "--ignored",
                "--test-threads=1",
            ])
            .env(OWN_PROCESS, "1")
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success() && text.contains("1 passed"),
            "{test}:\n{text}"
        );
    }

    /// Set for a test [`in_own_process`] runs; without it those tests do
    /// nothing, so `cargo test -- --ignored` does not run them side by side.
    const OWN_PROCESS: &str = "SPACETRACE_TEST_OWN_PROCESS";

    /// A start that does not return is walked away from at its allowance,
    /// with the counter moving meanwhile (invariant 8); until it returns,
    /// the next replay is refused at once rather than starting a stream
    /// beside it; once it has, replays work again.
    #[test]
    fn a_start_that_hangs_is_abandoned_and_blocks_the_next_until_it_returns() {
        in_own_process("hanging_start_alone");
    }

    #[test]
    #[ignore = "run in a process of its own by \
                a_start_that_hangs_is_abandoned_and_blocks_the_next_until_it_returns"]
    fn hanging_start_alone() {
        if std::env::var_os(OWN_PROCESS).is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let progress = ScanProgress::default();
        // Long enough past the allowance that walking away at it cannot be
        // mistaken for waiting it out, on a loaded machine too: 6.1 s was
        // seen for a 5 s allowance with the build in a crowded directory.
        let hang = START_FLOOR + Duration::from_secs(4);
        SLOW_START.set(hang);
        let asked = Instant::now();
        let got = ask(
            &root,
            current_event_id(),
            Duration::from_millis(300),
            &progress,
        );
        SLOW_START.set(Duration::ZERO);
        let waited = asked.elapsed();
        assert_eq!(got.unwrap_err(), NoAnswer::Deadline);
        assert!(
            waited >= START_FLOOR && waited < hang - Duration::from_secs(1),
            "{waited:?}"
        );
        assert!(progress.journal_ms.load(Ordering::Relaxed) >= START_FLOOR.as_millis() as u64);

        let refused = Instant::now();
        let got = ask(
            &root,
            current_event_id(),
            Duration::from_secs(10),
            &progress,
        );
        assert_eq!(got.unwrap_err(), NoAnswer::Stuck);
        assert!(refused.elapsed() < Duration::from_secs(1));

        let deadline = asked + hang + Duration::from_secs(10);
        while HANGING.load(Ordering::SeqCst) {
            assert!(
                Instant::now() < deadline,
                "the hanging start never returned"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        let got = ask(
            &root,
            current_event_id(),
            Duration::from_secs(10),
            &progress,
        );
        assert!(got.is_ok(), "{got:?}");
    }

    /// A scan cancelled while the stream is starting stops then, not when
    /// the start returns (invariant 5).
    #[test]
    fn a_scan_cancelled_during_the_start_stops_at_once() {
        in_own_process("cancelled_start_alone");
    }

    #[test]
    #[ignore = "run in a process of its own by a_scan_cancelled_during_the_start_stops_at_once"]
    fn cancelled_start_alone() {
        if std::env::var_os(OWN_PROCESS).is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let progress = ScanProgress::default();
        progress.cancel();
        SLOW_START.set(Duration::from_secs(20));
        let asked = Instant::now();
        let got = ask(
            &root,
            current_event_id(),
            Duration::from_secs(30),
            &progress,
        );
        SLOW_START.set(Duration::ZERO);
        assert_eq!(got.unwrap_err(), NoAnswer::Cancelled);
        assert!(
            asked.elapsed() < Duration::from_secs(3),
            "{:?}",
            asked.elapsed()
        );
    }

    /// More changes than are worth holding: no answer, and at once — not at
    /// the end of the budget.
    #[test]
    fn a_replay_with_more_changes_than_its_cap_has_no_answer() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let since = current_event_id();
        for i in 0..50 {
            std::fs::write(root.join(format!("f{i}")), b"x").unwrap();
        }
        settle(&root);
        let progress = Arc::new(ScanProgress::default());
        // A budget far beyond anything the test waits for, so "at once" can
        // be told from "at the deadline" with room for the other replays this
        // one queues behind (one at a time per process). A watchdog cancels
        // the replay at 60 s, so a regression fails the test then, as
        // `Cancelled`, rather than holding it for the whole budget.
        let watched = Arc::clone(&progress);
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let watchdog = std::thread::spawn(move || {
            if stopped.recv_timeout(Duration::from_secs(60)).is_err() {
                watched.cancel();
            }
        });
        let got = replay(
            &root,
            since,
            Duration::from_secs(300),
            10,
            Marker::new(&root).unwrap(),
            &progress,
        );
        let _ = stop.send(());
        watchdog.join().unwrap();
        assert_eq!(got.unwrap_err(), NoAnswer::TooMany);
        let progress = ScanProgress::default();
        let all = replay(
            &root,
            since,
            Duration::from_secs(30),
            1000,
            Marker::new(&root).unwrap(),
            &progress,
        )
        .unwrap();
        assert!(
            all.len() >= 50,
            "under the cap, every change: {}",
            all.len()
        );
    }

    /// No budget, no answer: the caller falls back to walking.
    #[test]
    fn a_replay_with_no_budget_has_no_answer() {
        let dir = tempfile::tempdir().unwrap();
        let progress = ScanProgress::default();
        let since = current_event_id().saturating_sub(1_000_000);
        assert_eq!(
            ask(dir.path(), since, Duration::ZERO, &progress).unwrap_err(),
            NoAnswer::Deadline
        );
    }

    /// A cancelled scan does not sit out the replay's budget.
    #[test]
    fn a_cancelled_scan_stops_waiting_for_the_journal() {
        let dir = tempfile::tempdir().unwrap();
        let progress = ScanProgress::default();
        progress.cancel();
        let started = Instant::now();
        assert_eq!(
            ask(dir.path(), 1, Duration::from_secs(60), &progress).unwrap_err(),
            NoAnswer::Cancelled
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// A scan queued behind another root's replay is still a scan the user
    /// can stop (invariant 5), and a watcher still sees it move (invariant
    /// 8). The other replay is played here by holding the turn.
    #[test]
    fn a_replay_waiting_for_its_turn_moves_and_can_be_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let (release, held) = std::sync::mpsc::channel::<()>();
        let (taken, wait_taken) = std::sync::mpsc::channel::<()>();
        let holder = std::thread::spawn(move || {
            let _turn = TURN.lock().unwrap_or_else(|p| p.into_inner());
            taken.send(()).unwrap();
            // Held until the test says so, or for at most ten seconds.
            let _ = held.recv_timeout(Duration::from_secs(10));
        });
        wait_taken.recv().unwrap();

        let progress = Arc::new(ScanProgress::default());
        let waiting = Arc::clone(&progress);
        let since = current_event_id();
        let asker = std::thread::spawn(move || {
            let started = Instant::now();
            let got = ask(&root, since, Duration::from_secs(30), &waiting);
            (got, started.elapsed())
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while progress.journal_ms.load(Ordering::Relaxed) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let moved = progress.journal_ms.load(Ordering::Relaxed);
        progress.cancel();
        let (got, waited) = asker.join().unwrap();
        let _ = release.send(());
        holder.join().unwrap();

        assert!(moved > 0, "no counter moved while the replay was queued");
        assert_eq!(got.unwrap_err(), NoAnswer::Cancelled);
        assert!(
            waited < Duration::from_secs(8),
            "the cancelled replay sat out the other one's turn: {waited:?}"
        );
    }

    /// The stream holds its own reference to what its callback writes into,
    /// through the context's retain and release: a callback FSEvents still
    /// has queued when the replay gives up must not find the sink freed. And
    /// it gives it back: no sink is leaked per replay. Counted for this
    /// replay's sink alone, so streams other tests leave behind do not count.
    #[test]
    fn the_stream_holds_its_own_reference_to_the_sink() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let progress = ScanProgress::default();
        ask(
            &root,
            current_event_id(),
            Duration::from_secs(10),
            &progress,
        )
        .unwrap();
        let refs = LAST_REFS.take().expect("the replay made a sink");
        let taken = refs.taken.load(Ordering::SeqCst);
        assert!(taken > 0, "FSEvents never took a reference of its own");
        let deadline = Instant::now() + Duration::from_secs(10);
        while refs.given.load(Ordering::SeqCst) < taken && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            refs.given.load(Ordering::SeqCst),
            taken,
            "a reference was never given back"
        );
    }

    /// Wait until FSEvents has recorded everything written under `root` so
    /// far: write a marker and watch for it. Events arrive in order, so once
    /// the marker is in, everything before it is. Also what a rescan test
    /// calls before taking its base, so that the fixture's own records do
    /// not arrive after the cursor and make it read more than it means to.
    pub(crate) fn settle(root: &Path) {
        use std::os::unix::ffi::OsStrExt;
        let since = current_event_id();
        let marker = root.join(format!(".settle-{since}"));
        std::fs::write(&marker, b"").unwrap();
        let progress = ScanProgress::default();
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            // Not `Marker::new`: a test may have pointed that thread's
            // markers somewhere unwritable on purpose.
            let marker_now = Marker::write_in(&std::env::temp_dir()).unwrap();
            let seen = replay(
                root,
                since,
                Duration::from_secs(10),
                MAX_CHANGES,
                marker_now,
                &progress,
            )
            .unwrap_or_default()
            .iter()
            .any(|e| e.path == marker.as_os_str().as_bytes());
            if seen {
                std::fs::remove_file(&marker).unwrap();
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("FSEvents never reported {}", marker.display());
    }

    /// What each kind of record means to a rescan, including the ones it
    /// must not believe.
    #[test]
    fn records_translate_to_changes() {
        let kind = |flags| change(Vec::new(), flags).kind;
        let file = flag::ITEM_IS_FILE;
        assert_eq!(kind(file | 0x0000_1000), ChangeKind::Modified);
        assert_eq!(kind(file | flag::ITEM_CREATED), ChangeKind::Created);
        assert_eq!(kind(file | flag::ITEM_RENAMED), ChangeKind::Renamed);
        assert_eq!(
            kind(file | flag::ITEM_CREATED | flag::ITEM_REMOVED),
            ChangeKind::Removed,
            "coalesced: the strongest claim"
        );
        assert_eq!(kind(0), ChangeKind::SubtreeUnknown, "no kind of entry");
        assert_eq!(
            kind(flag::ITEM_IS_DIR | flag::MUST_SCAN_SUB_DIRS),
            ChangeKind::SubtreeUnknown
        );
        for lost in [
            flag::USER_DROPPED | flag::MUST_SCAN_SUB_DIRS,
            flag::KERNEL_DROPPED | flag::MUST_SCAN_SUB_DIRS,
            flag::EVENT_IDS_WRAPPED,
            flag::ROOT_CHANGED,
            flag::MOUNT,
            flag::UNMOUNT,
        ] {
            assert_eq!(kind(lost), ChangeKind::Lost, "{lost:#x}");
        }
        assert!(change(Vec::new(), flag::ITEM_IS_DIR | 0x0000_4000).is_dir);
        assert!(!change(Vec::new(), file).is_dir);
    }

    /// `/dev` is devfs: no history, so no cursor — and no incremental rescan
    /// of anything on it.
    #[test]
    fn a_filesystem_that_is_not_apfs_has_none() {
        use std::os::unix::fs::MetadataExt;
        let dev = std::fs::metadata("/dev").unwrap().dev();
        assert_eq!(journal_volume(Path::new("/dev"), dev), None);
    }
}
