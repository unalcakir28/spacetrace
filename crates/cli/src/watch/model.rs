//! What `spacetrace watch` knows about the tree between scans.
//!
//! **One record per directory the scanner descends into, and nothing per
//! file.** A record holds what the last listing of that directory charged for
//! its own entries — files, symlinks, and the directories it does not descend
//! into — plus the same figures at the moment the watch began. Subtree totals
//! are one reverse pass over the records, the same pass `TreeBuilder` makes:
//! a record is always pushed after its parent, so the two arena properties of
//! invariant 2 hold here by construction too.
//!
//! **Every number comes from scan-core.** A directory that changed is listed
//! again with `scan(dir, max_depth = 1)`, a directory that appeared is scanned
//! whole, and a full rescan is an ordinary `scan` of the root. This module only
//! adds up what those scans charged; it never decides what an entry costs. So
//! the size semantics — logical `size`, directories' own blocks only in `alloc`,
//! symlinks as themselves — are the scanner's by construction (invariant 1, 3).
//!
//! **Except where one listing cannot know the answer.** Hardlink deduplication
//! is a property of a whole walk: which name carries the bytes depends on every
//! other name the walk met. So hardlinked names (`nlink > 1`) are left out of
//! the per-folder figures and settled in a ledger beside them (`links.rs`),
//! keyed by the `(dev, ino)` every scan here reports for them: each file once,
//! charged to one of its names, as one walk would. Clones (macOS) are the same
//! problem with no `nlink` to give them away and no identity a listing hands
//! over, so the watch counts every clone at its own size — `--no-clone-dedupe`
//! semantics, said in the output — rather than charge some and not others.

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use spacetrace_scan_core::{
    scan_with_hardlinks, with_deadline, EntryKind, FileIdentity, LinkedName, Mounts, NodeId,
    RawMeta, ScanOptions, ScanProgress, ScanStats, Tree,
};

use super::links::{Inode, Ledger, Seen};

pub(crate) type DirId = u32;

pub(crate) const ROOT: DirId = 0;

/// How deep the scanner walks whatever it is asked: past this it records an
/// error and does not descend (scan-core's `MAX_WALK_DEPTH`, which is private).
/// A record past it would be listed by a partial rescan that the full walk
/// refuses to make, and the two would disagree.
const MAX_WALK_DEPTH: usize = 1024;

/// How many unreadable paths are kept by name. The count is kept whole.
const MAX_SAMPLES: usize = 64;

/// The window a rate is measured over: between one and two of these.
///
/// Long enough that one write landing just before a tick does not read as a
/// burst, short enough that "it stopped" shows within seconds.
pub(crate) const RATE_WINDOW: Duration = Duration::from_secs(10);

const PRESENT: u8 = 1;
/// Existed when the watch began. Without it a directory that was empty then
/// and a directory that did not exist look the same.
const BASELINE: u8 = 2;

struct Dir {
    /// The name as the filesystem has it, not as the tree prints it. The tree
    /// keeps names lossily, and a path rebuilt from a lossy name names a file
    /// that does not exist — so a folder called `\xffx` on Linux could never
    /// be listed again. Same 16 bytes as the `Box<str>` it replaces.
    name: Box<OsStr>,
    parent: DirId,
    /// Tracked subdirectories, sorted by name. A directory that went away stays
    /// here, absent, so that the space it held is reported as gone rather than
    /// simply forgotten.
    children: Vec<DirId>,
    depth: u16,
    flags: u8,
    /// What its last listing charged, tracked subdirectories and hardlinked
    /// names excluded — those are the ledger's.
    direct_size: u64,
    direct_alloc: u64,
    /// The directory's own blocks, which `alloc` counts and `size` does not.
    own_alloc: u64,
    /// Subtree totals, by [`Model::aggregate`].
    size: u64,
    alloc: u64,
    /// Subtree totals when the watch began; 0 for what did not exist then.
    base_size: u64,
    base_alloc: u64,
    /// Logical subtree size at the last two rate marks.
    marks: [u64; 2],
    /// Paths below this directory, in its own listing, that could not be read.
    errors: u32,
}

impl Dir {
    fn has(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }

    fn set(&mut self, flag: u8, on: bool) {
        if on {
            self.flags |= flag;
        } else {
            self.flags &= !flag;
        }
    }
}

/// Why a partial update was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// A folder whose name the tree only keeps lossily, and which more than
    /// one real name fits, so it cannot be listed on its own. Only a full scan,
    /// which walks the real names, counts it.
    Unnamed(PathBuf),
}

/// What one update did.
#[derive(Debug, Default)]
pub(crate) struct Update {
    /// `Some` when the update was not applied and only a full rescan can give
    /// the right answer. The model is left exactly as it was.
    pub refused: Option<Refusal>,
}

/// The two sides of the whole tree, at the start and now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Totals {
    pub base_size: u64,
    pub size: u64,
    pub base_alloc: u64,
    pub alloc: u64,
}

impl Totals {
    pub fn delta(&self) -> i64 {
        self.size as i64 - self.base_size as i64
    }
}

/// How a reported directory changed, in `spacetrace diff`'s words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum MoverKind {
    Grown,
    Shrunk,
    Added,
    Removed,
}

/// One directory worth naming.
#[derive(Debug, Clone)]
pub(crate) struct Mover {
    pub id: DirId,
    /// Below the root, `/`-separated, as `spacetrace diff` prints it.
    pub path: String,
    pub kind: MoverKind,
    pub old_size: u64,
    pub new_size: u64,
    pub old_alloc: u64,
    pub new_alloc: u64,
}

impl Mover {
    pub fn delta(&self) -> i64 {
        self.new_size as i64 - self.old_size as i64
    }
}

pub(crate) struct Model {
    root: PathBuf,
    opts: ScanOptions,
    root_dev: u64,
    /// The mount table, read without blocking. A folder in it is approached
    /// the way the scanner approaches one: on a thread that can be abandoned.
    mounts: Mounts,
    dirs: Vec<Dir>,
    /// Unreadable paths, by name, at most [`MAX_SAMPLES`].
    samples: Vec<(PathBuf, String)>,
    mark_times: [Instant; 2],
    /// Directories that became tracked since the caller last asked, with their
    /// paths: the ones a per-directory watcher has to start watching.
    new_dirs: Vec<(DirId, PathBuf)>,
    /// Whether anyone reads `new_dirs`. A backend that watches the whole tree
    /// with one handle does not, and on a large root the first scan would
    /// otherwise build a path for every folder for nobody (83k of them on a
    /// 1.25M-entry tree).
    announce: bool,
    /// Every hardlinked file, and the folder charged for it.
    ledger: Ledger,
}

/// A scan as the model takes it in: the tree, what the walk reported, and
/// which file each hardlinked name in it is.
pub(crate) struct Scanned {
    pub tree: Tree,
    pub stats: ScanStats,
    /// Every hardlinked name in it with the file it names, by node. Empty
    /// without hardlink deduplication: then a hardlink is just a file, and
    /// settles nothing.
    links: Vec<LinkedName>,
}

impl Scanned {
    pub(crate) fn new(tree: Tree, stats: ScanStats, mut links: Vec<LinkedName>) -> Scanned {
        // Sorted rather than hashed: looked up once per hardlinked name, and
        // a backup tree has hundreds of thousands of them.
        links.sort_unstable_by_key(|l| l.node);
        Scanned { tree, stats, links }
    }

    /// Scan `path` the way every scan in the watch is taken: with the identity
    /// of each hardlinked name.
    pub(crate) fn of(
        path: &Path,
        opts: ScanOptions,
        progress: Arc<ScanProgress>,
    ) -> std::io::Result<Scanned> {
        let (tree, stats, links) = scan_with_hardlinks(path, opts, progress)?;
        Ok(Scanned::new(tree, stats, links))
    }

    /// The file a hardlinked name names.
    fn inode(&self, node: NodeId) -> Option<Inode> {
        let at = self.links.binary_search_by_key(&node, |l| l.node).ok()?;
        Some((self.links[at].dev, self.links[at].ino))
    }

    /// What `c` comes to for the folder whose listing holds it: added to
    /// `direct`, except for hardlinked names, which go to `linked` for the
    /// ledger to settle.
    ///
    /// Everything below `c` too, for a folder the model does not track on its
    /// own (one no single real name fits); every other child either has no
    /// children in the tree or is tracked.
    fn charge(&self, c: NodeId, direct: &mut (u64, u64), linked: &mut Vec<Seen>) {
        // Nearly every child is a plain file: no walk, no allocation.
        let n = self.tree.node(c);
        if n.children_len == 0 && !hardlinked(&self.tree, c) {
            direct.0 += n.own_size;
            direct.1 += n.own_alloc;
            return;
        }
        let mut stack = vec![c];
        while let Some(x) = stack.pop() {
            let n = self.tree.node(x);
            let inode = hardlinked(&self.tree, x).then(|| self.inode(x)).flatten();
            if let Some(inode) = inode {
                // Only the name the scan charged carries the figures; the
                // others stand at 0, and the ledger takes the largest.
                linked.push(Seen {
                    inode,
                    nlink: n.nlink,
                    size: n.size,
                    alloc: n.alloc,
                });
                continue;
            }
            direct.0 += n.own_size;
            direct.1 += n.own_alloc;
            stack.extend(self.tree.children(x));
        }
    }
}

/// The walk options the watch runs every scan with.
///
/// Clone deduplication is switched off whatever was asked, for the reason in
/// the module comment: a partial rescan cannot see a clone family that crosses
/// its boundary, and charging some clones and not others would be wrong in
/// both directions.
pub(crate) fn watch_options(mut opts: ScanOptions) -> ScanOptions {
    opts.dedupe_clones = false;
    opts.expected_entries = None;
    opts
}

/// Whether the scanner descends into a directory named `name` at `depth`
/// below the root, as far as the name and the depth decide it. The filesystem
/// boundary is the one rule this cannot answer; see [`Model::same_device`].
pub(crate) fn descends(opts: &ScanOptions, depth: usize, name: &str) -> bool {
    if depth == 0 {
        return true;
    }
    if depth >= MAX_WALK_DEPTH || opts.max_depth.is_some_and(|max| depth >= max) {
        return false;
    }
    // The scanner compares the lossy name against the list, exactly so.
    !opts.exclude_names.iter().any(|x| x == name)
}

/// The device a path lives on, the way the scanner reads it.
pub(crate) fn device_of(path: &Path) -> Option<u64> {
    let md = std::fs::symlink_metadata(path).ok()?;
    let (meta, _) = RawMeta::for_path(path, &md, FileIdentity::Needed);
    Some(meta.dev)
}

/// The mount table, when the walk consults one: exactly when `scan` does.
pub(crate) fn read_mounts(opts: &ScanOptions) -> Mounts {
    match opts.mount_timeout {
        Some(_) => Mounts::read(),
        None => Mounts::none(),
    }
}

/// `lstat` of a folder that may be a mount point, with the scanner's patience.
///
/// `None` when it did not answer in time, or could not be read: either way it
/// is not a folder to enter. A folder that is not in the table is asked
/// directly, as the walk asks it — only a boundary can belong to a server that
/// has gone.
pub(crate) fn probe(opts: &ScanOptions, mounts: &Mounts, path: &Path) -> Option<std::fs::Metadata> {
    let Some(limit) = opts.mount_timeout.filter(|_| mounts.contains(path)) else {
        return std::fs::symlink_metadata(path).ok();
    };
    let owned = path.to_path_buf();
    with_deadline(limit, move || std::fs::symlink_metadata(&owned))?.ok()
}

/// The device a folder lives on, read through [`probe`].
pub(crate) fn device_through(opts: &ScanOptions, mounts: &Mounts, path: &Path) -> Option<u64> {
    let md = probe(opts, mounts, path)?;
    let (meta, _) = RawMeta::for_path(path, &md, FileIdentity::Needed);
    Some(meta.dev)
}

/// The real names in `dir` that a lossy name stands for, for the names that
/// lost something. Read only when a listing holds such a name, which on
/// anything but an old Linux disk is never.
fn real_names(dir: &Path) -> HashMap<String, Vec<OsString>> {
    let mut map: HashMap<String, Vec<OsString>> = HashMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return map;
    };
    for entry in entries.flatten() {
        let real = entry.file_name();
        let lossy = real.to_string_lossy();
        if lossy.contains('\u{FFFD}') {
            map.entry(lossy.into_owned()).or_default().push(real);
        }
    }
    map
}

/// Turns the tree's lossy names back into real ones, one directory at a time.
struct Names<'a> {
    dir: &'a Path,
    lossy: Option<HashMap<String, Vec<OsString>>>,
}

impl<'a> Names<'a> {
    fn new(dir: &'a Path) -> Self {
        Names { dir, lossy: None }
    }

    /// The real name behind `name`, or `None` when more than one fits (or
    /// none does any more).
    fn real(&mut self, name: &str) -> Option<OsString> {
        if !name.contains('\u{FFFD}') {
            return Some(OsString::from(name));
        }
        let dir = self.dir;
        let map = self.lossy.get_or_insert_with(|| real_names(dir));
        match map.get(name).map(Vec::as_slice) {
            Some([only]) => Some(only.clone()),
            _ => None,
        }
    }
}

/// A folder's place in `(depth, path)` order: its depth below the root, then
/// its path one component at a time. Built only for a hardlinked file whose
/// charge has to move, which is the one time the order is asked.
fn dir_order(dirs: &[Dir], mut dir: DirId) -> (u16, Vec<&OsStr>) {
    let depth = dirs[dir as usize].depth;
    let mut parts = Vec::with_capacity(usize::from(depth));
    while dir != ROOT {
        let d = &dirs[dir as usize];
        parts.push(&*d.name);
        dir = d.parent;
    }
    parts.reverse();
    (depth, parts)
}

/// What a model does with one entry of a scan, for the folder whose listing
/// holds it.
enum Child {
    /// Counted in the folder's own figures, or the ledger's.
    Charge,
    /// A folder the scanner enters but no single real name fits, so it cannot
    /// be listed on its own (`Refusal::Unnamed`).
    Unnamed,
    /// A folder with a record of its own: its real name and path, and the
    /// record it already has under that name, present or not.
    Track(OsString, PathBuf, Option<DirId>),
}

fn hardlinked(tree: &Tree, id: NodeId) -> bool {
    let n = tree.node(id);
    n.kind == EntryKind::File && n.nlink > 1
}

impl Model {
    /// The model of a finished scan of the root, which becomes the baseline
    /// as of `now`. The clock is the caller's, as for every other method that
    /// measures a rate, so a test can say when things happened.
    pub(crate) fn new(scanned: &Scanned, opts: ScanOptions, announce: bool, now: Instant) -> Model {
        let root = scanned.tree.root_path().to_path_buf();
        let root_dev = device_of(&root).unwrap_or(0);
        let mut model = Model {
            root,
            mounts: read_mounts(&opts),
            opts,
            root_dev,
            dirs: vec![Dir {
                name: Box::from(OsStr::new("")),
                parent: ROOT,
                children: Vec::new(),
                depth: 0,
                flags: PRESENT,
                direct_size: 0,
                direct_alloc: 0,
                own_alloc: 0,
                size: 0,
                alloc: 0,
                base_size: 0,
                base_alloc: 0,
                marks: [0; 2],
                errors: 0,
            }],
            samples: Vec::new(),
            mark_times: [now; 2],
            new_dirs: Vec::new(),
            announce,
            ledger: Ledger::default(),
        };
        model.take_in(scanned, ROOT, true);
        model.aggregate();
        for d in &mut model.dirs {
            d.base_size = d.size;
            d.base_alloc = d.alloc;
            d.marks = [d.size; 2];
            d.flags |= BASELINE;
        }
        model
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn options(&self) -> &ScanOptions {
        &self.opts
    }

    /// Directories currently tracked.
    pub(crate) fn tracked(&self) -> usize {
        self.dirs.iter().filter(|d| d.has(PRESENT)).count()
    }

    pub(crate) fn is_present(&self, id: DirId) -> bool {
        self.dirs.get(id as usize).is_some_and(|d| d.has(PRESENT))
    }

    pub(crate) fn depth(&self, id: DirId) -> usize {
        usize::from(self.dirs[id as usize].depth)
    }

    pub(crate) fn parent(&self, id: DirId) -> Option<DirId> {
        (id != ROOT).then(|| self.dirs[id as usize].parent)
    }

    /// Whether `id` is `ancestor` or below it.
    pub(crate) fn is_within(&self, mut id: DirId, ancestor: DirId) -> bool {
        loop {
            if id == ancestor {
                return true;
            }
            if id == ROOT {
                return false;
            }
            id = self.dirs[id as usize].parent;
        }
    }

    /// Absolute path of a record.
    ///
    /// Walks to the root, so it is for one record at a time — a relisting, a
    /// row on screen — and never for every record (D6).
    pub(crate) fn path(&self, id: DirId) -> PathBuf {
        let mut parts = Vec::new();
        let mut cur = id;
        while cur != ROOT {
            let d = &self.dirs[cur as usize];
            parts.push(&*d.name);
            cur = d.parent;
        }
        let mut path = self.root.clone();
        path.extend(parts.iter().rev());
        path
    }

    /// Path below the root, `/`-separated; the root itself is empty. The same
    /// key `spacetrace diff` reports by.
    pub(crate) fn rel_path(&self, id: DirId) -> String {
        let mut parts = Vec::new();
        let mut cur = id;
        while cur != ROOT {
            let d = &self.dirs[cur as usize];
            parts.push(d.name.to_string_lossy());
            cur = d.parent;
        }
        parts.reverse();
        parts.join("/")
    }

    /// Where an event about `path` lands, in one walk: the tracked folder
    /// whose listing holds it ([`Model::locate`] without `itself`), and the
    /// tracked folder at exactly `path` if it is one.
    pub(crate) fn event_dirs(&self, path: &Path) -> Option<(DirId, Option<DirId>)> {
        let id = self.locate(path, true)?;
        let rel = path.strip_prefix(&self.root).ok()?;
        if rel.components().count() != self.depth(id) {
            return Some((id, None));
        }
        Some((self.parent(id).unwrap_or(ROOT), Some(id)))
    }

    /// Every tracked folder at or below `id`, with its path, built on the way
    /// down (D6).
    pub(crate) fn subtree_paths(&self, id: DirId) -> Vec<(DirId, PathBuf)> {
        let mut out = Vec::new();
        let mut stack = vec![(id, self.path(id))];
        while let Some((id, path)) = stack.pop() {
            let d = &self.dirs[id as usize];
            if !d.has(PRESENT) {
                continue;
            }
            for &c in &d.children {
                stack.push((c, path.join(&*self.dirs[c as usize].name)));
            }
            out.push((id, path));
        }
        out
    }

    /// Hand every tracked folder at or below `id` to the watcher again: what
    /// was under a folder that was replaced or renamed back has lost its
    /// watches on inotify, whatever the folder's name says.
    pub(crate) fn announce_subtree(&mut self, id: DirId) {
        if !self.announce {
            return;
        }
        let all = self.subtree_paths(id);
        self.new_dirs.extend(all);
    }

    /// Records that became tracked since the last call, with their paths.
    pub(crate) fn take_new_dirs(&mut self) -> Vec<(DirId, PathBuf)> {
        std::mem::take(&mut self.new_dirs)
    }

    /// The deepest tracked directory that holds `path`.
    ///
    /// With `itself`, `path` counts if it is a tracked directory; without, the
    /// search stops at its parent — which is what an event about an entry
    /// needs, since the entry is charged by its parent's listing. A path
    /// inside a directory the scanner does not descend into lands on the
    /// nearest one it does: that listing is what charges the skipped
    /// directory's own blocks.
    pub(crate) fn locate(&self, path: &Path, itself: bool) -> Option<DirId> {
        let rel = path.strip_prefix(&self.root).ok()?;
        let mut parts: Vec<_> = rel.components().collect();
        if !itself {
            // The root itself has no parent inside the watch; an event about
            // it is about the root's own listing.
            parts.pop();
        }
        let mut cur = ROOT;
        for part in parts {
            match self.child(cur, part.as_os_str()) {
                Some(next) if self.dirs[next as usize].has(PRESENT) => cur = next,
                _ => break,
            }
        }
        Some(cur)
    }

    fn child(&self, parent: DirId, name: &OsStr) -> Option<DirId> {
        let children = &self.dirs[parent as usize].children;
        children
            .binary_search_by(|&c| (*self.dirs[c as usize].name).cmp(name))
            .ok()
            .map(|i| children[i])
    }

    /// The records for these folders under `parent`, created or brought back
    /// as need be; a new or returning one is announced through `new_dirs`.
    ///
    /// A batch, because one folder can hold tens of thousands of others:
    /// finding each against the children that were already sorted and sorting
    /// once at the end is `n log n`, where inserting each in place was a
    /// shift of the whole list per folder (40k of them: 9.5 s, measured).
    fn adopt(&mut self, parent: DirId, wanted: Vec<(OsString, PathBuf)>) -> Vec<DirId> {
        let known = self.dirs[parent as usize].children.len();
        let depth = self.dirs[parent as usize].depth + 1;
        let mut ids = Vec::with_capacity(wanted.len());
        let mut added = false;
        for (name, path) in wanted {
            let found = self.dirs[parent as usize].children[..known]
                .binary_search_by(|&c| (*self.dirs[c as usize].name).cmp(&*name))
                .ok()
                .map(|i| self.dirs[parent as usize].children[i]);
            let id = match found {
                Some(id) if self.dirs[id as usize].has(PRESENT) => {
                    ids.push(id);
                    continue;
                }
                Some(id) => {
                    self.dirs[id as usize].set(PRESENT, true);
                    id
                }
                None => {
                    let id = self.dirs.len() as DirId;
                    self.dirs.push(Dir {
                        name: name.into_boxed_os_str(),
                        parent,
                        children: Vec::new(),
                        depth,
                        flags: PRESENT,
                        direct_size: 0,
                        direct_alloc: 0,
                        own_alloc: 0,
                        size: 0,
                        alloc: 0,
                        base_size: 0,
                        base_alloc: 0,
                        // It did not exist at either mark, so it held nothing.
                        marks: [0; 2],
                        errors: 0,
                    });
                    self.dirs[parent as usize].children.push(id);
                    added = true;
                    id
                }
            };
            if self.announce {
                self.new_dirs.push((id, path));
            }
            ids.push(id);
        }
        if added {
            let mut children = std::mem::take(&mut self.dirs[parent as usize].children);
            children.sort_unstable_by(|&a, &b| {
                self.dirs[a as usize].name.cmp(&self.dirs[b as usize].name)
            });
            self.dirs[parent as usize].children = children;
        }
        ids
    }

    /// Whether the scanner would descend into this directory of a listing,
    /// which [`descends`] allows.
    ///
    /// `listed` says the scan this came from did descend into it, which settles
    /// the filesystem boundary without asking: only an empty directory, or one
    /// a shallow listing never enters, has to be asked for its device.
    fn tracks(&self, path: &Path, known: bool, listed: bool) -> bool {
        if !self.opts.one_filesystem || known || listed {
            return true;
        }
        self.same_device(path)
    }

    /// The one-file-system rule, for a directory the model has not met.
    ///
    /// Through [`probe`]: a folder on another device is a mount point, and a
    /// mount point may belong to a server that has stopped answering — an
    /// `lstat` there would hang the whole watch (invariant 7).
    fn same_device(&self, path: &Path) -> bool {
        device_through(&self.opts, &self.mounts, path).is_some_and(|dev| dev == self.root_dev)
    }

    /// What to do with entry `c` of `tree`, in the listing of `m` at `path`;
    /// `depth` is the entry's below the root. The one rule both a subtree
    /// scan and a relisting take entries in by.
    fn classify(
        &self,
        tree: &Tree,
        c: NodeId,
        m: DirId,
        depth: usize,
        path: &Path,
        names: &mut Names,
    ) -> Child {
        let n = tree.node(c);
        let name = tree.name(c);
        if !n.is_dir() || !descends(&self.opts, depth, name) {
            return Child::Charge;
        }
        let Some(real) = names.real(name) else {
            return Child::Unnamed;
        };
        let child_path = path.join(&real);
        let known = self.child(m, &real);
        // A folder the scan entered is on the root's filesystem; only one it
        // did not enter, empty or past a shallow listing, has to be asked.
        let listed = n.children_len > 0;
        if !self.tracks(&child_path, known.is_some(), listed) {
            return Child::Charge;
        }
        Child::Track(real, child_path, known)
    }

    /// Write a scan of `at`'s whole subtree into the model, and settle the
    /// ledger over it. `whole` is a scan of the whole root (see
    /// `Ledger::settle`).
    fn take_in(&mut self, scanned: &Scanned, at: DirId, whole: bool) {
        self.absorb(scanned, at);
        let dirs = &self.dirs;
        self.ledger.settle(whole, |dir| dir_order(dirs, dir));
    }

    /// Write a scan of `at`'s whole subtree into the model.
    ///
    /// The scan is the authority: every tracked directory it holds is set to
    /// what it says, and every one it does not hold is gone. The ledger is
    /// left unsettled, for the caller to settle once everything that came with
    /// the same change is in.
    fn absorb(&mut self, scanned: &Scanned, at: DirId) {
        let tree = &scanned.tree;
        self.forget_errors_under(at);
        let mut stack = vec![(tree.root(), at, self.path(at))];
        while let Some((t, m, path)) = stack.pop() {
            let depth = self.depth(m) + 1;
            let mut direct = (0u64, 0u64);
            let mut linked = Vec::new();
            let mut names = Names::new(&path);
            let mut wanted = Vec::new();
            let mut nodes = Vec::new();
            for c in tree.children(t) {
                match self.classify(tree, c, m, depth, &path, &mut names) {
                    Child::Track(real, child_path, _) => {
                        wanted.push((real, child_path));
                        nodes.push(c);
                    }
                    // A folder no single real name fits is counted here,
                    // whole, from what the scan found — right for this scan;
                    // a listing of `m` that meets it again refuses.
                    Child::Charge | Child::Unnamed => {
                        scanned.charge(c, &mut direct, &mut linked);
                    }
                }
            }
            let paths: Vec<PathBuf> = wanted.iter().map(|w| w.1.clone()).collect();
            let ids = self.adopt(m, wanted);
            for ((id, c), child_path) in ids.iter().zip(nodes).zip(paths) {
                stack.push((c, *id, child_path));
            }
            let d = &mut self.dirs[m as usize];
            d.own_alloc = tree.node(t).own_alloc;
            d.direct_size = direct.0;
            d.direct_alloc = direct.1;
            self.ledger.replace(m, linked);
            self.drop_unseen(m, &ids.into_iter().collect());
        }
        self.record_errors(&scanned.stats, at);
    }

    /// Mark every tracked child of `m` that is not in `seen` as gone.
    fn drop_unseen(&mut self, m: DirId, seen: &HashSet<DirId>) {
        let gone: Vec<DirId> = self.dirs[m as usize]
            .children
            .iter()
            .copied()
            .filter(|c| self.dirs[*c as usize].has(PRESENT) && !seen.contains(c))
            .collect();
        for id in gone {
            self.mark_gone(id);
        }
    }

    fn mark_gone(&mut self, id: DirId) {
        let mut stack = vec![id];
        while let Some(id) = stack.pop() {
            let d = &mut self.dirs[id as usize];
            if !d.has(PRESENT) {
                continue;
            }
            d.set(PRESENT, false);
            d.direct_size = 0;
            d.direct_alloc = 0;
            d.own_alloc = 0;
            d.errors = 0;
            stack.extend(d.children.iter().copied());
            self.ledger.forget(id);
        }
    }

    fn shallow_options(&self) -> ScanOptions {
        let mut opts = self.opts.clone();
        opts.max_depth = Some(1);
        // A pool per listing is the price of going through `scan`; one thread
        // keeps it to one spawn.
        opts.threads = Some(1);
        opts
    }

    /// Options for scanning the whole subtree of a record at `depth`, so that
    /// `--depth` stops where it stops for a scan of the root.
    ///
    /// The scanner's own depth guard counts from wherever its scan starts, so
    /// it is folded into the limit here; without that a subtree scan would go
    /// deeper than a scan of the root ever does.
    fn deep_options(&self, depth: usize) -> ScanOptions {
        let mut opts = self.opts.clone();
        let limit = self
            .opts
            .max_depth
            .map_or(MAX_WALK_DEPTH, |max| max.min(MAX_WALK_DEPTH));
        opts.max_depth = Some(limit.saturating_sub(depth).max(1));
        opts
    }

    /// List one directory again and take what changed in it.
    ///
    /// Directories that appeared in it are scanned whole before anything is
    /// written, so that a refusal — a folder in it that only a full scan can
    /// name — leaves the model untouched.
    pub(crate) fn relist(&mut self, id: DirId) -> std::io::Result<Update> {
        let path = self.path(id);
        let depth = self.depth(id) + 1;
        let listing = Scanned::of(&path, self.shallow_options(), Arc::default())?;
        let tree = &listing.tree;
        let top = tree.root();

        // One pass over the listing, deciding everything and writing nothing:
        // a refusal must leave the model as it was.
        let mut refused = None;
        let mut names = Names::new(&path);
        let mut direct = (0u64, 0u64);
        let mut linked = Vec::new();
        let mut seen: HashSet<DirId> = HashSet::new();
        let mut own_allocs = Vec::new();
        let mut fresh = Vec::new();
        for c in tree.children(top) {
            let n = tree.node(c);
            let (real, child_path, known) =
                match self.classify(tree, c, id, depth, &path, &mut names) {
                    Child::Track(real, child_path, known) => (real, child_path, known),
                    Child::Unnamed => {
                        let name = tree.name(c);
                        refused.get_or_insert_with(|| Refusal::Unnamed(path.join(name)));
                        listing.charge(c, &mut direct, &mut linked);
                        continue;
                    }
                    Child::Charge => {
                        listing.charge(c, &mut direct, &mut linked);
                        continue;
                    }
                };
            if let Some(k) = known.filter(|&k| self.dirs[k as usize].has(PRESENT)) {
                seen.insert(k);
                // The parent's listing is where a folder's own blocks are read.
                own_allocs.push((k, n.own_alloc));
                continue;
            }
            // Gone between the listing and this scan: a later event says so.
            let Ok(subtree) = Scanned::of(&child_path, self.deep_options(depth), Arc::default())
            else {
                continue;
            };
            fresh.push((real, child_path, subtree));
        }
        if let Some(refusal) = refused {
            return Ok(Update {
                refused: Some(refusal),
            });
        }
        let vanished: Vec<DirId> = self.dirs[id as usize]
            .children
            .iter()
            .copied()
            .filter(|c| self.dirs[*c as usize].has(PRESENT) && !seen.contains(c))
            .collect();

        // Accepted: this folder's own entries, then each new subtree, and the
        // ledger settled once over all of it.
        for (k, own_alloc) in own_allocs {
            self.dirs[k as usize].own_alloc = own_alloc;
        }
        let d = &mut self.dirs[id as usize];
        d.own_alloc = tree.node(top).own_alloc;
        d.direct_size = direct.0;
        d.direct_alloc = direct.1;
        self.ledger.replace(id, linked);
        for v in vanished {
            self.mark_gone(v);
        }
        self.forget_own_errors(id, &path);
        self.dirs[id as usize].errors = u32::try_from(listing.stats.errors).unwrap_or(u32::MAX);
        self.keep_samples(&listing.stats);
        let wanted = fresh.iter().map(|f| (f.0.clone(), f.1.clone())).collect();
        let ids = self.adopt(id, wanted);
        for (k, (_, _, subtree)) in ids.into_iter().zip(fresh) {
            self.absorb(&subtree, k);
        }
        let dirs = &self.dirs;
        self.ledger.settle(false, |dir| dir_order(dirs, dir));
        Ok(Update::default())
    }

    /// Scan one record's whole subtree again — what the event stream asks for
    /// when it says it lost events below a path.
    ///
    /// Never refused: a hardlinked name in it is settled against the ones
    /// outside by the ledger, and a folder no single real name fits is counted
    /// whole, from this scan.
    pub(crate) fn rescan(&mut self, id: DirId) -> std::io::Result<()> {
        let path = self.path(id);
        let scanned = Scanned::of(&path, self.deep_options(self.depth(id)), Arc::default())?;
        self.take_in(&scanned, id, false);
        Ok(())
    }

    /// Take a fresh scan of the root as the truth about now.
    ///
    /// The baseline stays what it was; everything else is replaced, the
    /// ledger included — a name it kept that the scan did not meet is gone.
    /// Always right, whatever the events missed — this is the fallback every
    /// other path in the watch ends in.
    pub(crate) fn resync(&mut self, scanned: &Scanned) {
        // Read again with every full scan, which reads it too: a share
        // mounted since the start is a boundary like any other.
        self.mounts = read_mounts(&self.opts);
        self.take_in(scanned, ROOT, true);
    }

    /// End a tick for the ledger: whether a hardlinked file came to be
    /// counted under none of its names this tick, and the folder, below the
    /// root, of one that has been so for two (see `links.rs`).
    pub(crate) fn doubted_links(&mut self) -> (bool, Option<String>) {
        let dirs = &self.dirs;
        let doubts = self.ledger.doubts(|dir| dir_order(dirs, dir));
        (doubts.fresh, doubts.long.map(|dir| self.rel_path(dir)))
    }

    /// Whether a hardlinked file is counted under none of its names.
    #[cfg(test)]
    fn unsure_link(&self) -> bool {
        self.ledger.doubtful()
    }

    /// Subtree totals, from what each directory's own listing charged and
    /// the hardlinked files the ledger charges it for.
    pub(crate) fn aggregate(&mut self) {
        debug_assert!(!self.ledger.unsettled(), "aggregated mid-batch");
        let charged = self.ledger.charged();
        for (i, d) in self.dirs.iter_mut().enumerate() {
            let (size, alloc) = charged.get(i).copied().unwrap_or_default();
            d.size = d.direct_size + size;
            d.alloc = d.own_alloc + d.direct_alloc + alloc;
        }
        for i in (1..self.dirs.len()).rev() {
            let (size, alloc, parent) = {
                let d = &self.dirs[i];
                (d.size, d.alloc, d.parent as usize)
            };
            let p = &mut self.dirs[parent];
            p.size += size;
            p.alloc += alloc;
        }
    }

    pub(crate) fn totals(&self) -> Totals {
        let r = &self.dirs[ROOT as usize];
        Totals {
            base_size: r.base_size,
            size: r.size,
            base_alloc: r.base_alloc,
            alloc: r.alloc,
        }
    }

    /// Unreadable paths: how many, and some by name.
    pub(crate) fn errors(&self) -> (u64, &[(PathBuf, String)]) {
        let count = self
            .dirs
            .iter()
            .filter(|d| d.has(PRESENT))
            .map(|d| u64::from(d.errors))
            .sum();
        (count, &self.samples)
    }

    /// Change in the files directly inside the root, which no row covers:
    /// `spacetrace diff` reports directories below the root, never the root.
    pub(crate) fn root_files_delta(&self) -> i64 {
        let r = &self.dirs[ROOT as usize];
        let base_children: u64 = r
            .children
            .iter()
            .map(|&c| self.dirs[c as usize].base_size)
            .sum();
        let now_children: u64 = r.children.iter().map(|&c| self.dirs[c as usize].size).sum();
        (r.size - now_children) as i64 - (r.base_size - base_children) as i64
    }

    /// The directories that explain the change, biggest first.
    ///
    /// The rule is `spacetrace diff`'s, so that the two answer alike: a
    /// directory one child explains at least 90% of is passed over for that
    /// child, a directory that appeared or went away is reported once and not
    /// entered, and the root is never a row. Logical size, as in `diff`.
    pub(crate) fn movers(&self, min: u64) -> Vec<Mover> {
        const CONCENTRATION: f64 = 0.9;
        let mut out = Vec::new();
        // Paths are built on the way down, one segment per level entered.
        let mut stack = vec![(ROOT, String::new())];
        while let Some((id, prefix)) = stack.pop() {
            for &c in &self.dirs[id as usize].children {
                let d = &self.dirs[c as usize];
                let delta = d.size as i64 - d.base_size as i64;
                if delta.unsigned_abs() < min {
                    continue;
                }
                let name = d.name.to_string_lossy();
                let path = match prefix.is_empty() {
                    true => name.into_owned(),
                    false => format!("{prefix}/{name}"),
                };
                let kind = match (d.has(BASELINE), d.has(PRESENT)) {
                    (false, _) => MoverKind::Added,
                    (true, false) => MoverKind::Removed,
                    (true, true) if self.dominated(c, delta, CONCENTRATION) => {
                        stack.push((c, path));
                        continue;
                    }
                    (true, true) if delta >= 0 => MoverKind::Grown,
                    (true, true) => MoverKind::Shrunk,
                };
                out.push(Mover {
                    id: c,
                    path,
                    kind,
                    old_size: d.base_size,
                    new_size: d.size,
                    old_alloc: d.base_alloc,
                    new_alloc: d.alloc,
                });
            }
        }
        out.sort_unstable_by(|a, b| {
            b.delta()
                .abs()
                .cmp(&a.delta().abs())
                .then_with(|| a.path.cmp(&b.path))
        });
        out
    }

    /// Whether one child that existed on both sides explains the change.
    fn dominated(&self, id: DirId, delta: i64, concentration: f64) -> bool {
        if delta == 0 {
            return false;
        }
        let threshold = (delta.abs() as f64 * concentration) as i64;
        self.dirs[id as usize].children.iter().any(|&c| {
            let d = &self.dirs[c as usize];
            if !d.has(BASELINE) || !d.has(PRESENT) {
                return false;
            }
            let child = d.size as i64 - d.base_size as i64;
            child.signum() == delta.signum() && child.abs() >= threshold
        })
    }

    /// Move the rate marks forward once a window has passed.
    pub(crate) fn roll_marks(&mut self, now: Instant) {
        if now.duration_since(self.mark_times[1]) < RATE_WINDOW {
            return;
        }
        for d in &mut self.dirs {
            d.marks = [d.marks[1], d.size];
        }
        self.mark_times = [self.mark_times[1], now];
    }

    /// Bytes per second over the current window, logical.
    pub(crate) fn rate(&self, id: DirId, now: Instant) -> f64 {
        let secs = now.duration_since(self.mark_times[0]).as_secs_f64();
        if secs <= 0.0 {
            return 0.0;
        }
        let d = &self.dirs[id as usize];
        (d.size as f64 - d.marks[0] as f64) / secs
    }

    /// [`Model::rate`] for the files directly inside the root.
    pub(crate) fn root_files_rate(&self, now: Instant) -> f64 {
        let secs = now.duration_since(self.mark_times[0]).as_secs_f64();
        if secs <= 0.0 {
            return 0.0;
        }
        let r = &self.dirs[ROOT as usize];
        let (mut now_size, mut then) = (r.size as f64, r.marks[0] as f64);
        for &c in &r.children {
            let d = &self.dirs[c as usize];
            now_size -= d.size as f64;
            then -= d.marks[0] as f64;
        }
        (now_size - then) / secs
    }

    /// Drop what a long watch leaves behind: directories that came and went
    /// and were never part of the baseline.
    ///
    /// Ids change, so the old-to-new map is handed back — `DirId::MAX` for a
    /// record that was dropped — and whatever the caller holds by id must go
    /// through it. `None` when nothing moved.
    pub(crate) fn compact_if_worth_it(&mut self) -> Option<Vec<DirId>> {
        let dead = self
            .dirs
            .iter()
            .filter(|d| !d.has(PRESENT) && !d.has(BASELINE))
            .count();
        if dead < 1024 || dead < self.dirs.len() / 2 {
            return None;
        }
        let old = std::mem::take(&mut self.dirs);
        let mut remap = vec![DirId::MAX; old.len()];
        let mut order = Vec::with_capacity(old.len() - dead);
        let mut stack = vec![ROOT];
        while let Some(id) = stack.pop() {
            remap[id as usize] = order.len() as DirId;
            order.push(id);
            for &c in old[id as usize].children.iter().rev() {
                let d = &old[c as usize];
                if d.has(PRESENT) || d.has(BASELINE) {
                    stack.push(c);
                }
            }
        }
        let mut slots: Vec<Option<Dir>> = old.into_iter().map(Some).collect();
        for id in order {
            let mut d = slots[id as usize]
                .take()
                .expect("each record is visited once");
            d.parent = if id == ROOT {
                ROOT
            } else {
                remap[d.parent as usize]
            };
            d.children = d
                .children
                .iter()
                .filter(|&&c| remap[c as usize] != DirId::MAX)
                .map(|&c| remap[c as usize])
                .collect();
            self.dirs.push(d);
        }
        // Announcements name old ids. One whose record was dropped is a
        // folder that came and went before anyone watched it: nothing to do.
        self.new_dirs = std::mem::take(&mut self.new_dirs)
            .into_iter()
            .filter_map(|(id, path)| {
                let new = remap.get(id as usize).copied()?;
                (new != DirId::MAX).then_some((new, path))
            })
            .collect();
        self.ledger.remap(&remap);
        Some(remap)
    }

    fn forget_errors_under(&mut self, at: DirId) {
        let prefix = self.path(at);
        self.samples.retain(|(p, _)| !p.starts_with(&prefix));
        let mut stack = vec![at];
        while let Some(id) = stack.pop() {
            let d = &mut self.dirs[id as usize];
            d.errors = 0;
            stack.extend(d.children.iter().copied());
        }
    }

    fn forget_own_errors(&mut self, id: DirId, path: &Path) {
        self.samples
            .retain(|(p, _)| p != path && p.parent() != Some(path));
        self.dirs[id as usize].errors = 0;
    }

    fn keep_samples(&mut self, stats: &ScanStats) {
        for sample in &stats.error_samples {
            if self.samples.len() >= MAX_SAMPLES {
                break;
            }
            self.samples.push(sample.clone());
        }
    }

    /// Spread a subtree scan's error count over the records the named
    /// samples belong to; what the scanner only counted stays with `at`.
    fn record_errors(&mut self, stats: &ScanStats, at: DirId) {
        let mut named = 0u64;
        for (path, _) in &stats.error_samples {
            let Some(owner) = self.locate(path, false) else {
                continue;
            };
            let owner = if self.is_within(owner, at) { owner } else { at };
            self.dirs[owner as usize].errors += 1;
            named += 1;
        }
        let rest = stats.errors.saturating_sub(named);
        let d = &mut self.dirs[at as usize];
        d.errors = d
            .errors
            .saturating_add(u32::try_from(rest).unwrap_or(u32::MAX));
        self.keep_samples(stats);
    }
}

#[cfg(test)]
impl Model {
    /// Every tracked directory with its subtree totals, by path.
    fn dir_sizes(&self) -> Vec<(String, u64, u64)> {
        let mut out = Vec::new();
        let mut stack = vec![(ROOT, String::new())];
        while let Some((id, path)) = stack.pop() {
            let d = &self.dirs[id as usize];
            if !d.has(PRESENT) {
                continue;
            }
            out.push((path.clone(), d.size, d.alloc));
            for &c in &d.children {
                let name = self.dirs[c as usize].name.to_string_lossy();
                let child = match path.is_empty() {
                    true => name.into_owned(),
                    false => format!("{path}/{name}"),
                };
                stack.push((c, child));
            }
        }
        out.sort();
        out
    }

    fn records(&self) -> usize {
        self.dirs.len()
    }

    /// Every file the ledger counts, with the folder charged for it.
    fn charged_dirs(&self) -> HashMap<Inode, PathBuf> {
        self.ledger
            .charges()
            .map(|(inode, dir)| (inode, self.path(dir)))
            .collect()
    }
}

/// The model without any event in sight: every test here drives it by hand —
/// change the disk, then list the directory an event would have named — so
/// what is checked is the accounting, deterministically. Event delivery is
/// `crates/cli/tests/watch.rs`'s job.
///
/// The oracle throughout is the scanner itself: after any update, every
/// tracked directory must hold exactly what a fresh scan of the same disk
/// says it holds, with the same options.
#[cfg(test)]
mod tests {
    use super::*;
    use spacetrace_scan_core::scan;
    use std::fs;

    fn opts() -> ScanOptions {
        watch_options(ScanOptions::default())
    }

    fn fresh(root: &Path, opts: &ScanOptions) -> Tree {
        scan(root, opts.clone(), Arc::default()).unwrap().0
    }

    fn scanned(root: &Path, opts: &ScanOptions) -> Scanned {
        Scanned::of(root, opts.clone(), Arc::default()).unwrap()
    }

    fn start(root: &Path, opts: ScanOptions) -> (Model, Tree) {
        start_at(root, opts, Instant::now())
    }

    /// [`start`], with the baseline taken as of `at`.
    fn start_at(root: &Path, opts: ScanOptions, at: Instant) -> (Model, Tree) {
        let first = scanned(root, &opts);
        let model = Model::new(&first, opts, true, at);
        (model, first.tree)
    }

    /// A fresh scan of the disk that charges each hardlinked file to the
    /// folder the model charges it to.
    ///
    /// Which name a scan charges is undefined (invariant 3), so a scan of its
    /// own can disagree with the model folder by folder while both are right.
    /// This one cannot: every name counted in full, then every hardlinked name
    /// taken out again but one in the folder the model charges. Its total must
    /// still be a real scan's — which it is only if the model counts each file
    /// exactly once, in a folder that holds a name of it.
    fn fresh_as_charged(model: &Model, opts: &ScanOptions) -> Tree {
        let real = scanned(model.root(), opts);
        if !opts.dedupe_hardlinks {
            return real.tree;
        }
        let inode_at: HashMap<PathBuf, Inode> = real
            .links
            .iter()
            .map(|l| (real.tree.path(l.node), (l.dev, l.ino)))
            .collect();
        let mut every_name = opts.clone();
        every_name.dedupe_hardlinks = false;
        let mut tree = fresh(model.root(), &every_name);
        let mut charged = model.charged_dirs();
        // Which names are hardlinked comes from the deduplicating scan, not
        // from the link count in this one: on Windows a scan that does not
        // deduplicate reads no link count at all (`FileIdentity::Skipped`),
        // and every name in it says 1.
        let names: Vec<(NodeId, Inode)> = tree
            .iter()
            .filter_map(|n| Some((n, *inode_at.get(&tree.path(n))?)))
            .collect();
        for (n, inode) in names {
            let path = tree.path(n);
            if charged.get(&inode).map(PathBuf::as_path) == path.parent() {
                charged.remove(&inode);
                continue;
            }
            tree.remove_subtree(n);
        }
        // What is left lost its other names since its folder was listed: one
        // link, an ordinary file to the scan, counted where it is.
        for dir in charged.values() {
            assert!(dir.is_dir(), "charged to {dir:?}, which is gone");
        }
        assert_eq!(tree.total_size(), real.tree.total_size(), "each file once");
        assert_eq!(
            tree.total_alloc(),
            real.tree.total_alloc(),
            "each file once"
        );
        tree
    }

    fn write(path: &Path, bytes: usize) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, vec![7u8; bytes]).unwrap();
    }

    /// List what an event about `path` would have marked, as the tick does.
    fn touched(model: &mut Model, path: &Path) {
        let id = model.locate(path, false).expect("inside the root");
        let update = model.relist(id).unwrap();
        assert_eq!(update.refused, None, "nothing here needs a full scan");
        model.aggregate();
    }

    /// Every directory the model tracks agrees with a fresh scan, and so does
    /// the total — the scan charging hardlinks where the model does.
    fn assert_agrees_with_a_fresh_scan(model: &Model, opts: &ScanOptions) {
        let tree = fresh_as_charged(model, opts);
        assert_eq!(model.totals().size, tree.total_size(), "logical total");
        assert_eq!(model.totals().alloc, tree.total_alloc(), "on-disk total");
        for (path, size, alloc) in model.dir_sizes() {
            let node = tree
                .find(&path)
                .unwrap_or_else(|| panic!("{path:?} is tracked but not on disk"));
            assert_eq!(size, tree.node(node).size, "logical size of {path:?}");
            assert_eq!(alloc, tree.node(node).alloc, "on-disk size of {path:?}");
        }
    }

    fn rows(model: &Model) -> Vec<(String, MoverKind, i64)> {
        model
            .movers(1)
            .into_iter()
            .map(|m| (m.path.clone(), m.kind, m.delta()))
            .collect()
    }

    #[test]
    fn a_growing_file_is_charged_to_the_folder_that_explains_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().canonicalize().unwrap();
        write(&root.join("a/b/log"), 1_000);
        write(&root.join("c/other"), 50);
        let (mut model, _) = start(root, opts());

        write(&root.join("a/b/log"), 6_000);
        touched(&mut model, &root.join("a/b/log"));

        assert_eq!(model.totals().delta(), 5_000);
        // `a` changed by exactly what `a/b` did, so `a/b` is the answer and
        // `a` is not a row of its own — `diff`'s rule.
        assert_eq!(rows(&model), [("a/b".to_string(), MoverKind::Grown, 5_000)]);
        assert_agrees_with_a_fresh_scan(&model, &opts());
    }

    #[test]
    fn a_deleted_file_is_a_negative_change() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().canonicalize().unwrap();
        write(&root.join("a/keep"), 100);
        write(&root.join("a/gone"), 4_000);
        let (mut model, _) = start(root, opts());

        fs::remove_file(root.join("a/gone")).unwrap();
        touched(&mut model, &root.join("a/gone"));

        assert_eq!(model.totals().delta(), -4_000);
        assert_eq!(rows(&model), [("a".to_string(), MoverKind::Shrunk, -4_000)]);
        assert_agrees_with_a_fresh_scan(&model, &opts());
    }

    #[test]
    fn a_new_folder_is_measured_whole_when_its_parent_is_listed() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().canonicalize().unwrap();
        write(&root.join("old/f"), 10);
        let (mut model, _) = start(root, opts());

        // Several levels at once: on most backends only the topmost one is
        // in an event, because the rest were created inside it before anyone
        // could be watching there.
        write(&root.join("new/x/y/one"), 3_000);
        write(&root.join("new/x/two"), 2_000);
        touched(&mut model, &root.join("new"));

        assert_eq!(model.totals().delta(), 5_000);
        assert_eq!(rows(&model), [("new".to_string(), MoverKind::Added, 5_000)]);
        assert_agrees_with_a_fresh_scan(&model, &opts());
        let root_path = model.root().to_path_buf();
        let announced: Vec<_> = model
            .take_new_dirs()
            .into_iter()
            .map(|(_, p)| p.strip_prefix(&root_path).unwrap().to_path_buf())
            .collect();
        for expected in ["new", "new/x", "new/x/y"] {
            assert!(
                announced.contains(&PathBuf::from(expected)),
                "{expected} must be handed to the watcher: {announced:?}"
            );
        }
    }

    #[test]
    fn a_file_moved_between_folders_moves_its_bytes_and_not_the_total() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().canonicalize().unwrap();
        write(&root.join("from/f"), 7_000);
        fs::create_dir_all(root.join("to")).unwrap();
        let (mut model, _) = start(root, opts());

        fs::rename(root.join("from/f"), root.join("to/f")).unwrap();
        touched(&mut model, &root.join("from/f"));
        touched(&mut model, &root.join("to/f"));

        assert_eq!(model.totals().delta(), 0);
        assert_eq!(
            rows(&model),
            [
                ("from".to_string(), MoverKind::Shrunk, -7_000),
                ("to".to_string(), MoverKind::Grown, 7_000),
            ]
        );
        assert_agrees_with_a_fresh_scan(&model, &opts());
    }

    #[test]
    fn a_folder_moved_between_folders_is_gone_from_one_and_new_in_the_other() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().canonicalize().unwrap();
        write(&root.join("from/sub/deep/f"), 9_000);
        fs::create_dir_all(root.join("to")).unwrap();
        let (mut model, _) = start(root, opts());

        fs::rename(root.join("from/sub"), root.join("to/sub")).unwrap();
        // In the order the events arrive on every backend: the old name
        // first. Its parent finds it missing; the new parent finds it new.
        touched(&mut model, &root.join("from/sub"));
        touched(&mut model, &root.join("to/sub"));

        assert_eq!(model.totals().delta(), 0);
        // `diff`'s rule again: a folder that appeared or went away does not
        // stand in for its parent, so the parents are the rows.
        assert_eq!(
            rows(&model),
            [
                ("from".to_string(), MoverKind::Shrunk, -9_000),
                ("to".to_string(), MoverKind::Grown, 9_000),
            ]
        );
        assert_agrees_with_a_fresh_scan(&model, &opts());
        // The old records stay, absent, and nothing resolves into them.
        assert_eq!(
            model.locate(&root.join("from/sub/deep/f"), false),
            model.locate(&root.join("from"), true)
        );
    }

    /// Invariant 3 across time: a second name for bytes already counted adds
    /// nothing. A link names two folders and both are listed — the backends
    /// report both — in either order; whichever comes first, no total counts
    /// the file twice, and once both are in the file is counted once, charged
    /// to the same name either way. From then on the charge stays with that
    /// name until it goes.
    /// The folder a lasting doubt names is the first of the file's known
    /// folders in `(depth, path)` order, so the message names the same folder
    /// whichever was listed first.
    #[test]
    fn a_lasting_doubt_names_the_first_folder_holding_the_file() {
        for first in ["b/x/l", "c/l"] {
            let dir = tempfile::tempdir().unwrap();
            let root = &dir.path().canonicalize().unwrap();
            write(&root.join("a/big"), 1_000);
            fs::create_dir_all(root.join("b/x")).unwrap();
            fs::create_dir_all(root.join("c")).unwrap();
            let (mut model, _) = start(root, opts());

            // Two new names, both listed; the original's folder is not.
            fs::hard_link(root.join("a/big"), root.join("b/x/l")).unwrap();
            fs::hard_link(root.join("a/big"), root.join("c/l")).unwrap();
            let second = if first == "c/l" { "b/x/l" } else { "c/l" };
            touched(&mut model, &root.join(first));
            touched(&mut model, &root.join(second));
            assert!(model.unsure_link(), "{first} first");

            assert_eq!(model.doubted_links(), (true, None));
            let (_, long) = model.doubted_links();
            assert_eq!(long.as_deref(), Some("c"), "{first} first");
        }
    }

    #[test]
    fn a_new_hardlink_adds_nothing_in_either_order_and_its_charge_stays_put() {
        for first in ["b/link", "a/big"] {
            let dir = tempfile::tempdir().unwrap();
            let root = &dir.path().canonicalize().unwrap();
            write(&root.join("a/big"), 400_000);
            fs::create_dir_all(root.join("b")).unwrap();
            let (mut model, _) = start(root, opts());
            let size_of = |model: &Model, name: &str| {
                let sizes = model.dir_sizes();
                sizes.iter().find(|s| s.0 == name).unwrap().1
            };

            fs::hard_link(root.join("a/big"), root.join("b/link")).unwrap();
            touched(&mut model, &root.join(first));
            assert!(model.totals().delta() <= 0, "never twice ({first} first)");
            assert!(model.unsure_link(), "{first} first");
            // A tick's grace for the other folder's event; then a full scan.
            assert_eq!(model.doubted_links(), (true, None));
            assert!(model.doubted_links().1.is_some(), "{first} first");
            let second = if first == "a/big" { "b/link" } else { "a/big" };
            touched(&mut model, &root.join(second));
            assert!(!model.unsure_link(), "{first} first");
            assert_eq!(model.doubted_links(), (false, None));
            assert_eq!(model.totals().delta(), 0, "one inode, counted once");
            assert_eq!(size_of(&model, "a"), 400_000, "{first} first");
            assert_agrees_with_a_fresh_scan(&model, &opts());

            // Listing the folder that holds the charge changes nothing either.
            touched(&mut model, &root.join("a/big"));
            touched(&mut model, &root.join("b/link"));
            assert_eq!(size_of(&model, "a"), 400_000);
            assert!(rows(&model).is_empty(), "{:?}", rows(&model));

            fs::remove_file(root.join("a/big")).unwrap();
            touched(&mut model, &root.join("a/big"));
            assert_eq!(model.totals().delta(), 0, "the bytes are still on disk");
            assert_eq!(
                rows(&model),
                [
                    ("a".to_string(), MoverKind::Shrunk, -400_000),
                    ("b".to_string(), MoverKind::Grown, 400_000),
                ]
            );
            assert_agrees_with_a_fresh_scan(&model, &opts());

            fs::remove_file(root.join("b/link")).unwrap();
            touched(&mut model, &root.join("b/link"));
            assert_eq!(model.totals().delta(), -400_000, "the last name went");
            assert_agrees_with_a_fresh_scan(&model, &opts());
        }
    }

    /// The folder charged for each hardlinked file.
    fn charges(model: &Model) -> HashMap<Inode, PathBuf> {
        model.charged_dirs()
    }

    fn link(from: &Path, to: &Path) {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::hard_link(from, to).unwrap();
    }

    /// `diff`'s rows between two trees, in the model's words.
    fn diff_rows(old: &Tree, new: &Tree) -> (Vec<(String, MoverKind, i64)>, i64) {
        let report = spacetrace_diff::diff(
            old,
            new,
            &spacetrace_diff::DiffOptions {
                min_delta: 1,
                ..Default::default()
            },
        );
        let rows = report
            .changes
            .iter()
            .map(|c| {
                let kind = match c.kind {
                    spacetrace_diff::ChangeKind::Grown => MoverKind::Grown,
                    spacetrace_diff::ChangeKind::Shrunk => MoverKind::Shrunk,
                    spacetrace_diff::ChangeKind::Added => MoverKind::Added,
                    spacetrace_diff::ChangeKind::Removed => MoverKind::Removed,
                };
                (c.path.clone(), kind, c.delta())
            })
            .collect();
        (rows, report.delta())
    }

    /// A cargo build, as the disk sees it: object files written into `deps`
    /// and hardlinked into an incremental session folder, the previous
    /// session deleted, the binary written again and its uplifted name
    /// linked to it anew. Build after build, every relisting stays a
    /// relisting — no refusal, so no full rescan — and after every build the
    /// model is what a fresh scan says, folder by folder and in total, and
    /// its rows are `diff`'s. A pair nothing touched keeps its charge
    /// throughout, so no frame moves bytes between folders that did not
    /// change.
    #[cfg(unix)]
    #[test]
    fn a_build_tree_full_of_hardlinks_is_followed_without_a_full_rescan() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().canonicalize().unwrap();
        let debug = root.join("proj/target/debug");
        let deps = debug.join("deps");
        let session = |n: u32| debug.join(format!("incremental/app-x/s-{n}"));
        write(&root.join("proj/src/main.rs"), 2_000);
        write(&root.join("other/notes"), 1_000);
        write(&root.join("other/pair1"), 7_000);
        link(&root.join("other/pair1"), &root.join("other/pair2"));
        write(&deps.join("libdep-h.rlib"), 80_000);
        link(&deps.join("libdep-h.rlib"), &debug.join("libdep.rlib"));
        let build = |n: u32, sizes: [usize; 3]| {
            let [binary, a, b] = sizes;
            // A fresh inode for every output, as rustc writes them.
            for (name, size) in [("app-h", binary), ("app-h.a.o", a), ("app-h.b.o", b)] {
                let _ = fs::remove_file(deps.join(name));
                write(&deps.join(name), size);
            }
            link(&deps.join("app-h.a.o"), &session(n).join("a.o"));
            link(&deps.join("app-h.b.o"), &session(n).join("b.o"));
            if n > 1 {
                fs::remove_dir_all(session(n - 1)).unwrap();
            }
            let _ = fs::remove_file(debug.join("app"));
            link(&deps.join("app-h"), &debug.join("app"));
        };
        build(1, [300_000, 50_000, 40_000]);
        let (mut model, _) = start(root, opts());
        // The baseline as the model charges it, which a scan of its own may
        // not (invariant 3); the disk has not changed yet.
        let first = fresh_as_charged(&model, &opts());
        let inode_of = |path: PathBuf| {
            use std::os::unix::fs::MetadataExt;
            let md = fs::symlink_metadata(path).unwrap();
            (md.dev(), md.ino())
        };
        let before = charges(&model);
        let untouched: Vec<_> = [root.join("other/pair1"), deps.join("libdep-h.rlib")]
            .into_iter()
            .map(|path| {
                let inode = inode_of(path);
                (inode, before[&inode].clone())
            })
            .collect();

        for (n, sizes) in [
            (2, [320_000, 55_000, 40_000]),
            (3, [310_000, 20_000, 90_000]),
            (4, [500_000, 1_000, 1_000]),
        ] {
            build(n, sizes);
            // What the events name, deepest folder last, as a tick lists them.
            for path in [debug.join("app"), deps.join("app-h"), session(n)] {
                touched(&mut model, &path);
            }
            assert_agrees_with_a_fresh_scan(&model, &opts());
            let (expected, delta) = diff_rows(&first, &fresh_as_charged(&model, &opts()));
            assert!(!expected.is_empty(), "build {n} changed something");
            assert_eq!(rows(&model), expected, "build {n}");
            assert_eq!(model.totals().delta(), delta, "build {n}");
            let now = charges(&model);
            for (inode, path) in &untouched {
                assert_eq!(now.get(inode), Some(path), "build {n} moved a charge");
            }
        }
        // Three names of the latest objects at most: deps, one session, and
        // nothing left over from the sessions that were deleted.
        assert_eq!(model.ledger.names(), 2 + 2 + 2 + 2 + 2, "every name, once");
    }

    /// A charge stays with the folder that has it: a new name in a folder that
    /// comes first in path order does not take it, and neither does a full
    /// rescan whose own walk charged the other folder. Only when the folder
    /// loses its last name of the file does the charge move.
    #[test]
    fn a_charge_stays_with_its_folder_until_the_folder_has_no_name_of_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().canonicalize().unwrap();
        write(&root.join("z/f"), 50_000);
        link(&root.join("z/f"), &root.join("z/g"));
        let (mut model, _) = start(root, opts());
        let charged = |model: &Model| charges(model).into_values().collect::<Vec<_>>();
        assert_eq!(charged(&model), [root.join("z")]);

        link(&root.join("z/f"), &root.join("a/h"));
        touched(&mut model, &root.join("a"));
        touched(&mut model, &root.join("z/f"));
        assert_eq!(charged(&model), [root.join("z")]);
        model.resync(&scanned(root, &opts()));
        assert_eq!(charged(&model), [root.join("z")], "after a full rescan");
        model.aggregate();
        assert!(rows(&model).is_empty(), "{:?}", rows(&model));
        assert_agrees_with_a_fresh_scan(&model, &opts());

        fs::remove_file(root.join("z/f")).unwrap();
        touched(&mut model, &root.join("z/f"));
        assert_eq!(charged(&model), [root.join("z")], "z still has g");
        fs::remove_file(root.join("z/g")).unwrap();
        touched(&mut model, &root.join("z/g"));
        assert_eq!(charged(&model), [root.join("a")]);
        assert_eq!(model.totals().delta(), 0);
        assert_agrees_with_a_fresh_scan(&model, &opts());
    }

    /// A subtree rescan settles a hardlink one of whose names is outside it,
    /// where it once had to hand over to a full rescan.
    #[test]
    fn a_subtree_rescan_settles_hardlinks_across_its_edge() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().canonicalize().unwrap();
        write(&root.join("elsewhere/f"), 60_000);
        link(&root.join("elsewhere/f"), &root.join("lost/x/f"));
        let (mut model, _) = start(root, opts());
        let before = charges(&model);

        write(&root.join("elsewhere/g"), 9_000);
        link(&root.join("elsewhere/g"), &root.join("lost/y/g"));
        link(&root.join("elsewhere/f"), &root.join("lost/y/f2"));
        let lost = model.locate(&root.join("lost"), true).unwrap();
        model.rescan(lost).unwrap();
        // `g` is new, and its name outside the rescan is in an event of its
        // own; until that is listed, `g` counts under neither name.
        assert!(model.unsure_link());
        touched(&mut model, &root.join("elsewhere/g"));

        assert_eq!(model.totals().delta(), 9_000);
        assert_agrees_with_a_fresh_scan(&model, &opts());
        let f = before.keys().next().unwrap();
        assert_eq!(charges(&model)[f], before[f], "f keeps its charge");
    }

    /// The events missed a name: the ledger drops a file whose known names
    /// all went, while one it never heard of holds it still. That is exactly
    /// what the periodic full rescan is for, and it puts the ledger right.
    #[test]
    fn a_full_rescan_puts_right_a_ledger_that_missed_a_name() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().canonicalize().unwrap();
        write(&root.join("a/big"), 400_000);
        link(&root.join("a/big"), &root.join("c/keep"));
        fs::create_dir_all(root.join("b")).unwrap();
        let (mut model, _) = start(root, opts());

        // The link's event is lost; the deletes' are not.
        fs::hard_link(root.join("a/big"), root.join("b/link")).unwrap();
        fs::remove_file(root.join("a/big")).unwrap();
        fs::remove_file(root.join("c/keep")).unwrap();
        touched(&mut model, &root.join("a/big"));
        touched(&mut model, &root.join("c/keep"));
        assert_eq!(model.totals().delta(), -400_000, "wrong, as it must be");

        model.resync(&scanned(root, &opts()));
        model.aggregate();
        assert_eq!(model.totals().delta(), 0);
        assert_agrees_with_a_fresh_scan(&model, &opts());
    }

    #[test]
    fn without_hardlink_deduplication_nothing_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().canonicalize().unwrap();
        write(&root.join("a/big"), 30_000);
        fs::create_dir_all(root.join("b")).unwrap();
        let mut o = opts();
        o.dedupe_hardlinks = false;
        let (mut model, _) = start(root, o.clone());

        fs::hard_link(root.join("a/big"), root.join("b/link")).unwrap();
        touched(&mut model, &root.join("b/link"));

        assert_eq!(
            model.totals().delta(),
            30_000,
            "every name counted, as asked"
        );
        assert_agrees_with_a_fresh_scan(&model, &o);
    }

    #[test]
    fn a_folder_left_out_with_exclude_never_moves_the_numbers() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().canonicalize().unwrap();
        write(&root.join("node_modules/pkg/index.js"), 100);
        write(&root.join("src/main.rs"), 100);
        let mut o = opts();
        o.exclude_names = vec!["node_modules".to_string()];
        let (mut model, _) = start(root, o.clone());

        write(&root.join("node_modules/pkg/huge.bin"), 2_000_000);
        // The event lands on the nearest folder the scan enters.
        let at = model
            .locate(&root.join("node_modules/pkg/huge.bin"), false)
            .unwrap();
        assert_eq!(at, ROOT);
        touched(&mut model, &root.join("node_modules/pkg/huge.bin"));

        assert_eq!(model.totals().delta(), 0);
        assert!(rows(&model).is_empty(), "{:?}", rows(&model));
        assert_agrees_with_a_fresh_scan(&model, &o);
    }

    #[test]
    fn depth_stops_where_it_stops_for_a_scan() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().canonicalize().unwrap();
        write(&root.join("a/f"), 10);
        write(&root.join("a/b/c/f"), 10);
        let mut o = opts();
        o.max_depth = Some(2);
        let (mut model, _) = start(root, o.clone());

        write(&root.join("a/b/c/more"), 5_000);
        write(&root.join("a/new/inner/f"), 5_000);
        touched(&mut model, &root.join("a/b/c/more"));
        touched(&mut model, &root.join("a/new"));

        // A scan with `--depth 2` never sees below `a/*`, so neither does
        // this — whatever was written there.
        assert_agrees_with_a_fresh_scan(&model, &o);
        assert_eq!(model.totals().delta(), 0);
    }

    #[test]
    fn the_files_in_the_root_itself_are_accounted_separately() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().canonicalize().unwrap();
        write(&root.join("top"), 100);
        write(&root.join("a/f"), 100);
        let (mut model, _) = start(root, opts());

        write(&root.join("top"), 2_100);
        touched(&mut model, &root.join("top"));

        assert_eq!(model.totals().delta(), 2_000);
        assert_eq!(model.root_files_delta(), 2_000);
        assert!(rows(&model).is_empty(), "no folder below the root moved");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_counts_as_itself_and_is_never_followed() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = &dir.path().canonicalize().unwrap();
        write(&outside.path().join("big"), 1_000_000);
        fs::create_dir_all(root.join("a")).unwrap();
        let (mut model, _) = start(root, opts());

        std::os::unix::fs::symlink(outside.path().join("big"), root.join("a/link")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("a/dirlink")).unwrap();
        touched(&mut model, &root.join("a/link"));

        let link_len = fs::symlink_metadata(root.join("a/link")).unwrap().len()
            + fs::symlink_metadata(root.join("a/dirlink")).unwrap().len();
        assert_eq!(model.totals().delta(), link_len as i64);
        assert_agrees_with_a_fresh_scan(&model, &opts());
    }

    /// What the event stream missed, the full rescan puts right: changes made
    /// with nothing listing them, then one `resync`, and every directory is
    /// back to what the disk holds — including ones that appeared and went.
    #[test]
    fn a_full_rescan_restores_what_lost_events_got_wrong() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().canonicalize().unwrap();
        write(&root.join("a/f"), 1_000);
        write(&root.join("b/c/f"), 1_000);
        write(&root.join("doomed/f"), 1_000);
        let (mut model, _) = start(root, opts());

        write(&root.join("a/f"), 9_000);
        write(&root.join("b/c/new/g"), 4_000);
        fs::remove_dir_all(root.join("doomed")).unwrap();
        write(&root.join("fresh/h"), 2_000);
        model.aggregate();
        assert_eq!(model.totals().delta(), 0, "nothing told the model yet");

        model.resync(&scanned(root, &opts()));
        model.aggregate();

        assert_eq!(model.totals().delta(), 8_000 + 4_000 - 1_000 + 2_000);
        assert_agrees_with_a_fresh_scan(&model, &opts());
        let rows = rows(&model);
        assert!(
            rows.contains(&("doomed".to_string(), MoverKind::Removed, -1_000)),
            "{rows:?}"
        );
        assert!(
            rows.contains(&("fresh".to_string(), MoverKind::Added, 2_000)),
            "{rows:?}"
        );
    }

    /// The subtree rescan an FSEvents `MustScanSubDirs` asks for: everything
    /// below one folder, nothing outside it.
    #[test]
    fn a_subtree_rescan_restores_that_subtree() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().canonicalize().unwrap();
        write(&root.join("lost/x/f"), 1_000);
        write(&root.join("elsewhere/f"), 1_000);
        let (mut model, _) = start(root, opts());

        write(&root.join("lost/x/f"), 5_000);
        write(&root.join("lost/y/z/g"), 3_000);
        let lost = model.locate(&root.join("lost"), true).unwrap();
        model.rescan(lost).unwrap();
        model.aggregate();

        assert_eq!(model.totals().delta(), 7_000);
        assert_agrees_with_a_fresh_scan(&model, &opts());
    }

    /// The model against `spacetrace diff` itself: after a mix of changes, the
    /// rows are exactly what `diff` reports between the first scan and a fresh
    /// one, in the same order. Two implementations of one rule only stay one
    /// rule while something compares them.
    #[test]
    fn the_rows_are_what_diff_reports() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().canonicalize().unwrap();
        write(&root.join("proj/target/debug/a"), 10_000);
        write(&root.join("proj/src/lib.rs"), 2_000);
        write(&root.join("logs/app.log"), 500);
        write(&root.join("tmp/x/old"), 3_000);
        write(&root.join("spread/one/f"), 100);
        write(&root.join("spread/two/f"), 100);
        let (mut model, first) = start(root, opts());

        write(&root.join("proj/target/debug/a"), 60_000);
        write(&root.join("proj/target/debug/incremental/b"), 9_000);
        write(&root.join("logs/app.log"), 1_500);
        fs::remove_dir_all(root.join("tmp/x")).unwrap();
        write(&root.join("spread/one/f"), 5_100);
        write(&root.join("spread/two/f"), 4_100);
        write(&root.join("cache/new/blob"), 7_000);
        for path in [
            "proj/target/debug/a",
            "proj/target/debug/incremental",
            "logs/app.log",
            "tmp/x",
            "spread/one/f",
            "spread/two/f",
            "cache",
        ] {
            touched(&mut model, &root.join(path));
        }

        let (expected, delta) = diff_rows(&first, &fresh(root, &opts()));
        assert!(expected.len() >= 4, "the fixture must spread: {expected:?}");
        assert_eq!(rows(&model), expected);
        assert_eq!(model.totals().delta(), delta);
    }

    #[test]
    fn compaction_forgets_folders_that_came_and_went_and_keeps_the_answer() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().canonicalize().unwrap();
        write(&root.join("keep/f"), 100);
        write(&root.join("gone-later/f"), 100);
        let (mut model, _) = start(root, opts());

        for i in 0..1100 {
            fs::create_dir_all(root.join(format!("churn/t{i}"))).unwrap();
        }
        touched(&mut model, &root.join("churn"));
        // A hardlink charged in a folder made after the churn, whose record
        // compaction moves: the ledger has to move with it.
        write(&root.join("late/x/f"), 3_000);
        link(&root.join("late/x/f"), &root.join("keep/deep/er/l"));
        touched(&mut model, &root.join("late"));
        touched(&mut model, &root.join("keep/deep"));
        fs::remove_dir_all(root.join("churn")).unwrap();
        fs::remove_dir_all(root.join("gone-later")).unwrap();
        write(&root.join("keep/f"), 600);
        touched(&mut model, &root.join("churn"));
        touched(&mut model, &root.join("keep/f"));
        let before = (model.totals(), rows(&model));
        let records = model.records();

        model.compact_if_worth_it();
        model.aggregate();

        assert!(
            model.records() < records / 2,
            "{} of {records}",
            model.records()
        );
        assert_eq!((model.totals(), rows(&model)), before);
        // A baseline folder that went away is still reported as gone.
        assert!(before
            .1
            .iter()
            .any(|r| r.0 == "gone-later" && r.1 == MoverKind::Removed));
        // And the records that moved still resolve by path, the hardlink's
        // included.
        assert_agrees_with_a_fresh_scan(&model, &opts());
        write(&root.join("late/x/f"), 5_000);
        touched(&mut model, &root.join("late/x/f"));
        touched(&mut model, &root.join("keep/f"));
        assert_agrees_with_a_fresh_scan(&model, &opts());
        assert!(model
            .charged_dirs()
            .values()
            .any(|dir| *dir == root.join("late/x")));
    }

    #[test]
    fn a_rate_is_bytes_per_second_over_the_window() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().canonicalize().unwrap();
        write(&root.join("a/f"), 0);
        let opened = Instant::now();
        let (mut model, _) = start_at(root, opts(), opened);

        write(&root.join("a/f"), 20_000);
        touched(&mut model, &root.join("a/f"));
        let a = model.locate(&root.join("a"), true).unwrap();

        // Measured from the moment the model was built: twenty seconds on,
        // 20,000 bytes are 1000/s exactly.
        let rate = model.rate(a, opened + Duration::from_secs(20));
        assert_eq!(rate, 1_000.0);
        // Two windows with no growth and the rate is back to nothing.
        let later = opened + RATE_WINDOW * 3;
        model.roll_marks(later);
        model.roll_marks(later + RATE_WINDOW);
        assert_eq!(model.rate(a, later + RATE_WINDOW * 2), 0.0);
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_folder_is_counted_once_however_often_it_is_listed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().canonicalize().unwrap();
        write(&root.join("a/locked/secret"), 100);
        write(&root.join("a/open"), 100);
        fs::set_permissions(root.join("a/locked"), fs::Permissions::from_mode(0o000)).unwrap();
        // Root reads anything; the test has nothing to say then.
        if fs::read_dir(root.join("a/locked")).is_ok() {
            fs::set_permissions(root.join("a/locked"), fs::Permissions::from_mode(0o755)).unwrap();
            return;
        }
        let (mut model, _) = start(root, opts());
        let at_start = model.errors().0;

        write(&root.join("a/open"), 200);
        touched(&mut model, &root.join("a/open"));
        touched(&mut model, &root.join("a/locked/secret"));
        let after = model.errors().0;
        fs::set_permissions(root.join("a/locked"), fs::Permissions::from_mode(0o755)).unwrap();

        assert!(at_start >= 1, "the scan reports it (invariant 7)");
        assert_eq!(
            after, at_start,
            "listed twice more, still one unreadable folder"
        );
    }

    #[test]
    #[ignore]
    fn tmp_bench_wide_relist() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().canonicalize().unwrap();
        for i in 0..40_000 {
            fs::create_dir(root.join(format!("d{i}"))).unwrap();
        }
        let (mut model, _) = start(root, opts());
        write(&root.join("f"), 10);
        let t = Instant::now();
        model.relist(ROOT).unwrap();
        eprintln!("TMPBENCH relist of 40000 subfolders: {:?}", t.elapsed());
        fs::create_dir(root.join("zz-new")).unwrap();
        for i in 0..std::env::var("TMPN").map_or(20_000, |v| v.parse().unwrap()) {
            fs::create_dir(root.join(format!("n{i}"))).unwrap();
        }
        let t = Instant::now();
        model.relist(ROOT).unwrap();
        eprintln!("TMPBENCH relist adding 20001 subfolders: {:?}", t.elapsed());
    }
}
