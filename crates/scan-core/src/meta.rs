use std::fs::Metadata;
use std::path::Path;

/// What a directory entry is. Symlinks are never followed in v1, so they are
/// accounted for by their own (tiny) size rather than their target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum EntryKind {
    Dir = 0,
    File = 1,
    Symlink = 2,
    Other = 3,
}

impl EntryKind {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => EntryKind::Dir,
            1 => EntryKind::File,
            2 => EntryKind::Symlink,
            _ => EntryKind::Other,
        }
    }
}

/// Whether a scan will actually consult an entry's identity.
///
/// On Unix this changes nothing: `(dev, ino)` and the link count arrive with
/// the same `stat` the walk already did. On Windows none of the three are in a
/// directory listing, so they cost an open file handle per entry — a caller
/// that will not use them must not pay for it. Deduplication reads the link
/// count of files; `one_filesystem` reads the volume of directories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileIdentity {
    Needed,
    Skipped,
}

/// Platform-normalised metadata for one entry.
#[derive(Debug, Clone, Copy)]
pub struct RawMeta {
    pub kind: EntryKind,
    /// Logical size in bytes (what `ls` shows).
    pub size: u64,
    /// Bytes actually allocated on disk. Sparse files report less than `size`;
    /// small files usually report more because of block rounding.
    pub alloc: u64,
    /// Unix mtime in seconds.
    pub mtime: i64,
    pub nlink: u64,
    pub ino: u64,
    pub dev: u64,
}

impl RawMeta {
    /// Metadata for `path`, whose `md` the directory walk already read.
    ///
    /// The path is needed because a directory listing does not answer every
    /// question on every platform: on Windows the allocated size and the file
    /// identity each take their own call. The second half of the return value
    /// is what the platform could not answer.
    ///
    /// A failure is reported rather than swallowed (invariant #7) *and* rather
    /// than failing the entry. Dropping an entry from the tree over its
    /// allocated size would be the worse answer of the two: the name and the
    /// logical size are already in hand, so the entry stays and its `alloc`
    /// falls back to the logical size. Only the caller can count that, which
    /// is why it is handed back instead of logged here.
    pub fn for_path(
        path: &Path,
        md: &Metadata,
        identity: FileIdentity,
    ) -> (Self, Option<std::io::Error>) {
        let ft = md.file_type();
        let kind = if ft.is_dir() {
            EntryKind::Dir
        } else if ft.is_file() {
            EntryKind::File
        } else if ft.is_symlink() {
            EntryKind::Symlink
        } else {
            EntryKind::Other
        };
        let (fields, failure) = platform_fields(path, md, identity);
        let (alloc, mtime, nlink, ino, dev) = fields;
        let meta = RawMeta {
            kind,
            size: md.len(),
            alloc,
            mtime,
            nlink,
            ino,
            dev,
        };
        (meta, failure)
    }

    /// True when this entry may be reachable through more than one path and
    /// therefore needs (dev, ino) deduplication.
    pub fn is_hardlinked(&self) -> bool {
        self.kind == EntryKind::File && self.nlink > 1
    }
}

/// `(alloc, mtime, nlink, ino, dev)` — the fields whose source is per-platform.
type PlatformFields = (u64, i64, u64, u64, u64);

#[cfg(unix)]
fn platform_fields(
    _path: &Path,
    md: &Metadata,
    _identity: FileIdentity,
) -> (PlatformFields, Option<std::io::Error>) {
    use std::os::unix::fs::MetadataExt;
    // `blocks()` is always in 512-byte units, independent of the filesystem's
    // own block size (POSIX). One `stat` answered every field, so there is
    // nothing here that can fail and nothing the identity flag can save.
    let fields = (
        md.blocks() * 512,
        md.mtime(),
        md.nlink(),
        md.ino(),
        md.dev(),
    );
    (fields, None)
}

#[cfg(windows)]
fn platform_fields(
    path: &Path,
    md: &Metadata,
    identity: FileIdentity,
) -> (PlatformFields, Option<std::io::Error>) {
    use std::os::windows::fs::MetadataExt;

    // FILETIME counts 100-nanosecond ticks from 1601; Unix time counts seconds
    // from 1970.
    let mtime = (md.last_write_time() as i64 / 10_000_000) - 11_644_473_600;

    // A directory's own allocation is deliberately not asked for.
    // GetCompressedFileSizeW is documented for files, and a call that failed
    // per directory would report an error for every directory on the disk. The
    // consequence is a real divergence: on Windows `alloc` excludes directory
    // overhead, while on Unix it includes it. That is the strongest argument
    // for the fast path in TODO B4 — NtQueryDirectoryFileEx returns
    // AllocationSize for directories too, inside the listing itself.
    let mut failure = None;
    let alloc = if md.is_dir() {
        md.file_size()
    } else {
        match allocated_size(path) {
            Ok(bytes) => bytes,
            Err(e) => {
                failure = Some(e);
                md.file_size()
            }
        }
    };

    if identity == FileIdentity::Skipped {
        return ((alloc, mtime, 1, 0, 0), failure);
    }

    match file_identity(path) {
        Ok((nlink, ino, dev)) => ((alloc, mtime, nlink, ino, dev), failure),
        // The entry survives with dedup disabled for itself: counting one file
        // twice is a smaller error than dropping it from the tree, and the
        // caller is told either way.
        Err(e) => ((alloc, mtime, 1, 0, 0), failure.or(Some(e))),
    }
}

/// Bytes this file actually occupies, sparse holes and NTFS compression
/// included.
///
/// This is the call that makes `alloc` mean what invariant #1 says. The logical
/// size is the wrong answer exactly where it matters most: a sparse VM image
/// reports a length it never allocated, and those are among the largest entries
/// on a real disk (invariant #6).
#[cfg(windows)]
fn allocated_size(path: &Path) -> std::io::Result<u64> {
    use windows_sys::Win32::Storage::FileSystem::{GetCompressedFileSizeW, INVALID_FILE_SIZE};

    let wide = wide(path);
    let mut high: u32 = 0;
    // SAFETY: `wide` is NUL-terminated and outlives the call; `high` is a valid
    // writable u32 which the call only writes through.
    let low = unsafe { GetCompressedFileSizeW(wide.as_ptr(), &mut high) };
    // INVALID_FILE_SIZE is also a legitimate low word — a file whose size ends
    // in 0xFFFFFFFF returns it — so the last error is the only way to tell a
    // failure from that size. `last_os_error` is GetLastError without pulling
    // in another windows-sys feature for it.
    if low == INVALID_FILE_SIZE {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(0) {
            return Err(err);
        }
    }
    Ok(((high as u64) << 32) | low as u64)
}

/// `(nlink, file id, volume)` for one entry — what hardlink deduplication and
/// `one_filesystem` need, and what a directory listing does not carry.
///
/// Costs one open handle, which is why the caller declares whether it is
/// needed.
///
/// The handle is opened through `std` rather than `CreateFileW` so that it
/// closes itself. A hand-rolled open would have to be closed on every path out
/// of this function, and one leaked handle per entry exhausts the process on a
/// real disk — a correctness fix is not worth that risk when the standard
/// library already wraps the same call.
///
/// The flags are the part that matters: `FILE_READ_ATTRIBUTES` is the smallest
/// access that answers and it works on files whose contents cannot be read,
/// `BACKUP_SEMANTICS` is what allows a directory to be opened at all, and
/// `OPEN_REPARSE_POINT` stops a symlink or junction from being followed, which
/// invariant #3 requires.
#[cfg(windows)]
fn file_identity(path: &Path) -> std::io::Result<(u64, u64, u64)> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE,
    };

    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;

    // SAFETY: an all-zero BY_HANDLE_FILE_INFORMATION is a valid value — plain
    // integers and FILETIMEs, with no pointers and no enums.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: the handle is open for as long as `file` is alive, which covers
    // the call; `info` is a live, correctly typed, writable struct.
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }

    let ino = ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64;
    Ok((
        info.nNumberOfLinks as u64,
        ino,
        info.dwVolumeSerialNumber as u64,
    ))
}

/// A path as a NUL-terminated wide string.
///
/// No `\\?\` prefix is added here because the scan already canonicalises its
/// root, and `canonicalize` on Windows returns a verbatim path — so every child
/// path inherits the prefix and MAX_PATH is not the ceiling. That is load
/// bearing for deep trees: without it these calls would start failing at 260
/// characters while `read_dir` kept working.
#[cfg(windows)]
fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(not(any(unix, windows)))]
fn platform_fields(
    _path: &Path,
    _md: &Metadata,
    _identity: FileIdentity,
) -> (PlatformFields, Option<std::io::Error>) {
    ((0, 0, 1, 0, 0), None)
}

/// Best-effort display name for a path (used for the root node).
pub fn display_name(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
