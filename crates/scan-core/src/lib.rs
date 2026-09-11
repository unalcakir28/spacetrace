//! Parallel filesystem scanner producing a compact arena tree.
//!
//! The tree is laid out in BFS order so that every node's children occupy a
//! contiguous index range. That keeps per-node overhead low (no `Vec` per
//! node), makes aggregation a single reverse pass, and gives cache-friendly
//! traversal for the treemap layout.

mod age;
#[cfg(target_os = "macos")]
mod bulk;
mod capacity;
mod live;
mod meta;
mod mounts;
mod scan;
mod timeout;
mod tree;

pub use age::{age_profile, age_profile_at, median_bands, AgeBucket, AgeProfile, DEFAULT_EDGES};
pub use capacity::{capacity_of, Capacity};
pub use live::{LiveEntry, LiveTree};
pub use meta::{EntryKind, FileIdentity, RawMeta};
pub use mounts::Mounts;
pub use scan::{
    scan, Phase, ScanOptions, ScanProgress, ScanStats, StallWatch, MOUNT_TIMEOUT, STALL_GRACE,
};
pub use tree::{
    ImportedNode, Node, NodeId, Removed, SizeBasis, StoredNode, Tree, TreeAssembler, TreeError,
};
