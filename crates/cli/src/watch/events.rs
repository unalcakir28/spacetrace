//! The platform's change notification, set up the way the watch needs it.
//!
//! Two backends behind one shape. On Linux the watch talks to inotify itself
//! (`inotify.rs`): notify 8.2 asks inotify for IN_OPEN on every directory it
//! watches, so every directory anyone reads — the watch's own relistings
//! included — queues an event that is then thrown away. Everywhere else
//! notify does the work: FSEvents on macOS, ReadDirectoryChangesW on Windows,
//! each of which watches a whole tree with one handle.
//!
//! Either way what reaches the session is a [`Change`], already stripped of
//! reads, through the same bounded queue: full, it drops what does not fit
//! and raises the overflow flag, which the session treats as a loss.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::SyncSender;
use std::sync::Arc;

use anyhow::Result;
use spacetrace_scan_core::ScanOptions;

use super::model;
use crate::fmt;

/// What a backend saw, in the terms the session acts on.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Change {
    /// Something happened at these paths.
    At { paths: Vec<PathBuf>, how: How },
    /// Events were lost below these paths — everywhere, when there are none.
    Lost(Vec<PathBuf>),
    /// notify ran out of watches, and said so in an event. The inotify
    /// backend says so when the watch is added instead.
    #[cfg(not(target_os = "linux"))]
    Exhausted,
    /// Anything else the backend reported going wrong.
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum How {
    /// Created, or renamed into place or away: under a name the watch tracks
    /// as a folder, a folder it does not know, or none.
    Made,
    /// Metadata, link count included: how a new hardlink's original shows.
    Metadata,
    /// Written, removed, renamed away.
    Other,
}

pub(crate) struct Events {
    backend: Backend,
    /// Directories that could not be watched, so changes in them are only
    /// seen by the periodic rescan. Counted on screen, never dropped silently.
    pub(crate) unwatched: u64,
}

/// Why one directory could not be watched.
pub(crate) enum Unwatched {
    /// Gone, or replaced by something that is not a folder: the event about
    /// that is on its way.
    Gone,
    /// The per-user watch limit is used up.
    Exhausted,
    /// Anything else; the directory is counted as unwatched.
    Other,
}

impl Events {
    pub(crate) fn start(
        root: &Path,
        tx: SyncSender<Change>,
        overflowed: Arc<AtomicBool>,
    ) -> Result<Events> {
        Ok(Events {
            backend: Backend::start(root, tx, overflowed)?,
            unwatched: 0,
        })
    }

    /// Whether the watch adds a watch per directory. inotify watches one
    /// directory per watch, so the watch adds its own, exactly where the
    /// scanner descends: an excluded `node_modules` costs no watches.
    /// FSEvents and Windows watch a whole tree with one handle, and the
    /// events outside the scan are filtered.
    pub(crate) fn per_dir(&self) -> bool {
        self.backend.per_dir
    }

    /// Whether a new hardlink is reported in its original's folder too.
    /// FSEvents does, as that folder's metadata. inotify, the one backend
    /// that watches per directory, does not: a link count is the file's own
    /// metadata, told only to a watch on the file.
    pub(crate) fn hears_link_originals(&self) -> bool {
        !self.backend.per_dir
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
    pub(crate) fn watch_tree(
        &mut self,
        root: &Path,
        opts: &ScanOptions,
    ) -> Result<HashSet<PathBuf>> {
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
    pub(crate) fn watch_dir(&mut self, path: &Path, dirs: usize) -> Result<()> {
        if !self.per_dir() {
            return Ok(());
        }
        let Err(why) = self.backend.watch(path) else {
            return Ok(());
        };
        match why {
            Unwatched::Exhausted => Err(exhausted(path, Some(dirs))),
            Unwatched::Gone => Ok(()),
            Unwatched::Other => {
                self.unwatched += 1;
                Ok(())
            }
        }
    }
}

/// Out of inotify watches, in words that say what to do about it.
pub(crate) fn exhausted(path: &Path, dirs: Option<usize>) -> anyhow::Error {
    let read = |file: &str| {
        let text = std::fs::read_to_string(file).ok()?;
        text.trim().parse::<u64>().ok()
    };
    let global = read("/proc/sys/fs/inotify/max_user_watches");
    // A user namespace (a rootless container) has a limit of its own, and
    // when it is the lower one it is the one that ran out.
    let own = read("/proc/sys/user/max_inotify_watches")
        .filter(|own| global.is_none_or(|global| *own < global))
        .map_or(String::new(), |own| {
            format!(", user.max_inotify_watches = {own} in this user namespace")
        });
    let limit = global.map_or("unknown".to_string(), |n| n.to_string()) + &own;
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

/// EMFILE from `inotify_init`: the per-user instance limit, or the process's
/// descriptor limit — the kernel gives both the same number.
#[cfg(target_os = "linux")]
fn no_instance() -> anyhow::Error {
    let limit = std::fs::read_to_string("/proc/sys/fs/inotify/max_user_instances")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    anyhow::anyhow!(
        "cannot start watching: no inotify instance left \
         (fs.inotify.max_user_instances = {limit}), or this process is out of file \
         descriptors. Raise it with `sudo sysctl fs.inotify.max_user_instances=1024`"
    )
}

// ------------------------------------------------------------------- Linux

#[cfg(target_os = "linux")]
struct Backend {
    inotify: super::inotify::Inotify,
    per_dir: bool,
}

#[cfg(target_os = "linux")]
impl Backend {
    fn start(_root: &Path, tx: SyncSender<Change>, overflowed: Arc<AtomicBool>) -> Result<Backend> {
        match super::inotify::Inotify::start(tx, overflowed) {
            Ok(inotify) => Ok(Backend {
                inotify,
                per_dir: true,
            }),
            Err(e) if e.raw_os_error() == Some(libc::EMFILE) => Err(no_instance()),
            Err(e) => Err(anyhow::Error::new(e).context("cannot start watching")),
        }
    }

    fn watch(&mut self, path: &Path) -> Result<(), Unwatched> {
        let Err(err) = self.inotify.watch(path) else {
            return Ok(());
        };
        Err(match err.raw_os_error() {
            Some(libc::ENOSPC) => Unwatched::Exhausted,
            Some(libc::ENOENT | libc::ENOTDIR) => Unwatched::Gone,
            _ => Unwatched::Other,
        })
    }
}

// --------------------------------------------------------- everywhere else

#[cfg(not(target_os = "linux"))]
struct Backend {
    watcher: notify::RecommendedWatcher,
    per_dir: bool,
}

#[cfg(not(target_os = "linux"))]
impl Backend {
    fn start(root: &Path, tx: SyncSender<Change>, overflowed: Arc<AtomicBool>) -> Result<Backend> {
        use notify::{RecursiveMode, Watcher, WatcherKind};
        use std::sync::atomic::Ordering;
        use std::sync::mpsc::TrySendError;

        let handler = move |event: notify::Result<notify::Event>| {
            let Some(change) = from_notify(event) else {
                return;
            };
            // Disconnected is the session ending; nothing left to tell.
            if let Err(TrySendError::Full(_)) = tx.try_send(change) {
                overflowed.store(true, Ordering::Relaxed);
            }
        };
        let fail = |e: notify::Error| match e.kind {
            notify::ErrorKind::MaxFilesWatch => exhausted(root, None),
            _ => anyhow::Error::new(e).context(format!("cannot watch {}", root.display())),
        };
        let mut watcher = notify::recommended_watcher(handler).map_err(fail)?;
        // Android's notify backend is inotify too, one watch per folder.
        let per_dir = <notify::RecommendedWatcher as Watcher>::kind() == WatcherKind::Inotify;
        if !per_dir {
            watcher
                .watch(root, RecursiveMode::Recursive)
                .map_err(fail)?;
        }
        Ok(Backend { watcher, per_dir })
    }

    fn watch(&mut self, path: &Path) -> Result<(), Unwatched> {
        use notify::{RecursiveMode, Watcher};

        let Err(err) = self.watcher.watch(path, RecursiveMode::NonRecursive) else {
            return Ok(());
        };
        Err(match &err.kind {
            notify::ErrorKind::MaxFilesWatch => Unwatched::Exhausted,
            notify::ErrorKind::PathNotFound => Unwatched::Gone,
            notify::ErrorKind::Io(io) if io.kind() == std::io::ErrorKind::NotFound => {
                Unwatched::Gone
            }
            _ => Unwatched::Other,
        })
    }
}

/// A notify event as a [`Change`]; `None` for a read, which changes nothing
/// — and which the watch's own listings would otherwise report as activity.
#[cfg(not(target_os = "linux"))]
fn from_notify(event: notify::Result<notify::Event>) -> Option<Change> {
    use notify::event::ModifyKind;
    use notify::EventKind;

    let event = match event {
        Ok(event) => event,
        Err(err) if matches!(err.kind, notify::ErrorKind::MaxFilesWatch) => {
            return Some(Change::Exhausted)
        }
        Err(err) => return Some(Change::Failed(err.to_string())),
    };
    if event.need_rescan() {
        return Some(Change::Lost(event.paths));
    }
    let how = match event.kind {
        EventKind::Access(_) => return None,
        // FSEvents reports a rename as a name change at each end, without
        // saying which end; either may be a folder put in place.
        EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(_)) => How::Made,
        EventKind::Modify(ModifyKind::Metadata(_)) => How::Metadata,
        _ => How::Other,
    };
    Some(Change::At {
        paths: event.paths,
        how,
    })
}
