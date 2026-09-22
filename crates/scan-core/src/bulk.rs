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

/// `EF_MAY_SHARE_BLOCKS` from `<sys/stat.h>`, which libc does not carry
/// either: this file's blocks may be held by another file too.
const EF_MAY_SHARE_BLOCKS: u64 = 0x0000_0001;

/// `fsobj_type_t`. Only the three we model are named.
const VREG: u32 = 1;
const VDIR: u32 = 2;
const VLNK: u32 = 5;

/// Batch buffer, in `u64` words.
///
/// Large enough that a directory of a few thousand entries takes one or two
/// calls. **One per walk thread, reused for every directory it lists** — it
/// used to be allocated per call, which at 256 KiB was a fresh mapping from
/// the allocator for every directory on the disk. Measured on 1,064,452
/// entries the wall time did not move — the allocator hands back a fresh
/// mapping for a block this size and takes it away again cheaply. It is kept
/// because it removes one map/unmap pair per directory from a process whose
/// peak memory is already the thing macOS is worst at giving back (TODO D4),
/// not because it made the walk faster.
///
/// `u64` rather than `u8` so the eight-byte alignment `getattrlistbulk` wants
/// for its records is a property of the type instead of an assumption about
/// what the allocator happens to return for a byte slice.
const BUF_WORDS: usize = 32 * 1024;

thread_local! {
    static BUFFER: std::cell::RefCell<Vec<u64>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Run `f` with this thread's batch buffer.
///
/// Not re-entrant, and does not need to be: the only thing that runs inside is
/// the decoding of records already in the buffer.
fn with_buffer<R>(f: impl FnOnce(&mut [u8]) -> R) -> R {
    BUFFER.with(|cell| {
        let mut words = cell.borrow_mut();
        if words.is_empty() {
            words.resize(BUF_WORDS, 0);
        }
        let len = words.len() * std::mem::size_of::<u64>();
        // SAFETY: every bit pattern of a `u64` is a valid `u8`, the pointer
        // comes from a live allocation this borrow keeps alive, and `len`
        // describes exactly the same bytes.
        let bytes = unsafe { std::slice::from_raw_parts_mut(words.as_mut_ptr().cast::<u8>(), len) };
        f(bytes)
    })
}

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

/// What to ask each record for.
///
/// `extended` adds the two APFS attributes that identify a copy-on-write
/// clone. They live in `forkattr`, which the kernel only reads when
/// `FSOPT_ATTR_CMN_EXTENDED` is passed alongside — the two go together or
/// neither works, which is why `options_for` sits next to this.
fn attr_list(extended: bool) -> libc::attrlist {
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
    if extended {
        attrs.forkattr = libc::ATTR_CMNEXT_CLONEID | libc::ATTR_CMNEXT_EXT_FLAGS;
    }
    attrs
}

fn options_for(extended: bool) -> u64 {
    if extended {
        u64::from(libc::FSOPT_ATTR_CMN_EXTENDED)
    } else {
        0
    }
}

fn read_all(fd: libc::c_int, dir: &Path) -> Option<Vec<NamedMeta>> {
    with_buffer(|buf| read_into(fd, dir, buf))
}

fn read_into(fd: libc::c_int, dir: &Path, buf: &mut [u8]) -> Option<Vec<NamedMeta>> {
    let mut extended = true;
    let mut attrs = attr_list(extended);
    let mut out = Vec::new();
    let mut read_any = false;

    loop {
        // SAFETY: `fd` is open, `attrs` outlives the call, and the kernel
        // writes at most `buf.len()` bytes into a buffer we own.
        let count = unsafe {
            libc::getattrlistbulk(
                fd,
                &mut attrs as *mut _ as *mut libc::c_void,
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
                options_for(extended),
            )
        };
        if count < 0 {
            // A kernel or filesystem that refuses the APFS attributes would
            // otherwise take the whole fast path down with them, for every
            // directory on the volume. Asked again once, without them.
            //
            // **Only before any record has been read.** The call advances the
            // descriptor's position in the directory, so starting over after
            // a successful batch would silently skip it — and a directory
            // half reported is the one answer this scanner must never give
            // (invariant #5). Every other failure still hands the caller the
            // ordinary walk, which is what was correct yesterday.
            if extended && !read_any {
                extended = false;
                attrs = attr_list(extended);
                continue;
            }
            return None;
        }
        if count == 0 {
            return Some(out);
        }
        read_any = true;

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

/// The clone family a file's blocks belong to, or `None` when it has none.
///
/// APFS hands every file a clone id, so the id on its own says nothing — two
/// unrelated files each have one. `EF_MAY_SHARE_BLOCKS` is the bit that says
/// these blocks are actually held by more than one file, and gating on it is
/// what stops the key from grouping files that merely exist. Measured on a
/// fixture of one plain file and three clones: the plain file reported
/// `ext_flags = 0x0`, the three clones `0x41` and one shared id.
fn share_key(clone_id: u64, ext_flags: u64) -> Option<u64> {
    if ext_flags & EF_MAY_SHARE_BLOCKS == 0 {
        return None;
    }
    (clone_id != 0).then_some(clone_id)
}

/// The clone family of one file, asked about by path.
///
/// The bulk listing answers this for free, so this is only for the entries
/// that never went through it: a directory holding a mount point, which the
/// walk has to read the slow way (see the header). `getattrlist` rather than
/// the `fcntl(F_LOG2PHYS_EXT)` this used to do — it needs no open file
/// descriptor, and it returns the filesystem's own identity for the family
/// instead of a physical offset, so both listing paths key on the same thing.
///
/// `FSOPT_NOFOLLOW` because a symlink is counted as itself (invariant #3); a
/// link to a clone must not be charged as one.
pub(crate) fn clone_key(path: &Path) -> Option<u64> {
    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: as in `attr_list` — all-zero is "ask for nothing".
    let mut attrs: libc::attrlist = unsafe { std::mem::zeroed() };
    attrs.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
    attrs.commonattr = libc::ATTR_CMN_RETURNED_ATTRS;
    attrs.forkattr = libc::ATTR_CMNEXT_CLONEID | libc::ATTR_CMNEXT_EXT_FLAGS;

    // The record is a `u32` length, five returned bitmaps and two `u64`s; the
    // slack is for alignment the kernel is free to insert between them.
    let mut buf = [0u8; 64];
    // SAFETY: the path is a valid NUL-terminated string for the call, `attrs`
    // outlives it, and the kernel writes at most `buf.len()` bytes into a
    // buffer we own.
    let rc = unsafe {
        libc::getattrlist(
            c_path.as_ptr(),
            &mut attrs as *mut _ as *mut libc::c_void,
            buf.as_mut_ptr() as *mut libc::c_void,
            buf.len(),
            libc::FSOPT_ATTR_CMN_EXTENDED | libc::FSOPT_NOFOLLOW,
        )
    };
    if rc != 0 {
        return None;
    }

    let mut p = buf.as_ptr();
    // SAFETY: the kernel wrote a well-formed record into `buf`, and the reads
    // below stay inside the length it declared.
    unsafe {
        let _length: u32 = take(&mut p);
        let returned: [u32; 5] = take(&mut p);
        let forkattr = returned[4];
        if forkattr & libc::ATTR_CMNEXT_CLONEID == 0 || forkattr & libc::ATTR_CMNEXT_EXT_FLAGS == 0
        {
            return None;
        }
        let clone_id: u64 = take(&mut p);
        let ext_flags: u64 = take(&mut p);
        share_key(clone_id, ext_flags)
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
    let (common, dirattr, fileattr, forkattr) =
        (returned[0], returned[2], returned[3], returned[4]);

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

    // Last in the record, after the fork attributes were asked for in
    // `forkattr`. A filesystem that cannot answer simply leaves the bits out
    // of the returned bitmap and writes nothing here, which is the whole
    // reason the bitmap is read rather than assumed.
    let (mut clone_id, mut ext_flags) = (0u64, 0u64);
    if forkattr & libc::ATTR_CMNEXT_CLONEID != 0 {
        clone_id = take(&mut p);
    }
    if forkattr & libc::ATTR_CMNEXT_EXT_FLAGS != 0 {
        ext_flags = take(&mut p);
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
    // Only a regular file can share blocks, and asking the question of a
    // directory would key a whole family on whatever id it happens to carry.
    let share = (kind == EntryKind::File)
        .then(|| share_key(clone_id, ext_flags))
        .flatten();
    (length, Some(NamedMeta { name, meta, share }))
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
    ///
    /// The clone family is compared too, against the by-path call the walk
    /// uses when it cannot list in bulk. Those two disagreeing would mean a
    /// directory holding a mount point deduplicated differently from every
    /// other directory on the same disk, and nothing but this would say so.
    fn assert_same_answer_as_lstat(dir: &Path) {
        let bulk: BTreeMap<OsString, (RawMeta, Option<u64>)> = list(dir)
            .expect("getattrlistbulk should work on a temporary directory")
            .into_iter()
            .map(|e| (e.name, (e.meta, e.share)))
            .collect();

        let mut slow: BTreeMap<OsString, (RawMeta, Option<u64>)> = BTreeMap::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let md = entry.metadata().unwrap();
            let path = entry.path();
            let (meta, failure) = RawMeta::for_path(&path, &md, crate::meta::FileIdentity::Needed);
            assert!(failure.is_none());
            let share = (meta.kind == EntryKind::File)
                .then(|| clone_key(&path))
                .flatten();
            slow.insert(entry.file_name(), (meta, share));
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

    /// The clone family arrives inside the listing, which is the whole reason
    /// there is no probe phase after the walk any more.
    ///
    /// Three names, one family: the original and two `cp -c` clones must share
    /// a key, and a plain file of the same size must have none at all. Without
    /// the second half a `share_key` that returned `Some` for everything would
    /// pass, and every file on the disk would deduplicate against its
    /// neighbours.
    #[test]
    fn clones_share_a_key_and_plain_files_have_none() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("original.bin");
        // Incompressible, so the filesystem cannot quietly store it some other
        // way and change what is being measured.
        let bytes: Vec<u8> = (0..300_000u32)
            .map(|i| (i.wrapping_mul(2654435761) >> 24) as u8)
            .collect();
        std::fs::write(&original, &bytes).unwrap();
        std::fs::write(dir.path().join("unrelated.bin"), &bytes).unwrap();

        let cloned = |name: &str| {
            std::process::Command::new("cp")
                .arg("-c")
                .arg(&original)
                .arg(dir.path().join(name))
                .status()
                .is_ok_and(|s| s.success())
        };
        if !cloned("clone-a.bin") || !cloned("clone-b.bin") {
            eprintln!("SKIPPED: this filesystem does not support clones");
            return;
        }

        let by_name: BTreeMap<OsString, Option<u64>> = list(dir.path())
            .unwrap()
            .into_iter()
            .map(|e| (e.name, e.share))
            .collect();

        let family: Vec<Option<u64>> = ["original.bin", "clone-a.bin", "clone-b.bin"]
            .into_iter()
            .map(|n| by_name[OsStr::new(n)])
            .collect();
        assert!(
            family[0].is_some() && family.iter().all(|k| *k == family[0]),
            "the three names of one clone family must key alike: {family:?}"
        );
        assert_eq!(
            by_name[OsStr::new("unrelated.bin")],
            None,
            "a file that shares nothing must not join a family"
        );

        assert_same_answer_as_lstat(dir.path());
    }

    /// A directory can carry a clone id of its own, and keying on it would
    /// charge a whole family to whatever happened to be next to it.
    #[test]
    fn a_directory_never_belongs_to_a_clone_family() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/inside.bin"), vec![3u8; 40_000]).unwrap();

        for entry in list(dir.path()).unwrap() {
            if entry.meta.kind == EntryKind::Dir {
                assert_eq!(entry.share, None, "{:?} is a directory", entry.name);
            }
        }
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
