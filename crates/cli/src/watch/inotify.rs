//! inotify, asked for exactly what the watch acts on.
//!
//! notify 8.2 watches every directory for IN_OPEN as well, so each `opendir`
//! of a watched folder — by anyone, the watch's own relistings first — queues
//! an event, wakes its thread, and crosses the channel only to be dropped.
//! Here the mask names changes and nothing else: no IN_OPEN, no IN_ACCESS, no
//! IN_CLOSE_NOWRITE. A tree that is only being read is silent.
//!
//! One watch per folder, added by the caller where the scanner descends, and
//! one thread that reads the queue and hands each read on as a few
//! [`Change`]s: one per kind, each path once. A build writes one file a
//! hundred times between two ticks, and the session marks its folder once
//! whichever event it hears. The kernel's queue overflowing (IN_Q_OVERFLOW)
//! is a loss like the bounded channel's, and is reported as one.

use std::collections::{HashMap, HashSet};
use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use super::events::{Change, How};

/// What every watch asks for.
///
/// Every way a folder's listing can change — an entry made, removed, renamed
/// in or out, written to, its metadata — and the folder itself going or
/// moving. `IN_ONLYDIR` and `IN_DONT_FOLLOW` because a folder can be swapped
/// for a file or a symlink between the walk seeing it and the watch being
/// added, and the scanner follows neither; `IN_EXCL_UNLINK` because a file
/// still written to after it was deleted changes no listing.
const MASK: u32 = libc::IN_CREATE
    | libc::IN_DELETE
    | libc::IN_MODIFY
    | libc::IN_CLOSE_WRITE
    | libc::IN_ATTRIB
    | libc::IN_MOVED_FROM
    | libc::IN_MOVED_TO
    | libc::IN_DELETE_SELF
    | libc::IN_MOVE_SELF
    | libc::IN_ONLYDIR
    | libc::IN_DONT_FOLLOW
    | libc::IN_EXCL_UNLINK;

/// Room for a few thousand events per read: the kernel hands over whole
/// events only, as many as fit.
const BUFFER_BYTES: usize = 64 * 1024;

/// Each watch descriptor's folder.
type Watches = Arc<Mutex<HashMap<i32, PathBuf>>>;

pub(crate) struct Inotify {
    queue: Arc<File>,
    /// An eventfd the reader polls beside the queue, written to stop it.
    stop: File,
    watches: Watches,
    reader: Option<JoinHandle<()>>,
}

/// A descriptor a call just returned, owned from here on. A negative return
/// is the call's error, in `errno`.
fn owned(raw: libc::c_int) -> io::Result<File> {
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a descriptor just opened, which nothing else owns.
    Ok(File::from(unsafe { OwnedFd::from_raw_fd(raw) }))
}

/// A new inotify instance, non-blocking.
fn instance() -> io::Result<File> {
    // SAFETY: no pointers; the result goes straight to `owned`.
    owned(unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) })
}

/// Add `path` to `queue`'s watches; its descriptor. A folder already watched
/// — the same inode, under the same name or a new one after a rename — gets
/// the descriptor it already has.
fn add_watch(queue: &File, path: &Path) -> io::Result<i32> {
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: `c_path` is a NUL-terminated string that outlives the call.
    let wd = unsafe { libc::inotify_add_watch(queue.as_raw_fd(), c_path.as_ptr(), MASK) };
    if wd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(wd)
}

impl Inotify {
    /// An inotify instance and the thread that reads it into `tx`; a full
    /// `tx` drops the change and raises `overflowed`.
    pub(crate) fn start(
        tx: SyncSender<Change>,
        overflowed: Arc<AtomicBool>,
    ) -> io::Result<Inotify> {
        let queue = Arc::new(instance()?);
        // SAFETY: no pointers; the result goes straight to `owned`.
        let stop = owned(unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) })?;
        // The reader's own copy: one eventfd, so a write through either is
        // seen through both.
        let stopped = stop.try_clone()?;
        let watches = Watches::default();
        let reader = std::thread::Builder::new()
            .name("spacetrace-inotify".into())
            .spawn({
                let (queue, watches) = (Arc::clone(&queue), Arc::clone(&watches));
                move || read(&queue, &stopped, &watches, &tx, &overflowed)
            })?;
        Ok(Inotify {
            queue,
            stop,
            watches,
            reader: Some(reader),
        })
    }

    /// Watch one folder, or bring its path up to date if it is watched.
    pub(crate) fn watch(&mut self, path: &Path) -> io::Result<()> {
        // Held across the call: an event on the new watch can be read the
        // moment the watch exists, and has to find its folder.
        let mut watches = self
            .watches
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let wd = add_watch(&self.queue, path)?;
        watches.insert(wd, path.to_path_buf());
        Ok(())
    }
}

impl Drop for Inotify {
    fn drop(&mut self) {
        // An eventfd takes exactly eight bytes, a count to add.
        let _ = (&self.stop).write_all(&1u64.to_ne_bytes());
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

/// The reader thread: wait for events, read them all, hand them on.
fn read(
    queue: &File,
    stop: &File,
    watches: &Watches,
    tx: &SyncSender<Change>,
    overflowed: &AtomicBool,
) {
    // Plain bytes: each event is copied out with an unaligned read, so the
    // buffer promises no alignment.
    let mut buf = vec![0u8; BUFFER_BYTES];
    loop {
        let mut fds = [queue.as_raw_fd(), stop.as_raw_fd()].map(|fd| libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        });
        // SAFETY: `fds` is a live array of two `pollfd`s.
        if unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) } < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return last_word(tx, stop, Change::Failed(err.to_string()));
        }
        if fds[1].revents != 0 {
            return;
        }
        // Non-blocking, so drained until the kernel says it is empty.
        loop {
            let n = match (&*queue).read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return last_word(tx, stop, Change::Failed(e.to_string())),
            };
            for change in changes(&buf[..n], queue, watches) {
                match tx.try_send(change) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => overflowed.store(true, Ordering::Relaxed),
                    // The session ended; nobody left to tell.
                    Err(TrySendError::Disconnected(_)) => return,
                }
            }
        }
    }
}

/// Send the reader's last change, a failure, waiting for room for as long as
/// the session lives. Dropped for a full queue, as other changes are, it
/// would leave the session saying the watcher stopped and never why.
fn last_word(tx: &SyncSender<Change>, stop: &File, mut change: Change) {
    loop {
        match tx.try_send(change) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => return,
            Err(TrySendError::Full(back)) => change = back,
        }
        // Polled, not slept: dropping `Inotify` stops it while it waits.
        let mut fd = libc::pollfd {
            fd: stop.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one live `pollfd`.
        if unsafe { libc::poll(&mut fd, 1, 50) } > 0 {
            return;
        }
    }
}

/// What an event is to the session, for telling repeats apart.
#[derive(PartialEq, Eq, Hash)]
enum Key<'a> {
    /// Made or metadata at a path: the name matters, for it may be a folder.
    Named(How, i32, &'a [u8]),
    /// Anything else about an entry: whichever entry, the folder's listing is
    /// what changed.
    In(i32),
    /// The folder itself went or moved: its parent's listing changed.
    Itself(i32),
}

/// The events in one read, as changes: a loss first if there was one, then
/// one change per kind, each path in it once. `queue` is where the watches
/// of a folder that moved are taken off.
fn changes(bytes: &[u8], queue: &File, watches: &Watches) -> Vec<Change> {
    let header = std::mem::size_of::<libc::inotify_event>();
    let mut watches = watches
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut lost = false;
    let mut seen = HashSet::new();
    let (mut made, mut metadata, mut other) = (Vec::new(), Vec::new(), Vec::new());
    let mut at = 0;
    while at + header <= bytes.len() {
        // SAFETY: a whole header lies at `at`; read unaligned, because the
        // buffer promises no alignment.
        let event: libc::inotify_event =
            unsafe { std::ptr::read_unaligned(bytes[at..].as_ptr().cast()) };
        let start = at + header;
        let end = start + event.len as usize;
        // The kernel never splits an event across reads.
        let Some(raw) = bytes.get(start..end) else {
            break;
        };
        at = end;
        if event.mask & libc::IN_Q_OVERFLOW != 0 {
            lost = true;
            continue;
        }
        // The watch is gone: its folder was deleted or unmounted, which came
        // as an event of its own, or it was taken off.
        if event.mask & libc::IN_IGNORED != 0 {
            watches.remove(&event.wd);
            continue;
        }
        // A descriptor whose folder is not known any more: what it says is
        // about a folder the watch no longer has.
        let Some(dir) = watches.get(&event.wd) else {
            continue;
        };
        // The folder moved, its subfolders with it. Their watches would go on
        // reporting under paths that no longer name them — and that a new
        // folder may take — for as long as the folder exists. So they come
        // off. If it moved somewhere watched, the watch adds them again under
        // the new name, from the event about that name; and whatever stands
        // at the old one now is reported as made there, which is how FSEvents
        // reports a rename at each end.
        if event.mask & libc::IN_MOVE_SELF != 0 {
            let dir = dir.clone();
            let below: Vec<i32> = watches
                .iter()
                .filter(|(_, path)| path.starts_with(&dir))
                .map(|(&wd, _)| wd)
                .collect();
            for wd in below {
                watches.remove(&wd);
                // SAFETY: no pointers. A descriptor already gone is EINVAL,
                // which changes nothing.
                unsafe { libc::inotify_rm_watch(queue.as_raw_fd(), wd) };
            }
            if seen.insert(Key::Itself(event.wd)) {
                made.push(dir);
            }
            continue;
        }
        // NUL-padded to the next boundary.
        let name = &raw[..raw.iter().position(|&b| b == 0).unwrap_or(raw.len())];
        // The folder itself went or was unmounted over: its parent's listing
        // is what says how, and the kernel takes the watch off.
        let itself = event.mask & (libc::IN_DELETE_SELF | libc::IN_UNMOUNT);
        let (key, list) = if itself != 0 {
            (Key::Itself(event.wd), &mut other)
        } else if event.mask & (libc::IN_CREATE | libc::IN_MOVED_TO) != 0 {
            (Key::Named(How::Made, event.wd, name), &mut made)
        } else if event.mask & libc::IN_ATTRIB != 0 {
            (Key::Named(How::Metadata, event.wd, name), &mut metadata)
        } else {
            (Key::In(event.wd), &mut other)
        };
        if !seen.insert(key) {
            continue;
        }
        list.push(match name.is_empty() || itself != 0 {
            true => dir.clone(),
            false => dir.join(OsStr::from_bytes(name)),
        });
    }
    let mut out = Vec::new();
    if lost {
        out.push(Change::Lost(Vec::new()));
    }
    for (paths, how) in [
        (made, How::Made),
        (metadata, How::Metadata),
        (other, How::Other),
    ] {
        if !paths.is_empty() {
            out.push(Change::At { paths, how });
        }
    }
    out
}

/// The backend against the real kernel: a temp folder, a watch on it, and
/// what comes out of the channel.
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{self, Receiver};
    use std::time::Duration;

    fn watched(dirs: &[&Path]) -> (Inotify, Receiver<Change>) {
        let (tx, rx) = mpsc::sync_channel(1024);
        let mut inotify = Inotify::start(tx, Arc::default()).unwrap();
        for dir in dirs {
            inotify.watch(dir).unwrap();
        }
        (inotify, rx)
    }

    /// Every change up to and including the first that names `path`.
    fn until(rx: &Receiver<Change>, path: &Path) -> Vec<(PathBuf, How)> {
        let mut seen = Vec::new();
        loop {
            let change = rx
                .recv_timeout(Duration::from_secs(10))
                .unwrap_or_else(|_| panic!("nothing about {path:?}; before it: {seen:?}"));
            let Change::At { paths, how } = change else {
                panic!("{change:?}");
            };
            let done = paths.iter().any(|p| p == path);
            seen.extend(paths.into_iter().map(|p| (p, how)));
            if done {
                return seen;
            }
        }
    }

    /// The point of the backend: a folder read again and again — by `ls`,
    /// or by the watch's own relistings — queues nothing. notify's mask
    /// queued an IN_OPEN for every one of these.
    #[test]
    fn reading_a_watched_folder_is_no_event() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/f"), b"data").unwrap();
        let (_inotify, rx) = watched(&[root, &root.join("sub")]);

        for _ in 0..100 {
            for entry in std::fs::read_dir(root).unwrap() {
                let _ = std::fs::read_dir(entry.unwrap().path()).map(|d| d.count());
            }
            let _ = std::fs::read(root.join("sub/f")).unwrap();
        }
        std::fs::write(root.join("marker"), b"x").unwrap();

        let seen = until(&rx, &root.join("marker"));
        assert_eq!(seen, [(root.join("marker"), How::Made)], "only the write");
    }

    /// A burst in one folder, read in one go as the reader reads it, is one
    /// change per kind: every name made, once each, and one of the writes —
    /// the folder's listing is all a write can change.
    #[test]
    fn a_burst_in_one_folder_is_one_change_per_kind() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().canonicalize().unwrap();
        // An instance with no reader: the events wait in the kernel's queue.
        let queue = instance().unwrap();
        let wd = add_watch(&queue, root).unwrap();

        for i in 0..50 {
            std::fs::write(root.join("log"), vec![b'x'; i]).unwrap();
        }
        std::fs::write(root.join("new"), b"x").unwrap();
        let mut buf = vec![0u8; BUFFER_BYTES];
        let n = (&queue).read(&mut buf).unwrap();
        let watches = Watches::new(Mutex::new(HashMap::from([(wd, root.clone())])));

        assert_eq!(
            changes(&buf[..n], &queue, &watches),
            [
                Change::At {
                    paths: vec![root.join("log"), root.join("new")],
                    how: How::Made,
                },
                Change::At {
                    paths: vec![root.join("log")],
                    how: How::Other,
                },
            ]
        );
    }

    /// A new hardlink is heard of in the new name's folder only. The
    /// original's link count changes, but that is the file's metadata, and
    /// the kernel tells it to watches on the file itself, not to its folder
    /// (`fsnotify_link_count`). This is why the watch lists recent folders
    /// again when a link turns up whose other names it has not met
    /// (`Session::doubt_links`); FSEvents does name the original's folder.
    #[test]
    fn a_new_hardlink_is_heard_of_in_its_own_folder_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().canonicalize().unwrap();
        for sub in ["a", "b"] {
            std::fs::create_dir(root.join(sub)).unwrap();
        }
        std::fs::write(root.join("a/original"), b"data").unwrap();
        let (_inotify, rx) = watched(&[&root.join("a"), &root.join("b")]);

        std::fs::hard_link(root.join("a/original"), root.join("b/link")).unwrap();
        // Something heard in `a` afterwards, to know the queue is drained.
        std::fs::write(root.join("a/marker"), b"x").unwrap();

        let mut seen = until(&rx, &root.join("b/link"));
        seen.extend(until(&rx, &root.join("a/marker")));
        assert_eq!(seen.first(), Some(&(root.join("b/link"), How::Made)));
        assert!(
            !seen
                .iter()
                .any(|(path, _)| path.starts_with(root.join("a/original"))),
            "{seen:?}"
        );
    }

    /// A watched folder moved away takes its watches with it, its subfolders'
    /// too: what happens in it afterwards is not reported under the path it
    /// left — where a new folder may stand by then — and its watches do not
    /// linger until it is deleted, which may be never.
    #[test]
    fn a_folder_moved_away_takes_its_watches_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("a/sub")).unwrap();
        std::fs::create_dir(root.join("x")).unwrap();
        let (inotify, rx) = watched(&[root, &root.join("a"), &root.join("a/sub")]);

        std::fs::rename(root.join("a"), root.join("x/away")).unwrap();
        std::fs::write(root.join("x/away/sub/f"), b"x").unwrap();
        // Something heard after it, to know the queue is drained.
        std::fs::write(root.join("marker"), b"x").unwrap();

        let seen = until(&rx, &root.join("marker"));
        assert!(
            !seen
                .iter()
                .any(|(path, _)| path.starts_with(root.join("a/sub"))),
            "{seen:?}"
        );
        let watches = inotify.watches.lock().unwrap();
        assert!(
            !watches.values().any(|p| p.starts_with(root.join("a"))),
            "{watches:?}"
        );
    }

    /// A reader that fails says why, and the reason arrives even when the
    /// queue is full at that moment: it is the last thing the reader sends,
    /// and it waits for room. Without it the session could only say that
    /// the watcher stopped.
    #[test]
    fn a_reader_that_fails_says_why_through_a_full_queue() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().canonicalize().unwrap();
        let (tx, rx) = mpsc::sync_channel(1);
        let overflowed = Arc::new(AtomicBool::new(false));
        let mut inotify = Inotify::start(tx, Arc::clone(&overflowed)).unwrap();
        inotify.watch(root).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        for i in 0.. {
            if overflowed.load(Ordering::Relaxed) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the queue never filled"
            );
            std::fs::write(root.join(format!("f{i}")), b"x").unwrap();
        }

        // The queue's descriptor now names a folder, which polls readable
        // and fails every read with EISDIR. The next event wakes the reader.
        let folder = File::open(root).unwrap();
        // SAFETY: both descriptors are open; `dup2` swaps what the first
        // names, and the `File` that owns it still closes it once.
        assert!(unsafe { libc::dup2(folder.as_raw_fd(), inotify.queue.as_raw_fd()) } >= 0);
        std::fs::write(root.join("wake"), b"x").unwrap();

        let mut last = None;
        while let Ok(change) = rx.recv_timeout(Duration::from_secs(10)) {
            last = Some(change);
        }
        let Some(Change::Failed(why)) = last else {
            panic!("the last change was {last:?}");
        };
        assert!(why.contains("directory"), "{why}");
    }

    /// A watched folder that is deleted or moved says so under its own path,
    /// for its parent's listing to settle; a deleted one's watch is dropped.
    #[test]
    fn a_watched_folder_that_goes_names_itself() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().canonicalize().unwrap();
        for sub in ["doomed", "moved"] {
            std::fs::create_dir(root.join(sub)).unwrap();
        }
        let (inotify, rx) = watched(&[&root.join("doomed"), &root.join("moved")]);

        std::fs::remove_dir(root.join("doomed")).unwrap();
        assert_eq!(
            until(&rx, &root.join("doomed")),
            [(root.join("doomed"), How::Other)]
        );
        // A moved one is reported as made at the path it left: whatever is
        // there now, if anything, is not the folder that was watched.
        std::fs::rename(root.join("moved"), root.join("elsewhere")).unwrap();
        assert_eq!(
            until(&rx, &root.join("moved")),
            [(root.join("moved"), How::Made)]
        );

        // IN_IGNORED follows the delete; give the reader a moment for it.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let watches = inotify.watches.lock().unwrap();
            if watches.is_empty() {
                break;
            }
            drop(watches);
            assert!(std::time::Instant::now() < deadline, "the watch stayed");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
