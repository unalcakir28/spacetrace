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

    match query(path, identity) {
        Ok((alloc, nlink, ino, dev)) => ((alloc, mtime, nlink, ino, dev), None),
        // The entry keeps its name and logical size and stays in the tree,
        // while the caller counts the failure. Dropping it would be the larger
        // lie: the name and the length are already in hand.
        Err(e) => ((md.file_size(), mtime, 1, 0, 0), Some(e)),
    }
}

/// `(alloc, nlink, file id, volume)` — everything a directory listing on
/// Windows does not carry, from one open handle.
///
/// **Why a handle.** `alloc` is `FILE_STANDARD_INFO.AllocationSize`, and no
/// path-based Win32 call answers it. `GetCompressedFileSizeW` looks like one
/// and was tried first; it returns the *logical* size for any file that is
/// neither compressed nor sparse, which CI settled by reporting exactly 100001
/// for a 100001-byte file. Since the handle has to be opened anyway, the
/// identity is read off the same one.
///
/// The cost is one open per entry, and our own measurements put an extra
/// syscall per entry at roughly +36% (COMPETITORS.md §1.1). TODO B4 removes it:
/// `NtQueryDirectoryFileEx` returns allocation and file id inside the listing.
///
/// Directories are queried too, so that `alloc` means the same thing on both
/// platforms instead of quietly excluding directory overhead on one of them.
#[cfg(windows)]
fn query(path: &Path, identity: FileIdentity) -> std::io::Result<(u64, u64, u64, u64)> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileStandardInfo, GetFileInformationByHandle, GetFileInformationByHandleEx,
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        FILE_STANDARD_INFO,
    };

    // Opened through `std` rather than `CreateFileW` so the handle closes
    // itself. A hand-rolled open would have to be closed on every path out of
    // this function, and one leaked handle per entry exhausts the process on a
    // real disk — not a risk worth taking for a correctness fix.
    //
    // The flags are the part that matters: FILE_READ_ATTRIBUTES is the smallest
    // access that answers and it works on files whose contents cannot be read,
    // BACKUP_SEMANTICS is what allows a directory to be opened at all, and
    // OPEN_REPARSE_POINT stops a symlink or junction from being followed, which
    // invariant #3 requires.
    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let handle = file.as_raw_handle() as _;

    let mut standard = FILE_STANDARD_INFO::default();
    // SAFETY: the handle stays open for as long as `file` is alive, which
    // covers the call; the buffer is a live FILE_STANDARD_INFO and the length
    // passed is its own size.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileStandardInfo,
            &mut standard as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    // The field is signed and a real allocation cannot be negative, but a plain
    // cast would turn an unexpected negative into an enormous size — exactly
    // the kind of confidently wrong number this project refuses to print.
    let alloc = standard.AllocationSize.max(0) as u64;

    if identity == FileIdentity::Skipped {
        return Ok((alloc, 1, 0, 0));
    }

    // SAFETY: an all-zero BY_HANDLE_FILE_INFORMATION is a valid value — plain
    // integers and FILETIMEs, with no pointers and no enums.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: as above, plus `info` is live and correctly typed.
    let ok = unsafe { GetFileInformationByHandle(handle, &mut info) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }

    let ino = ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64;
    Ok((
        alloc,
        info.nNumberOfLinks as u64,
        ino,
        info.dwVolumeSerialNumber as u64,
    ))
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
