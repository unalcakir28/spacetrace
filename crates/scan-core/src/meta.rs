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
    pub fn from_metadata(md: &Metadata) -> Self {
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
        let (alloc, mtime, nlink, ino, dev) = platform_fields(md);
        RawMeta {
            kind,
            size: md.len(),
            alloc,
            mtime,
            nlink,
            ino,
            dev,
        }
    }

    /// True when this entry may be reachable through more than one path and
    /// therefore needs (dev, ino) deduplication.
    pub fn is_hardlinked(&self) -> bool {
        self.kind == EntryKind::File && self.nlink > 1
    }
}

#[cfg(unix)]
fn platform_fields(md: &Metadata) -> (u64, i64, u64, u64, u64) {
    use std::os::unix::fs::MetadataExt;
    // `blocks()` is always in 512-byte units, independent of the filesystem's
    // own block size (POSIX).
    (
        md.blocks() * 512,
        md.mtime(),
        md.nlink(),
        md.ino(),
        md.dev(),
    )
}

#[cfg(windows)]
fn platform_fields(md: &Metadata) -> (u64, i64, u64, u64, u64) {
    use std::os::windows::fs::MetadataExt;
    // TODO(win): allocated size needs GetFileInformationByHandleEx /
    // FILE_STANDARD_INFO, and the file id needs FileIdInfo. Until the native
    // Windows backend lands we fall back to the logical size and skip hardlink
    // dedup (nlink = 1).
    let mtime = (md.last_write_time() as i64 / 10_000_000) - 11_644_473_600;
    (md.file_size(), mtime, 1, 0, 0)
}

#[cfg(not(any(unix, windows)))]
fn platform_fields(_md: &Metadata) -> (u64, i64, u64, u64, u64) {
    (0, 0, 1, 0, 0)
}

/// Best-effort display name for a path (used for the root node).
pub fn display_name(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
