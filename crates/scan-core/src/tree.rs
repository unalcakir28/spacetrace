use std::collections::VecDeque;
use std::path::PathBuf;

use crate::meta::EntryKind;

pub type NodeId = u32;

pub const ROOT: NodeId = 0;
const NO_PARENT: NodeId = NodeId::MAX;

/// Which of the two measurements a caller wants to rank, lay out or total by.
///
/// Both are always recorded and neither is an approximation of the other, so
/// this is a question about the question being asked, not about accuracy:
///
/// * `Logical` answers "how many bytes are in these files" and matches
///   `du -sb`. It is what a file claims when asked its length.
/// * `OnDisk` answers "how much of the filesystem is this using" and matches
///   `du`. It is the only one that can be added up against `df`.
///
/// The two diverge in both directions and both are correct: a sparse file
/// claims a length it never allocated (a 1 TiB VM image holding 19 GiB), and a
/// tiny file allocates a whole block whatever its length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SizeBasis {
    /// File bytes only. The historical default, so it stays the default here.
    #[default]
    Logical,
    /// Allocated blocks, including the blocks directories themselves occupy.
    OnDisk,
}

/// One entry in the arena. Children of a node occupy the contiguous index
/// range `children_start .. children_start + children_len`.
///
/// The name is **not** here. It lives in the tree's shared name arena, and a
/// node only points into it — see [`Tree::name`]. A `String` per node cost 24
/// bytes inline plus its own heap allocation, and on a real disk the names come
/// to 8.6 MB of text held in 13.2 MB of allocations, one `malloc` header at a
/// time. The counters are sized to what a filesystem can actually hold rather
/// than to `u64` out of habit; together the two changes took this struct from
/// 104 bytes to 72.
#[derive(Debug, Clone)]
pub struct Node {
    pub parent: NodeId,
    /// Byte range of this node's name inside the tree's name arena.
    name_off: u32,
    name_len: u16,
    pub kind: EntryKind,
    /// Logical size of this node's whole subtree (own size for files).
    pub size: u64,
    /// Allocated size of this node's whole subtree.
    pub alloc: u64,
    /// Size of the entry itself, excluding children.
    pub own_size: u64,
    pub own_alloc: u64,
    pub mtime: i64,
    pub nlink: u32,
    /// Number of files in this subtree (a file counts itself).
    pub files: u32,
    /// Number of directories in this subtree, excluding itself.
    pub dirs: u32,
    pub children_start: NodeId,
    pub children_len: u32,
}

impl Node {
    pub fn is_dir(&self) -> bool {
        self.kind == EntryKind::Dir
    }

    pub fn has_parent(&self) -> bool {
        self.parent != NO_PARENT
    }

    /// This subtree's total under the given measure.
    pub fn measure(&self, basis: SizeBasis) -> u64 {
        match basis {
            SizeBasis::Logical => self.size,
            SizeBasis::OnDisk => self.alloc,
        }
    }
}

/// A scanned filesystem subtree.
#[derive(Debug, Clone)]
pub struct Tree {
    nodes: Vec<Node>,
    /// Every node's name, concatenated. Nodes hold offsets into this.
    names: String,
    /// Absolute path the scan started from.
    root_path: PathBuf,
}

impl Tree {
    pub(crate) fn new(nodes: Vec<Node>, names: String, root_path: PathBuf) -> Self {
        debug_assert!(!nodes.is_empty(), "a tree always has at least a root");
        Tree {
            nodes,
            names,
            root_path,
        }
    }

    /// This node's name.
    ///
    /// Offsets cannot be wrong: the only two places that write them —
    /// `TreeBuilder` and [`TreeAssembler`] — take a `&str` and intern it here,
    /// so a caller never gets to invent a range. The empty-string fallback is
    /// therefore unreachable, and exists only so that a lookup can never panic:
    /// on a disk tool, a nameless row beats a crash.
    pub fn name(&self, id: NodeId) -> &str {
        let n = self.node(id);
        let start = n.name_off as usize;
        self.names
            .get(start..start + n.name_len as usize)
            .unwrap_or_default()
    }

    /// The backing name arena, for a writer that stores the tree as-is.
    pub fn names(&self) -> &str {
        &self.names
    }

    /// Verify the arena invariants on a tree that is not trusted.
    ///
    /// A snapshot can arrive from another machine (`spacetrace --remote`, or an
    /// agent receiving a push), and the layout is not self-describing: a bad
    /// `children_start` indexes out of bounds, and a child pointing backwards
    /// turns every traversal into an infinite loop. Both are cheap to rule out
    /// in one linear pass, and doing it here means every consumer is covered.
    ///
    /// Reached through [`TreeAssembler::finish`], which is the only way to build
    /// a tree from stored rows.
    fn check(nodes: &[Node]) -> Result<(), TreeError> {
        let len = nodes.len();
        if len == 0 {
            return Err(TreeError::Empty);
        }
        if len > NodeId::MAX as usize {
            return Err(TreeError::TooLarge(len));
        }
        if nodes[ROOT as usize].has_parent() {
            return Err(TreeError::RootHasParent);
        }

        for (index, node) in nodes.iter().enumerate() {
            let id = index as NodeId;

            // Children must sit in bounds...
            let end = (node.children_start as u64) + (node.children_len as u64);
            if node.children_len > 0 {
                if end > len as u64 {
                    return Err(TreeError::ChildrenOutOfBounds {
                        node: id,
                        start: node.children_start,
                        len: node.children_len,
                        total: len,
                    });
                }
                // ...and strictly after their parent. This single check is what
                // makes cycles impossible: indices only ever increase downward.
                if node.children_start <= id {
                    return Err(TreeError::ChildrenNotAfterParent {
                        node: id,
                        start: node.children_start,
                    });
                }
            }

            if node.has_parent() && node.parent >= id {
                return Err(TreeError::ParentNotBeforeChild {
                    node: id,
                    parent: node.parent,
                });
            }
        }

        Ok(())
    }

    /// The sentinel stored in `Node::parent` for the root.
    pub const NO_PARENT: NodeId = NO_PARENT;

    pub fn root(&self) -> NodeId {
        ROOT
    }

    pub fn root_path(&self) -> &std::path::Path {
        &self.root_path
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id as usize]
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    pub fn children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let n = self.node(id);
        let start = n.children_start;
        (0..n.children_len).map(move |i| start + i)
    }

    /// Total logical bytes below (and including) the root.
    pub fn total_size(&self) -> u64 {
        self.node(ROOT).size
    }

    /// Total allocated bytes below (and including) the root.
    pub fn total_alloc(&self) -> u64 {
        self.node(ROOT).alloc
    }

    /// Reconstruct the absolute path of a node by walking up to the root.
    pub fn path(&self, id: NodeId) -> PathBuf {
        let mut parts: Vec<&str> = Vec::new();
        let mut cur = id;
        while cur != ROOT {
            parts.push(self.name(cur));
            cur = self.node(cur).parent;
        }
        let mut p = self.root_path.clone();
        for part in parts.iter().rev() {
            p.push(part);
        }
        p
    }

    /// Path relative to the scan root, using `/` separators. The root itself is
    /// the empty string. This is the key used to match nodes across snapshots.
    pub fn rel_path(&self, id: NodeId) -> String {
        let mut parts: Vec<&str> = Vec::new();
        let mut cur = id;
        while cur != ROOT {
            parts.push(self.name(cur));
            cur = self.node(cur).parent;
        }
        parts.reverse();
        parts.join("/")
    }

    /// Depth-first iteration over every node, root first.
    pub fn iter(&self) -> impl Iterator<Item = NodeId> + '_ {
        0..self.nodes.len() as NodeId
    }

    /// The `n` largest entries of the given kind, biggest first.
    pub fn largest(&self, n: usize, kind: Option<EntryKind>) -> Vec<NodeId> {
        let mut ids: Vec<NodeId> = self
            .iter()
            .filter(|&id| id != ROOT)
            .filter(|&id| kind.is_none_or(|k| self.node(id).kind == k))
            .collect();
        ids.sort_unstable_by_key(|&id| std::cmp::Reverse(self.node(id).size));
        ids.truncate(n);
        ids
    }

    /// Children of a node, biggest first under the given measure.
    ///
    /// The measure is a parameter rather than always logical size because the
    /// order changes: a sparse file that claims 1 TiB and holds 19 GiB belongs
    /// at the top of one ordering and well down the other, and a list that
    /// says "biggest first" has to mean the same thing as the figures beside it.
    pub fn children_by(&self, id: NodeId, basis: SizeBasis) -> Vec<NodeId> {
        let mut kids: Vec<NodeId> = self.children(id).collect();
        kids.sort_unstable_by_key(|&c| std::cmp::Reverse(self.node(c).measure(basis)));
        kids
    }

    /// Record that an entry is gone, and correct every total above it.
    ///
    /// This edits the tree in memory only; whatever happened on disk is the
    /// caller's business. It exists so that deleting one file does not cost a
    /// rescan: a fresh scan renumbers every node, which means a UI holding node
    /// ids has to throw away everything it knows — which folders were expanded,
    /// what was selected, where the user was — to answer a question whose answer
    /// is already known.
    ///
    /// The subtree is zeroed in place rather than spliced out, because the
    /// arena's layout is exactly what makes an id meaningful: children occupy a
    /// contiguous range after their parent, so cutting entries out of the middle
    /// would renumber everything after them and reintroduce the problem this is
    /// avoiding. The entries stay addressable and report zero bytes, and the
    /// parent no longer descends into them. The returned ids are the ones the
    /// caller should stop listing.
    ///
    /// Returns `None` for the root, which cannot be removed from its own tree,
    /// and for an id that does not exist.
    pub fn remove_subtree(&mut self, id: NodeId) -> Option<Removed> {
        if id == ROOT || id as usize >= self.nodes.len() {
            return None;
        }

        let mut nodes = Vec::new();
        let mut stack = vec![id];
        while let Some(current) = stack.pop() {
            nodes.push(current);
            stack.extend(self.children(current));
        }

        let entry = self.node(id);
        let removed = Removed {
            size: entry.size,
            alloc: entry.alloc,
            files: entry.files,
            // `dirs` excludes the node itself, so a removed directory takes one
            // more away from its ancestors than it counted for itself.
            dirs: entry.dirs + u32::from(entry.is_dir()),
            nodes,
        };

        let mut ancestor = entry.parent;
        while ancestor != NO_PARENT {
            let node = &mut self.nodes[ancestor as usize];
            // Saturating, not wrapping: the totals in a snapshot loaded from
            // disk are only as consistent as the file, and a corrupt one must
            // not panic here.
            node.size = node.size.saturating_sub(removed.size);
            node.alloc = node.alloc.saturating_sub(removed.alloc);
            node.files = node.files.saturating_sub(removed.files);
            node.dirs = node.dirs.saturating_sub(removed.dirs);
            ancestor = node.parent;
        }

        for &gone in &removed.nodes {
            let node = &mut self.nodes[gone as usize];
            node.size = 0;
            node.alloc = 0;
            node.own_size = 0;
            node.own_alloc = 0;
            node.files = 0;
            node.dirs = 0;
            // Nothing descends into it any more, so the entries below are
            // unreachable and only the entry itself needs hiding.
            node.children_len = 0;
        }

        Some(removed)
    }

    /// Resolve a `/`-separated path relative to the root.
    pub fn find(&self, rel: &str) -> Option<NodeId> {
        let mut cur = ROOT;
        for part in rel.split('/').filter(|s| !s.is_empty() && *s != ".") {
            cur = self.children(cur).find(|&c| self.name(c) == part)?;
        }
        Some(cur)
    }
}

/// One entry as handed to the builder. Its `size`/`alloc` are the entry's own
/// cost; subtree totals are computed later by [`TreeBuilder::aggregate`].
pub(crate) struct NewNode<'a> {
    pub(crate) name: &'a str,
    pub(crate) kind: EntryKind,
    pub(crate) size: u64,
    pub(crate) alloc: u64,
    pub(crate) mtime: i64,
    pub(crate) nlink: u32,
}

/// Builder used by the scanner to flatten its recursive result into the arena.
pub(crate) struct TreeBuilder {
    pub(crate) nodes: Vec<Node>,
    names: String,
}

impl TreeBuilder {
    pub(crate) fn with_capacity(cap: usize) -> Self {
        TreeBuilder {
            nodes: Vec::with_capacity(cap),
            // Roughly the average name length measured on a real disk (20.9
            // bytes). Only an opening guess — being wrong costs a few
            // reallocations, being absent costs one per doubling from zero.
            names: String::with_capacity(cap * 24),
        }
    }

    pub(crate) fn push(&mut self, parent: NodeId, entry: NewNode<'_>) -> NodeId {
        let id = self.nodes.len() as NodeId;
        let (name_off, name_len) = intern(&mut self.names, entry.name);
        self.nodes.push(Node {
            parent,
            name_off,
            name_len,
            kind: entry.kind,
            size: entry.size,
            alloc: entry.alloc,
            own_size: entry.size,
            own_alloc: entry.alloc,
            mtime: entry.mtime,
            nlink: entry.nlink,
            files: u32::from(entry.kind != EntryKind::Dir),
            dirs: 0,
            children_start: 0,
            children_len: 0,
        });
        id
    }

    /// Absolute path of a node, for the passes that run before the tree exists.
    pub(crate) fn path_of(&self, id: NodeId, root: &std::path::Path) -> PathBuf {
        let mut parts: Vec<&str> = Vec::new();
        let mut cur = id;
        while cur != ROOT {
            let n = &self.nodes[cur as usize];
            let from = n.name_off as usize;
            parts.push(
                self.names
                    .get(from..from + n.name_len as usize)
                    .unwrap_or_default(),
            );
            cur = n.parent;
        }
        let mut p = root.to_path_buf();
        for part in parts.iter().rev() {
            p.push(part);
        }
        p
    }

    /// Charge nothing for this entry, leaving it listed.
    ///
    /// Must run before [`TreeBuilder::aggregate`]: afterwards the bytes have
    /// already been added to every ancestor, and zeroing a leaf would leave
    /// the totals above it claiming space nothing accounts for.
    pub(crate) fn charge_nothing(&mut self, id: NodeId) {
        let n = &mut self.nodes[id as usize];
        n.size = 0;
        n.alloc = 0;
        n.own_size = 0;
        n.own_alloc = 0;
    }

    pub(crate) fn push_root(&mut self, entry: NewNode<'_>) -> NodeId {
        self.push(NO_PARENT, entry)
    }

    /// Roll subtree totals up from the leaves. Relies on the BFS layout: every
    /// child has a higher index than its parent, so one reverse pass is enough.
    pub(crate) fn aggregate(&mut self) {
        for i in (1..self.nodes.len()).rev() {
            let (size, alloc, files, dirs, is_dir) = {
                let n = &self.nodes[i];
                (n.size, n.alloc, n.files, n.dirs, n.is_dir())
            };
            let parent = self.nodes[i].parent as usize;
            let p = &mut self.nodes[parent];
            p.size += size;
            p.alloc += alloc;
            p.files += files;
            p.dirs += dirs + u32::from(is_dir);
        }
    }

    pub(crate) fn finish(mut self, root_path: PathBuf) -> Tree {
        self.aggregate();
        Tree::new(self.nodes, self.names, root_path)
    }
}

/// One entry as an importer describes it, before any arena exists.
///
/// Foreign formats — ncdu's JSON today, whatever comes next — are nested and
/// carry only each entry's own cost. Turning that into this crate's arena
/// means laying the nodes out in BFS order and summing the subtree totals
/// upward, which is exactly what the scanner already does at the end of a
/// walk. An importer that did its own version of that would be a second
/// implementation of the arena invariants (#2) and of aggregation, free to
/// drift; `Tree::from_nested` exists so there is only ever one.
#[derive(Debug, Clone)]
pub struct ImportedNode {
    pub name: String,
    pub kind: EntryKind,
    /// This entry's **own** logical bytes, not its subtree's.
    pub size: u64,
    /// This entry's **own** allocated bytes. For a directory that is the cost
    /// of the directory itself, the way `du` charges it.
    pub alloc: u64,
    pub mtime: i64,
    pub nlink: u32,
    pub children: Vec<ImportedNode>,
}

impl ImportedNode {
    /// A leaf with nothing but a name and a size.
    pub fn file(name: impl Into<String>, size: u64, alloc: u64) -> Self {
        ImportedNode {
            name: name.into(),
            kind: EntryKind::File,
            size,
            alloc,
            mtime: 0,
            nlink: 1,
            children: Vec::new(),
        }
    }

    /// An empty directory.
    pub fn dir(name: impl Into<String>) -> Self {
        ImportedNode {
            name: name.into(),
            kind: EntryKind::Dir,
            size: 0,
            alloc: 0,
            mtime: 0,
            nlink: 1,
            children: Vec::new(),
        }
    }

    fn count(&self) -> usize {
        1 + self.children.iter().map(ImportedNode::count).sum::<usize>()
    }
}

impl Tree {
    /// Build a tree from a nested description, through the same layout and
    /// aggregation the scanner uses.
    ///
    /// Children are laid out breadth-first and contiguously, so the arena
    /// invariants hold by construction rather than by an importer remembering
    /// them.
    pub fn from_nested(root_path: PathBuf, root: ImportedNode) -> Tree {
        let mut builder = TreeBuilder::with_capacity(root.count());
        // `NO_PARENT`, not `0`. A root that names itself as its parent looks
        // harmless — every path walk here stops at id 0 by index — but two
        // other readers use the sentinel instead, and both break: `save`
        // writes a non-null `parent_id` for entry 0, and loading that back
        // fails the structural check outright ("entry 0 is not a root: it
        // claims a parent"), so an imported scan could be stored and never
        // read again. `remove_subtree` walks ancestors to the sentinel too,
        // and would have spun forever on entry 0.
        let root_id = builder.push(
            Tree::NO_PARENT,
            NewNode {
                name: &root.name,
                kind: root.kind,
                size: root.size,
                alloc: root.alloc,
                mtime: root.mtime,
                nlink: root.nlink,
            },
        );

        // Breadth-first with an explicit queue: a recursive descent would lay
        // the children out depth-first and break invariant #2, and it would
        // also blow the stack on a deep tree read off an untrusted file.
        let mut queue: VecDeque<(NodeId, Vec<ImportedNode>)> = VecDeque::new();
        queue.push_back((root_id, root.children));
        while let Some((parent, children)) = queue.pop_front() {
            if children.is_empty() {
                continue;
            }
            // Recorded before pushing anything: the children of one parent go
            // in as one uninterrupted run, and that run's start and length are
            // what every later reader addresses them by. `push` does not do
            // this — the scanner fills these in during its own flatten pass —
            // and leaving them at zero produces a tree whose totals are right
            // and whose every `children()` call is empty.
            let start = builder.nodes.len() as NodeId;
            let len = children.len() as u32;
            for child in children {
                let id = builder.push(
                    parent,
                    NewNode {
                        name: &child.name,
                        kind: child.kind,
                        size: child.size,
                        alloc: child.alloc,
                        mtime: child.mtime,
                        nlink: child.nlink,
                    },
                );
                if !child.children.is_empty() {
                    queue.push_back((id, child.children));
                }
            }
            let p = &mut builder.nodes[parent as usize];
            p.children_start = start;
            p.children_len = len;
        }

        builder.finish(root_path)
    }
}

/// Append `name` to `arena` and return the range that addresses it.
///
/// A name longer than `u16::MAX` cannot be pointed at, and is truncated on a
/// character boundary rather than rejected: no filesystem produces one (255
/// bytes is the usual ceiling and `PATH_MAX` is 4096, so only the root's
/// display name comes anywhere near), and losing a scan over an unnameable
/// entry would be a worse answer than a shortened label.
fn intern(arena: &mut String, name: &str) -> (u32, u16) {
    let mut name = name;
    if name.len() > u16::MAX as usize {
        let mut end = u16::MAX as usize;
        while end > 0 && !name.is_char_boundary(end) {
            end -= 1;
        }
        name = &name[..end];
    }
    let off = arena.len() as u32;
    arena.push_str(name);
    (off, name.len() as u16)
}

/// One stored row on its way back into a tree.
///
/// Separate from [`Node`] because a node addresses its name by offset, and an
/// offset is only meaningful next to the arena it points into. Handing the
/// caller a `&str` and interning it here means a wrong offset cannot be
/// constructed at all.
pub struct StoredNode<'a> {
    pub parent: NodeId,
    pub name: &'a str,
    pub kind: EntryKind,
    pub size: u64,
    pub alloc: u64,
    pub own_size: u64,
    pub own_alloc: u64,
    pub mtime: i64,
    pub nlink: u32,
    pub files: u32,
    pub dirs: u32,
    pub children_start: NodeId,
    pub children_len: u32,
}

/// Rebuilds a tree from stored rows — a snapshot on disk, or one pulled from
/// another machine.
///
/// Totals are taken as already aggregated: a snapshot stores what the scan
/// computed, and recomputing it would hide a corrupt file rather than reveal
/// it. Structure is not taken on trust, though; [`TreeAssembler::finish`] is
/// the boundary every loaded tree passes through.
pub struct TreeAssembler {
    nodes: Vec<Node>,
    names: String,
}

impl TreeAssembler {
    pub fn with_capacity(nodes: usize) -> Self {
        TreeAssembler {
            nodes: Vec::with_capacity(nodes),
            names: String::with_capacity(nodes * 24),
        }
    }

    pub fn push(&mut self, row: StoredNode<'_>) {
        let (name_off, name_len) = intern(&mut self.names, row.name);
        self.nodes.push(Node {
            parent: row.parent,
            name_off,
            name_len,
            kind: row.kind,
            size: row.size,
            alloc: row.alloc,
            own_size: row.own_size,
            own_alloc: row.own_alloc,
            mtime: row.mtime,
            nlink: row.nlink,
            files: row.files,
            dirs: row.dirs,
            children_start: row.children_start,
            children_len: row.children_len,
        });
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Check the arena invariants and hand back the tree.
    pub fn finish(self, root_path: PathBuf) -> Result<Tree, TreeError> {
        Tree::check(&self.nodes)?;
        Ok(Tree::new(self.nodes, self.names, root_path))
    }
}

/// What an entry accounted for before [`Tree::remove_subtree`] took it out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removed {
    pub size: u64,
    pub alloc: u64,
    pub files: u32,
    /// Directories that went with it, counting the entry itself when it was one.
    pub dirs: u32,
    /// Every id that is now gone, the entry itself first.
    pub nodes: Vec<NodeId>,
}

/// Why a set of stored nodes could not be trusted as a tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeError {
    Empty,
    TooLarge(usize),
    RootHasParent,
    ChildrenOutOfBounds {
        node: NodeId,
        start: NodeId,
        len: NodeId,
        total: usize,
    },
    ChildrenNotAfterParent {
        node: NodeId,
        start: NodeId,
    },
    ParentNotBeforeChild {
        node: NodeId,
        parent: NodeId,
    },
}

impl std::fmt::Display for TreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TreeError::Empty => write!(f, "the snapshot has no entries"),
            TreeError::TooLarge(n) => write!(
                f,
                "the snapshot has {n} entries, more than the arena can address"
            ),
            TreeError::RootHasParent => write!(f, "entry 0 is not a root: it claims a parent"),
            TreeError::ChildrenOutOfBounds {
                node,
                start,
                len,
                total,
            } => write!(
                f,
                "entry {node} claims children {start}..{} but the snapshot has {total} entries",
                *start as u64 + *len as u64
            ),
            TreeError::ChildrenNotAfterParent { node, start } => write!(
                f,
                "entry {node} claims its children start at {start}, which is not after it"
            ),
            TreeError::ParentNotBeforeChild { node, parent } => write!(
                f,
                "entry {node} claims parent {parent}, which is not before it"
            ),
        }
    }
}

impl std::error::Error for TreeError {}
