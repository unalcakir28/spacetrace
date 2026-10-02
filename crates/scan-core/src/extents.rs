//! Shared and compressed extents on Linux: what `st_blocks` cannot say.
//!
//! `st_blocks` is per file. On a filesystem that shares blocks between files
//! — a reflink copy on btrfs or XFS, a btrfs snapshot — every name reports the
//! shared blocks as its own, so a walk that adds them up counts them once per
//! name. That is what `du` does, and it is why `du` over-counts there while
//! `df` does not. Measured in Docker on both filesystems: three reflinked
//! copies of a 100 MB file cost **0 bytes** of `df` beyond the first, while
//! `du` reports 300 MB.
//!
//! `FS_IOC_FIEMAP` closes that gap without privileges. It lists a file's
//! extents with their physical address and flags `FIEMAP_EXTENT_SHARED` on the
//! ones another file also references. Charging each physical byte range once
//! across the scan makes `alloc` what the disk holds for everything shared
//! *inside* the scanned tree. Extents split differently in two files — one
//! copy partially overwritten, so its old extent is now referenced in two
//! pieces — are why the claim is by byte range rather than by extent: see
//! [`Ranges`].
//!
//! **Which name is charged is decided after the walk, in `(depth, path)`
//! order** ([`Deferred`]): the shallowest name wins, so a tree with its
//! snapshots inside it charges `/usr` and not `/.snapshots/12/snapshot/usr`,
//! and two scans of the same tree charge the same names. Charged as the walk
//! met them, the bytes would move between those two names from one scan to
//! the next with nothing on disk changing, and a diff would report it as
//! growth.
//!
//! **btrfs compression is the second gap, and FIEMAP only half closes it.**
//! `st_blocks` reports a compressed extent at its *uncompressed* length.
//! Measured: a 100 MB log file under `compress=zstd` reports 100,003,840 bytes
//! in `st_blocks` while `compsize` says 2.9 MiB on disk and `df` moved by
//! 3.4 MB. FIEMAP marks such extents `FIEMAP_EXTENT_ENCODED` but reports the
//! *logical* length, and the start of the compressed data as the physical
//! address — so two neighbouring compressed extents appear to overlap by
//! 124 KiB, and a byte-range claim on them would be wrong. The compressed
//! length is only in the file extent item, which `BTRFS_IOC_TREE_SEARCH_V2`
//! reads and only with `CAP_SYS_ADMIN`. So:
//!
//! * **with it** (the agent, usually), each compressed extent is charged its
//!   on-disk length, once, keyed by its disk address — the number `compsize`
//!   calls "Disk Usage". One the file shares is claimed across the scan like
//!   any shared range; one it does not share is deduplicated within the file
//!   only, because a partial overwrite can leave the same file referencing it
//!   twice, and no other file can;
//! * **without it**, a compressed extent is left exactly as `st_blocks`
//!   counts it, uncompressed and once per name, and the file is counted in
//!   [`ScanStats::compressed_files_inexact`](crate::ScanStats) so the summary
//!   can say the total is high and why. Deduplicating those by address without
//!   knowing their size could charge a 4 KiB reference for a 124 KiB extent,
//!   and a total that is quietly *low* is the one mistake this scanner must
//!   not make.
//!
//! The scan-wide claim sets hold one entry per **shared** extent (merged into
//! runs where they are contiguous) and nothing for a file that shares
//! nothing: that file is charged its `st_blocks`, or its compressed size,
//! during the walk and never comes back.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::sync::Mutex;

use crate::tree::{NodeId, TreeBuilder};

/// One file's blocks, where they differ from what `st_blocks` says.
///
/// Built while the directory is listed. What only this file holds is settled
/// at once ([`Mapped::own_alloc`]); what it shares waits in [`Deferred`].
// Only the Linux listing builds one; on every other platform the type exists
// so the walk is not conditional, and the tests below build them by hand.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Mapped {
    /// Which address space the physical numbers below belong to. See
    /// [`Volumes`].
    pub domain: u32,
    /// What `st_blocks` counted for the extents described below, so it can be
    /// taken out again before they are charged their own way.
    pub stat_bytes: u64,
    /// Compressed extents only this file holds, at their on-disk length,
    /// each counted once however many times the file references it.
    pub own_bytes: u64,
    /// Shared extents, as physical byte ranges `[start, end)`.
    pub ranges: Vec<(u64, u64)>,
    /// Shared compressed extents whose on-disk length is known: `(disk
    /// address, on-disk bytes)`. Keyed by identity, never by range, because
    /// their physical span on disk is not what FIEMAP's length says.
    pub blobs: Vec<(u64, u64)>,
    /// What `st_blocks` counted for the compressed extents minus what they
    /// are charged on disk, before any sharing. **Signed**: a compressed
    /// extent the file now references only a sliver of is still held whole
    /// on disk, so it can cost more than `du` says — `compsize` agrees.
    pub compressed_saved: i64,
    /// The file has compressed extents whose on-disk length could not be
    /// read, so they stay at their uncompressed length.
    pub compressed_inexact: bool,
}

impl Mapped {
    /// Whether this file waits for the scan-wide claim after the walk.
    pub fn claims_anything(&self) -> bool {
        !self.ranges.is_empty() || !self.blobs.is_empty()
    }

    /// What the file costs before its shared extents are charged: what
    /// `st_blocks` said, with the described extents taken out and the ones
    /// only it holds put back at their real size.
    ///
    /// Saturating: `st_blocks` and FIEMAP are two reads of a live file, and
    /// one that grew in between must still produce a number rather than a
    /// panic in the walk.
    pub fn own_alloc(&self, stat_alloc: u64) -> u64 {
        stat_alloc
            .saturating_sub(self.stat_bytes)
            .saturating_add(self.own_bytes)
    }
}

/// What one file's shared extents came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Charged {
    /// Bytes nobody had been charged for yet, now charged to this file.
    pub new: u64,
    /// Bytes some name earlier in the order already carries — the part of
    /// `du`'s excess that is sharing.
    pub shared: u64,
}

/// Every physical extent some name in the scan has already been charged for.
#[derive(Debug, Default)]
pub(crate) struct Claims {
    ranges: HashMap<u32, Ranges>,
    blobs: HashSet<(u32, u64)>,
}

impl Claims {
    /// Claim one file's shared ranges and shared compressed extents.
    pub fn charge(&mut self, domain: u32, ranges: &[(u64, u64)], blobs: &[(u64, u64)]) -> Charged {
        let mut new = 0u64;
        let mut shared = 0u64;
        if !ranges.is_empty() {
            let set = self.ranges.entry(domain).or_default();
            for &(start, end) in ranges {
                let fresh = set.claim(start, end);
                new = new.saturating_add(fresh);
                shared = shared.saturating_add(end.saturating_sub(start) - fresh);
            }
        }
        for &(address, bytes) in blobs {
            if self.blobs.insert((domain, address)) {
                new = new.saturating_add(bytes);
            } else {
                shared = shared.saturating_add(bytes);
            }
        }
        Charged { new, shared }
    }
}

/// Byte ranges already charged on one filesystem, kept merged.
///
/// By range and not by extent, because two names can hold the same blocks
/// split at different places. Measured on both filesystems: after 10 MB in
/// the middle of one of three reflinked 100 MB copies was overwritten, that
/// copy reported its old extent as two pieces, `[P, P+40 MiB)` and
/// `[P+50 MiB, P+100 MB)`, while the other two still reported it whole. Keyed
/// by extent, the whole one and the two pieces would be three different keys
/// and the shared 90 MB would be charged twice; by range it is charged once in
/// whichever order the names arrive, and the 10 MB only the whole copies still
/// hold is charged to them.
///
/// Merged on insert, touching neighbours included, so a file written in one
/// go — which a fresh filesystem lays out contiguously — costs one entry
/// however many extents it has.
#[derive(Debug, Default)]
pub(crate) struct Ranges {
    /// `start -> end`, disjoint and never touching.
    spans: BTreeMap<u64, u64>,
}

impl Ranges {
    /// Mark `[start, end)` charged and return how many of its bytes were not
    /// charged before.
    pub fn claim(&mut self, start: u64, end: u64) -> u64 {
        if start >= end {
            return 0;
        }
        let mut merged_start = start;
        let mut merged_end = end;
        // The one span starting before `start` can still reach into it, or
        // touch it; every other candidate starts inside `[start, end]`.
        if let Some((&s, &e)) = self.spans.range(..start).next_back() {
            if e >= start {
                merged_start = s;
            }
        }
        let mut covered = 0u64;
        let absorbed: Vec<(u64, u64)> = self
            .spans
            .range(merged_start..=end)
            .map(|(&s, &e)| (s, e))
            .collect();
        for (s, e) in absorbed {
            self.spans.remove(&s);
            covered += e.min(end).saturating_sub(s.max(start));
            merged_end = merged_end.max(e);
        }
        self.spans.insert(merged_start, merged_end);
        (end - start) - covered
    }

    #[cfg(test)]
    fn spans(&self) -> Vec<(u64, u64)> {
        self.spans.iter().map(|(&s, &e)| (s, e)).collect()
    }
}

/// Files whose shared extents are charged after the walk, in a fixed order.
///
/// Flat rather than a `Box<Mapped>` per file: on a tree with its snapshots
/// inside it nearly every file lands here, and one record plus the ranges
/// themselves is what that costs. Measured on 200,000 snapshot-shared files:
/// see [`Deferred::settle`].
#[derive(Debug, Default)]
pub(crate) struct Deferred {
    files: Vec<DeferredFile>,
    ranges: Vec<(u64, u64)>,
    blobs: Vec<(u64, u64)>,
}

#[derive(Debug)]
struct DeferredFile {
    node: NodeId,
    domain: u32,
    /// Tie-break between two names that read the same after lossy decoding,
    /// so even they come out in the same order every time.
    ino: u64,
    ranges: Range<usize>,
    blobs: Range<usize>,
}

/// What [`Deferred::settle`] found, for the scan's statistics.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Settled {
    pub files_sharing: u64,
    pub shared_bytes: u64,
}

impl Deferred {
    /// Keep `mapped`'s shared extents for entry `node` until the walk is over.
    pub fn push(&mut self, node: NodeId, ino: u64, mapped: &Mapped) {
        let ranges = self.ranges.len()..self.ranges.len() + mapped.ranges.len();
        self.ranges.extend_from_slice(&mapped.ranges);
        let blobs = self.blobs.len()..self.blobs.len() + mapped.blobs.len();
        self.blobs.extend_from_slice(&mapped.blobs);
        self.files.push(DeferredFile {
            node,
            domain: mapped.domain,
            ino,
            ranges,
            blobs,
        });
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Charge every deferred file's shared extents, shallowest path first
    /// ([`PathOrder`](crate::tree::PathOrder)), adding what each one is
    /// charged to its node in `builder`.
    ///
    /// `step` is called once per file, and returning `false` from it stops the
    /// pass — that is how a cancelled scan gets out (invariant 5) and how the
    /// pass moves a counter (invariant 8).
    pub fn settle(
        mut self,
        builder: &mut TreeBuilder,
        claims: &mut Claims,
        mut step: impl FnMut(u64) -> bool,
    ) -> Option<Settled> {
        let order = builder.path_order();
        self.files
            .sort_unstable_by(|a, b| order.cmp((a.node, a.ino), (b.node, b.ino)));
        drop(order);

        let mut settled = Settled::default();
        for file in &self.files {
            let charged = claims.charge(
                file.domain,
                &self.ranges[file.ranges.clone()],
                &self.blobs[file.blobs.clone()],
            );
            let node = &mut builder.nodes[file.node as usize];
            node.alloc = node.alloc.saturating_add(charged.new);
            node.own_alloc = node.alloc;
            if charged.shared > 0 {
                settled.files_sharing += 1;
                settled.shared_bytes += charged.shared;
            }
            if !step(charged.new) {
                return None;
            }
        }
        Some(settled)
    }
}

/// What a filesystem lets this scanner see about shared blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FsKind {
    /// Every block belongs to one file, or nothing better than `st_blocks`
    /// can be asked: ext4, tmpfs, NFS, overlayfs, XFS made without reflink,
    /// and every platform but Linux.
    Plain,
    /// Shares extents between files and FIEMAP says which: btrfs, and XFS
    /// with the reflink feature.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Reflink { domain: u32, btrfs: bool },
    /// Shares blocks in ways no walk can see: ZFS block cloning and
    /// deduplication live in pool-wide tables, and OpenZFS on Linux answers
    /// no FIEMAP. Its `st_blocks` already reflects compression.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Opaque,
}

/// The filesystem a directory sits on, carried down the walk so that only a
/// change of device costs a lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Volume {
    pub dev: u64,
    pub kind: FsKind,
}

/// Where physical addresses are unique.
///
/// **Not the device number.** Every btrfs subvolume and snapshot has its own
/// `st_dev` — measured, 59, 63 and 88 for a top level, a subvolume and its
/// snapshot — while FIEMAP's addresses are the filesystem's own and a reflink
/// across subvolumes reports the same one from both sides. Keyed by device,
/// a file and its snapshot would never meet and the snapshot would be charged
/// again. So btrfs is keyed by its filesystem UUID, which
/// `BTRFS_IOC_FS_INFO` answers without privileges; XFS has no subvolumes and
/// is keyed by device.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DomainKey {
    Uuid([u8; 16]),
    Device(u64),
}

/// The filesystems met so far, so each is asked about once per device.
#[derive(Debug, Default)]
pub(crate) struct Volumes {
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    known: Mutex<VolumeTable>,
    /// How a filesystem is asked. A field so the tests can stand in one that
    /// never answers — there is no way to make a real `statfs` hang from a
    /// test, and the lock and deadline below exist for exactly that case.
    #[cfg(target_os = "linux")]
    detect: Detector,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy)]
struct Detector(fn(&std::path::Path, u64) -> linux::Detected);

#[cfg(target_os = "linux")]
impl Default for Detector {
    fn default() -> Self {
        Detector(linux::detect)
    }
}

#[derive(Debug, Default)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct VolumeTable {
    by_dev: HashMap<u64, FsKind>,
    domains: Vec<DomainKey>,
}

impl Volumes {
    /// What the filesystem holding `dir` (whose device is `dev`) can tell us,
    /// or `None` when it did not answer within `limit`.
    ///
    /// **The table's lock is never held across a syscall.** `statfs` on a
    /// mount whose server has gone away does not return, and a lock held
    /// through it would stop every other thread at the next device it
    /// crossed, healthy or not. So the table is read, released, the
    /// filesystem asked, and the table locked again to record the answer. Two
    /// threads meeting the same new device at once both ask; the second
    /// answer is the same as the first and is dropped.
    ///
    /// With `limit` set the question goes to a thread the walk can abandon,
    /// exactly as the first `lstat` of a mount point does (`mounts.rs`).
    #[cfg(target_os = "linux")]
    pub fn lookup(
        &self,
        dev: u64,
        dir: &std::path::Path,
        limit: Option<std::time::Duration>,
    ) -> Option<Volume> {
        if let Some(&kind) = self.table().by_dev.get(&dev) {
            return Some(Volume { dev, kind });
        }
        let detect = self.detect.0;
        let detected = match limit {
            Some(limit) => {
                let owned = dir.to_path_buf();
                crate::timeout::with_deadline(limit, move || detect(&owned, dev))?
            }
            None => detect(dir, dev),
        };
        let mut table = self.table();
        let kind = match detected {
            linux::Detected::Plain => FsKind::Plain,
            linux::Detected::Opaque => FsKind::Opaque,
            linux::Detected::Reflink { key, btrfs } => {
                let domain = match table.domains.iter().position(|k| *k == key) {
                    Some(index) => index as u32,
                    None => {
                        table.domains.push(key);
                        (table.domains.len() - 1) as u32
                    }
                };
                FsKind::Reflink { domain, btrfs }
            }
        };
        let kind = *table.by_dev.entry(dev).or_insert(kind);
        Some(Volume { dev, kind })
    }

    #[cfg(target_os = "linux")]
    fn table(&self) -> std::sync::MutexGuard<'_, VolumeTable> {
        self.known.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Nowhere else has anything to ask, so nothing is asked.
    #[cfg(not(target_os = "linux"))]
    pub fn lookup(
        &self,
        dev: u64,
        _dir: &std::path::Path,
        _limit: Option<std::time::Duration>,
    ) -> Option<Volume> {
        Some(Volume {
            dev,
            kind: FsKind::Plain,
        })
    }
}

#[cfg(target_os = "linux")]
pub(crate) mod linux {
    //! The syscalls. Raw `libc`, like `bulk.rs` on macOS: none of these
    //! ioctls has a wrapper in `libc` or `std`, and a crate for four of them
    //! would be a dependency in the agent's static binary for ~250 lines.

    use std::collections::HashSet;
    use std::ffi::CString;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::{DomainKey, Mapped};
    use crate::meta::RawMeta;

    /// `statfs.f_type` values, from `<linux/magic.h>` and OpenZFS. Compared
    /// as `u32` because `f_type` is a signed word whose width varies by
    /// architecture, and btrfs's magic has the top bit set.
    const BTRFS_SUPER_MAGIC: u32 = 0x9123_683E;
    const XFS_SUPER_MAGIC: u32 = 0x5846_5342;
    const ZFS_SUPER_MAGIC: u32 = 0x2FC1_2FC1;

    /// `_IOWR('f', 11, struct fiemap)`.
    const FS_IOC_FIEMAP: u32 = 0xC020_660B;
    /// `_IOR(0x94, 31, struct btrfs_ioctl_fs_info_args)`, 1024 bytes.
    const BTRFS_IOC_FS_INFO: u32 = 0x8400_941F;
    /// `_IOWR(0x94, 17, struct btrfs_ioctl_search_args_v2)`, 112-byte header.
    const BTRFS_IOC_TREE_SEARCH_V2: u32 = 0xC070_9411;
    /// `_IOR('X', 100, struct xfs_fsop_geom_v1)`, 112 bytes. The first
    /// version of the call, so every kernel that has XFS answers it; the
    /// feature flags sit at the same offset in every later version.
    const XFS_IOC_FSGEOMETRY_V1: u32 = 0x8070_5864;
    /// `XFS_FSOP_GEOM_FLAGS_REFLINK`, in the geometry's `flags` at byte 92.
    const XFS_GEOM_FLAGS_REFLINK: u32 = 1 << 20;

    const FIEMAP_EXTENT_LAST: u32 = 0x0000_0001;
    const FIEMAP_EXTENT_UNKNOWN: u32 = 0x0000_0002;
    const FIEMAP_EXTENT_DELALLOC: u32 = 0x0000_0004;
    const FIEMAP_EXTENT_ENCODED: u32 = 0x0000_0008;
    const FIEMAP_EXTENT_DATA_INLINE: u32 = 0x0000_0200;
    const FIEMAP_EXTENT_SHARED: u32 = 0x0000_2000;

    /// `BTRFS_EXTENT_DATA_KEY`: one file extent item per referenced range.
    const BTRFS_EXTENT_DATA_KEY: u32 = 108;
    const BTRFS_FILE_EXTENT_REG: u8 = 1;

    /// Extents asked for per FIEMAP call. A fragmented file loops; most files
    /// have one or two, so this is about not looping, not about memory — it
    /// lives on the walk thread's stack, which is 16 MiB.
    const EXTENTS_PER_CALL: usize = 64;

    pub(crate) enum Detected {
        Plain,
        Opaque,
        Reflink { key: DomainKey, btrfs: bool },
    }

    /// Which filesystem `dir` is on, by `statfs` magic.
    ///
    /// Can block on a mount that has stopped answering, which is why
    /// [`super::Volumes::lookup`] calls it with no lock held and, for a mount
    /// point, on a thread it can abandon.
    pub(crate) fn detect(dir: &Path, dev: u64) -> Detected {
        let Ok(c_dir) = CString::new(dir.as_os_str().as_bytes()) else {
            return Detected::Plain;
        };
        // SAFETY: all-zero is a valid `statfs`, and the kernel fills it.
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: `c_dir` is NUL-terminated and `st` is live for the call.
        if unsafe { libc::statfs(c_dir.as_ptr(), &mut st) } != 0 {
            return Detected::Plain;
        }
        match st.f_type as u32 {
            BTRFS_SUPER_MAGIC => Detected::Reflink {
                // Falling back to the device keeps the answer safe: sharing
                // across subvolumes is then charged per subvolume, as `du`
                // would, but nothing is ever charged to nobody.
                key: btrfs_fsid(&c_dir).map_or(DomainKey::Device(dev), DomainKey::Uuid),
                btrfs: true,
            },
            // An XFS made without reflink cannot share a block, so its files
            // are never opened — which is the one thing the walk did not do
            // before. A geometry that cannot be read is taken as "may share":
            // that only costs the opens, where the other guess would cost
            // correctness.
            XFS_SUPER_MAGIC => match xfs_reflink(&c_dir) {
                Some(false) => Detected::Plain,
                _ => Detected::Reflink {
                    key: DomainKey::Device(dev),
                    btrfs: false,
                },
            },
            ZFS_SUPER_MAGIC => Detected::Opaque,
            _ => Detected::Plain,
        }
    }

    /// `dir` opened for an ioctl, closed when the guard drops.
    fn open_dir(dir: &CString) -> Option<std::fs::File> {
        // SAFETY: NUL-terminated path; the result is checked before use.
        let fd = unsafe {
            libc::open(
                dir.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )
        };
        if fd < 0 {
            return None;
        }
        // SAFETY: `fd` was just opened and nothing else owns it.
        Some(unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(fd) })
    }

    /// The filesystem UUID, which every subvolume of one btrfs shares.
    fn btrfs_fsid(dir: &CString) -> Option<[u8; 16]> {
        let file = open_dir(dir)?;
        // `struct btrfs_ioctl_fs_info_args`: two u64s, then the 16-byte fsid.
        let mut args = [0u64; 128];
        // SAFETY: the descriptor is open and `args` is the 1024 bytes the
        // request number encodes.
        let rc =
            unsafe { libc::ioctl(file.as_raw_fd(), BTRFS_IOC_FS_INFO as _, args.as_mut_ptr()) };
        if rc != 0 {
            return None;
        }
        let mut fsid = [0u8; 16];
        fsid[..8].copy_from_slice(&args[2].to_ne_bytes());
        fsid[8..].copy_from_slice(&args[3].to_ne_bytes());
        Some(fsid)
    }

    /// Whether this XFS has the reflink feature, or `None` when it would not
    /// say.
    fn xfs_reflink(dir: &CString) -> Option<bool> {
        let file = open_dir(dir)?;
        let mut geometry = [0u32; 28];
        // SAFETY: the descriptor is open and `geometry` is the 112 bytes the
        // request number encodes.
        let rc = unsafe {
            libc::ioctl(
                file.as_raw_fd(),
                XFS_IOC_FSGEOMETRY_V1 as _,
                geometry.as_mut_ptr(),
            )
        };
        (rc == 0).then(|| geometry[23] & XFS_GEOM_FLAGS_REFLINK != 0)
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct FiemapExtent {
        logical: u64,
        physical: u64,
        length: u64,
        reserved64: [u64; 2],
        flags: u32,
        reserved: [u32; 3],
    }

    #[repr(C)]
    struct Fiemap {
        start: u64,
        length: u64,
        flags: u32,
        mapped_extents: u32,
        extent_count: u32,
        reserved: u32,
        extents: [FiemapExtent; EXTENTS_PER_CALL],
    }

    /// One extent, the fields that matter.
    struct Extent {
        logical: u64,
        physical: u64,
        length: u64,
        flags: u32,
    }

    fn interrupted() -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::Interrupted, "scan cancelled")
    }

    /// The extents of an open file, in logical order.
    ///
    /// No `FIEMAP_FLAG_SYNC`: it would make a disk scanner start writeback on
    /// every file it looks at. Data not yet written back comes back
    /// `DELALLOC` with no address, which is new data and shares nothing.
    ///
    /// `cancelled` is asked between batches, so a file of a million extents
    /// does not hold a cancelled scan for the thousands of calls it takes.
    fn fiemap(
        fd: libc::c_int,
        cancelled: &dyn Fn() -> bool,
        mut each: impl FnMut(Extent),
    ) -> std::io::Result<()> {
        // SAFETY: plain integers; all-zero is valid.
        let mut map: Fiemap = unsafe { std::mem::zeroed() };
        let mut start = 0u64;
        loop {
            map.start = start;
            map.length = u64::MAX - start;
            map.flags = 0;
            map.mapped_extents = 0;
            map.extent_count = EXTENTS_PER_CALL as u32;
            // SAFETY: `fd` is open, `map` is a `struct fiemap` followed by
            // exactly `extent_count` extents, and it outlives the call.
            let rc = unsafe { libc::ioctl(fd, FS_IOC_FIEMAP as _, &mut map as *mut Fiemap) };
            if rc != 0 {
                return Err(std::io::Error::last_os_error());
            }
            let mapped = (map.mapped_extents as usize).min(EXTENTS_PER_CALL);
            if mapped == 0 {
                return Ok(());
            }
            let mut last = false;
            for e in &map.extents[..mapped] {
                last |= e.flags & FIEMAP_EXTENT_LAST != 0;
                each(Extent {
                    logical: e.logical,
                    physical: e.physical,
                    length: e.length,
                    flags: e.flags,
                });
            }
            let tail = &map.extents[mapped - 1];
            let next = tail.logical.saturating_add(tail.length);
            // A kernel that answered without moving forward would loop here
            // forever; stopping keeps what was read and charges the rest as
            // `st_blocks` does.
            if last || next <= start {
                return Ok(());
            }
            if cancelled() {
                return Err(interrupted());
            }
            start = next;
        }
    }

    /// Map one regular file. `Ok(None)` when it neither shares nor
    /// compresses, which is the common case and costs nothing further.
    ///
    /// `search_denied` is shared by the whole scan: the first
    /// `TREE_SEARCH_V2` refused for want of `CAP_SYS_ADMIN` switches it off
    /// for every file after, rather than failing it again a million times.
    pub(crate) fn map(
        path: &Path,
        meta: &RawMeta,
        domain: u32,
        btrfs: bool,
        search_denied: &AtomicBool,
        cancelled: &dyn Fn() -> bool,
    ) -> std::io::Result<Option<Mapped>> {
        // `O_NONBLOCK` because the entry was a regular file when it was
        // listed and may be a FIFO by now, which a blocking open would wait
        // on forever; `O_NOFOLLOW` because a symlink is counted as itself
        // (invariant 3). No data is read, so no atime moves.
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY)
            .open(path)?;
        // The same inode `lstat` described, or the extents would be charged
        // against another file's `st_blocks`.
        let md = file.metadata()?;
        if !md.is_file() || md.dev() != meta.dev || md.ino() != meta.ino {
            return Err(std::io::Error::other("replaced while being scanned"));
        }
        let fd = file.as_raw_fd();

        let mut mapped = Mapped {
            domain,
            ..Mapped::default()
        };
        let mut encoded: Vec<Extent> = Vec::new();
        fiemap(fd, cancelled, |e| {
            // No address yet, or data kept inside the metadata: `st_blocks`
            // already says what there is to say.
            if e.flags
                & (FIEMAP_EXTENT_UNKNOWN | FIEMAP_EXTENT_DELALLOC | FIEMAP_EXTENT_DATA_INLINE)
                != 0
            {
                return;
            }
            if e.flags & FIEMAP_EXTENT_ENCODED != 0 {
                encoded.push(e);
                return;
            }
            if e.flags & FIEMAP_EXTENT_SHARED == 0 {
                return;
            }
            let Some(end) = e.physical.checked_add(e.length) else {
                return;
            };
            mapped.stat_bytes = mapped.stat_bytes.saturating_add(e.length);
            mapped.ranges.push((e.physical, end));
        })?;

        if !encoded.is_empty() {
            let sizes = if btrfs {
                compressed_sizes(fd, meta.ino, search_denied, cancelled)?
            } else {
                None
            };
            // Compressed extents only this file holds, each charged once:
            // after a partial overwrite one file can reference the same
            // compressed extent twice (measured), and the disk holds it once.
            // Local to the file, because no other file references it.
            let mut own: HashSet<u64> = HashSet::new();
            let mut charged_on_disk = 0u64;
            let mut counted_by_stat = 0u64;
            for e in &encoded {
                // The item for this extent starts where the extent does, and
                // FIEMAP reports a compressed extent's disk address as its
                // physical one — both checked, so a merged or shifted FIEMAP
                // answer falls back instead of borrowing a neighbour's size.
                let disk = sizes.as_ref().and_then(|items| {
                    let at = items.binary_search_by_key(&e.logical, |i| i.0).ok()?;
                    let (_, address, disk) = items[at];
                    (address == e.physical).then_some(disk)
                });
                let Some(disk) = disk else {
                    mapped.compressed_inexact = true;
                    continue;
                };
                counted_by_stat = counted_by_stat.saturating_add(e.length);
                if e.flags & FIEMAP_EXTENT_SHARED != 0 {
                    mapped.blobs.push((e.physical, disk));
                    charged_on_disk = charged_on_disk.saturating_add(disk);
                } else if own.insert(e.physical) {
                    mapped.own_bytes = mapped.own_bytes.saturating_add(disk);
                    charged_on_disk = charged_on_disk.saturating_add(disk);
                }
            }
            mapped.stat_bytes = mapped.stat_bytes.saturating_add(counted_by_stat);
            mapped.compressed_saved = counted_by_stat as i64 - charged_on_disk as i64;
        }

        // `stat_bytes` is non-zero exactly when some extent is charged its own
        // way, shared or compressed.
        let interesting =
            mapped.claims_anything() || mapped.compressed_inexact || mapped.stat_bytes != 0;
        Ok(interesting.then_some(mapped))
    }

    /// One compressed extent item: `(file offset, disk address, on-disk
    /// bytes)`.
    type CompressedItem = (u64, u64, u64);

    /// `struct btrfs_ioctl_search_key`, 104 bytes.
    #[repr(C)]
    struct SearchKey {
        tree_id: u64,
        min_objectid: u64,
        max_objectid: u64,
        min_offset: u64,
        max_offset: u64,
        min_transid: u64,
        max_transid: u64,
        min_type: u32,
        max_type: u32,
        nr_items: u32,
        unused: u32,
        unused1: u64,
        unused2: u64,
        unused3: u64,
        unused4: u64,
    }

    const SEARCH_BUF_BYTES: usize = 64 * 1024;

    /// `struct btrfs_ioctl_search_args_v2` with its buffer attached.
    #[repr(C)]
    struct SearchArgs {
        key: SearchKey,
        buf_size: u64,
        buf: [u64; SEARCH_BUF_BYTES / 8],
    }

    /// `(file offset, disk address, on-disk bytes)` for every compressed
    /// extent item of inode `ino`, sorted by offset, or `None` when the tree
    /// cannot be read. An error only for a cancelled scan.
    fn compressed_sizes(
        fd: libc::c_int,
        ino: u64,
        denied: &AtomicBool,
        cancelled: &dyn Fn() -> bool,
    ) -> std::io::Result<Option<Vec<CompressedItem>>> {
        if denied.load(Ordering::Relaxed) {
            return Ok(None);
        }
        // 64 KiB: on the heap, because the walk thread's stack is shared by
        // every level of recursion above this call.
        // SAFETY: plain integers; all-zero is valid.
        let mut args: Box<SearchArgs> = unsafe { Box::new(std::mem::zeroed()) };
        let mut out = Vec::new();
        let mut min_offset = 0u64;
        loop {
            if cancelled() {
                return Err(interrupted());
            }
            args.key = SearchKey {
                // 0 is "the subvolume this descriptor is in".
                tree_id: 0,
                min_objectid: ino,
                max_objectid: ino,
                min_offset,
                max_offset: u64::MAX,
                min_transid: 0,
                max_transid: u64::MAX,
                min_type: BTRFS_EXTENT_DATA_KEY,
                max_type: BTRFS_EXTENT_DATA_KEY,
                nr_items: u32::MAX,
                unused: 0,
                unused1: 0,
                unused2: 0,
                unused3: 0,
                unused4: 0,
            };
            args.buf_size = SEARCH_BUF_BYTES as u64;
            // SAFETY: `fd` is open and `args` is a v2 search header followed
            // by exactly `buf_size` bytes.
            let rc = unsafe {
                libc::ioctl(
                    fd,
                    BTRFS_IOC_TREE_SEARCH_V2 as _,
                    &mut *args as *mut SearchArgs,
                )
            };
            if rc != 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EPERM) {
                    denied.store(true, Ordering::Relaxed);
                }
                return Ok(None);
            }
            let found = args.key.nr_items as usize;
            if found == 0 {
                return Ok(Some(out));
            }
            // SAFETY: the buffer is plain bytes the kernel just wrote, and
            // `bytes` covers exactly the allocation.
            let bytes = unsafe {
                std::slice::from_raw_parts(args.buf.as_ptr().cast::<u8>(), SEARCH_BUF_BYTES)
            };
            let Some((last_offset, items)) = parse_items(bytes, found, ino) else {
                return Ok(None);
            };
            out.extend(items);
            match last_offset {
                Some(offset) if offset < u64::MAX => min_offset = offset + 1,
                _ => return Ok(Some(out)),
            }
        }
    }

    /// The compressed extent items in one batch of search results, and the
    /// key offset of the last item, from which the next batch starts. `None`
    /// for a batch whose framing does not hold together.
    fn parse_items(
        bytes: &[u8],
        found: usize,
        ino: u64,
    ) -> Option<(Option<u64>, Vec<CompressedItem>)> {
        let mut items = Vec::new();
        let mut at = 0usize;
        let mut last_offset = None;
        for _ in 0..found {
            // `struct btrfs_ioctl_search_header`: transid, objectid, offset,
            // type, len — 32 bytes, then `len` bytes of item.
            let header = bytes.get(at..at + 32)?;
            let objectid = u64::from_ne_bytes(header[8..16].try_into().ok()?);
            let offset = u64::from_ne_bytes(header[16..24].try_into().ok()?);
            let kind = u32::from_ne_bytes(header[24..28].try_into().ok()?);
            let len = u32::from_ne_bytes(header[28..32].try_into().ok()?) as usize;
            let item = bytes.get(at + 32..at + 32 + len)?;
            at += 32 + len;
            last_offset = Some(offset);
            if objectid != ino || kind != BTRFS_EXTENT_DATA_KEY {
                continue;
            }
            if let Some(entry) = compressed_item(offset, item) {
                items.push(entry);
            }
        }
        Some((last_offset, items))
    }

    /// One `struct btrfs_file_extent_item`, when it is a regular compressed
    /// extent with blocks behind it. Packed little-endian on disk and handed
    /// over as-is: generation (8), ram_bytes (8), compression (1),
    /// encryption (1), other_encoding (2), type (1), then disk_bytenr (8) and
    /// disk_num_bytes (8) at offsets 21 and 29.
    fn compressed_item(offset: u64, item: &[u8]) -> Option<CompressedItem> {
        let compression = *item.get(16)?;
        let kind = *item.get(20)?;
        if compression == 0 || kind != BTRFS_FILE_EXTENT_REG {
            return None;
        }
        let address = u64::from_le_bytes(item.get(21..29)?.try_into().ok()?);
        let disk = u64::from_le_bytes(item.get(29..37)?.try_into().ok()?);
        (address != 0).then_some((offset, address, disk))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::EntryKind;
    use crate::tree::NewNode;

    #[test]
    fn a_fresh_range_is_charged_in_full() {
        let mut r = Ranges::default();
        assert_eq!(r.claim(100, 200), 100);
        assert_eq!(r.spans(), vec![(100, 200)]);
    }

    #[test]
    fn the_same_range_twice_is_charged_once() {
        let mut r = Ranges::default();
        assert_eq!(r.claim(100, 200), 100);
        assert_eq!(r.claim(100, 200), 0);
        assert_eq!(r.spans(), vec![(100, 200)]);
    }

    /// The case byte ranges exist for: one name holds an extent whole, the
    /// other the same extent in two pieces with a hole where it was
    /// overwritten. Whichever arrives first, the shared bytes are charged
    /// once and the hole is charged to the one still holding it.
    #[test]
    fn an_extent_split_differently_in_two_files_is_charged_once_in_either_order() {
        let whole = [(0u64, 100u64)];
        let pieces = [(0u64, 40u64), (50, 100)];

        let mut a = Ranges::default();
        let first: u64 = whole.iter().map(|&(s, e)| a.claim(s, e)).sum();
        let second: u64 = pieces.iter().map(|&(s, e)| a.claim(s, e)).sum();
        assert_eq!((first, second), (100, 0));

        let mut b = Ranges::default();
        let first: u64 = pieces.iter().map(|&(s, e)| b.claim(s, e)).sum();
        let second: u64 = whole.iter().map(|&(s, e)| b.claim(s, e)).sum();
        assert_eq!(
            (first, second),
            (90, 10),
            "the whole copy owes only the 10 bytes nobody else holds"
        );
        assert_eq!(b.spans(), vec![(0, 100)], "and the set is merged again");
    }

    #[test]
    fn a_range_spanning_several_charged_ones_pays_only_for_the_gaps() {
        let mut r = Ranges::default();
        r.claim(10, 20);
        r.claim(30, 40);
        r.claim(50, 60);
        assert_eq!(r.claim(0, 70), 70 - 30);
        assert_eq!(r.spans(), vec![(0, 70)]);
    }

    #[test]
    fn touching_ranges_merge_and_disjoint_ones_do_not() {
        let mut r = Ranges::default();
        r.claim(0, 10);
        r.claim(10, 20);
        r.claim(30, 40);
        assert_eq!(r.spans(), vec![(0, 20), (30, 40)]);
        // A range starting inside the first and ending inside the second.
        assert_eq!(r.claim(15, 35), 10);
        assert_eq!(r.spans(), vec![(0, 40)]);
    }

    #[test]
    fn an_empty_or_inverted_range_claims_nothing() {
        let mut r = Ranges::default();
        assert_eq!(r.claim(5, 5), 0);
        assert_eq!(r.claim(9, 3), 0);
        assert!(r.spans().is_empty());
    }

    /// `st_blocks` with the described extents taken out and the file's own
    /// compressed extents put back: the unshared rest stays as it was.
    #[test]
    fn only_the_mapped_part_of_a_file_changes() {
        let mapped = Mapped {
            stat_bytes: 1000,
            ranges: vec![(0, 1000)],
            ..Mapped::default()
        };
        assert_eq!(mapped.own_alloc(5096), 4096);

        let mut claims = Claims::default();
        assert_eq!(
            claims.charge(0, &mapped.ranges, &[]),
            Charged {
                new: 1000,
                shared: 0
            }
        );
        assert_eq!(
            claims.charge(0, &mapped.ranges, &[]),
            Charged {
                new: 0,
                shared: 1000
            }
        );
    }

    /// The same physical numbers on two filesystems are two different
    /// blocks. Measured: two separate btrfs volumes in one container both put
    /// their first data extent at 13,631,488.
    #[test]
    fn the_same_address_in_two_domains_is_two_extents() {
        let mut claims = Claims::default();
        assert_eq!(claims.charge(0, &[(0, 1000)], &[]).new, 1000);
        assert_eq!(claims.charge(1, &[(0, 1000)], &[]).new, 1000);
    }

    #[test]
    fn a_shared_compressed_extent_is_charged_its_disk_size_once() {
        let mut claims = Claims::default();
        let blob = [(7_000_000u64, 4096u64)];
        assert_eq!(
            claims.charge(0, &[], &blob),
            Charged {
                new: 4096,
                shared: 0
            }
        );
        assert_eq!(
            claims.charge(0, &[], &blob),
            Charged {
                new: 0,
                shared: 4096
            }
        );
    }

    /// A file that grew between `lstat` and FIEMAP must still produce a
    /// number, not an underflow panic in the walk.
    #[test]
    fn a_file_mapped_larger_than_its_stat_does_not_underflow() {
        let grown = Mapped {
            stat_bytes: 10_000,
            ranges: vec![(0, 10_000)],
            ..Mapped::default()
        };
        assert_eq!(grown.own_alloc(4096), 0);
    }

    fn node(name: &str, kind: EntryKind) -> NewNode<'_> {
        NewNode {
            name,
            kind,
            size: 0,
            alloc: 0,
            mtime: 0,
            nlink: 1,
        }
    }

    /// Builds `root/{zz/copy, aa/deep/copy, .snap/copy}` with the blocks of
    /// one 100-byte extent shared by all three, pushing the directories in
    /// `order` — which is what a different thread count does to the arena.
    /// Returns the builder and the node id of each copy by path.
    fn shared_tree(order: &[&str]) -> (TreeBuilder, Vec<(String, NodeId)>) {
        let mut b = TreeBuilder::with_capacity(16);
        let root = b.push_root(node("root", EntryKind::Dir));
        let names = order;
        let start = b.push_block(
            root,
            names
                .iter()
                .map(|n| node(n, EntryKind::Dir))
                .collect::<Vec<_>>()
                .into_iter(),
        );
        let mut files = Vec::new();
        for (i, name) in names.iter().enumerate() {
            let dir = start + i as NodeId;
            if *name == "aa" {
                let deep = b.push_block(dir, [node("deep", EntryKind::Dir)].into_iter());
                let f = b.push_block(deep, [node("copy", EntryKind::File)].into_iter());
                files.push(("aa/deep/copy".to_string(), f));
            } else {
                let f = b.push_block(dir, [node("copy", EntryKind::File)].into_iter());
                files.push((format!("{name}/copy"), f));
            }
        }
        (b, files)
    }

    fn settle_and_report(order: &[&str]) -> Vec<(String, u64)> {
        let (mut b, files) = shared_tree(order);
        let mut deferred = Deferred::default();
        // Pushed in the arena's order, as the walk would.
        for (_, id) in &files {
            let mapped = Mapped {
                stat_bytes: 100,
                ranges: vec![(5000, 5100)],
                ..Mapped::default()
            };
            deferred.push(*id, 1, &mapped);
        }
        let settled = deferred
            .settle(&mut b, &mut Claims::default(), |_| true)
            .unwrap();
        assert_eq!(settled.shared_bytes, 200, "two of three are repeats");
        let mut out: Vec<(String, u64)> = files
            .into_iter()
            .map(|(path, id)| (path, b.nodes[id as usize].alloc))
            .collect();
        out.sort();
        out
    }

    /// The arena order changes with the thread count; who carries the shared
    /// bytes must not. Depth decides first: the copy under `aa/deep` is one
    /// level deeper than the other two and loses to both, although `aa` sorts
    /// before `zz`. Between equal depths path order decides, so `.snap/copy`
    /// beats `zz/copy`.
    #[test]
    fn the_same_name_carries_the_bytes_whatever_order_the_arena_was_filled_in() {
        let one = settle_and_report(&["zz", "aa", ".snap"]);
        let two = settle_and_report(&[".snap", "zz", "aa"]);
        let three = settle_and_report(&["aa", ".snap", "zz"]);
        assert_eq!(one, two);
        assert_eq!(one, three);
        assert_eq!(
            one,
            vec![
                (".snap/copy".to_string(), 100),
                ("aa/deep/copy".to_string(), 0),
                ("zz/copy".to_string(), 0),
            ]
        );
    }

    /// The pass stops when told to, which is how a cancelled scan leaves it.
    #[test]
    fn settling_stops_when_the_step_says_so() {
        let (mut b, files) = shared_tree(&["zz", "aa", ".snap"]);
        let mut deferred = Deferred::default();
        for (_, id) in &files {
            deferred.push(
                *id,
                1,
                &Mapped {
                    stat_bytes: 100,
                    ranges: vec![(0, 100)],
                    ..Mapped::default()
                },
            );
        }
        let mut calls = 0;
        let out = deferred.settle(&mut b, &mut Claims::default(), |_| {
            calls += 1;
            false
        });
        assert_eq!(out, None);
        assert_eq!(calls, 1);
    }
}

/// The syscall side, against the kernel this runs on. Every test here works
/// on any Linux filesystem — FIEMAP and `statfs` answer on ext4 too — so CI's
/// ordinary job runs them; the reflink job runs them again on btrfs and XFS.
#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, Instant};

    fn never_answers(_: &std::path::Path, _: u64) -> linux::Detected {
        std::thread::sleep(Duration::from_secs(600));
        linux::Detected::Plain
    }

    /// A filesystem that stops answering costs its own lookup the deadline
    /// and nothing else: the table's lock is not held while it is asked, so
    /// a lookup of another device on another thread goes straight through.
    /// With the lock held across `statfs` the second lookup would wait on the
    /// first for as long as the kernel keeps it.
    #[test]
    fn a_filesystem_that_never_answers_blocks_nobody_else() {
        let volumes = std::sync::Arc::new(Volumes {
            detect: Detector(never_answers),
            ..Volumes::default()
        });
        // Something already known, as the volume the walk started on is.
        volumes.table().by_dev.insert(
            1,
            FsKind::Reflink {
                domain: 0,
                btrfs: true,
            },
        );

        let stuck = std::sync::Arc::clone(&volumes);
        let started = Instant::now();
        let first = std::thread::spawn(move || {
            stuck.lookup(
                2,
                std::path::Path::new("/dead"),
                Some(Duration::from_millis(300)),
            )
        });
        std::thread::sleep(Duration::from_millis(50));
        let other = volumes.lookup(1, std::path::Path::new("/alive"), None);
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "the lookup of a known device waited on the stuck one: {:?}",
            started.elapsed()
        );
        assert_eq!(
            other.map(|v| v.kind),
            Some(FsKind::Reflink {
                domain: 0,
                btrfs: true
            })
        );

        assert_eq!(
            first.join().unwrap(),
            None,
            "past the deadline it is unreachable"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(
            !volumes.table().by_dev.contains_key(&2),
            "a device that never answered is not remembered as anything"
        );
    }

    /// Detection against an independent oracle: a filesystem is treated as
    /// sharing exactly when `cp --reflink=always` can share on it. That is
    /// the check that an XFS made with `-m reflink=0` is left alone — its
    /// files are never opened — and that a reflink-capable one is not.
    #[test]
    fn detection_agrees_with_cp_about_reflinks() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), vec![7u8; 64 * 1024]).unwrap();
        let can_reflink = std::process::Command::new("cp")
            .arg("--reflink=always")
            .arg(dir.path().join("a"))
            .arg(dir.path().join("b"))
            .status()
            .is_ok_and(|s| s.success());
        let dev = std::fs::metadata(dir.path()).unwrap().dev();
        let detected = matches!(
            linux::detect(dir.path(), dev),
            linux::Detected::Reflink { .. }
        );
        assert_eq!(
            detected,
            can_reflink,
            "{} — cp --reflink says {can_reflink}, detection says {detected}",
            dir.path().display()
        );
    }

    /// A file of many extents is mapped in batches, and a cancelled scan is
    /// noticed between them rather than after the last.
    #[test]
    fn mapping_stops_between_batches_when_the_scan_is_cancelled() {
        use std::os::unix::fs::FileExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fragmented.bin");
        // Data in every other 64 KiB, so each written piece is an extent of
        // its own on any filesystem: 200 of them, more than three batches.
        let file = std::fs::File::create(&path).unwrap();
        for i in 0..200u64 {
            file.write_all_at(&[1u8; 4096], i * 128 * 1024).unwrap();
        }
        file.sync_all().unwrap();
        drop(file);
        let md = std::fs::symlink_metadata(&path).unwrap();
        let (meta, _) =
            crate::meta::RawMeta::for_path(&path, &md, crate::meta::FileIdentity::Needed);

        let calls = std::cell::Cell::new(0u32);
        let cancelled = || {
            calls.set(calls.get() + 1);
            true
        };
        let out = linux::map(&path, &meta, 0, false, &AtomicBool::new(false), &cancelled);
        let err = out.expect_err("a cancelled scan must not finish mapping");
        assert_eq!(err.kind(), std::io::ErrorKind::Interrupted);
        assert_eq!(calls.get(), 1, "asked once, after the first batch");

        let not_cancelled = || false;
        assert!(
            linux::map(
                &path,
                &meta,
                0,
                false,
                &AtomicBool::new(false),
                &not_cancelled
            )
            .is_ok(),
            "and the same file maps when nobody cancels"
        );
    }
}
