//! Parallel filesystem scanner producing a compact arena tree.
//!
//! Every node's children occupy a contiguous index range and every child sits
//! at a higher index than its parent. That keeps per-node overhead low (no
//! `Vec` per node), makes aggregation a single reverse pass, and gives
//! cache-friendly traversal for the treemap layout.
//!
//! Those two properties are the whole promise; the order is not breadth-first
//! and is not stable between scans. The walk writes each directory into the
//! arena as soon as it has been listed, so a node id identifies an entry
//! within one tree and nothing beyond it — across snapshots the path is the
//! identity.

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
