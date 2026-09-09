use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rayon::prelude::*;

use crate::capacity::Capacity;
use crate::meta::{display_name, EntryKind, FileIdentity, RawMeta};
use crate::tree::{NewNode, NodeId, Tree, TreeBuilder};

/// How many failing paths we keep for the report before we only count them.
const MAX_REPORTED_ERRORS: usize = 64;

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
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions {
            exclude_names: Vec::new(),
            one_filesystem: false,
            max_depth: None,
            dedupe_hardlinks: true,
        }
    }
}

/// Live counters a UI can poll while a scan runs, and the switch that stops it.
#[derive(Debug, Default)]
pub struct ScanProgress {
    pub files: AtomicU64,
    pub dirs: AtomicU64,
    pub bytes: AtomicU64,
    pub errors: AtomicU64,
    /// Set by [`ScanProgress::cancel`] and read once per directory.
    cancelled: AtomicBool,
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
}

/// What the scan found, beyond the tree itself.
#[derive(Debug, Clone, Default)]
pub struct ScanStats {
    pub files: u64,
    pub dirs: u64,
    pub errors: u64,
    /// Hardlinked files met more than once and counted only the first time.
    pub hardlinks_deduped: u64,
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

/// One entry as returned by the recursive phase, before flattening.
struct RawEntry {
    name: String,
    kind: EntryKind,
    size: u64,
    alloc: u64,
    mtime: i64,
    nlink: u64,
    children: Option<Vec<RawEntry>>,
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
        Some(read_dir_parallel(&root_path, 1, &ctx))
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
    let root_id = builder.push_root(NewNode {
        name: display_name(&root_path),
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
    let tree = builder.finish(root_path);

    let stats = ScanStats {
        files: progress.files.load(Ordering::Relaxed),
        dirs: progress.dirs.load(Ordering::Relaxed),
        errors: progress.errors.load(Ordering::Relaxed),
        hardlinks_deduped: ctx.hardlinks_deduped.load(Ordering::Relaxed),
        error_samples: ctx.errors.into_inner().unwrap(),
        duration_ms: started.elapsed().as_millis() as u64,
        // Asked once, after the walk: it describes the mount, not the tree,
        // and a failure here must not fail the scan.
        capacity: crate::capacity::capacity_of(tree.root_path()),
    };
    Ok((tree, stats))
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

fn read_dir_parallel(dir: &Path, depth: usize, ctx: &Ctx) -> Vec<RawEntry> {
    // Checked before the syscall, so a cancelled scan stops issuing I/O
    // immediately instead of draining whatever rayon had already queued.
    if ctx.progress.is_cancelled() {
        return Vec::new();
    }
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => {
            ctx.note_error(dir, &e);
            return Vec::new();
        }
    };

    // Read the whole directory first, then fan out. Doing the syscalls for one
    // directory on a single thread keeps readdir sequential (which is what the
    // kernel is fastest at) while different directories still run in parallel.
    let mut pending: Vec<(PathBuf, String, RawMeta)> = Vec::new();
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
        let name = entry.file_name().to_string_lossy().into_owned();
        pending.push((path, name, meta));
    }

    pending
        .into_par_iter()
        .map(|(path, name, meta)| build_entry(path, name, meta, depth, ctx))
        .collect()
}

fn build_entry(path: PathBuf, name: String, meta: RawMeta, depth: usize, ctx: &Ctx) -> RawEntry {
    let is_dir = meta.kind == EntryKind::Dir;

    let descend = is_dir
        && !ctx.opts.exclude_names.contains(&name)
        && ctx.opts.max_depth.is_none_or(|max| depth < max)
        && (!ctx.opts.one_filesystem || meta.dev == ctx.root_dev);

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
        Some(read_dir_parallel(&path, depth + 1, ctx))
    } else {
        None
    };

    RawEntry {
        name,
        kind: meta.kind,
        size,
        alloc,
        mtime: meta.mtime,
        nlink: meta.nlink,
        children,
    }
}

/// Flatten the recursive result into the arena in BFS order, so that every
/// node's children end up in one contiguous index range.
fn flatten(builder: &mut TreeBuilder, root_id: NodeId, root_children: Vec<RawEntry>) {
    let mut queue: VecDeque<(NodeId, Vec<RawEntry>)> = VecDeque::new();
    queue.push_back((root_id, root_children));

    while let Some((parent_id, entries)) = queue.pop_front() {
        if entries.is_empty() {
            continue;
        }
        let start = builder.nodes.len() as NodeId;
        let len = entries.len() as u32;

        let mut grandchildren: Vec<(NodeId, Vec<RawEntry>)> = Vec::new();
        for (i, entry) in entries.into_iter().enumerate() {
            let id = builder.push(
                parent_id,
                NewNode {
                    name: entry.name,
                    kind: entry.kind,
                    size: entry.size,
                    alloc: entry.alloc,
                    mtime: entry.mtime,
                    nlink: entry.nlink,
                },
            );
            debug_assert_eq!(id, start + i as NodeId);
            if let Some(kids) = entry.children {
                if !kids.is_empty() {
                    grandchildren.push((id, kids));
                }
            }
        }

        let parent = &mut builder.nodes[parent_id as usize];
        parent.children_start = start;
        parent.children_len = len;

        queue.extend(grandchildren);
    }
}
