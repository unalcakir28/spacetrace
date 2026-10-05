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
mod clones;
#[cfg(target_os = "linux")]
mod dents;
mod disk;
mod extents;
#[cfg(target_os = "macos")]
mod fsevents;
mod journal;
mod meta;
mod mounts;
// Only Windows reads a volume; everywhere else the parser runs in the tests.
#[cfg_attr(not(windows), allow(dead_code))]
mod ntfs;
mod partial;
mod rescan;
mod scan;
#[cfg(all(test, unix))]
mod testing;
mod timeout;
mod tree;

pub use age::{age_profile, age_profile_at, median_bands, AgeBucket, AgeProfile, DEFAULT_EDGES};
pub use capacity::{capacity_of, Capacity};
pub use journal::{Fallback, Incremental, Rescan, RescanKind};
pub use meta::{EntryKind, FileIdentity, RawMeta};
pub use mounts::Mounts;
pub use partial::PartialTree;
pub use rescan::{Base, LoadBase};
pub use scan::{
    probe_mount, rescan, scan, scan_with_hardlinks, stored_root, Decided, DiskMode, LinkedName,
    Pace, Phase, ScanOptions, ScanProgress, ScanStats, StallWatch, MAX_CAPACITY_HINT,
    MOUNT_TIMEOUT, STALL_GRACE,
};
pub use timeout::with_deadline;
pub use tree::{
    ImportedNode, Node, NodeId, Removed, SizeBasis, StoredNode, Tree, TreeAssembler, TreeError,
};
