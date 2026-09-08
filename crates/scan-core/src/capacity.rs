//! How big the filesystem is, and how much of it is left.
//!
//! A scan measures what a folder *uses*. Answering "when does this fill up"
//! needs the other half: what the filesystem holds in total. That number does
//! not come out of walking a tree, so it is asked of the OS once per scan.
//!
//! Deliberately its own module and its own error path: on an exotic mount this
//! can fail, and failing to read capacity must never fail a scan.

use std::path::Path;

/// Capacity of the filesystem a scanned root sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Capacity {
    /// Total size of the filesystem in bytes.
    pub total: u64,
    /// Bytes available to this process.
    ///
    /// Not the same as "unused": most filesystems keep a reserve that only
    /// root may use, and reporting that as free space would make a forecast
    /// optimistic by exactly the amount that matters.
    pub available: u64,
}

impl Capacity {
    /// `total - available`: space that is not ours to use.
    ///
    /// **Not** the same as "files on this volume", and not the same as what
    /// `df` prints in its Used column. On a filesystem whose space is shared
    /// between volumes — APFS containers, btrfs subvolumes, thin LVM pools —
    /// this includes whatever the siblings are using. `df` on macOS reports a
    /// volume's own bytes instead, which is why its percentage can read 6%
    /// where this reads 70% on the same mount.
    ///
    /// For "will I run out of room", this is the number that matters and `df`'s
    /// is the misleading one; for "how much have I put here", it is the other
    /// way round. Say which one you mean.
    pub fn unavailable(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }

    /// Fraction of the filesystem that is not available, in `0.0..=1.0`.
    ///
    /// See [`Capacity::unavailable`] before putting this in front of anyone.
    pub fn unavailable_fraction(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        self.unavailable() as f64 / self.total as f64
    }

    /// Fraction still available, in `0.0..=1.0`. Matches `df`'s Avail column.
    pub fn free_fraction(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        self.available.min(self.total) as f64 / self.total as f64
    }
}

/// Read the capacity of the filesystem containing `path`.
///
/// Returns `None` when the platform cannot answer, which callers should treat
/// as "unknown" rather than as an error.
///
/// Exported as `capacity_of` rather than `capacity::of`, because the module is
/// private and `of` alone says nothing at a call site.
pub fn capacity_of(path: &Path) -> Option<Capacity> {
    platform::capacity(path)
}

#[cfg(unix)]
mod platform {
    use super::Capacity;
    use std::path::Path;

    pub fn capacity(path: &Path) -> Option<Capacity> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: `stat` is only read when statvfs reports success, and the
        // path is a valid NUL-terminated C string for the duration of the call.
        let stat = unsafe {
            let mut stat = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
            if libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) != 0 {
                return None;
            }
            stat.assume_init()
        };

        // f_frsize is the fragment size, which is what the block counts are in.
        // Some platforms leave it zero, in which case f_bsize is the fallback.
        let block = if stat.f_frsize > 0 {
            stat.f_frsize as u64
        } else {
            stat.f_bsize as u64
        };
        if block == 0 {
            return None;
        }

        Some(Capacity {
            total: (stat.f_blocks as u64).saturating_mul(block),
            // f_bavail, not f_bfree: the difference is the root-only reserve.
            available: (stat.f_bavail as u64).saturating_mul(block),
        })
    }
}

#[cfg(windows)]
mod platform {
    use super::Capacity;
    use std::path::Path;

    pub fn capacity(path: &Path) -> Option<Capacity> {
        use std::os::windows::ffi::OsStrExt;

        // GetDiskFreeSpaceExW wants a directory, and a trailing NUL.
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        wide.push(0);

        let mut free_to_caller: u64 = 0;
        let mut total: u64 = 0;
        // SAFETY: the buffer is NUL-terminated and the out-parameters are
        // owned locals of the right width.
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free_to_caller,
                &mut total,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || total == 0 {
            return None;
        }
        Some(Capacity {
            total,
            // "Free bytes available to the caller" already accounts for quotas,
            // which is the same intent as f_bavail on Unix.
            available: free_to_caller,
        })
    }
}

#[cfg(not(any(unix, windows)))]
mod platform {
    use super::Capacity;
    use std::path::Path;

    pub fn capacity(_path: &Path) -> Option<Capacity> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_current_filesystem_reports_a_plausible_capacity() {
        let dir = tempfile::tempdir().unwrap();
        let capacity = capacity_of(dir.path()).expect("every supported platform can answer this");

        assert!(capacity.total > 0, "a mounted filesystem has a size");
        assert!(
            capacity.available <= capacity.total,
            "available {} exceeds total {}",
            capacity.available,
            capacity.total
        );
        // Sanity: no real disk is smaller than a megabyte.
        assert!(capacity.total > 1024 * 1024);
    }

    #[test]
    fn a_nonexistent_path_reports_unknown_rather_than_panicking() {
        assert!(capacity_of(Path::new("/definitely/not/a/real/mount/point/xyzzy")).is_none());
    }

    #[test]
    fn the_fractions_are_consistent_with_each_other() {
        let c = Capacity {
            total: 1000,
            available: 250,
        };
        assert_eq!(c.unavailable(), 750);
        assert!((c.unavailable_fraction() - 0.75).abs() < 1e-9);
        assert!((c.free_fraction() - 0.25).abs() < 1e-9);
        assert!((c.unavailable_fraction() + c.free_fraction() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn an_empty_filesystem_does_not_divide_by_zero() {
        let c = Capacity {
            total: 0,
            available: 0,
        };
        assert_eq!(c.unavailable(), 0);
        assert_eq!(c.unavailable_fraction(), 0.0);
        assert_eq!(c.free_fraction(), 0.0);
    }

    /// available > total should not underflow into a huge "unavailable", and
    /// the free fraction must stay inside 0..=1.
    #[test]
    fn inconsistent_numbers_saturate_instead_of_wrapping() {
        let c = Capacity {
            total: 100,
            available: 500,
        };
        assert_eq!(c.unavailable(), 0);
        assert_eq!(c.free_fraction(), 1.0);
    }
}
