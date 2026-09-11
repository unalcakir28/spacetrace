//! macOS: one syscall for a directory's names *and* their metadata.
//!
//! The ordinary walk costs one `readdir` per directory plus one `lstat` per
//! entry. `getattrlistbulk` answers both at once, in batches.
//! **Measured on `~/github` (297,695 entries, single-threaded listing):
//! 1710 ms → 520 ms, 3.3×**, with the two paths agreeing on every byte of
//! every total. The distributions did not overlap, so this is not the kind of
//! number the machine's noise can produce.
//!
//! Two things about this file are load-bearing.
//!
//! **It must produce the same `RawMeta` as `lstat` does, not merely a
//! plausible one.** A second metadata path that quietly disagrees with the
//! first is the failure mode `store::digest` warns about in its own header:
//! the disagreement shows up years later as "your snapshot is corrupt" about
//! a snapshot that is fine. `same_answer_as_lstat` in the tests is the guard.
//!
//! **It is not used on a directory that contains a mount point.** The whole
//! directory comes back in one call, so there is no per-entry moment at which
//! a filesystem that has stopped answering can be given a deadline — the
//! protection in `mounts.rs` needs the slow path, and the walk hands those
//! directories to it.

use std::ffi::{CStr, CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::meta::{EntryKind, NamedMeta, RawMeta};

/// Not in libc's Apple constants.
const ATTR_CMN_ERROR: libc::attrgroup_t = 0x2000_0000;

/// `fsobj_type_t`. Only the three we model are named.
const VREG: u32 = 1;
const VDIR: u32 = 2;
const VLNK: u32 = 5;

/// Batch buffer. Large enough that a directory of a few thousand entries takes
/// one or two calls, small enough to sit on the stack of every walk thread
/// without thought — it is heap-allocated per call, and at 256 KiB the
/// allocation is noise next to the syscalls it saves.
const BUF_BYTES: usize = 256 * 1024;

/// Every entry in `dir`, or `None` when this path cannot serve.
///
/// `None` rather than an error, and deliberately for *any* failure: the
/// caller's fallback is the ordinary walk, which is not merely a consolation
/// but the thing that was correct yesterday. A filesystem that does not
/// implement this call answers `ENOTSUP`, and there is nothing to report about
/// a machine whose scan simply takes the route it always took.
pub(crate) fn list(dir: &Path) -> Option<Vec<NamedMeta>> {
    let c_dir = CString::new(dir.as_os_str().as_bytes()).ok()?;
    // SAFETY: `c_dir` is a valid NUL-terminated path for the duration of the
    // call, and the result is checked before use.
    let fd = unsafe { libc::open(c_dir.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if fd < 0 {
        return None;
    }
    let out = read_all(fd, dir);
    // SAFETY: `fd` was opened above and is not used after this point.
    unsafe { libc::close(fd) };
    out
}

fn attr_list() -> libc::attrlist {
    // SAFETY: `attrlist` is a plain struct of integers; all-zero is a valid
    // "ask for nothing" value, which the fields below then narrow.
    let mut attrs: libc::attrlist = unsafe { std::mem::zeroed() };
    attrs.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
    attrs.commonattr = libc::ATTR_CMN_RETURNED_ATTRS
        | ATTR_CMN_ERROR
        | libc::ATTR_CMN_NAME
        | libc::ATTR_CMN_DEVID
        | libc::ATTR_CMN_OBJTYPE
        | libc::ATTR_CMN_MODTIME
        | libc::ATTR_CMN_FILEID;
    attrs.dirattr = libc::ATTR_DIR_LINKCOUNT | libc::ATTR_DIR_ALLOCSIZE | libc::ATTR_DIR_DATALENGTH;
    attrs.fileattr =
        libc::ATTR_FILE_LINKCOUNT | libc::ATTR_FILE_ALLOCSIZE | libc::ATTR_FILE_DATALENGTH;
    attrs
}

fn read_all(fd: libc::c_int, dir: &Path) -> Option<Vec<NamedMeta>> {
    let mut attrs = attr_list();
    let mut buf = vec![0u8; BUF_BYTES];
    let mut out = Vec::new();

    loop {
        // SAFETY: `fd` is open, `attrs` outlives the call, and the kernel
        // writes at most `buf.len()` bytes into a buffer we own.
        let count = unsafe {
            libc::getattrlistbulk(
                fd,
                &mut attrs as *mut _ as *mut libc::c_void,
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
                0,
            )
        };
        // Any failure hands the whole directory back to the ordinary walk
        // rather than returning half of it. A partial directory is the one
        // answer this scanner must never give (invariant #5).
        if count < 0 {
            return None;
        }
        if count == 0 {
            return Some(out);
        }

        let mut entry = buf.as_ptr();
        for _ in 0..count {
            // SAFETY: the kernel guarantees `count` well-formed records, each
            // starting with its own length, laid out as the attribute bitmap
            // describes. `decode` reads only within the record it is given.
            let (length, decoded) = unsafe { decode(entry, dir) };
            if let Some(item) = decoded {
                out.push(item);
            }
            // SAFETY: records are contiguous and `length` covers this one.
            entry = unsafe { entry.add(length) };
        }
    }
}

/// Read one field and step over it.
///
/// Strictly sequential with no padding, and only for attributes the kernel
/// says it returned — the layout the `getattrlistbulk` manual page's own
/// sample walks. `read_unaligned` because nothing promises these offsets land
/// on a boundary the type would like.
///
/// # Safety
/// `cursor` must point at `size_of::<T>()` readable bytes inside the record.
unsafe fn take<T: Copy>(cursor: &mut *const u8) -> T {
    let value = (*cursor as *const T).read_unaligned();
    *cursor = cursor.add(std::mem::size_of::<T>());
    value
}

/// Decode one record. Returns its length in bytes — always, so the caller can
/// step past a record it could not use — and the entry when there is one.
///
/// # Safety
/// `start` must point at a well-formed record produced by `getattrlistbulk`.
unsafe fn decode(start: *const u8, dir: &Path) -> (usize, Option<NamedMeta>) {
    let mut p = start;
    let length: u32 = take(&mut p);

    // `attribute_set_t` is five bitmaps with no header: commonattr, volattr,
    // dirattr, fileattr, forkattr. Reading these one slot along — as if it
    // were `attrlist`, which *does* have a header — makes every entry look
    // like an error, which is exactly what happened the first time.
    let returned: [u32; 5] = take(&mut p);
    let (common, dirattr, fileattr) = (returned[0], returned[2], returned[3]);

    let mut error: u32 = 0;
    if common & ATTR_CMN_ERROR != 0 {
        error = take(&mut p);
    }

    let mut name = None;
    if common & libc::ATTR_CMN_NAME != 0 {
        let at = p;
        let reference: libc::attrreference_t = take(&mut p);
        let bytes =
            CStr::from_ptr(at.offset(reference.attr_dataoffset as isize) as *const libc::c_char);
        name = Some(OsStr::from_bytes(bytes.to_bytes()).to_os_string());
    }

    let mut dev = 0u64;
    if common & libc::ATTR_CMN_DEVID != 0 {
        let raw: libc::dev_t = take(&mut p);
        dev = raw as u64;
    }

    let mut kind = EntryKind::Other;
    if common & libc::ATTR_CMN_OBJTYPE != 0 {
        let obj: u32 = take(&mut p);
        kind = match obj {
            VREG => EntryKind::File,
            VDIR => EntryKind::Dir,
            VLNK => EntryKind::Symlink,
            _ => EntryKind::Other,
        };
    }

    let mut mtime = 0i64;
    if common & libc::ATTR_CMN_MODTIME != 0 {
        let ts: libc::timespec = take(&mut p);
        mtime = ts.tv_sec;
    }

    let mut ino = 0u64;
    if common & libc::ATTR_CMN_FILEID != 0 {
        ino = take(&mut p);
    }

    let (mut nlink, mut alloc, mut size) = (0u64, 0u64, 0u64);
    if dirattr & libc::ATTR_DIR_LINKCOUNT != 0 {
        let n: u32 = take(&mut p);
        nlink = n as u64;
    }
    if dirattr & libc::ATTR_DIR_ALLOCSIZE != 0 {
        let v: i64 = take(&mut p);
        alloc = v as u64;
    }
    if dirattr & libc::ATTR_DIR_DATALENGTH != 0 {
        let v: i64 = take(&mut p);
        size = v as u64;
    }
    if fileattr & libc::ATTR_FILE_LINKCOUNT != 0 {
        let n: u32 = take(&mut p);
        nlink = n as u64;
    }
    if fileattr & libc::ATTR_FILE_ALLOCSIZE != 0 {
        let v: i64 = take(&mut p);
        alloc = v as u64;
    }
    if fileattr & libc::ATTR_FILE_DATALENGTH != 0 {
        let v: i64 = take(&mut p);
        size = v as u64;
    }

    let length = length as usize;

    // An entry the kernel could not describe, or one it did not name, is left
    // out of the batch. The caller cannot fall back for a single entry — it
    // has already committed to this path for the whole directory — so the
    // only honest thing is to not have it, and the walk counts what it did
    // not get the same way it counts an unreadable path.
    let Some(name) = name else {
        return (length, None);
    };
    if error != 0 {
        return (length, None);
    }

    // `ATTR_DIR_LINKCOUNT` is the directory's *real* hard-link count, which on
    // APFS is 1. `st_nlink` is the synthesised 2-plus-subdirectories every
    // Unix tool prints, and it is what the ordinary path stores. One `lstat`
    // per directory keeps the two paths reporting the same number; measured,
    // it costs nothing (3.31× with it, 3.29× without), because the kernel just
    // handed us that inode and it is still warm.
    if kind == EntryKind::Dir {
        if let Ok(md) = std::fs::symlink_metadata(dir.join(&name)) {
            nlink = md.nlink();
        }
    }

    let meta = RawMeta {
        kind,
        size,
        alloc,
        mtime,
        nlink,
        ino,
        dev,
    };
    (length, Some(NamedMeta { name, meta }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::ffi::OsString;

    /// Both paths, over the same directory, entry by entry.
    ///
    /// This is the test that makes the fast path safe to have. A faster answer
    /// that differs from the slow one in any field is not an optimisation, it
    /// is a second source of truth — and the first field to drift silently
    /// would be one nobody reads until a total comes out wrong.
    fn assert_same_answer_as_lstat(dir: &Path) {
        let bulk: BTreeMap<OsString, RawMeta> = list(dir)
            .expect("getattrlistbulk should work on a temporary directory")
            .into_iter()
            .map(|e| (e.name, e.meta))
            .collect();

        let mut slow: BTreeMap<OsString, RawMeta> = BTreeMap::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let md = entry.metadata().unwrap();
            let (meta, failure) =
                RawMeta::for_path(&entry.path(), &md, crate::meta::FileIdentity::Needed);
            assert!(failure.is_none());
            slow.insert(entry.file_name(), meta);
        }

        assert_eq!(
            bulk.keys().collect::<Vec<_>>(),
            slow.keys().collect::<Vec<_>>(),
            "the two paths must see the same entries"
        );
        for (name, expected) in &slow {
            assert_eq!(bulk.get(name), Some(expected), "disagreed about {name:?}");
        }
    }

    #[test]
    fn the_same_answer_as_lstat_for_files_and_directories() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("small.txt"), b"hello").unwrap();
        // Not block-aligned, so `size` and `alloc` differ and a test that
        // read one for the other would fail.
        std::fs::write(dir.path().join("odd.bin"), vec![7u8; 100]).unwrap();
        std::fs::write(dir.path().join("big.bin"), vec![0u8; 200_000]).unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::create_dir(dir.path().join("sub/deeper")).unwrap();
        assert_same_answer_as_lstat(dir.path());
    }

    /// Invariant 3 depends on this: a symlink is counted as itself, never as
    /// its target. If the bulk call followed links, a link to a directory
    /// would be walked into and a link to a large file would be counted at the
    /// target's size.
    #[test]
    fn symlinks_are_described_as_themselves() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("real")).unwrap();
        std::fs::write(dir.path().join("real/inside.bin"), vec![0u8; 50_000]).unwrap();
        std::fs::write(dir.path().join("target.bin"), vec![0u8; 50_000]).unwrap();
        std::os::unix::fs::symlink("target.bin", dir.path().join("to-file")).unwrap();
        std::os::unix::fs::symlink("real", dir.path().join("to-dir")).unwrap();
        std::os::unix::fs::symlink("/nowhere-at-all", dir.path().join("broken")).unwrap();

        assert_same_answer_as_lstat(dir.path());

        let by_name: BTreeMap<OsString, RawMeta> = list(dir.path())
            .unwrap()
            .into_iter()
            .map(|e| (e.name, e.meta))
            .collect();
        for link in ["to-file", "to-dir", "broken"] {
            assert_eq!(
                by_name[OsStr::new(link)].kind,
                EntryKind::Symlink,
                "{link} should be a symlink, not what it points at"
            );
        }
    }

    /// A hardlinked file has to arrive with `nlink > 1`, or the deduplication
    /// that invariant 3 promises never triggers on this path.
    #[test]
    fn a_hardlink_arrives_with_its_link_count() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("one.bin"), vec![0u8; 40_000]).unwrap();
        std::fs::hard_link(dir.path().join("one.bin"), dir.path().join("two.bin")).unwrap();

        assert_same_answer_as_lstat(dir.path());
        for entry in list(dir.path()).unwrap() {
            assert!(
                entry.meta.is_hardlinked(),
                "{:?} should look hardlinked",
                entry.name
            );
        }
    }

    #[test]
    fn an_empty_directory_yields_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(list(dir.path()).unwrap().is_empty());
    }

    /// More entries than one buffer holds, so the batching loop runs more than
    /// once. A loop that only ever ran once would pass every test above.
    #[test]
    fn a_directory_larger_than_one_batch_is_read_completely() {
        let dir = tempfile::tempdir().unwrap();
        // Long names so the records are fat and several batches are needed.
        for i in 0..4_000 {
            let name = format!("entry-{i:05}-{}", "x".repeat(180));
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        assert_eq!(list(dir.path()).unwrap().len(), 4_000);
        assert_same_answer_as_lstat(dir.path());
    }

    /// A path that is not a directory cannot be listed this way, and the
    /// answer has to be "use the other path" rather than a panic.
    #[test]
    fn a_file_is_not_listable_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("plain.txt");
        std::fs::write(&file, b"x").unwrap();
        assert!(list(&file).is_none());
        assert!(list(&dir.path().join("does-not-exist")).is_none());
    }
}
