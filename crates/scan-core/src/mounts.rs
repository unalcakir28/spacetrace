//! Where one filesystem ends and another begins.
//!
//! Read once at the start of a scan, for one purpose: to know **before
//! touching a path** that it is a mount point. That matters because the walk's
//! first act on any entry is an `lstat`, and an `lstat` into a filesystem
//! whose server has gone away never returns and cannot portably be
//! interrupted. Knowing the boundary in advance is what makes it possible to
//! approach it carefully instead of walking into it.
//!
//! **A mount point is not a dead mount.** Everything here reports structure,
//! never health: a healthy network share is a mount point too and must be
//! scanned like anything else. Deciding that one has stopped answering is the
//! caller's job, and it costs a timeout.
//!
//! Unknown platforms return an empty set, which is exactly the behaviour the
//! scanner had before this module existed.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The directories other filesystems are mounted on.
///
/// Never an error: a scanner that refused to run because it could not read the
/// mount table would be trading a working scan for a precaution. An empty set
/// degrades to "treat every directory as ordinary", which is what every
/// version before this did.
#[derive(Debug, Clone, Default)]
pub struct Mounts {
    points: HashSet<PathBuf>,
    /// The directories that *contain* a mount point.
    ///
    /// Derived so the walk can pay once per directory instead of once per
    /// entry. A directory not in here cannot hold a boundary, and on a real
    /// disk that is all but a handful of them — which is what keeps this
    /// protection out of the hottest loop in the program.
    parents: HashSet<PathBuf>,
}

impl Mounts {
    /// The mount table as the kernel currently reports it.
    pub fn read() -> Self {
        Self::build(read_points())
    }

    /// An empty table, for callers that have switched the protection off and
    /// for tests.
    pub fn none() -> Self {
        Mounts::default()
    }

    /// Build one from known paths. Tests use this; so does any caller that has
    /// a table from somewhere else.
    pub fn from_paths<I, P>(paths: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        Self::build(paths.into_iter().map(Into::into).collect())
    }

    fn build(points: HashSet<PathBuf>) -> Self {
        let parents = points
            .iter()
            .filter_map(|p| p.parent().map(Path::to_path_buf))
            .collect();
        Mounts { points, parents }
    }

    /// Whether anything mounted lives directly inside `dir`.
    ///
    /// Asked once per directory the walk lists. When it is false — the
    /// overwhelmingly common case — not a single entry in that directory is
    /// looked up.
    pub fn holds_a_boundary(&self, dir: &Path) -> bool {
        self.parents.contains(dir)
    }

    /// Whether `path` is the root of a mounted filesystem.
    ///
    /// An exact match on the path as the kernel spells it. No normalisation:
    /// the walk builds its paths by pushing directory entry names onto a
    /// canonicalised root, which is the same spelling `getmntinfo` and
    /// `mountinfo` report, and guessing at equivalences here would risk
    /// treating an ordinary directory as a boundary.
    pub fn contains(&self, path: &Path) -> bool {
        self.points.contains(path)
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }
}

#[cfg(target_os = "macos")]
fn read_points() -> HashSet<PathBuf> {
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStrExt;

    let mut set = HashSet::new();
    // SAFETY: `getmntinfo` fills `buf` with a pointer to a static array it
    // owns and returns how many entries are valid. Nothing here frees it, and
    // it is only read before the next call on this thread.
    unsafe {
        let mut buf: *mut libc::statfs = std::ptr::null_mut();
        // MNT_NOWAIT, deliberately. MNT_WAIT asks every filesystem to refresh
        // its statistics first, which on a server that has gone away is the
        // very hang this module exists to avoid — the precaution would become
        // the bug.
        let count = libc::getmntinfo(&mut buf, libc::MNT_NOWAIT);
        if count <= 0 || buf.is_null() {
            return set;
        }
        for i in 0..count as isize {
            let entry = &*buf.offset(i);
            let name = CStr::from_ptr(entry.f_mntonname.as_ptr());
            set.insert(PathBuf::from(std::ffi::OsStr::from_bytes(name.to_bytes())));
        }
    }
    set
}

#[cfg(target_os = "linux")]
fn read_points() -> HashSet<PathBuf> {
    // `/proc/self/mountinfo` rather than `/etc/mtab` or a libc call: it is the
    // kernel's own view, it is readable without privileges, and reading a
    // procfs file cannot block on a dead server the way `statfs` can.
    match std::fs::read_to_string("/proc/self/mountinfo") {
        Ok(text) => parse_mountinfo(&text),
        Err(_) => HashSet::new(),
    }
}

/// Mount points out of `/proc/self/mountinfo`.
///
/// Compiled under `test` on every Unix so the parser can be exercised from a
/// Mac, but **not** on Windows: it decodes raw bytes through `OsStringExt`,
/// which has no Windows counterpart. Leaving `test` unqualified broke the
/// Windows build, and only in CI — `cargo check --target …-windows-msvc`
/// without `--all-targets` does not compile test code.
///
/// The mount point is the fifth space-separated field. Octal escapes are
/// undone because the kernel writes `\040` for a space, and a path with a
/// space in it is common enough on removable media to matter.
#[cfg(any(target_os = "linux", all(test, unix)))]
fn parse_mountinfo(text: &str) -> HashSet<PathBuf> {
    use std::os::unix::ffi::OsStringExt;

    text.lines()
        .filter_map(|line| line.split(' ').nth(4))
        .map(|field| PathBuf::from(std::ffi::OsString::from_vec(unescape(field))))
        .collect()
}

/// `\040` and friends back to their bytes.
#[cfg(any(target_os = "linux", all(test, unix)))]
fn unescape(field: &str) -> Vec<u8> {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // Only a backslash followed by three octal digits is an escape; a
        // lone backslash is a legal character in a path and stays one.
        let escape = bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && bytes[i + 1..i + 4]
                .iter()
                .all(|b| (b'0'..=b'7').contains(b));
        if !escape {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        let value = bytes[i + 1..i + 4]
            .iter()
            .fold(0u32, |acc, b| acc * 8 + u32::from(b - b'0'));
        out.push(value as u8);
        i += 4;
    }
    out
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn read_points() -> HashSet<PathBuf> {
    // Windows has volume mount points too, and they can hang for the same
    // reasons; the equivalent walk there is `FindFirstVolumeMountPoint`.
    // Left undone rather than guessed at, because none of it can be tested
    // from here and an untested precaution is a liability.
    HashSet::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Against the real kernel, because a parser that agrees with a fixture
    /// and disagrees with the machine is worth nothing. Every system this
    /// runs on has a root filesystem; if that is missing, the table was not
    /// read at all.
    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn the_root_filesystem_is_in_the_table() {
        let mounts = Mounts::read();
        assert!(
            mounts.contains(Path::new("/")),
            "read {} mount points and none of them was /",
            mounts.len()
        );
    }

    /// An ordinary directory must not be mistaken for a boundary: doing so
    /// would put every scan of a home directory through the slow, careful
    /// path for no reason.
    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn an_ordinary_directory_is_not_a_mount_point() {
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("plain");
        std::fs::create_dir(&inner).unwrap();
        assert!(!Mounts::read().contains(&inner));
    }

    #[test]
    fn an_empty_table_finds_nothing() {
        assert!(!Mounts::none().contains(Path::new("/")));
        assert!(Mounts::none().is_empty());
    }

    #[test]
    fn a_table_can_be_built_from_known_paths() {
        let mounts = Mounts::from_paths(["/mnt/one", "/mnt/two"]);
        assert!(mounts.contains(Path::new("/mnt/one")));
        assert!(!mounts.contains(Path::new("/mnt/three")));
        assert_eq!(mounts.len(), 2);
    }

    #[test]
    #[cfg(unix)]
    fn mountinfo_lines_yield_their_mount_points() {
        let text = "\
25 0 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw
26 25 0:22 / /proc rw,nosuid - proc proc rw
27 25 0:6 / /mnt/backup rw - nfs4 server:/export rw
";
        let points = parse_mountinfo(text);
        assert!(points.contains(Path::new("/")));
        assert!(points.contains(Path::new("/proc")));
        assert!(points.contains(Path::new("/mnt/backup")));
        assert_eq!(points.len(), 3);
    }

    /// The kernel writes `\040` for a space, and removable media are full of
    /// names with spaces in them. Taking the field literally would leave a
    /// mount point that never matches anything.
    #[test]
    #[cfg(unix)]
    fn an_escaped_space_in_a_mount_point_is_decoded() {
        let text = "40 25 8:2 / /media/My\\040Backup\\040Disk rw - vfat /dev/sdb1 rw\n";
        let points = parse_mountinfo(text);
        assert!(
            points.contains(Path::new("/media/My Backup Disk")),
            "got {points:?}"
        );
    }

    /// A backslash is a legal character in a Unix path, so only a complete
    /// octal escape may be consumed.
    #[test]
    #[cfg(unix)]
    fn a_lone_backslash_survives() {
        assert_eq!(unescape(r"a\b"), b"a\\b");
        assert_eq!(unescape(r"a\04"), b"a\\04");
        assert_eq!(unescape(r"a\040b"), b"a b");
    }

    #[test]
    #[cfg(unix)]
    fn a_short_or_malformed_mountinfo_line_is_skipped_not_panicked_on() {
        assert!(parse_mountinfo("garbage\n\n25 0 8:1 /\n").is_empty());
    }

    /// The per-directory filter. It is what keeps the mount check out of the
    /// per-entry loop, so it has to agree with `contains` exactly.
    #[test]
    fn only_directories_that_hold_a_mount_point_are_flagged() {
        let mounts = Mounts::from_paths(["/mnt/backup", "/Volumes/photos"]);
        assert!(mounts.holds_a_boundary(Path::new("/mnt")));
        assert!(mounts.holds_a_boundary(Path::new("/Volumes")));
        assert!(
            !mounts.holds_a_boundary(Path::new("/mnt/backup")),
            "the mount point itself holds nothing; its parent does"
        );
        assert!(!mounts.holds_a_boundary(Path::new("/home")));
    }

    /// On this machine, the directory holding the mount table's own entries
    /// must be flagged. A derived index that disagrees with the real table
    /// would switch the protection off silently.
    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn every_real_mount_point_has_a_flagged_parent() {
        let mounts = Mounts::read();
        for point in &mounts.points {
            if let Some(parent) = point.parent() {
                assert!(
                    mounts.holds_a_boundary(parent),
                    "{} is mounted but {} is not flagged",
                    point.display(),
                    parent.display()
                );
            }
        }
    }
}
