use std::path::PathBuf;

use crate::meta::EntryKind;

pub type NodeId = u32;

pub const ROOT: NodeId = 0;
const NO_PARENT: NodeId = NodeId::MAX;

/// One entry in the arena. Children of a node occupy the contiguous index
/// range `children_start .. children_start + children_len`.
#[derive(Debug, Clone)]
pub struct Node {
    pub parent: NodeId,
    pub name: String,
    pub kind: EntryKind,
    /// Logical size of this node's whole subtree (own size for files).
    pub size: u64,
    /// Allocated size of this node's whole subtree.
    pub alloc: u64,
    /// Size of the entry itself, excluding children.
    pub own_size: u64,
    pub own_alloc: u64,
    pub mtime: i64,
    pub nlink: u64,
    /// Number of files in this subtree (a file counts itself).
    pub files: u64,
    /// Number of directories in this subtree, excluding itself.
    pub dirs: u64,
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
}

/// A scanned filesystem subtree.
#[derive(Debug, Clone)]
pub struct Tree {
    nodes: Vec<Node>,
    /// Absolute path the scan started from.
    root_path: PathBuf,
}

impl Tree {
    pub(crate) fn new(nodes: Vec<Node>, root_path: PathBuf) -> Self {
        debug_assert!(!nodes.is_empty(), "a tree always has at least a root");
        Tree { nodes, root_path }
    }

    /// Rebuild a tree from stored nodes, e.g. after loading a snapshot.
    /// The caller must preserve the BFS layout: a node's children occupy
    /// `children_start .. children_start + children_len`, and every child has a
    /// higher index than its parent. Totals are taken as already aggregated.
    ///
    /// Only use this for nodes you produced yourself. Anything that came off a
    /// disk or a network must go through [`Tree::from_parts_checked`] first.
    pub fn from_parts(nodes: Vec<Node>, root_path: PathBuf) -> Self {
        Tree::new(nodes, root_path)
    }

    /// Rebuild a tree from nodes that are not trusted, verifying the arena
    /// invariants before anything walks them.
    ///
    /// A snapshot can arrive from another machine (`spacetrace --remote`, or an
    /// agent receiving a push), and the layout is not self-describing: a bad
    /// `children_start` indexes out of bounds, and a child pointing backwards
    /// turns every traversal into an infinite loop. Both are cheap to rule out
    /// in one linear pass, and doing it here means every consumer is covered.
    pub fn from_parts_checked(nodes: Vec<Node>, root_path: PathBuf) -> Result<Self, TreeError> {
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

        Ok(Tree::new(nodes, root_path))
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
            let n = self.node(cur);
            parts.push(&n.name);
            cur = n.parent;
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
            let n = self.node(cur);
            parts.push(&n.name);
            cur = n.parent;
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

    /// Children of a node sorted by size, biggest first.
    pub fn children_by_size(&self, id: NodeId) -> Vec<NodeId> {
        let mut kids: Vec<NodeId> = self.children(id).collect();
        kids.sort_unstable_by_key(|&c| std::cmp::Reverse(self.node(c).size));
        kids
    }

    /// Resolve a `/`-separated path relative to the root.
    pub fn find(&self, rel: &str) -> Option<NodeId> {
        let mut cur = ROOT;
        for part in rel.split('/').filter(|s| !s.is_empty() && *s != ".") {
            cur = self.children(cur).find(|&c| self.node(c).name == part)?;
        }
        Some(cur)
    }
}

/// One entry as handed to the builder. Its `size`/`alloc` are the entry's own
/// cost; subtree totals are computed later by [`TreeBuilder::aggregate`].
pub(crate) struct NewNode {
    pub(crate) name: String,
    pub(crate) kind: EntryKind,
    pub(crate) size: u64,
    pub(crate) alloc: u64,
    pub(crate) mtime: i64,
    pub(crate) nlink: u64,
}

/// Builder used by the scanner to flatten its recursive result into the arena.
pub(crate) struct TreeBuilder {
    pub(crate) nodes: Vec<Node>,
}

impl TreeBuilder {
    pub(crate) fn with_capacity(cap: usize) -> Self {
        TreeBuilder {
            nodes: Vec::with_capacity(cap),
        }
    }

    pub(crate) fn push(&mut self, parent: NodeId, entry: NewNode) -> NodeId {
        let id = self.nodes.len() as NodeId;
        self.nodes.push(Node {
            parent,
            name: entry.name,
            kind: entry.kind,
            size: entry.size,
            alloc: entry.alloc,
            own_size: entry.size,
            own_alloc: entry.alloc,
            mtime: entry.mtime,
            nlink: entry.nlink,
            files: u64::from(entry.kind != EntryKind::Dir),
            dirs: 0,
            children_start: 0,
            children_len: 0,
        });
        id
    }

    pub(crate) fn push_root(&mut self, entry: NewNode) -> NodeId {
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
            p.dirs += dirs + u64::from(is_dir);
        }
    }

    pub(crate) fn finish(mut self, root_path: PathBuf) -> Tree {
        self.aggregate();
        Tree::new(self.nodes, root_path)
    }
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
