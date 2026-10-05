//! Linux: a directory's names in large batches, and each entry's metadata
//! asked relative to the directory.
//!
//! The ordinary walk lists with `read_dir` and asks each entry by name. Under
//! that, `std` already asks relative to the directory's descriptor, so the
//! syscalls are close to these; what it adds per entry is three allocations
//! (the `CString` inside every `DirEntry`, the `PathBuf` from `path()`, the
//! `OsString` from `file_name()`) and an `Arc` clone and drop, on top of
//! libc's small `getdents64` buffer. On btrfs and XFS it also opens every
//! regular file by its full path for FIEMAP, which resolves every component
//! of the path again. Here the names come straight out of one per-thread
//! buffer, the metadata out of an `fstatat` relative to the directory, and the
//! FIEMAP open is an `openat`.
//!
//! **What that buys depends on the allocator.** Measured in Docker
//! (linux/aarch64, ext4), interleaved, medians, 5 October 2026. With glibc,
//! modest: a million generated entries 194 → 167 ms at 6 threads and 951 →
//! 881 ms at 1; a 115,586-entry copy of real `/usr` trees and a cargo
//! registry 34 → 32 ms and 145 → 131 ms; cold 266 → 258 ms. `perf` puts
//! nearly all of a warm glibc walk in the kernel — the dentry lookup of each
//! name, ext4 hashing names for `getdents64`, filling the stat — and both
//! paths make one stat per entry, so what is left to remove is the user-space
//! part, about a tenth. With the static musl the agent ships as, the
//! allocations were most of the walk: 1660 → 513 ms and 1540 → 905 ms on the
//! million, 268 → 103 ms and 219 → 169 ms on the real tree, cold 384 → 296
//! ms. `dut`'s multiples are against `du`, not against a walk that already
//! asked by descriptor.
//!
//! **`fstatat`, not `statx`.** `statx` with a mask of the six fields the walk
//! reads was measured against plain `fstatat` on this path, with glibc and
//! with the static musl the agent ships as: no difference at 1 or 6 threads,
//! warm or cold (5 October 2026). It cost a hand-defined `struct statx` (musl's
//! `libc` does not declare one), a probe for kernels and seccomp profiles
//! without it, and a fallback — so it went. What it might still buy is on NFS,
//! where leaving atime out of the mask can save a `GETATTR`; that is reasoned,
//! not measured, and not reason enough to keep a second stat path.
//!
//! Two things about this file are load-bearing, and both are the macOS bulk
//! listing's (`bulk.rs`) as well.
//!
//! **It must produce the same `RawMeta` as `lstat` does.** A second metadata
//! path that quietly disagrees with the first surfaces years later as "your
//! snapshot is corrupt". `assert_same_answer_as_lstat` in the tests compares
//! the two field by field, and both go through `RawMeta::from_stat`.
//!
//! **It is not used on a directory that contains a mount point.** Not because
//! it could not be — the stat here is per entry, unlike macOS's — but because
//! the walk's protection for that case (`mounts.rs`, invariant 7) lives in the
//! ordinary path and one implementation of it is enough.

use std::ffi::{CStr, CString, OsStr};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use crate::meta::RawMeta;

/// The least room each `getdents64` call is given. The vector it reads into
/// doubles as it grows, so a call made into a large directory's buffer can be
/// handed more.
///
/// Not a tuning knob: 32, 64 and 256 KiB walked a million entries — with
/// directories of up to 8,000 — in the same time at 1 and at 6 threads, within
/// noise. The kernel's work per entry (hashing the name on ext4, filling the
/// record) is the cost, not the number of calls. For comparison, glibc's
/// `readdir` buffer is 32 KiB and musl's 2 KiB.
const CHUNK: usize = 64 * 1024;

/// What a walk thread keeps between directories.
///
/// A directory of a million entries grows the buffer to tens of megabytes, and
/// keeping that for the rest of the scan on a thread that will never list such
/// a directory again is memory nobody uses. Above this it is given back.
const KEEP_BYTES: usize = 1024 * 1024;

/// `AT_SYMLINK_NOFOLLOW`, because a symlink is counted as itself (invariant 3).
/// `AT_NO_AUTOMOUNT`, because without it asking about an autofs trigger mounts
/// it: a scan of `/net` would try to mount every host it lists, and one that is
/// down hangs the walk. With it the trigger is described as the directory it
/// is — what `lstat` has done since Linux 4.14.
const STAT_FLAGS: libc::c_int = libc::AT_SYMLINK_NOFOLLOW | libc::AT_NO_AUTOMOUNT;

thread_local! {
    static BUFFER: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// A directory opened for listing.
pub(crate) struct Dir {
    fd: OwnedFd,
}

impl Dir {
    /// Open `path` the way `opendir` does, so an error here is the error the
    /// ordinary path would have reported for the same directory.
    pub(crate) fn open(path: &Path) -> std::io::Result<Dir> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY)
            .open(path)?;
        Ok(Dir {
            fd: OwnedFd::from(file),
        })
    }

    /// Every entry's record, read into this thread's buffer, and `f` run over
    /// them.
    ///
    /// An error after some records were read hands over what was read *and*
    /// the error, exactly as `read_dir` does: its iterator yields the entries
    /// it got, then the error, then stops. The caller reports the error and
    /// keeps the entries. A record that cannot be parsed is such an error
    /// too; see `checked`.
    pub(crate) fn with_entries<R>(
        &self,
        f: impl FnOnce(Entries<'_>, Option<std::io::Error>) -> R,
    ) -> R {
        BUFFER.with(|cell| {
            let mut buf = cell.borrow_mut();
            buf.clear();
            let failure = self.read_all(&mut buf).err();
            let (entries, failure) = checked(&buf, failure);
            let out = f(entries, failure);
            if buf.capacity() > KEEP_BYTES {
                buf.clear();
                buf.shrink_to(KEEP_BYTES);
            }
            out
        })
    }

    /// `getdents64` until it says the directory is over, appending raw
    /// records. Each call writes straight into the vector's spare capacity, so
    /// nothing is copied after the kernel puts it there.
    fn read_all(&self, buf: &mut Vec<u8>) -> std::io::Result<()> {
        loop {
            buf.reserve(CHUNK);
            let spare = buf.spare_capacity_mut();
            // SAFETY: the descriptor is open, and the kernel writes at most
            // `spare.len()` bytes into memory this vector owns and has not
            // initialised yet.
            let got = unsafe {
                libc::syscall(
                    libc::SYS_getdents64,
                    self.fd.as_raw_fd(),
                    spare.as_mut_ptr(),
                    spare.len(),
                )
            };
            if got < 0 {
                let err = std::io::Error::last_os_error();
                // A signal that lands while a network filesystem is being
                // asked. Nothing was read, so asking again loses nothing.
                if err.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            if got == 0 {
                return Ok(());
            }
            // SAFETY: the kernel initialised exactly `got` bytes, and `got`
            // is no more than the spare capacity it was given.
            unsafe { buf.set_len(buf.len() + got as usize) };
        }
    }

    /// Metadata for `name`, an entry of this directory, as `lstat` would give
    /// it.
    pub(crate) fn stat(&self, name: &CStr) -> std::io::Result<RawMeta> {
        // SAFETY: all-zero is a valid `stat`, which the kernel fills.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: the descriptor is open, `name` is NUL-terminated, `st` is
        // live for the call.
        let rc = unsafe { libc::fstatat(self.fd.as_raw_fd(), name.as_ptr(), &mut st, STAT_FLAGS) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // Casts, because the widths of these differ between targets and
        // between glibc and musl.
        Ok(RawMeta::from_stat(
            st.st_mode as u32,
            st.st_size as u64,
            st.st_blocks as u64,
            st.st_mtime as i64,
            st.st_nlink as u64,
            st.st_ino as u64,
            st.st_dev as u64,
        ))
    }

    /// `name`, an entry of this directory, opened to ask the filesystem about
    /// its extents — relative to the directory, so the kernel resolves one
    /// component instead of the whole path again.
    pub(crate) fn open_file(&self, name: &OsStr) -> std::io::Result<std::fs::File> {
        let name = CString::new(name.as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        loop {
            // SAFETY: the descriptor is open, `name` is NUL-terminated; the
            // result is checked before use.
            let fd = unsafe {
                libc::openat(
                    self.fd.as_raw_fd(),
                    name.as_ptr(),
                    crate::extents::linux::OPEN_FLAGS,
                )
            };
            if fd >= 0 {
                // SAFETY: `fd` was just opened and nothing else owns it.
                return Ok(unsafe { std::fs::File::from_raw_fd(fd) });
            }
            // Asked again on a signal, as `File::open` does for the path the
            // ordinary walk opens by.
            let err = std::io::Error::last_os_error();
            if err.kind() != std::io::ErrorKind::Interrupted {
                return Err(err);
            }
        }
    }
}

/// The records one directory produced, in the order the kernel gave them.
pub(crate) struct Entries<'a> {
    buf: &'a [u8],
}

/// The size of a record's fixed part: `d_ino` (8), `d_off` (8), `d_reclen`
/// (2), `d_type` (1).
const HEADER: usize = 19;

/// One record at the start of `buf`: its name, or `None` for a record that
/// names no entry, and its length. `None` when no well-formed record starts
/// there — the buffer is over, or the kernel wrote something this does not
/// read the way it expects.
fn record(buf: &[u8]) -> Option<(Option<&CStr>, usize)> {
    let header = buf.get(..HEADER)?;
    let reclen = usize::from(u16::from_ne_bytes([header[16], header[17]]));
    let record = buf.get(..reclen).filter(|_| reclen > HEADER)?;
    let name = CStr::from_bytes_until_nul(&record[HEADER..]).ok()?;
    let ino = u64::from_ne_bytes(header[..8].try_into().ok()?);
    // `d_ino == 0` is a slot that names nothing; glibc's `readdir` skips it,
    // and so does this. `.` and `..` are not children.
    let named = ino != 0 && !matches!(name.to_bytes(), b"." | b"..");
    Some((named.then_some(name), reclen))
}

/// The records of `buf` that parse, and the failure to report.
///
/// One record that does not parse would otherwise end the listing at that
/// point without a word, and the entries after it would be missing from a
/// tree that looks complete — the silent failure invariant 7 rules out. So
/// it is reported as an I/O error against the directory, the way an error
/// from `getdents64` itself is. One already reported is kept instead: it came
/// from the kernel and says more.
fn checked(buf: &[u8], failure: Option<std::io::Error>) -> (Entries<'_>, Option<std::io::Error>) {
    let mut at = 0;
    while let Some((_, len)) = record(&buf[at..]) {
        at += len;
    }
    let failure = match failure {
        None if at < buf.len() => Some(std::io::Error::from_raw_os_error(libc::EIO)),
        failure => failure,
    };
    (Entries { buf: &buf[..at] }, failure)
}

/// One record: the entry's name.
#[derive(Clone, Copy)]
pub(crate) struct Dirent<'a> {
    pub name: &'a CStr,
}

impl<'a> Dirent<'a> {
    pub(crate) fn os_name(&self) -> &'a OsStr {
        OsStr::from_bytes(self.name.to_bytes())
    }
}

impl<'a> Iterator for Entries<'a> {
    type Item = Dirent<'a>;

    /// `struct linux_dirent64`: the fixed part (`HEADER`), then the
    /// NUL-terminated name, padded to `d_reclen`. Parsed from bytes rather
    /// than cast, so a record the kernel did not write the way this expects
    /// cannot be read past; `checked` has already cut the buffer before the
    /// first such record, and reported it.
    ///
    /// `d_type` is not used, on purpose. It is `DT_UNKNOWN` on filesystems
    /// that do not keep it (XFS made without `ftype`, several network ones),
    /// and the stat that follows answers the type anyway — it has to be made
    /// for every entry, because every entry needs its size, blocks and time.
    /// So `d_type` could save nothing here and would add a second opinion
    /// about the kind.
    fn next(&mut self) -> Option<Dirent<'a>> {
        loop {
            let (name, len) = record(self.buf)?;
            self.buf = &self.buf[len..];
            if let Some(name) = name {
                return Some(Dirent { name });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::EntryKind;
    use crate::testing::{lstat_answers, running_as_root, zoo, Answer};
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt;

    fn fast(dir: &Path) -> BTreeMap<OsString, Answer> {
        let listing = Dir::open(dir).expect("the directory opens");
        listing.with_entries(|entries, failure| {
            assert!(failure.is_none(), "{failure:?}");
            entries
                .map(|e| {
                    let answer = listing.stat(e.name).map_err(|e| e.raw_os_error());
                    (e.os_name().to_os_string(), answer)
                })
                .collect()
        })
    }

    /// Both paths, over the same directory, entry by entry and field by
    /// field — the counterpart of the macOS test of the same name, and the
    /// test that makes this path safe to have.
    ///
    /// The count is compared first and separately: a comparison that zipped
    /// two lists would pass over an empty one, which is how the macOS version
    /// of this test once passed while every entry was wrong.
    fn assert_same_answer_as_lstat(dir: &Path) -> usize {
        let fast = fast(dir);
        let slow = lstat_answers(dir);
        assert_eq!(
            fast.keys().collect::<Vec<_>>(),
            slow.keys().collect::<Vec<_>>(),
            "the two paths must see the same entries"
        );
        for (name, expected) in &slow {
            assert_eq!(fast.get(name), Some(expected), "disagreed about {name:?}");
        }
        slow.len()
    }

    #[test]
    fn the_same_answer_as_lstat_for_everything_a_disk_holds() {
        let dir = tempfile::tempdir().unwrap();
        let made = zoo(dir.path());
        let seen = assert_same_answer_as_lstat(dir.path());
        assert_eq!(seen, made, "every entry of the zoo was compared");
        assert_same_answer_as_lstat(&dir.path().join("sub"));

        // And the kinds are what they are, not what they point at.
        let by_name = fast(dir.path());
        let kind = |n: &str| by_name[OsStr::new(n)].as_ref().unwrap().kind;
        assert_eq!(kind("to-dir"), EntryKind::Symlink);
        assert_eq!(kind("broken"), EntryKind::Symlink);
        assert_eq!(kind("fifo"), EntryKind::Other);
        assert_eq!(kind("socket"), EntryKind::Other);
        assert_eq!(kind("sub"), EntryKind::Dir);
        let sparse = by_name[OsStr::new("sparse.img")].as_ref().unwrap();
        assert!(
            sparse.alloc < sparse.size / 100,
            "a sparse file reports its holes as unallocated: {sparse:?}"
        );
        assert!(by_name[OsStr::new("big.bin")]
            .as_ref()
            .unwrap()
            .is_hardlinked());
    }

    /// More records than one `getdents64` call returns, so the loop that
    /// appends batches runs several times. A loop that only ever ran once
    /// would pass every other test here.
    #[test]
    fn a_directory_larger_than_one_batch_is_read_completely() {
        let dir = tempfile::tempdir().unwrap();
        // About 220 bytes a record: 1,500 of them are five chunks.
        for i in 0..1_500 {
            let name = format!("entry-{i:05}-{}", "x".repeat(190));
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let listing = Dir::open(dir.path()).unwrap();
        let bytes = listing.with_entries(|entries, _| entries.buf.len());
        assert!(bytes > 4 * CHUNK, "only {bytes} bytes of records");
        assert_eq!(assert_same_answer_as_lstat(dir.path()), 1_500);
    }

    /// A directory that can be listed but not searched (`r` without `x`):
    /// every name comes back and every stat is refused. Both paths have to
    /// refuse the same entries for the same reason, because the walk counts
    /// each refusal as an unreadable path (invariant 7).
    #[test]
    fn unreadable_entries_are_refused_alike() {
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::write(locked.join("a"), b"1").unwrap();
        std::fs::create_dir(locked.join("b")).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o444)).unwrap();

        let seen = assert_same_answer_as_lstat(&locked);
        assert_eq!(seen, 2);
        if !running_as_root() {
            let answers = fast(&locked);
            assert!(
                answers.values().all(|a| *a == Err(Some(libc::EACCES))),
                "without search permission every entry is refused: {answers:?}"
            );
        }

        // Unlistable altogether: the open fails as `read_dir` fails.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        if !running_as_root() {
            let fast = Dir::open(&locked).err().and_then(|e| e.raw_os_error());
            let slow = std::fs::read_dir(&locked)
                .err()
                .and_then(|e| e.raw_os_error());
            assert_eq!(fast, slow);
            assert_eq!(fast, Some(libc::EACCES));
        }
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn an_empty_directory_yields_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let listing = Dir::open(dir.path()).unwrap();
        assert_eq!(listing.with_entries(|entries, _| entries.count()), 0);
    }

    /// A file is not a directory, and the answer is the error `read_dir`
    /// would give, not a panic or an empty listing.
    #[test]
    fn a_file_does_not_open_as_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("plain.txt");
        std::fs::write(&file, b"x").unwrap();
        let err = Dir::open(&file).err().and_then(|e| e.raw_os_error());
        assert_eq!(err, Some(libc::ENOTDIR));
        let err = Dir::open(&dir.path().join("missing"))
            .err()
            .and_then(|e| e.raw_os_error());
        assert_eq!(err, Some(libc::ENOENT));
    }

    /// The length of every record the tests make: room for a name of 12.
    const LEN: usize = 32;

    /// One `linux_dirent64` record, as the kernel lays it out.
    fn raw_record(ino: u64, name: &[u8]) -> Vec<u8> {
        let mut record = vec![0u8; LEN];
        record[..8].copy_from_slice(&ino.to_ne_bytes());
        record[16..18].copy_from_slice(&(LEN as u16).to_ne_bytes());
        record[HEADER..HEADER + name.len()].copy_from_slice(name);
        record
    }

    fn names(entries: Entries<'_>) -> Vec<String> {
        entries
            .map(|e| e.name.to_str().unwrap().to_owned())
            .collect()
    }

    /// A record the parser cannot read is not read past, and it does not end
    /// the listing silently either: the records before it are kept and the
    /// directory gets an error, as for a failing `getdents64`.
    #[test]
    fn a_malformed_record_is_reported_as_an_error_of_the_directory() {
        let good = [raw_record(7, b"abc"), raw_record(8, b"def")].concat();
        let (entries, failure) = checked(&good, None);
        assert_eq!(names(entries), ["abc", "def"]);
        assert!(failure.is_none(), "well-formed records are no error");

        let mut zero_length = good.clone();
        zero_length[LEN + 16..LEN + 18].copy_from_slice(&0u16.to_ne_bytes());
        let mut no_nul = good.clone();
        no_nul[LEN + HEADER..2 * LEN].fill(b'x');
        for (what, buf) in [
            ("truncated", &good[..LEN + 20]),
            ("zero length", &zero_length[..]),
            ("no NUL in the record", &no_nul[..]),
        ] {
            let (entries, failure) = checked(buf, None);
            assert_eq!(
                names(entries),
                ["abc"],
                "{what}: the records before it stay"
            );
            let failure = failure.unwrap_or_else(|| panic!("{what}: reported"));
            assert_eq!(failure.raw_os_error(), Some(libc::EIO), "{what}");
        }

        // The kernel's own error wins over the parser's: it says more.
        let kernel = std::io::Error::from_raw_os_error(libc::ESTALE);
        let (_, failure) = checked(&good[..LEN + 6], Some(kernel));
        assert_eq!(failure.and_then(|e| e.raw_os_error()), Some(libc::ESTALE));
    }

    /// A record whose inode number is 0 names nothing — a slot some
    /// filesystems leave for a deleted entry — and is skipped, as glibc's
    /// `readdir` skips it. `.` and `..` are skipped too.
    #[test]
    fn a_record_without_an_inode_is_not_an_entry() {
        let buf = [
            raw_record(5, b"."),
            raw_record(0, b"deleted"),
            raw_record(9, b"kept"),
            raw_record(2, b".."),
        ]
        .concat();
        let (entries, failure) = checked(&buf, None);
        assert!(failure.is_none(), "an empty slot is not malformed");
        assert_eq!(names(entries), ["kept"]);
    }
}
