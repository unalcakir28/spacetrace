//! Parallel filesystem scanner producing a compact arena tree.
//!
//! The tree is laid out in BFS order so that every node's children occupy a
//! contiguous index range. That keeps per-node overhead low (no `Vec` per
//! node), makes aggregation a single reverse pass, and gives cache-friendly
//! traversal for the treemap layout.

mod capacity;
mod meta;
mod scan;
mod tree;

pub use capacity::{capacity_of, Capacity};
pub use meta::{EntryKind, FileIdentity, RawMeta};
pub use scan::{scan, Phase, ScanOptions, ScanProgress, ScanStats};
pub use tree::{Node, NodeId, Removed, SizeBasis, StoredNode, Tree, TreeAssembler, TreeError};
