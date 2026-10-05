//! Windows: find the NTFS volume a scan root is on, open it, and hand its
//! bytes to the parser in `mod.rs`.
//!
//! **Every way this can fail ends in the ordinary walk.** No administrator
//! (the volume will not open), ReFS, FAT or exFAT (it opens, but
//! `FSCTL_GET_NTFS_VOLUME_DATA` refuses), a network drive (there is no local
//! volume behind it), a boot sector that disagrees with what NTFS itself
//! says, a table that will not parse: each is `None`, and the caller walks as
//! if this file did not exist. The caller is never told which happened —
//! there is nothing it could do differently, and the tree is meant to be the
//! same either way.
//!
//! **Only a whole volume, by default.** The table is read whole whatever the
//! root, so its cost follows the volume while the walk's follows the folder:
//! a scan of a folder of a thousand files would read the entire table of a
//! system drive holding millions. The subtree itself is no problem — the
//! tests list one — but which folders are worth it is a measurement nobody
//! has made yet on a real Windows disk, so until then only a volume's root is
//! read this way.
//!
//! **Where the two paths may answer differently on a real system volume, the
//! table is the one that is right.** The walk opens what it lists, so a
//! directory whose ACL shuts out even an administrator (`System Volume
//! Information`, which holds the shadow copies) is an error with nothing
//! under it, and a file nobody may open (`pagefile.sys`) falls back to its
//! logical size. The table reads neither through an open, so it lists and
//! charges both — which is what the disk holds (invariant 1) and why it can
//! report fewer errors and more bytes than the walk of the same volume. The
//! differential test below builds a tree where neither happens.
//!
//! **Nothing here has run outside Windows CI.** It type-checks on any host;
//! whether it opens the volume, flushes it and reads it is what the tests at
//! the bottom of this file are for.

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use windows_sys::Win32::Storage::FileSystem::{
    FlushFileBuffers, GetDriveTypeW, GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};
use windows_sys::Win32::System::Ioctl::{FSCTL_GET_NTFS_VOLUME_DATA, NTFS_VOLUME_DATA_BUFFER};
use windows_sys::Win32::System::IO::DeviceIoControl;

use super::{read_geometry, read_table, Table, RECORD_MASK, ROOT_RECORD};
use crate::meta::FileIdentity;
use crate::scan::{Phase, ScanProgress};

/// `GetDriveTypeW` for a network drive.
const DRIVE_REMOTE: u32 = 4;

/// How many unreadable records a table may have before the whole read is
/// distrusted. A few are a live volume caught mid-write, and are reported
/// one by one; one in a hundred is a parser and a disk disagreeing, which the
/// walk answers better.
fn too_many_bad(table: &Table) -> bool {
    table.bad_count > 16 + table.capacity() as u64 / 100
}

/// The table of the volume `root` is on, and `root`'s file reference in it,
/// or `None` to walk instead.
///
/// `any_directory` lifts the whole-volume rule; only the tests set it, and
/// hand the table to the scan themselves.
/// `progress` counts records read in `rows_done` of `rows_total`, and its
/// cancellation stops the read.
pub(crate) fn read(
    root: &Path,
    any_directory: bool,
    progress: &ScanProgress,
) -> Option<(Table, u64)> {
    // One handle on the root, which the walk opens too: its file index is
    // the root's MFT reference, and its volume serial says which volume the
    // table must belong to.
    let (_, _, reference, serial) = crate::meta::query(root, FileIdentity::Needed).ok()?;
    if !any_directory && reference & RECORD_MASK != ROOT_RECORD {
        return None;
    }
    let device = device_path(root)?;
    // Opening a volume for reading is what takes an administrator; without
    // one, this is where the fast path ends.
    let volume = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(&device)
        .ok()?;
    let data = ntfs_volume_data(&volume)?;
    if data.VolumeSerialNumber as u32 as u64 != serial {
        return None;
    }
    flush(&device);

    let sector = u64::from(data.BytesPerSector).max(512);
    let mut reader = Aligned::new(volume, sector);
    let geometry = read_geometry(&mut reader).ok()?;
    // Two sources for the same three numbers; if they disagree, one of them
    // is not describing this volume, and the read would be garbage.
    let agrees = geometry.bytes_per_cluster == u64::from(data.BytesPerCluster)
        && geometry.bytes_per_record == u64::from(data.BytesPerFileRecordSegment)
        && geometry.mft_lcn == data.MftStartLcn as u64;
    if !agrees {
        return None;
    }
    let table = read_table(&mut reader, geometry, |done, total| {
        match done {
            0 => progress.begin_rows(Phase::Walking, total),
            _ => progress.rows_done.store(done, Ordering::Relaxed),
        }
        !progress.is_cancelled()
    })
    .ok()?;
    if too_many_bad(&table) || !table.is_directory(reference) {
        return None;
    }
    Some((table, reference))
}

/// `\\?\Volume{…}` for the volume `root` is on, or `None` for a path with no
/// local volume behind it.
fn device_path(root: &Path) -> Option<PathBuf> {
    let plain = plain_path(root)?;
    let wide: Vec<u16> = plain.encode_wide().chain(Some(0)).collect();
    let mut mount = vec![0u16; 32 * 1024];
    // SAFETY: `wide` is NUL-terminated and alive for the call; `mount` is a
    // writable buffer whose length is passed in characters.
    let ok = unsafe { GetVolumePathNameW(wide.as_ptr(), mount.as_mut_ptr(), mount.len() as u32) };
    if ok == 0 {
        return None;
    }
    // SAFETY: `mount` was NUL-terminated by the call above.
    if unsafe { GetDriveTypeW(mount.as_ptr()) } == DRIVE_REMOTE {
        return None;
    }
    // A volume GUID path is 49 characters and its NUL.
    let mut name = vec![0u16; 64];
    // SAFETY: as above, both buffers live for the call, the length is in
    // characters.
    let ok = unsafe {
        GetVolumeNameForVolumeMountPointW(mount.as_ptr(), name.as_mut_ptr(), name.len() as u32)
    };
    if ok == 0 {
        return None;
    }
    let len = name.iter().position(|&c| c == 0)?;
    // The name ends in a backslash, which would open the volume's root
    // directory rather than the volume.
    let name = name[..len]
        .strip_suffix(&[u16::from(b'\\')])
        .unwrap_or(&name[..len]);
    Some(PathBuf::from(OsString::from_wide(name)))
}

/// `root` without the `\\?\` that `canonicalize` puts in front of a drive
/// letter, which the volume functions are not documented to accept. `None`
/// for `\\?\UNC\…`, a network path.
fn plain_path(root: &Path) -> Option<OsString> {
    let wide: Vec<u16> = root.as_os_str().encode_wide().collect();
    let verbatim: Vec<u16> = r"\\?\".encode_utf16().collect();
    let Some(rest) = wide.strip_prefix(verbatim.as_slice()) else {
        return Some(root.as_os_str().to_owned());
    };
    if rest.get(1) == Some(&u16::from(b':')) {
        return Some(OsString::from_wide(rest));
    }
    let unc: Vec<u16> = r"UNC\".encode_utf16().collect();
    if rest.starts_with(&unc) {
        return None;
    }
    Some(root.as_os_str().to_owned())
}

/// What NTFS says about its own volume, or `None` if it is not NTFS.
fn ntfs_volume_data(volume: &File) -> Option<NTFS_VOLUME_DATA_BUFFER> {
    // SAFETY: the structure is plain integers, so all zeros is a valid value.
    let mut data: NTFS_VOLUME_DATA_BUFFER = unsafe { std::mem::zeroed() };
    let mut returned = 0u32;
    // SAFETY: the handle is open for the duration of the call; the output
    // buffer is a live, correctly sized NTFS_VOLUME_DATA_BUFFER; no input
    // buffer and no OVERLAPPED, so the call is synchronous.
    let ok = unsafe {
        DeviceIoControl(
            volume.as_raw_handle() as _,
            FSCTL_GET_NTFS_VOLUME_DATA,
            std::ptr::null(),
            0,
            &mut data as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<NTFS_VOLUME_DATA_BUFFER>() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(data)
}

/// Ask NTFS to write out what it holds in memory.
///
/// A volume handle reads the disk, not NTFS's cache, and a record changed a
/// moment ago — a file just written — can still be only in the cache. A
/// flush closes that gap; it needs a handle opened for writing, which an
/// administrator may have. If it fails, the read goes ahead and sees the disk
/// as the lazy writer last left it, at most a few seconds behind.
fn flush(device: &Path) {
    let Ok(writable) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(device)
    else {
        return;
    };
    // SAFETY: the handle is open for the duration of the call.
    unsafe { FlushFileBuffers(writable.as_raw_handle() as _) };
}

/// A volume handle refuses a read that does not start and end on a sector
/// boundary, or whose buffer is not sector-aligned in memory. This turns any
/// read into reads it accepts: straight through when the caller's already
/// is one (the parser's large chunks are), through an aligned buffer of its
/// own when not (the boot sector, a record read again).
struct Aligned {
    file: File,
    sector: u64,
    pos: u64,
    storage: Vec<u8>,
    start: usize,
}

/// The bounce buffer, beyond which one call returns a short read.
const BOUNCE: usize = 1 << 20;

/// Memory alignment that satisfies every sector size in use.
const ALIGN: usize = 4096;

impl Aligned {
    fn new(file: File, sector: u64) -> Self {
        let storage = vec![0u8; BOUNCE + ALIGN];
        let start = storage.as_ptr().align_offset(ALIGN).min(ALIGN);
        Aligned {
            file,
            sector,
            pos: 0,
            storage,
            start,
        }
    }
}

impl Read for Aligned {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let sector = self.sector;
        let direct = self.pos % sector == 0
            && buf.len() as u64 % sector == 0
            && (buf.as_ptr() as usize) % ALIGN == 0;
        if direct {
            self.file.seek(SeekFrom::Start(self.pos))?;
            let n = self.file.read(buf)?;
            self.pos += n as u64;
            return Ok(n);
        }
        let from = self.pos - self.pos % sector;
        let skip = (self.pos - from) as usize;
        let want = (skip + buf.len()).min(BOUNCE);
        let len = (want as u64).div_ceil(sector) * sector;
        let len = (len as usize).min(BOUNCE);
        let bounce = &mut self.storage[self.start..self.start + len];
        self.file.seek(SeekFrom::Start(from))?;
        self.file.read_exact(bounce)?;
        let take = (len - skip).min(buf.len());
        buf[..take].copy_from_slice(&bounce[skip..skip + take]);
        self.pos += take as u64;
        Ok(take)
    }
}

impl Seek for Aligned {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        self.pos = match to {
            SeekFrom::Start(at) => at,
            SeekFrom::Current(by) => self
                .pos
                .checked_add_signed(by)
                .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?,
            // A volume's length is not something this reader needs.
            SeekFrom::End(_) => return Err(io::Error::from(io::ErrorKind::Unsupported)),
        };
        Ok(self.pos)
    }
}

/// The differential test: the same tree walked and read from the table must
/// be the same tree, field for field — the Windows counterpart of
/// `assert_same_answer_as_lstat`. It runs where the volume can be opened,
/// which on the CI runner it can (GitHub's Windows runners are elevated, and
/// the workflow sets `SPACETRACE_REQUIRE_MFT` so that a refusal fails the job
/// rather than skipping). Elsewhere it says it skipped.
#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsStr;
    use std::fs;
    use std::io::{Seek, SeekFrom, Write};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    use crate::ntfs::Table;
    use crate::scan::{scan_source, ScanOptions, ScanProgress, Source};
    use crate::{EntryKind, Tree};

    /// Something a test needs and could not get. In CI, which runs elevated
    /// and sets `SPACETRACE_REQUIRE_MFT=1`, that fails the test unless it is
    /// `optional`: a test that skips there passes by testing less. Anywhere
    /// else it says so and the test carries on, or returns.
    fn unavailable(what: &str, optional: bool) {
        let required = std::env::var("SPACETRACE_REQUIRE_MFT").as_deref() == Ok("1");
        assert!(
            optional || !required,
            "{what}: not available on an elevated runner"
        );
        eprintln!("SKIPPED: {what}");
    }

    /// The table of `root`'s volume and `root`'s reference in it.
    fn table_or_skip(root: &Path, any_directory: bool) -> Option<(Table, u64)> {
        let found = super::read(root, any_directory, &ScanProgress::default());
        if found.is_none() {
            unavailable(&format!("the table of {}", root.display()), false);
        }
        found
    }

    fn quietly(program: &str, args: &[&OsStr]) -> bool {
        Command::new(program)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    /// A case the corpus could not make. A WOF file and a forced 8.3 alias
    /// are optional: they depend on how the runner's image is set up, and
    /// the fixture images cover them regardless.
    fn missing(case: &str, optional: bool) {
        unavailable(&format!("the {case} case in the corpus"), optional);
    }

    /// Everything the table has to agree with the walk on.
    fn corpus() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let path = |rel: &str| root.join(rel);

        fs::create_dir_all(path("nested/a/b/c")).unwrap();
        fs::write(path("nested/a/b/c/leaf.txt"), b"hello\n").unwrap();
        fs::create_dir(path("emptydir")).unwrap();
        fs::write(path("empty"), b"").unwrap();
        fs::write(path("tiny"), b"x").unwrap();
        fs::write(path("resident300"), vec![b'r'; 300]).unwrap();
        fs::write(path("odd.bin"), vec![b'o'; 100_001]).unwrap();

        // More entries than one index block: an $INDEX_ALLOCATION, whose
        // size is what a directory is charged.
        fs::create_dir(path("big")).unwrap();
        for i in 0..2500 {
            fs::write(path(&format!("big/entry-{i}")), b"").unwrap();
        }

        fs::create_dir(path("links1")).unwrap();
        fs::create_dir(path("links2")).unwrap();
        fs::write(path("links1/orig"), vec![b'l'; 5000]).unwrap();
        fs::hard_link(path("links1/orig"), path("links2/second")).unwrap();
        fs::hard_link(path("links1/orig"), path("links1/third")).unwrap();

        fs::write(path("ads.txt"), b"main stream\n").unwrap();
        fs::write(path("ads.txt:extra"), vec![b'a'; 3000]).unwrap();

        let long: String = "ı".repeat(100) + &"😀".repeat(40) + ".txt";
        fs::write(path(&long), b"long\n").unwrap();
        fs::write(path("Long Name With Spaces.txt"), b"dos\n").unwrap();
        // 8.3 aliases may be switched off on the volume; forcing one makes
        // sure the DOS-namespace rule is exercised either way.
        if !quietly(
            "fsutil",
            &[
                "file".as_ref(),
                "setshortname".as_ref(),
                path("Long Name With Spaces.txt").as_os_str(),
                "LONGNA~1.TXT".as_ref(),
            ],
        ) {
            missing("8.3 alias", true);
        }

        let sparse = path("sparse.bin");
        fs::write(&sparse, b"").unwrap();
        if quietly(
            "fsutil",
            &["sparse".as_ref(), "setflag".as_ref(), sparse.as_os_str()],
        ) {
            let mut file = fs::OpenOptions::new().write(true).open(&sparse).unwrap();
            file.set_len(4 << 20).unwrap();
            file.seek(SeekFrom::Start((4 << 20) - 4096)).unwrap();
            file.write_all(&[b's'; 4096]).unwrap();
        } else {
            missing("sparse file", false);
        }

        fs::create_dir(path("comp")).unwrap();
        let text = "compressible line of text\n".repeat(8000);
        fs::write(path("comp/text.txt"), &text).unwrap();
        if !quietly(
            "compact",
            &["/c".as_ref(), path("comp/text.txt").as_os_str()],
        ) {
            missing("NTFS-compressed file", false);
        }
        fs::write(path("wof.bin"), &text).unwrap();
        if !quietly(
            "compact",
            &[
                "/c".as_ref(),
                "/exe:xpress4k".as_ref(),
                path("wof.bin").as_os_str(),
            ],
        ) {
            missing("WOF-compressed file", true);
        }

        fs::write(path("target.txt"), b"target\n").unwrap();
        if std::os::windows::fs::symlink_file("target.txt", path("link.txt")).is_err() {
            missing("file symlink", false);
        }
        if std::os::windows::fs::symlink_dir("nested", path("dirlink")).is_err() {
            missing("directory symlink", false);
        }
        if !quietly(
            "cmd",
            &[
                "/c".as_ref(),
                "mklink".as_ref(),
                "/J".as_ref(),
                path("junction").as_os_str(),
                path("nested").as_os_str(),
            ],
        ) {
            missing("junction", false);
        }

        // Deleted records: their headers stay in the table without the
        // in-use flag.
        for i in 0..20 {
            fs::write(path(&format!("gone{i}")), b"x").unwrap();
        }
        for i in 0..20 {
            fs::remove_file(path(&format!("gone{i}"))).unwrap();
        }
        dir
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Row {
        kind: EntryKind,
        size: u64,
        alloc: u64,
        mtime: i64,
        nlink: u32,
    }

    fn rows(tree: &Tree) -> BTreeMap<String, Row> {
        let mut found = BTreeMap::new();
        let mut stack = vec![(tree.root(), String::new())];
        while let Some((id, prefix)) = stack.pop() {
            for child in tree.children(id) {
                let path = format!("{prefix}/{}", tree.name(child));
                let node = tree.node(child);
                found.insert(
                    path.clone(),
                    Row {
                        kind: node.kind,
                        size: node.own_size,
                        alloc: node.own_alloc,
                        mtime: node.mtime,
                        nlink: node.nlink,
                    },
                );
                stack.push((child, path));
            }
        }
        found
    }

    /// Every disagreement, not the first: one CI run on a machine nobody
    /// here has should say everything it can.
    fn differences(
        walked: &BTreeMap<String, Row>,
        read: &BTreeMap<String, Row>,
        sizes: impl Fn(&Row) -> bool,
    ) -> Vec<String> {
        let mut found = Vec::new();
        for (path, w) in walked {
            let Some(r) = read.get(path) else {
                found.push(format!("{path}: walked, not in the table"));
                continue;
            };
            let mut fields = vec![
                ("kind", format!("{:?}", w.kind), format!("{:?}", r.kind)),
                ("mtime", w.mtime.to_string(), r.mtime.to_string()),
                ("nlink", w.nlink.to_string(), r.nlink.to_string()),
            ];
            if sizes(w) {
                fields.push(("size", w.size.to_string(), r.size.to_string()));
                fields.push(("alloc", w.alloc.to_string(), r.alloc.to_string()));
            }
            for (field, a, b) in fields {
                if a != b {
                    found.push(format!("{path}: {field}: walk {a}, table {b}"));
                }
            }
        }
        for path in read.keys().filter(|path| !walked.contains_key(*path)) {
            found.push(format!("{path}: in the table, not walked"));
        }
        found
    }

    #[test]
    fn the_table_and_the_walk_agree_entry_for_entry() {
        let dir = corpus();
        let root = dir.path().canonicalize().unwrap();
        let Some((table, reference)) = table_or_skip(&root, true) else {
            return;
        };
        let walk = |opts: &ScanOptions| ScanOptions {
            read_mft: false,
            ..opts.clone()
        };

        // Deduplication off: every name carries its own file, so every field
        // of every entry can be compared.
        let each = ScanOptions {
            dedupe_hardlinks: false,
            ..ScanOptions::default()
        };
        let (walked, walk_stats) = scan_source(&root, walk(&each), Source::Volume).unwrap();
        let (read, read_stats) =
            scan_source(&root, each, Source::Table(table.clone(), reference)).unwrap();
        let diff = differences(&rows(&walked), &rows(&read), |_| true);
        assert!(
            diff.is_empty(),
            "{} disagreements:\n{}",
            diff.len(),
            diff.join("\n")
        );
        assert_eq!(walk_stats.errors, 0, "{:?}", walk_stats.error_samples);
        assert_eq!(read_stats.errors, 0, "{:?}", read_stats.error_samples);
        assert_eq!(walked.total_size(), read.total_size());
        assert_eq!(walked.total_alloc(), read.total_alloc());

        // With deduplication and `-x`: link counts and volumes are read, and
        // which name of a hardlinked file carries it is undefined, so those
        // names are compared by their sum.
        let full = ScanOptions {
            one_filesystem: true,
            ..ScanOptions::default()
        };
        let (walked, walk_stats) = scan_source(&root, walk(&full), Source::Volume).unwrap();
        let (read, read_stats) = scan_source(&root, full, Source::Table(table, reference)).unwrap();
        let diff = differences(&rows(&walked), &rows(&read), |row| row.nlink <= 1);
        assert!(
            diff.is_empty(),
            "{} disagreements with deduplication on:\n{}",
            diff.len(),
            diff.join("\n")
        );
        assert_eq!(walked.total_size(), read.total_size());
        assert_eq!(walked.total_alloc(), read.total_alloc());
        assert_eq!(walk_stats.hardlinks_deduped, 2);
        assert_eq!(read_stats.hardlinks_deduped, 2);
        assert_eq!(
            (walk_stats.files, walk_stats.dirs),
            (read_stats.files, read_stats.dirs)
        );
    }

    /// The default path on a whole volume — the system drive, which on the
    /// runner holds a few million records — reads the table, and what it
    /// reads matches what the file system says about a file everybody has.
    #[test]
    fn a_whole_volume_is_read_from_its_table() {
        let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        let root = PathBuf::from(format!(r"{drive}\")).canonicalize().unwrap();
        // The whole-volume rule admits the root: this is the call the
        // default path makes. Should it fail, the scan below would walk the
        // whole drive instead, which takes many minutes.
        let Some((table, reference)) = table_or_skip(&root, false) else {
            return;
        };
        let opts = ScanOptions {
            dedupe_hardlinks: false,
            ..ScanOptions::default()
        };
        let (from_table, table_stats) =
            scan_source(&root, opts.clone(), Source::Table(table, reference)).unwrap();
        let (default, stats) = scan_source(&root, opts, Source::Volume).unwrap();
        eprintln!(
            "system volume: {} files, {} dirs, {} errors, {} ms",
            stats.files, stats.dirs, stats.errors, stats.duration_ms
        );
        assert!(stats.files > 50_000, "{} files", stats.files);

        // The volume is live, so two reads a few seconds apart are close, not
        // equal. A walk would differ in kind: errors where it may not open a
        // folder, and minutes rather than seconds.
        let close = |a: u64, b: u64| a.abs_diff(b) <= a.max(b) / 100;
        assert!(
            close(stats.files, table_stats.files),
            "{stats:?} vs {table_stats:?}"
        );
        assert!(
            close(stats.dirs, table_stats.dirs),
            "{stats:?} vs {table_stats:?}"
        );
        assert_eq!(
            stats.errors, table_stats.errors,
            "{:?}",
            stats.error_samples
        );

        let on_disk = fs::metadata(root.join(r"Windows\System32\ntdll.dll")).unwrap();
        for tree in [&default, &from_table] {
            let ntdll = tree
                .find("Windows/System32/ntdll.dll")
                .expect("ntdll.dll is listed");
            assert_eq!(tree.node(ntdll).size, on_disk.len());
            assert_eq!(tree.node(ntdll).kind, EntryKind::File);
            assert!(tree.find("$MFT").is_none(), "metafiles are not entries");
        }
    }

    /// A folder below a volume's root walks, whatever the privileges.
    #[test]
    fn a_folder_below_a_volume_root_is_walked() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        assert!(super::read(&root, false, &ScanProgress::default()).is_none());
    }

    /// `\\?\` in front of a drive letter goes; a network path has no volume.
    #[test]
    fn verbatim_prefixes_are_understood() {
        let plain = super::plain_path(Path::new(r"\\?\C:\Users")).unwrap();
        assert_eq!(plain, OsStr::new(r"C:\Users"));
        assert!(super::plain_path(Path::new(r"\\?\UNC\server\share")).is_none());
        let plain = super::plain_path(Path::new(r"D:\data")).unwrap();
        assert_eq!(plain, OsStr::new(r"D:\data"));
    }
}
