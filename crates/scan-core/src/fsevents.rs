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

    fn replay(
        &self,
        root: &Path,
        since: u64,
        budget: Duration,
        progress: &ScanProgress,
    ) -> Result<Vec<Change>, NoAnswer> {
        replay(root, since, budget, progress)
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
    fn FSEventStreamFlushSync(stream: *mut c_void);
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
/// The history is over when FSEvents says so (`HistoryDone`). A flush after
/// that hands over anything it had buffered but not yet sent, so nothing
/// issued before the caller took its new cursor is left in flight when the
/// stream stops. What arrives after that point is extra, never missing:
/// the next scan replays it again from its own cursor. Measured on this
/// Mac: 200 files each written and replayed at once, none missed — with the
/// flush and without it, so the flush is a guard and not the mechanism.
///
/// `progress.journal_ms` counts the wait — for the turn below and for
/// FSEvents — so a watcher sees a counter move while FSEvents works through a
/// log that holds nothing under this root (invariant 8). Cancellation is
/// checked as often, in the queue as well (invariant 5).
///
/// **One replay at a time per process, and the budget starts with this
/// one's turn.** Concurrent history reads queue behind each other: 22 run at
/// once — the incremental test suite — had replays that take tens of
/// milliseconds alone miss a one-second budget, and none missed it run one
/// after another. A budget that measured the queue would turn a busy agent's
/// rescans into full scans for no reason about the journal. Waiting for the
/// turn is bounded by the other replay's own budget.
fn replay(
    root: &Path,
    since: EventId,
    budget: Duration,
    progress: &ScanProgress,
) -> Result<Vec<Change>, NoAnswer> {
    replay_capped(root, since, budget, MAX_CHANGES, progress)
}

/// [`replay`], holding at most `cap` distinct changes.
fn replay_capped(
    root: &Path,
    since: EventId,
    budget: Duration,
    cap: usize,
    progress: &ScanProgress,
) -> Result<Vec<Change>, NoAnswer> {
    use std::os::unix::ffi::OsStrExt;

    let asked = Instant::now();
    let _turn = take_turn(asked, progress)?;
    let barrier_marker = Marker::new().ok_or(NoAnswer::Failed)?;
    // The marker's directory is watched beside the root, and its records are
    // not changes under the root — unless it is under the root, when they
    // are, and are left in like any other.
    let skip = (!barrier_marker.dir.starts_with(root))
        .then(|| barrier_marker.dir.as_os_str().as_bytes().to_vec());
    let sink = Arc::new(Sink::new(
        cap,
        barrier_marker.path.as_os_str().as_bytes().to_vec(),
        skip,
    ));
    let bytes = root.as_os_str().as_bytes();
    let marker_dir = barrier_marker.dir.as_os_str().as_bytes();

    // SAFETY: every CF object created here is released on every path out.
    // The callback reaches `sink` through the context's `info`, and the
    // stream owns a reference of its own to it (`retain_sink`), which
    // FSEvents gives back only when it will not call the callback again — so
    // a callback still queued when this gives up finds the sink alive
    // whatever this function has dropped by then. The barrier on the queue
    // before returning is the second guard: nothing of this stream is
    // running there afterwards.
    unsafe {
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
        #[cfg(test)]
        std::thread::sleep(SLOW_START.get());
        if FSEventStreamStart(stream) == 0 {
            FSEventStreamInvalidate(stream);
            FSEventStreamRelease(stream);
            dispatch_release(queue);
            return Err(NoAnswer::Failed);
        }
        // The budget is for the journal's answer, and starts once the stream
        // is asking. Starting it is not that: `FSEventStreamStart` took 0.45
        // to 1.9 s per call for a binary sitting in a directory of 320,000
        // entries (a busy `target/debug/deps`), against 0.3 ms anywhere
        // else — measured, same binary, same minute — and counted in the
        // budget it turned every rescan from there into a deadline fallback.
        // `journal_ms` still counts it, from `asked`.
        let started = Instant::now();

        // The history first, then the barrier: written only now, the marker
        // reaches the stream live, behind everything fseventsd had queued.
        let outcome = sink
            .wait(asked, started, budget, progress, |s| s.history_done)
            .and_then(|()| barrier_marker.write())
            .and_then(|()| sink.wait(asked, started, budget, progress, |s| s.synced));
        if outcome.is_ok() {
            FSEventStreamFlushSync(stream);
        }
        FSEventStreamStop(stream);
        FSEventStreamInvalidate(stream);
        FSEventStreamRelease(stream);
        // A serial queue runs its blocks in order, so an empty one submitted
        // now returns only once every callback already queued has finished.
        dispatch_sync_f(queue, std::ptr::null_mut(), barrier);
        dispatch_release(queue);
        progress
            .journal_ms
            .store(asked.elapsed().as_millis() as u64, Ordering::Relaxed);
        outcome?;
    }
    Ok(sink.take())
}

/// One replay at a time per process; see [`replay`].
static TURN: Mutex<()> = Mutex::new(());

#[cfg(test)]
thread_local! {
    /// Time added before `FSEventStreamStart` on this thread: how a test
    /// plays the slow start measured for a binary in a crowded directory.
    static SLOW_START: std::cell::Cell<Duration> = const { std::cell::Cell::new(Duration::ZERO) };
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

/// References the streams took to their sinks, and gave back, ever. Read by
/// a test.
#[cfg(test)]
static SINK_RETAINS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static SINK_RELEASES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The context's `retain`: the stream takes a reference of its own to the
/// sink, so the sink outlives every callback FSEvents may still make.
extern "C" fn retain_sink(info: *const c_void) -> *const c_void {
    #[cfg(test)]
    SINK_RETAINS.fetch_add(1, Ordering::SeqCst);
    // SAFETY: `info` is `Arc::as_ptr` of a live `Arc<Sink>` (the replay's own
    // reference keeps it alive while the stream is created).
    unsafe { Arc::increment_strong_count(info as *const Sink) };
    info
}

/// The context's `release`: FSEvents gives back what `retain_sink` took.
extern "C" fn release_sink(info: *const c_void) {
    #[cfg(test)]
    SINK_RELEASES.fetch_add(1, Ordering::SeqCst);
    // SAFETY: balances exactly one `retain_sink` on the same pointer.
    unsafe { Arc::decrement_strong_count(info as *const Sink) };
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
    /// This replay's [`Marker`] path: seeing it is the barrier.
    marker: Vec<u8>,
    /// The marker directory, where it is not under the root: its records are
    /// the barrier's, not changes.
    skip: Option<Vec<u8>>,
}

#[derive(Default)]
struct SinkState {
    /// A set: FSEvents can deliver one record more than once for a path,
    /// and a rescan acts on each distinct one once.
    events: HashSet<Change>,
    history_done: bool,
    /// The marker written after `HistoryDone` has come back.
    synced: bool,
    overflowed: bool,
}

impl Sink {
    fn new(cap: usize, marker: Vec<u8>, skip: Option<Vec<u8>>) -> Sink {
        Sink {
            state: Mutex::new(SinkState::default()),
            done: Condvar::new(),
            cap,
            marker,
            skip,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SinkState> {
        // The callback must not panic across the FFI boundary, and a waiter
        // must not lose the events over a poisoned lock.
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Block until `reached` holds, the budget — counted from `started`,
    /// when this replay got its turn — is spent, or the scan is cancelled.
    /// `journal_ms` counts from `asked`, the queue included, so it never runs
    /// backwards.
    fn wait(
        &self,
        asked: Instant,
        started: Instant,
        budget: Duration,
        progress: &ScanProgress,
        reached: impl Fn(&SinkState) -> bool,
    ) -> Result<(), NoAnswer> {
        let mut guard = self.lock();
        loop {
            if guard.overflowed {
                return Err(NoAnswer::TooMany);
            }
            if reached(&guard) {
                return Ok(());
            }
            let waited = started.elapsed();
            progress
                .journal_ms
                .store(asked.elapsed().as_millis() as u64, Ordering::Relaxed);
            if progress.is_cancelled() {
                return Err(NoAnswer::Cancelled);
            }
            let Some(left) = budget.checked_sub(waited).filter(|d| !d.is_zero()) else {
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
    // SAFETY: `info` is the `Sink` the replay keeps alive until the stream
    // has been stopped; without `UseCFTypes` the paths are an array of
    // `count` NUL-terminated C strings, beside `count` flag words.
    let sink = unsafe { &*(info as *const Sink) };
    let paths = paths as *const *const c_char;
    let mut state = sink.lock();
    for i in 0..count {
        let flags = unsafe { *flags.add(i) };
        if flags & flag::HISTORY_DONE != 0 {
            state.history_done = true;
            continue;
        }
        let path = unsafe { std::ffi::CStr::from_ptr(*paths.add(i)) }.to_bytes();
        if path == sink.marker.as_slice() {
            state.synced = true;
        }
        if sink.skip.as_deref().is_some_and(|dir| under(path, dir)) {
            continue;
        }
        if state.overflowed {
            continue;
        }
        state.events.insert(change(path.to_vec(), flags));
        if state.events.len() > sink.cap {
            state.overflowed = true;
            state.events = HashSet::new();
        }
    }
    if state.history_done || state.synced || state.overflowed {
        sink.done.notify_all();
    }
}

/// Whether `path` is `dir` or below it.
fn under(path: &[u8], dir: &[u8]) -> bool {
    path.strip_prefix(dir)
        .is_some_and(|rest| rest.is_empty() || rest[0] == b'/')
}

/// A file this process writes after the history is over, and waits to see
/// come back: the barrier that makes a replay cover everything up to now.
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
/// **Why it works.** The kernel hands fseventsd its records in the order
/// they happen, through one queue for every volume, and a stream delivers
/// live records in the order fseventsd takes them. So once a marker written
/// after `HistoryDone` has come back, every change finished before the
/// replay began has come back before it. In a directory of this program's
/// own under the temporary directory, never in the root: a scan writes
/// nothing where it reads. Removed again when the replay ends.
struct Marker {
    dir: std::path::PathBuf,
    path: std::path::PathBuf,
}

impl Marker {
    /// A fresh name in the marker directory, which is made if missing.
    /// Canonical, because FSEvents reports real paths.
    fn new() -> Option<Marker> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join("spacetrace-fsevents");
        std::fs::create_dir_all(&dir).ok()?;
        let dir = dir.canonicalize().ok()?;
        let name = format!(
            "sync-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        Some(Marker {
            path: dir.join(name),
            dir,
        })
    }

    fn write(&self) -> Result<(), NoAnswer> {
        std::fs::write(&self.path, b"").map_err(|_| NoAnswer::Failed)
    }
}

impl Drop for Marker {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
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
        let events = replay(&root, since, Duration::from_secs(30), &progress).unwrap();
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
        let got = replay(
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
            let events = replay(&root, since, Duration::from_secs(10), &progress).unwrap();
            if !events.iter().any(|e| e.path == file.as_os_str().as_bytes()) {
                missed.push(round);
            }
        }
        assert!(
            missed.is_empty(),
            "rounds whose change the replay missed: {missed:?}"
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
        let progress = ScanProgress::default();
        let started = Instant::now();
        // A budget far beyond anything the test waits for, so "at once" can
        // be told from "at the deadline" with room for the other replays this
        // one queues behind (one at a time per process).
        let got = replay_capped(&root, since, Duration::from_secs(300), 10, &progress);
        assert_eq!(got.unwrap_err(), NoAnswer::TooMany);
        assert!(started.elapsed() < Duration::from_secs(60));
        let all = replay_capped(&root, since, Duration::from_secs(30), 1000, &progress).unwrap();
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
            replay(dir.path(), since, Duration::ZERO, &progress).unwrap_err(),
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
            replay(dir.path(), 1, Duration::from_secs(60), &progress).unwrap_err(),
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
            let got = replay(&root, since, Duration::from_secs(30), &waiting);
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
    /// has queued when the replay gives up must not find the sink freed.
    #[test]
    fn the_stream_holds_its_own_reference_to_the_sink() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let before = SINK_RETAINS.load(Ordering::SeqCst);
        let progress = ScanProgress::default();
        replay(
            &root,
            current_event_id(),
            Duration::from_secs(10),
            &progress,
        )
        .unwrap();
        let taken = SINK_RETAINS.load(Ordering::SeqCst);
        assert!(taken > before, "FSEvents never took a reference of its own");
        // And gives every one back: no sink is leaked per replay. Other
        // replays may run in between; all that were taken by now come back.
        let deadline = Instant::now() + Duration::from_secs(10);
        while SINK_RELEASES.load(Ordering::SeqCst) < taken && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            SINK_RELEASES.load(Ordering::SeqCst) >= taken,
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
            let seen = replay(root, since, Duration::from_secs(10), &progress)
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
