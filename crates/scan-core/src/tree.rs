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
    ///
    /// **One node's worth of work, and it walks to the root to do it.** That is
    /// the right shape for answering a question about one entry and the wrong
    /// one for every entry: over a whole tree it repeats each ancestor's name
    /// once per descendant, and its cost is the sum of every node's depth
    /// rather than the size of the tree. Worse, depth is not bounded by
    /// anything this crate controls — a snapshot arriving from another machine
    /// can be as deep as it likes — so a loop over this is quadratic in the
    /// worst case on input the tool does not choose.
    ///
    /// Use [`Tree::for_each_path`] when the answer is wanted for more than a
    /// handful of entries.
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

    /// Visit every node with its path relative to the root, depth first.
    ///
    /// The path is built on the way **down**: descending into a directory
    /// appends one segment to a buffer and leaving it cuts the segment off
    /// again, so each name is written once no matter how many entries sit
    /// below it. [`Tree::rel_path`] climbs to the root for every entry
    /// instead, which costs the sum of every node's depth and allocates twice
    /// per call; this allocates nothing per node and hands out a borrow of the
    /// buffer.
    ///
    /// The root is visited first, with the empty string. Children are visited
    /// in arena order — the order the directory listed them in — and a node's
    /// whole subtree is finished before its next sibling begins.
    ///
    /// `max_depth` counts the root as 0 and stops the descent rather than
    /// filtering what it produced, so a shallow limit does not pay for the
    /// depths it discards.
    ///
    /// **Iterative, not recursive**, for the reason `from_nested` is: the
    /// depth here comes from the tree, a tree can be loaded from a file this
    /// crate did not write, and recursion would turn a deep one into a stack
    /// overflow.
    pub fn for_each_path(&self, max_depth: Option<usize>, mut visit: impl FnMut(NodeId, &str)) {
        visit(ROOT, "");
        if max_depth == Some(0) {
            return;
        }

        let mut path = String::new();
        // Each frame is the children still to visit under one directory, and
        // the length to cut the buffer back to once they are done.
        let mut stack: Vec<(std::ops::Range<NodeId>, usize)> = Vec::new();
        let root = self.node(ROOT);
        if root.children_len > 0 {
            stack.push((
                root.children_start..root.children_start + root.children_len,
                0,
            ));
        }
        // Depth is the stack's own height, so a limit is a refusal to push
        // rather than a test on every entry. A report of the top three levels
        // of a ten-million-entry tree then costs the top three levels.
        let can_descend = |stack: &Vec<(std::ops::Range<NodeId>, usize)>| {
            max_depth.is_none_or(|m| stack.len() < m)
        };

        while let Some((range, cut)) = stack.last_mut() {
            let cut = *cut;
            let Some(id) = range.next() else {
                path.truncate(cut);
                stack.pop();
                continue;
            };

            let mark = path.len();
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(self.name(id));
            visit(id, &path);

            let node = self.node(id);
            if node.children_len > 0 && can_descend(&stack) {
                stack.push((
                    node.children_start..node.children_start + node.children_len,
                    mark,
                ));
            } else {
                path.truncate(mark);
            }
        }
    }

    /// Every node, root first.
    ///
    /// Arena order, which is neither depth-first nor breadth-first — it is the
    /// order the walk happened to finish directories in. A parent always
    /// precedes its children, and that is the only ordering to rely on; for a
    /// particular traversal use [`Tree::children`] from the root.
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

/// Builds the arena, one directory's worth of children at a time.
///
/// Every node except the root arrives through [`TreeBuilder::push_block`],
/// which is what makes the arena invariants structural rather than remembered:
/// a caller cannot push a child without saying whose child it is, and cannot
/// push a directory's children in two pieces.
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

    fn push_one(&mut self, parent: NodeId, entry: NewNode<'_>) -> NodeId {
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

    /// Add one directory's children as a single uninterrupted run, and point
    /// the parent at it. Returns the first child's id; child `i` is `start + i`.
    ///
    /// **This is the only way to add a child, and that is the point.** The two
    /// arena invariants — children contiguous, every child after its parent —
    /// are properties of *how nodes are added*, not things a reader can check
    /// cheaply at the far end. Writing `children_start` and `children_len` here
    /// rather than at the call site is what stops them being forgotten: the
    /// ncdu importer forgot exactly that (C8) and produced a tree whose totals
    /// were right and whose every `children()` call was empty.
    ///
    /// **The order blocks arrive in does not matter.** A parent is always
    /// already in the arena when its children are added — it has to be, to be
    /// named here — so a child's index exceeds its parent's whatever order the
    /// directories finish in. That is what lets the walk write straight into
    /// the arena from whichever thread listed a directory first, instead of
    /// building a second tree in breadth-first order and copying it across.
    ///
    /// The length is counted from what was actually pushed rather than taken
    /// from the iterator, so an `ExactSizeIterator` that lies produces a short
    /// block rather than a `children_len` pointing past it.
    pub(crate) fn push_block<'a>(
        &mut self,
        parent: NodeId,
        children: impl ExactSizeIterator<Item = NewNode<'a>>,
    ) -> NodeId {
        let start = self.nodes.len() as NodeId;
        self.nodes.reserve(children.len());
        for entry in children {
            self.push_one(parent, entry);
        }
        let len = self.nodes.len() as NodeId - start;
        // An empty block leaves the parent alone: `children_start` stays 0,
        // which is what an unreadable or empty directory has always stored and
        // what `Tree::check` accepts only while `children_len` is 0 too.
        if len == 0 {
            return start;
        }
        let p = &mut self.nodes[parent as usize];
        p.children_start = start;
        p.children_len = len;
        start
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
        debug_assert!(
            self.nodes.is_empty(),
            "the root is entry 0 or the arena has no root at all"
        );
        self.push_one(NO_PARENT, entry)
    }

    /// Roll subtree totals up from the leaves. Relies on the arena layout:
    /// every child has a higher index than its parent, so one reverse pass
    /// reaches a node only after everything below it has been added in.
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
        // A scanned tree never passes through `Tree::check` — that runs on
        // stored rows, where the bytes are not trusted. So nothing verified
        // the walk's own output, and the layout it produces is now decided by
        // the order directories happen to finish in rather than by a single
        // flatten pass. In a debug build every scan test becomes a check of
        // that layout, which is a great deal more evidence than the handful of
        // unit tests below could be on their own. Debug only: this is a linear
        // pass over the whole arena, and in release the same check still
        // guards the path that matters (`TreeAssembler::finish`).
        debug_assert_eq!(
            Tree::check(&self.nodes),
            Ok(()),
            "the walk produced an arena that loading would reject"
        );
        Tree::new(self.nodes, self.names, root_path)
    }
}

/// One entry as an importer describes it, before any arena exists.
///
/// Foreign formats — ncdu's JSON today, whatever comes next — are nested and
/// carry only each entry's own cost. Turning that into this crate's arena
/// means placing each directory's children as one contiguous run after their
/// parent and summing the subtree totals upward, which is exactly what the
/// scanner already does. An importer that did its own version of that would be a second
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
    /// Every directory's children go in as one contiguous run after their
    /// parent ([`TreeBuilder::push_block`]), so the arena invariants hold by
    /// construction rather than by an importer remembering them.
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
        let root_id = builder.push_root(NewNode {
            name: &root.name,
            kind: root.kind,
            size: root.size,
            alloc: root.alloc,
            mtime: root.mtime,
            nlink: root.nlink,
        });

        // An explicit queue rather than a recursive descent, because a nested
        // file read off disk can be arbitrarily deep and recursing on it would
        // blow the stack. The order it visits parents in is not itself a
        // requirement — `push_block` keeps the invariants whatever order
        // blocks arrive in — so this only has to be some order that reaches
        // every parent before its children.
        let mut queue: VecDeque<(NodeId, Vec<ImportedNode>)> = VecDeque::new();
        queue.push_back((root_id, root.children));
        while let Some((parent, children)) = queue.pop_front() {
            if children.is_empty() {
                continue;
            }
            let start = builder.push_block(
                parent,
                children.iter().map(|child| NewNode {
                    name: &child.name,
                    kind: child.kind,
                    size: child.size,
                    alloc: child.alloc,
                    mtime: child.mtime,
                    nlink: child.nlink,
                }),
            );
            for (index, child) in children.into_iter().enumerate() {
                if !child.children.is_empty() {
                    queue.push_back((start + index as NodeId, child.children));
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, size: u64) -> NewNode<'_> {
        NewNode {
            name,
            kind: EntryKind::File,
            size,
            alloc: size,
            mtime: 0,
            nlink: 1,
        }
    }

    fn dir(name: &str) -> NewNode<'_> {
        NewNode {
            name,
            kind: EntryKind::Dir,
            size: 0,
            alloc: 0,
            mtime: 0,
            nlink: 1,
        }
    }

    /// The claim B1-K rests on: the arena does not need breadth-first order,
    /// it needs children contiguous and every child after its parent. This
    /// pushes the blocks in the order a parallel walk produces them — one
    /// branch followed all the way down before its sibling is listed at all —
    /// and asks for everything the layout is supposed to guarantee.
    ///
    /// In breadth-first order the ids would be a=1 b=2 deep=3 x=4 leaf=5.
    /// Here they are a=1 b=2 deep=3 leaf=4 x=5, which is a layout the old
    /// flatten pass could not produce.
    #[test]
    fn blocks_may_arrive_in_any_order_the_walk_finishes_them_in() {
        let mut builder = TreeBuilder::with_capacity(6);
        let root = builder.push_root(dir("root"));

        let top = builder.push_block(root, [dir("a"), dir("b")].into_iter());
        let (a, b) = (top, top + 1);
        // `a`'s whole subtree, before `b` has been looked at.
        let deep = builder.push_block(a, [dir("deep")].into_iter());
        builder.push_block(deep, [file("leaf", 10)].into_iter());
        // Only now the sibling, so its child lands after `a`'s grandchild.
        builder.push_block(b, [file("x", 30)].into_iter());

        let tree = builder.finish(PathBuf::from("/synthetic"));

        assert_eq!(
            Tree::check(tree.nodes()),
            Ok(()),
            "a tree loaded back from this layout must be accepted"
        );
        assert_eq!(tree.name(deep + 1), "leaf", "depth-first id assignment");
        assert_eq!(tree.name(deep + 2), "x", "the sibling came last");

        // The reverse pass is the thing that depends on the ordering property,
        // so the totals are the real test of it.
        assert_eq!(tree.total_size(), 40);
        assert_eq!(tree.node(a).size, 10);
        assert_eq!(tree.node(b).size, 30);
        assert_eq!(tree.node(deep).size, 10);
        assert_eq!(tree.node(root).files, 2);
        assert_eq!(tree.node(root).dirs, 3, "a, b and deep");

        // And the structure is navigable, which is what C8's bug broke while
        // leaving every total correct.
        assert_eq!(
            tree.find("a/deep/leaf").map(|id| tree.name(id)),
            Some("leaf")
        );
        assert_eq!(tree.find("b/x").map(|id| tree.name(id)), Some("x"));
        assert_eq!(tree.children(a).count(), 1);
        assert_eq!(tree.children(b).count(), 1);
        assert_eq!(
            tree.rel_path(tree.find("a/deep/leaf").unwrap()),
            "a/deep/leaf"
        );
    }

    /// Every node's subtree total is its own cost plus its children's totals.
    /// Stated here as arithmetic rather than as expected numbers, because it
    /// has to hold for a layout nobody wrote down in advance.
    #[test]
    fn every_subtree_total_is_its_own_cost_plus_its_children() {
        let mut builder = TreeBuilder::with_capacity(8);
        let root = builder.push_root(dir("root"));
        let top = builder.push_block(root, [dir("one"), file("loose", 7)].into_iter());
        let inner = builder.push_block(top, [file("p", 3), file("q", 5)].into_iter());
        builder.push_block(inner, [].into_iter());

        let tree = builder.finish(PathBuf::from("/synthetic"));

        for id in tree.iter() {
            let node = tree.node(id);
            let children: u64 = tree.children(id).map(|c| tree.node(c).size).sum();
            assert_eq!(
                node.size,
                node.own_size + children,
                "entry {id} ({})",
                tree.name(id)
            );
        }
        assert_eq!(tree.total_size(), 15);
    }

    /// An unreadable or empty directory must look exactly like one that was
    /// never given a block: `children_start` at 0 is only legal while
    /// `children_len` is 0, and a block that wrote the start without any
    /// children would fail the structural check on load.
    #[test]
    fn an_empty_block_leaves_the_parent_childless() {
        let mut builder = TreeBuilder::with_capacity(2);
        let root = builder.push_root(dir("root"));
        let start = builder.push_block(root, [].into_iter());

        assert_eq!(start, 1, "where the block would have begun");
        let tree = builder.finish(PathBuf::from("/synthetic"));
        assert_eq!(tree.node(root).children_len, 0);
        assert_eq!(tree.node(root).children_start, 0);
        assert_eq!(tree.children(root).count(), 0);
        assert_eq!(Tree::check(tree.nodes()), Ok(()));
    }

    /// The whole point of `for_each_path` is to give the same answer more
    /// cheaply, so the test is differential: every node, both ways.
    #[test]
    fn every_path_agrees_with_the_one_rel_path_builds() {
        let mut builder = TreeBuilder::with_capacity(16);
        let root = builder.push_root(dir("root"));
        let top = builder.push_block(root, [dir("a b"), file("c,d", 1), dir("empty")].into_iter());
        let inner = builder.push_block(top, [dir("deep"), file("leaf", 2)].into_iter());
        builder.push_block(inner, [file("bottom", 3)].into_iter());
        builder.push_block(top + 2, [].into_iter());
        let tree = builder.finish(PathBuf::from("/synthetic"));

        let mut seen: Vec<(NodeId, String)> = Vec::new();
        tree.for_each_path(None, |id, path| seen.push((id, path.to_string())));

        assert_eq!(
            seen.len(),
            tree.len(),
            "every node is visited exactly once, the root included"
        );
        for (id, path) in &seen {
            assert_eq!(path, &tree.rel_path(*id), "entry {id}");
        }
        // And the names that need quoting elsewhere are carried through as they
        // are: the separator is this function's business, escaping is not.
        assert!(seen.iter().any(|(_, p)| p == "a b/deep/bottom"));
        assert!(seen.iter().any(|(_, p)| p == "c,d"));
    }

    /// Depth first, and a subtree is finished before its next sibling starts.
    /// The CSV export's row order depends on this, and a reader diffing two
    /// exports depends on the row order.
    #[test]
    fn a_subtree_is_finished_before_the_next_sibling() {
        let mut builder = TreeBuilder::with_capacity(8);
        let root = builder.push_root(dir("root"));
        let top = builder.push_block(root, [dir("first"), file("second", 1)].into_iter());
        builder.push_block(top, [file("under", 2)].into_iter());
        let tree = builder.finish(PathBuf::from("/synthetic"));

        let mut order: Vec<String> = Vec::new();
        tree.for_each_path(None, |_, path| order.push(path.to_string()));
        assert_eq!(order, vec!["", "first", "first/under", "second"]);
    }

    /// A depth limit stops the descent instead of filtering afterwards, so it
    /// has to cut at the right level and not one either side of it.
    #[test]
    fn a_depth_limit_stops_the_descent() {
        let mut builder = TreeBuilder::with_capacity(8);
        let root = builder.push_root(dir("root"));
        let top = builder.push_block(root, [dir("one"), file("flat", 1)].into_iter());
        let mid = builder.push_block(top, [dir("two")].into_iter());
        builder.push_block(mid, [file("three", 2)].into_iter());
        let tree = builder.finish(PathBuf::from("/synthetic"));

        let at = |max: Option<usize>| {
            let mut seen: Vec<String> = Vec::new();
            tree.for_each_path(max, |_, p| seen.push(p.to_string()));
            seen
        };
        assert_eq!(at(Some(0)), vec![""], "the root alone");
        assert_eq!(at(Some(1)), vec!["", "one", "flat"]);
        assert_eq!(at(Some(2)), vec!["", "one", "one/two", "flat"]);
        assert_eq!(
            at(Some(3)),
            vec!["", "one", "one/two", "one/two/three", "flat"]
        );
        assert_eq!(at(None), at(Some(3)), "no limit reaches the bottom");
        assert_eq!(
            at(Some(99)),
            at(None),
            "a limit past the bottom changes nothing"
        );
    }

    /// A snapshot can be loaded from a file this crate did not write, so the
    /// depth is not something the scanner's `PATH_MAX` bounds. Recursing would
    /// abort the process here rather than return an answer.
    #[test]
    fn a_very_deep_tree_does_not_overflow_the_stack() {
        const DEPTH: usize = 50_000;
        let mut builder = TreeBuilder::with_capacity(DEPTH + 1);
        let mut parent = builder.push_root(dir("root"));
        for _ in 0..DEPTH {
            parent = builder.push_block(parent, [dir("d")].into_iter());
        }
        let tree = builder.finish(PathBuf::from("/synthetic"));

        let mut deepest = 0usize;
        let mut count = 0usize;
        tree.for_each_path(None, |_, path| {
            count += 1;
            deepest = deepest.max(path.len());
        });
        assert_eq!(count, DEPTH + 1);
        // "d" plus a separator for every level below the first.
        assert_eq!(deepest, DEPTH * 2 - 1);
    }

    /// `children_len` is counted from what was pushed, not taken from the
    /// iterator's own claim, so a wrong `len()` cannot produce a block that
    /// points past the end of the arena.
    #[test]
    fn the_block_length_comes_from_what_was_actually_pushed() {
        struct Liar(std::vec::IntoIter<NewNode<'static>>);
        impl Iterator for Liar {
            type Item = NewNode<'static>;
            fn next(&mut self) -> Option<Self::Item> {
                self.0.next()
            }
        }
        impl ExactSizeIterator for Liar {
            fn len(&self) -> usize {
                99
            }
        }

        let mut builder = TreeBuilder::with_capacity(4);
        let root = builder.push_root(dir("root"));
        builder.push_block(root, Liar(vec![file("only", 1)].into_iter()));

        let tree = builder.finish(PathBuf::from("/synthetic"));
        assert_eq!(tree.node(root).children_len, 1);
        assert_eq!(Tree::check(tree.nodes()), Ok(()));
    }
}
