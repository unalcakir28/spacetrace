//! Parallel filesystem scanner producing a compact arena tree.
//!
//! The tree is laid out in BFS order so that every node's children occupy a
//! contiguous index range. That keeps per-node overhead low (no `Vec` per
//! node), makes aggregation a single reverse pass, and gives cache-friendly
//! traversal for the treemap layout.

mod meta;
mod scan;
mod tree;

pub use meta::{EntryKind, RawMeta};
pub use scan::{scan, ScanOptions, ScanProgress, ScanStats};
pub use tree::{Node, NodeId, Tree};
