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
//! across the scan, in whichever name claims it first, makes `alloc` what the
//! disk holds for everything shared *inside* the scanned tree. Extents split
//! differently in two files — one copy partially overwritten, so its old
//! extent is now referenced in two pieces — are why the claim is by byte
//! range rather than by extent: see [`Ranges`].
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
//!   calls "Disk Usage";
//! * **without it**, a compressed extent is left exactly as `st_blocks`
//!   counts it, uncompressed and once per name, and the file is counted in
//!   [`ScanStats::compressed_files_inexact`](crate::ScanStats) so the summary
//!   can say the total is high and why. Deduplicating those by address without
//!   knowing their size could charge a 4 KiB reference for a 124 KiB extent,
//!   and a total that is quietly *low* is the one mistake this scanner must
//!   not make.
//!
//! Only files that actually share or compress pay anything beyond the FIEMAP
//! itself. A file with neither comes back as `None` and is charged its
//! `st_blocks` exactly as before, which is why the claim sets hold one entry
//! per shared extent and not one per file.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Mutex;

/// One file's blocks, where they differ from what `st_blocks` says.
///
/// Built while the directory is listed; charged later, in [`Claims::charge`],
/// together with the rest of its directory.
// Only the Linux listing builds one; on every other platform the type exists
// so the walk is not conditional, and the tests below build them by hand.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Mapped {
    /// Which address space the physical numbers below belong to. See
    /// [`Volumes`].
    pub domain: u32,
    /// What `st_blocks` counted for the extents listed below, so it can be
    /// taken out again before they are charged their own way.
    pub stat_bytes: u64,
    /// Shared extents, as physical byte ranges `[start, end)`.
    pub ranges: Vec<(u64, u64)>,
    /// Compressed extents whose on-disk length is known: `(disk address,
    /// on-disk bytes)`. Keyed by identity, never by range, because their
    /// physical span on disk is not what FIEMAP's length says.
    pub blobs: Vec<(u64, u64)>,
    /// `stat_bytes` minus the on-disk length of `blobs`, for the summary:
    /// what compression saves on this file before any sharing.
    pub compressed_saved: u64,
    /// The file has compressed extents whose on-disk length could not be
    /// read, so they stay at their uncompressed length.
    pub compressed_inexact: bool,
}

impl Mapped {
    /// Whether charging this file touches the process-wide claim sets.
    pub fn claims_anything(&self) -> bool {
        !self.ranges.is_empty() || !self.blobs.is_empty()
    }
}

/// What one file ends up charged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Charged {
    pub alloc: u64,
    /// Bytes `st_blocks` counted that some other name had already been
    /// charged for — the part of `du`'s excess that is sharing.
    pub shared: u64,
}

/// Every physical extent some name in the scan has already been charged for.
#[derive(Debug, Default)]
pub(crate) struct Claims {
    ranges: HashMap<u32, Ranges>,
    blobs: HashSet<(u32, u64)>,
}

impl Claims {
    /// Charge `mapped`, whose `st_blocks` said `alloc`, and record what it
    /// claimed.
    ///
    /// Saturating rather than checked: `st_blocks` and FIEMAP are two reads of
    /// a live file, and one that grew in between must still produce a number
    /// rather than a panic in the walk.
    pub fn charge(&mut self, mapped: &Mapped, alloc: u64) -> Charged {
        let mut charged = 0u64;
        let mut shared = 0u64;
        if !mapped.ranges.is_empty() {
            let ranges = self.ranges.entry(mapped.domain).or_default();
            for &(start, end) in &mapped.ranges {
                let new = ranges.claim(start, end);
                charged = charged.saturating_add(new);
                shared = shared.saturating_add(end.saturating_sub(start) - new);
            }
        }
        for &(address, bytes) in &mapped.blobs {
            if self.blobs.insert((mapped.domain, address)) {
                charged = charged.saturating_add(bytes);
            } else {
                shared = shared.saturating_add(bytes);
            }
        }
        Charged {
            alloc: alloc
                .saturating_sub(mapped.stat_bytes)
                .saturating_add(charged),
            shared,
        }
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

/// What a filesystem lets this scanner see about shared blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FsKind {
    /// Every block belongs to one file, or nothing better than `st_blocks`
    /// can be asked: ext4, tmpfs, NFS, overlayfs, and every platform but
    /// Linux.
    Plain,
    /// Shares extents between files and FIEMAP says which: btrfs and XFS.
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
}

#[derive(Debug, Default)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct VolumeTable {
    by_dev: HashMap<u64, FsKind>,
    domains: Vec<DomainKey>,
}

impl Volumes {
    /// What the filesystem holding `dir` (whose device is `dev`) can tell us.
    ///
    /// Asked only when the walk crosses onto a device it has not seen in this
    /// directory's ancestry, so a whole volume costs one `statfs`.
    #[cfg(target_os = "linux")]
    pub fn lookup(&self, dev: u64, dir: &std::path::Path) -> Volume {
        let mut table = self.known.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(&kind) = table.by_dev.get(&dev) {
            return Volume { dev, kind };
        }
        let kind = match linux::detect(dir, dev) {
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
        table.by_dev.insert(dev, kind);
        Volume { dev, kind }
    }

    /// Nowhere else has anything to ask, so nothing is asked.
    #[cfg(not(target_os = "linux"))]
    pub fn lookup(&self, dev: u64, _dir: &std::path::Path) -> Volume {
        Volume {
            dev,
            kind: FsKind::Plain,
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) mod linux {
    //! The syscalls. Raw `libc`, like `bulk.rs` on macOS: none of these
    //! ioctls has a wrapper in `libc` or `std`, and a crate for three of them
    //! would be a dependency in the agent's static binary for ~200 lines.

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
    /// `dir` is a directory the walk is already listing, so the filesystem
    /// has answered for it; this is not a first approach to a mount point,
    /// which `mounts.rs` guards.
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
            XFS_SUPER_MAGIC => Detected::Reflink {
                key: DomainKey::Device(dev),
                btrfs: false,
            },
            ZFS_SUPER_MAGIC => Detected::Opaque,
            _ => Detected::Plain,
        }
    }

    /// The filesystem UUID, which every subvolume of one btrfs shares.
    fn btrfs_fsid(dir: &CString) -> Option<[u8; 16]> {
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
        // `struct btrfs_ioctl_fs_info_args`: two u64s, then the 16-byte fsid.
        let mut args = [0u64; 128];
        // SAFETY: `fd` is open and `args` is the 1024 bytes the request
        // number encodes.
        let rc = unsafe { libc::ioctl(fd, BTRFS_IOC_FS_INFO as _, args.as_mut_ptr()) };
        // SAFETY: opened above, not used after this.
        unsafe { libc::close(fd) };
        if rc != 0 {
            return None;
        }
        let mut fsid = [0u8; 16];
        fsid[..8].copy_from_slice(&args[2].to_ne_bytes());
        fsid[8..].copy_from_slice(&args[3].to_ne_bytes());
        Some(fsid)
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

    /// The extents of an open file, in logical order.
    ///
    /// No `FIEMAP_FLAG_SYNC`: it would make a disk scanner start writeback on
    /// every file it looks at. Data not yet written back comes back
    /// `DELALLOC` with no address, which is new data and shares nothing.
    fn fiemap(fd: libc::c_int, mut each: impl FnMut(Extent)) -> std::io::Result<()> {
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
        fiemap(fd, |e| {
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
                compressed_sizes(fd, meta.ino, search_denied)
            } else {
                None
            };
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
                mapped.stat_bytes = mapped.stat_bytes.saturating_add(e.length);
                mapped.compressed_saved = mapped
                    .compressed_saved
                    .saturating_add(e.length.saturating_sub(disk));
                // Every measured compressed extent is claimed, shared or not:
                // one file can reference the same compressed extent twice
                // after a partial overwrite (measured), and the disk holds
                // it once.
                mapped.blobs.push((e.physical, disk));
            }
        }

        let interesting = mapped.claims_anything() || mapped.compressed_inexact;
        Ok(interesting.then_some(mapped))
    }

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
    /// cannot be read.
    fn compressed_sizes(
        fd: libc::c_int,
        ino: u64,
        denied: &AtomicBool,
    ) -> Option<Vec<(u64, u64, u64)>> {
        if denied.load(Ordering::Relaxed) {
            return None;
        }
        // 64 KiB: on the heap, because the walk thread's stack is shared by
        // every level of recursion above this call.
        // SAFETY: plain integers; all-zero is valid.
        let mut args: Box<SearchArgs> = unsafe { Box::new(std::mem::zeroed()) };
        let mut out = Vec::new();
        let mut min_offset = 0u64;
        loop {
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
                return None;
            }
            let found = args.key.nr_items as usize;
            if found == 0 {
                return Some(out);
            }
            // SAFETY: the buffer is plain bytes the kernel just wrote, and
            // `bytes` covers exactly the allocation.
            let bytes = unsafe {
                std::slice::from_raw_parts(args.buf.as_ptr().cast::<u8>(), SEARCH_BUF_BYTES)
            };
            let mut at = 0usize;
            let mut last_offset = None;
            for _ in 0..found {
                // `struct btrfs_ioctl_search_header`: transid, objectid,
                // offset, type, len — 32 bytes, then `len` bytes of item.
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
                    out.push(entry);
                }
            }
            match last_offset {
                Some(offset) if offset < u64::MAX => min_offset = offset + 1,
                _ => return Some(out),
            }
        }
    }

    /// One `struct btrfs_file_extent_item`, when it is a regular compressed
    /// extent with blocks behind it. Packed little-endian on disk and handed
    /// over as-is: generation (8), ram_bytes (8), compression (1),
    /// encryption (1), other_encoding (2), type (1), then disk_bytenr (8) and
    /// disk_num_bytes (8) at offsets 21 and 29.
    fn compressed_item(offset: u64, item: &[u8]) -> Option<(u64, u64, u64)> {
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

    fn shared(domain: u32, ranges: &[(u64, u64)]) -> Mapped {
        Mapped {
            domain,
            stat_bytes: ranges.iter().map(|&(s, e)| e - s).sum(),
            ranges: ranges.to_vec(),
            ..Mapped::default()
        }
    }

    /// `alloc` is `st_blocks` with the mapped extents taken out and charged
    /// their own way: the unshared rest of the file stays as it was.
    #[test]
    fn only_the_mapped_part_of_a_file_changes() {
        let mut claims = Claims::default();
        // 4096 bytes nobody else holds, plus a 1000-byte shared range.
        let one = shared(0, &[(0, 1000)]);
        assert_eq!(
            claims.charge(&one, 5096),
            Charged {
                alloc: 5096,
                shared: 0
            }
        );
        let two = shared(0, &[(0, 1000)]);
        assert_eq!(
            claims.charge(&two, 5096),
            Charged {
                alloc: 4096,
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
        assert_eq!(claims.charge(&shared(0, &[(0, 1000)]), 1000).alloc, 1000);
        assert_eq!(claims.charge(&shared(1, &[(0, 1000)]), 1000).alloc, 1000);
    }

    /// Compressed extents are charged their on-disk size, once, however many
    /// names reference them.
    #[test]
    fn a_compressed_extent_is_charged_its_disk_size_once() {
        let mut claims = Claims::default();
        let file = Mapped {
            domain: 0,
            stat_bytes: 131_072,
            blobs: vec![(7_000_000, 4096)],
            compressed_saved: 131_072 - 4096,
            ..Mapped::default()
        };
        assert_eq!(
            claims.charge(&file, 131_072),
            Charged {
                alloc: 4096,
                shared: 0
            }
        );
        assert_eq!(
            claims.charge(&file, 131_072),
            Charged {
                alloc: 0,
                shared: 4096
            }
        );
    }

    /// A file that grew between `lstat` and FIEMAP must still produce a
    /// number, not an underflow panic in the walk.
    #[test]
    fn a_file_mapped_larger_than_its_stat_does_not_underflow() {
        let mut claims = Claims::default();
        let grown = shared(0, &[(0, 10_000)]);
        assert_eq!(claims.charge(&grown, 4096).alloc, 10_000);
    }
}
