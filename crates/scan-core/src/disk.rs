//! Whether a filesystem sits on a spinning disk.
//!
//! The walk's defaults are tuned for flash, where a request costs the same
//! wherever it lands and many in flight keep the device busy. A spinning disk
//! is the opposite: one head, and the cost of a request is mostly the seek to
//! it. There, many threads asking for scattered inodes at once make the head
//! travel between them, and the walk is better off asking in the order the
//! inodes lie on the disk, from one thread (see `HDD_THREADS` in `scan.rs`
//! for what was measured, and how).
//!
//! **Linux only, and only where the answer is trustworthy.** The kernel says
//! `queue/rotational` for every block device, but says `1` for a great many
//! that do not spin: a virtio disk in a VM reports it by default — measured,
//! the Docker Desktop VM's disk, an SSD on the host, reports `1` — and so do
//! the emulated disks of QEMU, VMware, VirtualBox and Hyper-V. Trusting it there
//! would quietly slow down the scans of most cloud machines. So a leaf device
//! counts as spinning only when it says so *and* is not on a virtual bus.
//!
//! **macOS has no detection, deliberately.** It would need IOKit — a framework
//! link and a hundred lines of CoreFoundation for one property — and half of
//! what it would unlock does not exist there: `getattrlistbulk` returns each
//! entry's name and attributes in one call, in whatever order the filesystem
//! walks them, so there is no per-entry stat whose order the walk could choose.
//! What remains is the thread count, and `--disk hdd` sets that by hand.
//!
//! The decision on real hardware is reasoned, not measured: no spinning disk
//! was at hand. What `HDD_THREADS` cites is a simulated one, and the check
//! that detection fires was made on that simulated disk with `rotational` set
//! to 1 (and stays off on the loop and virtio devices beside it, which also
//! say 1).

use std::path::Path;

/// Whether the filesystem `root` is on (device `dev`) sits on a spinning disk
/// — any of them, for one built from several (md RAID, LVM, a cache in front
/// of a disk, a btrfs of several devices), because a disk that seeks is what
/// the walk's pace has to suit. `None` when it cannot be told: not on a block
/// device at all (NFS, SMB, FUSE, tmpfs, overlayfs, ZFS), or sysfs does not
/// say.
///
/// Never an error: a scanner that refused to run because it could not read
/// sysfs would be trading a scan for a tuning hint. It can block, though — a
/// loop device's backing file is asked with a `stat`, which on a mount that
/// has stopped answering never returns — so the walk asks under a deadline
/// (`Pace::settle`).
#[cfg(target_os = "linux")]
pub(crate) fn spinning(dev: u64, root: &Path) -> Option<bool> {
    linux::spinning(dev, root, 0)
}

/// Nowhere else is asked; see the module header.
#[cfg(not(target_os = "linux"))]
pub(crate) fn spinning(_dev: u64, _root: &Path) -> Option<bool> {
    None
}

#[cfg(target_os = "linux")]
mod linux {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};

    /// Stacked devices nest — a partition on LVM on md on disks, a loop file
    /// on any of those — but not deeply; past this something loops.
    const MAX_DEPTH: usize = 8;

    /// Whether any disk under the filesystem at `path` (device `dev`) spins.
    pub(super) fn spinning(dev: u64, path: &Path, depth: usize) -> Option<bool> {
        if libc::major(dev as libc::dev_t) != 0 {
            return built_from(&whole_disk(&block_link(dev))?, depth);
        }
        // An anonymous device: btrfs gives one to every subvolume, mounted or
        // nested, so neither the device number nor the mount table leads to a
        // disk. Its own sysfs directory lists every device it spans.
        let devices = btrfs_devices(path)?;
        any_spins(
            devices
                .flatten()
                .map(|device| whole_disk(&device.path()).and_then(|d| built_from(&d, depth))),
        )
    }

    /// `/sys/fs/btrfs/<uuid>/devices` for the btrfs `path` is on; `None` for
    /// any other filesystem, which `BTRFS_IOC_FS_INFO` refuses.
    fn btrfs_devices(path: &Path) -> Option<std::fs::ReadDir> {
        let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        let fsid = crate::extents::linux::btrfs_fsid(&path)?;
        std::fs::read_dir(format!("/sys/fs/btrfs/{}/devices", uuid_text(&fsid))).ok()
    }

    /// A filesystem UUID as sysfs spells it: lowercase, hyphenated 8-4-4-4-12.
    pub(super) fn uuid_text(fsid: &[u8; 16]) -> String {
        let mut text = String::with_capacity(36);
        for (i, byte) in fsid.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                text.push('-');
            }
            text.push_str(&format!("{byte:02x}"));
        }
        text
    }

    /// Whether any disk at the bottom of the sysfs block directory `disk`
    /// spins.
    fn built_from(disk: &Path, depth: usize) -> Option<bool> {
        if depth > MAX_DEPTH {
            return None;
        }
        // A loop device is a file on some other filesystem, and that
        // filesystem's disks are the ones that seek.
        if let Ok(file) = std::fs::read_to_string(disk.join("loop/backing_file")) {
            let file = Path::new(file.trim_end_matches('\n'));
            let md = std::fs::metadata(file).ok()?;
            return spinning(md.dev(), file, depth + 1);
        }
        // Device mapper and md list what they are built from.
        let slaves: Vec<PathBuf> = std::fs::read_dir(disk.join("slaves"))
            .map(|entries| entries.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        if slaves.is_empty() {
            return spins(disk);
        }
        any_spins(
            slaves
                .iter()
                .map(|slave| whole_disk(slave).and_then(|d| built_from(&d, depth + 1))),
        )
    }

    /// `Some(true)` if any answer is, `Some(false)` if every answer is and
    /// there was one, `None` otherwise: one disk that cannot be read is
    /// enough not to know that none of them seeks.
    pub(super) fn any_spins(answers: impl Iterator<Item = Option<bool>>) -> Option<bool> {
        let mut all = Some(false);
        let mut none_at_all = true;
        for answer in answers {
            none_at_all = false;
            match answer {
                Some(true) => return Some(true),
                Some(false) => {}
                None => all = None,
            }
        }
        if none_at_all {
            return None;
        }
        all
    }

    /// `/sys/dev/block/MAJ:MIN`.
    fn block_link(dev: u64) -> PathBuf {
        PathBuf::from(format!(
            "/sys/dev/block/{}:{}",
            libc::major(dev as libc::dev_t),
            libc::minor(dev as libc::dev_t)
        ))
    }

    /// A link to a sysfs block directory, resolved, and stepped up from a
    /// partition to the disk it is on — the disk is what has a queue.
    fn whole_disk(link: &Path) -> Option<PathBuf> {
        let path = std::fs::canonicalize(link).ok()?;
        if path.join("partition").exists() {
            return path.parent().map(Path::to_path_buf);
        }
        Some(path)
    }

    /// Whether this leaf really spins: it says so, and nothing says it is a
    /// virtual disk that says so by default.
    fn spins(leaf: &Path) -> Option<bool> {
        let rotational = std::fs::read_to_string(leaf.join("queue/rotational")).ok()?;
        if rotational.trim() != "1" {
            return Some(false);
        }
        let read = |name: &str| std::fs::read_to_string(leaf.join(name)).unwrap_or_default();
        Some(!is_virtual(
            &leaf.to_string_lossy(),
            &read("device/vendor"),
            &read("device/model"),
        ))
    }

    /// A disk a hypervisor made up, by where it sits in sysfs or what it calls
    /// itself. Those report `rotational` = 1 whatever is underneath.
    pub(super) fn is_virtual(sysfs_path: &str, vendor: &str, model: &str) -> bool {
        const BUSES: [&str; 4] = ["/virtio", "/vbd-", "/VMBUS:", "/xen"];
        const MAKERS: [&str; 7] = ["QEMU", "VMware", "VBOX", "Msft", "Virtual", "Google", "Xen"];
        BUSES.iter().any(|bus| sysfs_path.contains(bus))
            || MAKERS
                .iter()
                .any(|m| vendor.trim().starts_with(m) || model.trim().starts_with(m))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The hypervisors' disks, which all say they spin: the Docker
        /// Desktop VM's virtio disk (path measured on that VM), a QEMU SATA
        /// disk, VMware's SCSI disk, Hyper-V's. And a real one, which is not.
        #[test]
        fn virtual_disks_are_recognised_and_real_ones_are_not() {
            let virtio =
                "/sys/devices/platform/40000000.pci/pci0000:00/0000:00:06.0/virtio2/block/vda";
            assert!(is_virtual(virtio, "0x1af4", ""));
            assert!(is_virtual(
                "/sys/devices/pci0000:00/ata1/host0/target0:0:0/0:0:0:0/block/sda",
                "ATA     ",
                "QEMU HARDDISK   "
            ));
            assert!(is_virtual(
                "/sys/devices/pci0000:00/host2/block/sda",
                "VMware  ",
                "Virtual disk    "
            ));
            assert!(is_virtual(
                "/sys/devices/LNXSYSTM:00/VMBUS:00/block/sda",
                "Msft    ",
                "Virtual Disk    "
            ));
            assert!(is_virtual("/sys/devices/vbd-51712/block/xvda", "", ""));
            assert!(!is_virtual(
                "/sys/devices/pci0000:00/0000:00:17.0/ata3/host2/target2:0:0/2:0:0:0/block/sdb",
                "ATA     ",
                "ST4000DM004-2CV1"
            ));
        }

        /// One disk that spins is enough; every disk has to answer before
        /// none of them is said to spin; nothing at all is not an answer.
        #[test]
        fn a_filesystem_spins_if_any_of_its_disks_does() {
            let of = |answers: &[Option<bool>]| any_spins(answers.iter().copied());
            assert_eq!(of(&[Some(false), Some(true)]), Some(true));
            assert_eq!(
                of(&[None, Some(true)]),
                Some(true),
                "unknown, then spinning"
            );
            assert_eq!(of(&[Some(false), Some(false)]), Some(false));
            assert_eq!(of(&[Some(false), None]), None, "one unread disk");
            assert_eq!(of(&[]), None, "no disk listed");
        }

        /// The UUID `BTRFS_IOC_FS_INFO` returns, spelled the way
        /// `/sys/fs/btrfs/` names its directories.
        #[test]
        fn a_btrfs_uuid_is_spelled_as_sysfs_spells_it() {
            let fsid: [u8; 16] = std::array::from_fn(|i| (i * 17) as u8);
            assert_eq!(uuid_text(&fsid), "00112233-4455-6677-8899-aabbccddeeff");
        }

        /// btrfs gives a nested subvolume its own anonymous device, which no
        /// line of the mount table carries; it is traced to its filesystem's
        /// disks all the same, with the same answer as the subvolume mounted
        /// above it. Runs where the temporary directory is on btrfs and the
        /// `btrfs` command is installed — the CI job `test (btrfs + XFS)`.
        #[test]
        fn a_nested_btrfs_subvolume_is_traced_to_its_disks() {
            let dir = tempfile::tempdir().unwrap();
            let sub = dir.path().join("nested");
            let made = std::process::Command::new("btrfs")
                .args(["subvolume", "create"])
                .arg(&sub)
                .output();
            if !made.is_ok_and(|out| out.status.success()) {
                return;
            }
            let dev = |p: &Path| std::fs::metadata(p).unwrap().dev();
            assert_ne!(dev(&sub), dev(dir.path()), "a subvolume of its own");
            let above = spinning(dev(dir.path()), dir.path(), 0);
            let nested = spinning(dev(&sub), &sub, 0);
            // Unprivileged, an empty subvolume goes as a directory does.
            let _ = std::fs::remove_dir(&sub);
            assert!(above.is_some(), "the mounted subvolume is traced");
            assert_eq!(nested, above, "and the nested one to the same disks");
        }
    }
}
