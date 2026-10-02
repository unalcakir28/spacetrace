//! Equivalence with `du`, on the platforms that ship it.
//!
//! Invariant #1 says `alloc` matches `du` byte for byte and the repo calls that
//! a test condition. It was not one: the claim rested on manual runs, while the
//! automated tests compared totals against constants the test itself had just
//! written. That cannot catch a systematic error in how blocks are counted —
//! only an implementation nobody here wrote can.
//!
//! Two oracles are used, deliberately different in kind:
//!
//! * `alloc` is compared against `du`, an external program.
//! * `size` is compared against the naive serial walk at the bottom of this
//!   file. `du` cannot answer for logical size: BSD's `-A` rounds every file up
//!   to a block, and GNU's `--apparent-size` counts each directory's own inode
//!   size, which `size` excludes by design. A second dumb implementation is
//!   the better oracle here anyway, because it shares no code with the parallel
//!   walk or the reverse-pass aggregation.
//!
//! Windows ships no `du`, so its counterpart is construction-based instead:
//! see `windows_metadata.rs`.

#![cfg(unix)]

use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use spacetrace_scan_core::{scan, EntryKind, ScanOptions, ScanProgress};

/// A tree that exercises every case where `size` and `alloc` are allowed to
/// disagree. A fixture of plain files would pass even if block accounting were
/// wrong, because then both numbers happen to be close.
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // Block rounding upwards: three bytes still occupy a whole block.
    fs::write(root.join("tiny.txt"), b"abc").unwrap();
    // A file that is an exact multiple of the block size, so a rounding bug
    // that only ever rounds up stays visible in the tiny case above.
    fs::write(root.join("exact.bin"), vec![0u8; 8192]).unwrap();

    fs::create_dir(root.join("nested")).unwrap();
    fs::create_dir(root.join("nested/deeper")).unwrap();
    // Directory blocks count towards `alloc` but never towards `size`; an empty
    // directory is the case where that difference is the whole story.
    fs::create_dir(root.join("nested/deeper/empty")).unwrap();
    fs::write(root.join("nested/deeper/leaf.log"), vec![b'x'; 100]).unwrap();

    // Sparse: a length with no blocks behind it. This is the case invariant #6
    // exists for, and the one where reporting `size` as disk usage is worst.
    let sparse = fs::File::create(root.join("sparse.img")).unwrap();
    sparse.set_len(4 * 1024 * 1024).unwrap();
    drop(sparse);

    // Hardlink: one inode, two names. Both `du` and the scanner must charge it
    // once (invariant #3).
    fs::write(root.join("linked.dat"), vec![b'l'; 3000]).unwrap();
    fs::hard_link(
        root.join("linked.dat"),
        root.join("nested/linked-again.dat"),
    )
    .unwrap();

    // Symlink: counted at its own size, never followed. Pointed at a real file
    // so that following it would visibly inflate the total.
    std::os::unix::fs::symlink(root.join("exact.bin"), root.join("nested/link-to-exact")).unwrap();

    dir
}

/// `du`'s total for `path`, in bytes, or `None` when `du` is not installed.
///
/// GNU and BSD disagree on how to ask for bytes. GNU understands
/// `--block-size=1` and answers in bytes; BSD rejects it and only counts
/// 512-byte blocks, which is exact because POSIX fixes that unit regardless of
/// the filesystem's own block size. `-A` is not an option in either dialect:
/// BSD rounds each file up to a block and GNU adds directory inode sizes.
fn du_bytes(path: &Path) -> Option<u64> {
    if let Some(out) = run_du(&["-s", "--block-size=1"], path) {
        return Some(out);
    }
    // BSD, or a GNU too old for the long option.
    run_du_with_block_env(path).map(|blocks| blocks * 512)
}

fn run_du(args: &[&str], path: &Path) -> Option<u64> {
    let out = Command::new("du").args(args).arg(path).output().ok()?;
    if !out.status.success() {
        return None;
    }
    parse_du(&out.stdout)
}

fn run_du_with_block_env(path: &Path) -> Option<u64> {
    let out = Command::new("du")
        .env("BLOCKSIZE", "512")
        .arg("-s")
        .arg(path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_du(&out.stdout)
}

fn parse_du(stdout: &[u8]) -> Option<u64> {
    let text = String::from_utf8_lossy(stdout);
    let first = text.split_whitespace().next()?;
    first.parse().ok()
}

/// True when `du` could not be run at all. Reported rather than silently
/// skipped: a test that passes by doing nothing is the failure mode this file
/// exists to prevent.
fn du_missing(path: &Path) -> bool {
    let missing = du_bytes(path).is_none();
    if missing {
        eprintln!("SKIPPED: `du` is not available, so the external oracle could not run");
    }
    missing
}

#[test]
fn alloc_matches_du_byte_for_byte() {
    let dir = fixture();
    if du_missing(dir.path()) {
        return;
    }
    let expected = du_bytes(dir.path()).unwrap();

    let (tree, stats) = scan(
        dir.path(),
        ScanOptions::default(),
        Arc::new(ScanProgress::default()),
    )
    .unwrap();

    assert_eq!(stats.errors, 0, "the fixture is fully readable");
    assert_eq!(
        tree.total_alloc(),
        expected,
        "alloc must equal du exactly, not approximately"
    );
}

/// The same comparison with deduplication off. `du` counts a hardlinked inode
/// once, so this direction must *disagree* — the point is that the difference is
/// exactly the second copy and nothing else, which proves the dedup accounting
/// is not hiding an unrelated error.
#[test]
fn without_dedupe_the_gap_to_du_is_exactly_the_second_copy() {
    let dir = fixture();
    if du_missing(dir.path()) {
        return;
    }
    let du_total = du_bytes(dir.path()).unwrap();

    let opts = ScanOptions {
        dedupe_hardlinks: false,
        ..ScanOptions::default()
    };
    let (tree, stats) = scan(dir.path(), opts, Arc::new(ScanProgress::default())).unwrap();

    let copy_alloc = fs::metadata(dir.path().join("linked.dat"))
        .unwrap()
        .blocks()
        * 512;
    assert_eq!(stats.hardlinks_deduped, 0, "dedupe was switched off");
    assert_eq!(
        tree.total_alloc(),
        du_total + copy_alloc,
        "the only extra bytes should be the second name for the same inode"
    );
}

#[test]
fn dedupe_charges_a_hardlinked_inode_once() {
    let dir = fixture();
    let (tree, stats) = scan(
        dir.path(),
        ScanOptions::default(),
        Arc::new(ScanProgress::default()),
    )
    .unwrap();

    assert_eq!(stats.hardlinks_deduped, 1, "linked.dat has two names");

    // *Which* of the two names carries the bytes is deliberately unspecified.
    // The walk is parallel, so whichever thread claims the inode first wins,
    // and that genuinely differs by platform: macOS charged the copy at the
    // root, Linux the one under `nested/`. The invariant is "once", not "the
    // first path", so the assertion is on the pair.
    //
    // Both names stay listed either way — they are real directory entries —
    // and exactly one of them contributes, which is what keeps every parent
    // total honest no matter which side won.
    let one = tree.find("linked.dat").expect("the first name is listed");
    let other = tree
        .find("nested/linked-again.dat")
        .expect("the second name is listed");

    let sizes = [tree.node(one).size, tree.node(other).size];
    let allocs = [tree.node(one).alloc, tree.node(other).alloc];

    assert!(
        sizes.contains(&0),
        "one of the two names must contribute nothing: {sizes:?}"
    );
    assert_eq!(
        sizes[0] + sizes[1],
        3000,
        "and together they must add up to exactly one copy"
    );
    assert!(
        allocs.contains(&0),
        "the same must hold for alloc: {allocs:?}"
    );
}

#[test]
fn logical_size_matches_an_independent_walk() {
    let dir = fixture();
    let (tree, _) = scan(
        dir.path(),
        ScanOptions::default(),
        Arc::new(ScanProgress::default()),
    )
    .unwrap();

    let mut seen = HashSet::new();
    let expected = naive_logical_size(dir.path(), &mut seen);

    assert_eq!(
        tree.total_size(),
        expected,
        "the parallel walk and a dumb serial one must agree on file bytes"
    );
}

/// Guards the distinction itself: if `size` and `alloc` were ever conflated,
/// every other assertion here could still pass on a filesystem that allocates
/// generously. A hole is the one case where the two numbers cannot be swapped.
#[test]
fn a_sparse_file_reports_fewer_allocated_bytes_than_its_length() {
    let dir = fixture();
    let sparse = dir.path().join("sparse.img");
    let md = fs::metadata(&sparse).unwrap();
    if md.blocks() * 512 >= md.len() {
        eprintln!("SKIPPED: this filesystem materialised the hole, so there is nothing to compare");
        return;
    }

    let (tree, _) = scan(
        dir.path(),
        ScanOptions::default(),
        Arc::new(ScanProgress::default()),
    )
    .unwrap();

    let node = tree.find("sparse.img").expect("sparse.img was scanned");
    let node = tree.node(node);
    assert_eq!(node.size, md.len(), "logical size is the claimed length");
    assert_eq!(
        node.alloc,
        md.blocks() * 512,
        "allocated size is what the filesystem actually holds"
    );
    assert!(
        node.alloc < node.size,
        "a hole must survive into the tree, or the two measures were conflated"
    );
}

#[test]
fn symlinks_are_charged_at_their_own_size_not_their_targets() {
    let dir = fixture();
    let (tree, _) = scan(
        dir.path(),
        ScanOptions::default(),
        Arc::new(ScanProgress::default()),
    )
    .unwrap();

    let link = tree
        .find("nested/link-to-exact")
        .expect("the symlink was scanned");
    let link = tree.node(link);
    assert_eq!(link.kind, EntryKind::Symlink);
    assert!(
        link.size < 8192,
        "following the link would have charged the 8 KiB target"
    );
    assert_eq!(
        link.size,
        fs::symlink_metadata(dir.path().join("nested/link-to-exact"))
            .unwrap()
            .len()
    );
}

/// A deliberately dumb serial walk, kept as close to the documented semantics
/// as possible and sharing no code with the scanner: file bytes only, a
/// directory's own inode size excluded, symlinks at their own size and never
/// followed, a hardlinked inode counted the first time it is met.
fn naive_logical_size(dir: &Path, seen: &mut HashSet<(u64, u64)>) -> u64 {
    let mut total = 0;
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let md = entry.metadata().unwrap();
        if md.is_dir() {
            total += naive_logical_size(&entry.path(), seen);
            continue;
        }
        // Mirrors `RawMeta::is_hardlinked`: only regular files with more than
        // one link are deduplicated, and the key is (dev, ino).
        if md.is_file() && md.nlink() > 1 && !seen.insert((md.dev(), md.ino())) {
            continue;
        }
        total += md.len();
    }
    total
}

// ------------------------------------------------------------- APFS clones

/// Make `dst` a copy-on-write clone of `src`, or report that this filesystem
/// does not do clones.
#[cfg(target_os = "macos")]
fn clone_file(src: &Path, dst: &Path) -> bool {
    Command::new("cp")
        .arg("-c")
        .arg(src)
        .arg(dst)
        .status()
        .is_ok_and(|s| s.success())
}

/// A clone is the case where `du` and the disk disagree, and the disk is right.
///
/// `du` charges every clone its full size because every clone reports it in
/// `st_blocks`; the filesystem holds those blocks once. Measured in the small:
/// three 100 MiB clones cost 0 MiB of free space. Measured on a developer's
/// tree: 7.31 GiB of 23 GiB reported.
///
/// So this is the one place where matching `du` byte for byte would mean being
/// wrong, and `alloc` deliberately diverges. The assertion is on the size of
/// the divergence, which is exactly the clones' own bytes — the same shape as
/// the hardlink test above.
#[cfg(target_os = "macos")]
#[test]
fn clones_are_charged_once_and_du_is_the_one_that_overcounts() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("original.bin");
    // Comfortably over CLONE_MIN_BYTES, and incompressible so the filesystem
    // cannot quietly store it some other way.
    let bytes: Vec<u8> = (0..300_000u32)
        .map(|i| (i.wrapping_mul(2654435761) >> 24) as u8)
        .collect();
    fs::write(&original, &bytes).unwrap();

    if !clone_file(&original, &dir.path().join("clone1.bin"))
        || !clone_file(&original, &dir.path().join("clone2.bin"))
    {
        eprintln!("SKIPPED: this filesystem does not support clones");
        return;
    }

    let one_copy = fs::metadata(&original).unwrap().blocks() * 512;
    let du_total = du_bytes(dir.path()).expect("du is available on macOS");

    let (tree, stats) = scan(
        dir.path(),
        ScanOptions::default(),
        Arc::new(ScanProgress::default()),
    )
    .unwrap();

    assert_eq!(
        stats.clones_deduped, 2,
        "two of the three share the first's blocks"
    );
    assert_eq!(
        du_total - tree.total_alloc(),
        2 * one_copy,
        "the gap to du should be exactly the two clones nobody has to store"
    );

    // The lowest node id keeps the bytes, so the answer does not depend on
    // which thread probed first.
    let charged: Vec<&str> = ["original.bin", "clone1.bin", "clone2.bin"]
        .into_iter()
        .filter(|n| tree.node(tree.find(n).unwrap()).alloc > 0)
        .collect();
    assert_eq!(
        charged.len(),
        1,
        "exactly one name carries the bytes: {charged:?}"
    );
}

/// With the pass switched off we are back to agreeing with `du`, which is the
/// proof that the divergence above is the clone accounting and nothing else.
#[cfg(target_os = "macos")]
#[test]
fn without_clone_dedupe_we_match_du_again() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("original.bin");
    fs::write(&original, vec![7u8; 300_000]).unwrap();
    if !clone_file(&original, &dir.path().join("clone1.bin")) {
        eprintln!("SKIPPED: this filesystem does not support clones");
        return;
    }

    let du_total = du_bytes(dir.path()).expect("du is available on macOS");
    let opts = ScanOptions {
        dedupe_clones: false,
        ..ScanOptions::default()
    };
    let (tree, stats) = scan(dir.path(), opts, Arc::new(ScanProgress::default())).unwrap();

    assert_eq!(stats.clones_deduped, 0);
    assert_eq!(tree.total_alloc(), du_total);
}

// ------------------------------------------------ btrfs and XFS reflinks

/// The Linux half of the clone story: reflinked copies on btrfs and XFS.
///
/// These only mean something on a filesystem that shares extents, and CI's
/// runners sit on ext4. So each test looks at where `TMPDIR` points, and on
/// anything else prints why it did nothing — a reflink test that passed on
/// ext4 would be passing by testing nothing. CI runs them a second time with
/// `TMPDIR` on loop-mounted btrfs and XFS.
///
/// `df` is the oracle here as well as `du`: `du` can only show the scanner
/// diverging from it, while `df` says which side of the divergence is right.
#[cfg(target_os = "linux")]
mod reflinks {
    use super::du_bytes;
    use spacetrace_scan_core::{scan, ScanOptions, ScanProgress, ScanStats, Tree};
    use std::fs;
    use std::io::Write;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{FileExt, MetadataExt};
    use std::path::Path;
    use std::process::Command;
    use std::sync::{Arc, Mutex};

    const MB: u64 = 1_000_000;
    const MIB: u64 = 1 << 20;
    /// How far `df` may move for what should cost nothing: a reflink writes
    /// a few metadata blocks, and btrfs keeps metadata twice.
    const DF_SLACK: u64 = MIB;

    /// One big-file test at a time. `df` is filesystem-wide, so a second test
    /// writing 100 MB next to this one would be counted as this one's cost.
    static SERIAL: Mutex<()> = Mutex::new(());

    /// The filesystem `path` is on, when it is one that shares extents.
    fn reflink_fs(path: &Path) -> Result<&'static str, String> {
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: all-zero is a valid `statfs`; the path is NUL-terminated.
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
            return Err("statfs failed".into());
        }
        match st.f_type as u32 {
            0x9123_683E => Ok("btrfs"),
            0x5846_5342 => Ok("xfs"),
            other => Err(format!("filesystem magic {other:#x}")),
        }
    }

    /// Set by the CI job that mounts btrfs and XFS for these tests. There, a
    /// test that could not run is a failure: the job exists to run them, and
    /// one that skipped would pass by testing nothing — exactly what happened
    /// unseen if the mount or the reflink support were ever lost.
    const REQUIRED: &str = "SPACETRACE_REQUIRE_REFLINK";

    /// Say why `test` did nothing — or, where it is required to run, fail.
    ///
    /// Not for a test that does not apply (compression on XFS): that one is
    /// skipped plainly, because no filesystem would make it run.
    fn could_not_run(test: &str, why: &str) {
        if std::env::var_os(REQUIRED).is_some() {
            panic!("{test} could not run, and {REQUIRED} says it must: {why}");
        }
        eprintln!("SKIPPED {test}: {why}");
    }

    /// A temporary directory on btrfs or XFS, or `None` after saying why not.
    fn shared_tempdir(test: &str) -> Option<tempfile::TempDir> {
        let dir = tempfile::tempdir().unwrap();
        match reflink_fs(dir.path()) {
            Ok(_) => Some(dir),
            Err(why) => {
                could_not_run(
                    test,
                    &format!(
                        "{} is not on btrfs or XFS ({why}); \
                         point TMPDIR at a reflink-capable mount to run it",
                        dir.path().display()
                    ),
                );
                None
            }
        }
    }

    /// `cp --reflink=always`, which fails rather than copying when the
    /// filesystem cannot share — XFS made without `reflink=1`, for one.
    fn reflink(src: &Path, dst: &Path) -> bool {
        Command::new("cp")
            .arg("--reflink=always")
            .arg(src)
            .arg(dst)
            .status()
            .is_ok_and(|s| s.success())
    }

    /// `len` bytes nothing can compress, so the filesystem stores exactly
    /// what was written and the arithmetic below is about sharing alone.
    fn write_noise(path: &Path, len: u64, seed: u64) {
        let mut file = fs::File::create(path).unwrap();
        let mut state = seed | 1;
        let mut chunk = vec![0u8; MIB as usize];
        let mut left = len;
        while left > 0 {
            for word in chunk.chunks_exact_mut(8) {
                // xorshift64: fast, and nothing a compressor can find.
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                word.copy_from_slice(&state.to_ne_bytes());
            }
            let n = left.min(MIB) as usize;
            file.write_all(&chunk[..n]).unwrap();
            left -= n as u64;
        }
        file.sync_all().unwrap();
    }

    /// Bytes in use on the filesystem holding `path`, once it has stopped
    /// moving.
    ///
    /// `sync` is not enough on its own: XFS frees a deleted file's blocks in
    /// the background, so the previous test's temporary directory can still
    /// be coming off the count — measured, 188 KiB of a 100 MB reading. Read
    /// until two readings a tenth of a second apart agree, for up to five
    /// seconds.
    fn df_used(path: &Path) -> u64 {
        let read = || {
            // SAFETY: takes no arguments and cannot fail.
            unsafe { libc::sync() };
            let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
            // SAFETY: as in `reflink_fs`.
            let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
            assert_eq!(unsafe { libc::statvfs(c.as_ptr(), &mut st) }, 0);
            (st.f_blocks - st.f_bfree) as u64 * st.f_frsize as u64
        };
        let mut last = read();
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let now = read();
            if now == last {
                return now;
            }
            last = now;
        }
        last
    }

    /// `df` moved from `before` to `after` by less than `slack`, either way.
    fn barely_moved(before: u64, after: u64, slack: u64) -> bool {
        before.abs_diff(after) < slack
    }

    fn blocks(path: &Path) -> u64 {
        fs::metadata(path).unwrap().blocks() * 512
    }

    fn run(root: &Path, opts: ScanOptions) -> (Tree, ScanStats) {
        scan(root, opts, Arc::new(ScanProgress::default())).unwrap()
    }

    /// The two oracles at once. `du` minus the on-disk total must be exactly
    /// the bytes the scan says it deduplicated, no more and no less — the
    /// same shape as the hardlink test above, applied to extents.
    fn assert_du_gap_is_exactly_the_shared_bytes(root: &Path, tree: &Tree, stats: &ScanStats) {
        let du = du_bytes(root).expect("du is part of coreutils");
        assert_eq!(
            du - tree.total_alloc(),
            stats.shared_bytes_deduped + stats.compressed_bytes_saved,
            "du {du}, alloc {}, shared {}, compressed {}",
            tree.total_alloc(),
            stats.shared_bytes_deduped,
            stats.compressed_bytes_saved
        );
    }

    fn alloc_of(tree: &Tree, name: &str) -> u64 {
        tree.node(tree.find(name).unwrap()).alloc
    }

    /// Invariant 1 on Linux, the controlled measurement: three reflinked
    /// copies of a 100 MB file take one copy of space, `df` agrees, and the
    /// on-disk total says so while `du` reports three.
    #[test]
    fn three_reflinked_copies_cost_one_copy_and_df_agrees() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        let Some(dir) = shared_tempdir("three_reflinked_copies") else {
            return;
        };
        let root = dir.path();
        let before = df_used(root);
        write_noise(&root.join("orig.bin"), 100 * MB, 1);
        let after_original = df_used(root);
        if !reflink(&root.join("orig.bin"), &root.join("copy1.bin"))
            || !reflink(&root.join("orig.bin"), &root.join("copy2.bin"))
        {
            could_not_run("three_reflinked_copies", "this filesystem refuses reflinks");
            return;
        }
        let after_copies = df_used(root);
        let one_copy = blocks(&root.join("orig.bin"));
        eprintln!(
            "{}: df used {before} -> {after_original} after the original, \
             -> {after_copies} after two reflinks; du {}",
            reflink_fs(root).unwrap(),
            du_bytes(root).unwrap()
        );
        assert!(
            barely_moved(after_original, after_copies, DF_SLACK),
            "the filesystem should have stored the copies for free"
        );

        let (tree, stats) = run(root, ScanOptions::default());
        eprintln!(
            "  scan: alloc {}, shared {}",
            tree.total_alloc(),
            stats.shared_bytes_deduped
        );

        let files: u64 = ["orig.bin", "copy1.bin", "copy2.bin"]
            .iter()
            .map(|n| alloc_of(&tree, n))
            .sum();
        assert_eq!(files, one_copy, "three names, one copy on disk");
        assert_eq!(stats.clones_deduped, 2);
        assert_eq!(stats.shared_bytes_deduped, 2 * one_copy);
        assert!(
            barely_moved(before + tree.total_alloc(), after_copies, DF_SLACK),
            "the on-disk total and df must agree: {} vs {}",
            tree.total_alloc(),
            after_copies.saturating_sub(before)
        );
        assert_du_gap_is_exactly_the_shared_bytes(root, &tree, &stats);
        assert_eq!(stats.files_unmapped, 0);
    }

    /// Extents split differently in two files. Overwriting 10 MiB in the
    /// middle of one copy leaves it holding the old extent in two pieces while
    /// the others hold it whole — measured on both filesystems — and the only
    /// new space is the 10 MiB written.
    #[test]
    fn a_partly_overwritten_copy_is_charged_only_for_what_it_no_longer_shares() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        let Some(dir) = shared_tempdir("partly_overwritten_copy") else {
            return;
        };
        let root = dir.path();
        write_noise(&root.join("orig.bin"), 100 * MB, 2);
        if !reflink(&root.join("orig.bin"), &root.join("copy1.bin"))
            || !reflink(&root.join("orig.bin"), &root.join("copy2.bin"))
        {
            could_not_run(
                "partly_overwritten_copy",
                "this filesystem refuses reflinks",
            );
            return;
        }
        let one_copy = blocks(&root.join("orig.bin"));

        // The patch is generated before `df` is read, so the only write
        // between the two readings is the overwrite itself.
        let patch = root.join("patch.bin");
        write_noise(&patch, 10 * MIB, 3);
        let patch_bytes = fs::read(&patch).unwrap();
        fs::remove_file(&patch).unwrap();
        let before = df_used(root);
        let target = fs::OpenOptions::new()
            .write(true)
            .open(root.join("copy2.bin"))
            .unwrap();
        target.write_all_at(&patch_bytes, 40 * MIB).unwrap();
        target.sync_all().unwrap();
        drop(target);
        let after = df_used(root);
        eprintln!(
            "{}: df used {before} -> {after} for a 10 MiB overwrite",
            reflink_fs(root).unwrap(),
        );
        assert!(
            barely_moved(before + 10 * MIB, after, DF_SLACK),
            "the overwrite should cost the 10 MiB written and nothing else"
        );

        let (tree, stats) = run(root, ScanOptions::default());
        let files: u64 = ["orig.bin", "copy1.bin", "copy2.bin"]
            .iter()
            .map(|n| alloc_of(&tree, n))
            .sum();
        assert_eq!(
            files,
            one_copy + 10 * MIB,
            "one copy plus the overwritten piece, whichever name was met first"
        );
        assert_du_gap_is_exactly_the_shared_bytes(root, &tree, &stats);
    }

    /// Sharing with something outside the scanned root is charged inside it,
    /// once. Same rule as APFS clones: the on-disk total of a tree is what the
    /// tree references, each block once — not what deleting the tree would
    /// free, which depends on everything else on the disk and is not a
    /// property of the tree.
    #[test]
    fn a_copy_outside_the_root_does_not_make_the_inside_one_free() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        let Some(dir) = shared_tempdir("copy_outside_the_root") else {
            return;
        };
        let inside = dir.path().join("inside");
        let outside = dir.path().join("outside");
        fs::create_dir(&inside).unwrap();
        fs::create_dir(&outside).unwrap();
        write_noise(&inside.join("orig.bin"), 20 * MB, 4);
        if !reflink(&inside.join("orig.bin"), &outside.join("copy.bin")) {
            could_not_run("copy_outside_the_root", "this filesystem refuses reflinks");
            return;
        }
        let one_copy = blocks(&inside.join("orig.bin"));

        let (tree, stats) = run(&inside, ScanOptions::default());
        assert_eq!(alloc_of(&tree, "orig.bin"), one_copy);
        assert_eq!(stats.shared_bytes_deduped, 0);
        assert_eq!(stats.clones_deduped, 0);

        let (tree, stats) = run(&outside, ScanOptions::default());
        assert_eq!(alloc_of(&tree, "copy.bin"), one_copy);
        assert_eq!(stats.shared_bytes_deduped, 0);

        let (tree, stats) = run(dir.path(), ScanOptions::default());
        assert_eq!(
            alloc_of(&tree, "inside/orig.bin") + alloc_of(&tree, "outside/copy.bin"),
            one_copy,
            "both in the root: once"
        );
        assert_du_gap_is_exactly_the_shared_bytes(dir.path(), &tree, &stats);
    }

    /// A hardlink and a reflink of the same file: the repeat name claims
    /// nothing, so the extents are left for the name that is charged. Getting
    /// the order wrong charges the blocks to nobody.
    #[test]
    fn a_hardlink_and_a_reflink_of_one_file_are_each_counted_once() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        let Some(dir) = shared_tempdir("hardlink_and_reflink") else {
            return;
        };
        let root = dir.path();
        fs::create_dir(root.join("a")).unwrap();
        write_noise(&root.join("a/orig.bin"), 20 * MB, 5);
        fs::hard_link(root.join("a/orig.bin"), root.join("linked.bin")).unwrap();
        if !reflink(&root.join("a/orig.bin"), &root.join("copy.bin")) {
            could_not_run("hardlink_and_reflink", "this filesystem refuses reflinks");
            return;
        }
        let one_copy = blocks(&root.join("a/orig.bin"));

        let (tree, stats) = run(root, ScanOptions::default());
        let files: u64 = ["a/orig.bin", "linked.bin", "copy.bin"]
            .iter()
            .map(|n| alloc_of(&tree, n))
            .sum();
        assert_eq!(files, one_copy);
        assert_eq!(stats.hardlinks_deduped, 1);
        assert_eq!(stats.clones_deduped, 1);
        // `du` already counts the hardlink once, so its excess is the reflink.
        assert_eq!(stats.shared_bytes_deduped, one_copy);
        assert_du_gap_is_exactly_the_shared_bytes(root, &tree, &stats);
    }

    /// A btrfs snapshot inside the root. Every subvolume has its own device
    /// number while the extents are the filesystem's, so this is the test
    /// that fails if the claims are keyed by device: the snapshot would never
    /// meet its origin and the whole file would be charged twice.
    #[test]
    fn a_snapshot_inside_the_root_costs_nothing_more() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        let Some(dir) = shared_tempdir("snapshot_inside_the_root") else {
            return;
        };
        if reflink_fs(dir.path()) != Ok("btrfs") {
            eprintln!("SKIPPED snapshot_inside_the_root: only btrfs has snapshots");
            return;
        }
        let root = dir.path();
        let btrfs = |args: &[&str]| {
            Command::new("btrfs")
                .args(args)
                .current_dir(root)
                .output()
                .is_ok_and(|o| o.status.success())
        };
        if !btrfs(&["subvolume", "create", "live"]) {
            could_not_run("snapshot_inside_the_root", "cannot create a subvolume here");
            return;
        }
        write_noise(&root.join("live/data.bin"), 20 * MB, 7);
        if !btrfs(&["subvolume", "snapshot", "live", "snap"]) {
            could_not_run("snapshot_inside_the_root", "cannot snapshot here");
            return;
        }
        let one_copy = blocks(&root.join("live/data.bin"));
        assert_ne!(
            fs::metadata(root.join("live")).unwrap().dev(),
            fs::metadata(root.join("snap")).unwrap().dev(),
            "the premise: a snapshot is a device of its own"
        );

        let (tree, stats) = run(root, ScanOptions::default());
        assert_eq!(
            alloc_of(&tree, "live/data.bin") + alloc_of(&tree, "snap/data.bin"),
            one_copy
        );
        assert_eq!(stats.shared_bytes_deduped, one_copy);
        assert_du_gap_is_exactly_the_shared_bytes(root, &tree, &stats);
    }

    /// Switched off, the scan agrees with `du` again — the proof that the
    /// divergence above is the extent accounting and nothing else.
    #[test]
    fn without_clone_dedupe_alloc_is_du_again() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        let Some(dir) = shared_tempdir("without_clone_dedupe") else {
            return;
        };
        let root = dir.path();
        write_noise(&root.join("orig.bin"), 20 * MB, 6);
        if !reflink(&root.join("orig.bin"), &root.join("copy.bin")) {
            could_not_run("without_clone_dedupe", "this filesystem refuses reflinks");
            return;
        }
        let opts = ScanOptions {
            dedupe_clones: false,
            ..ScanOptions::default()
        };
        let (tree, stats) = run(root, opts);
        assert_eq!(tree.total_alloc(), du_bytes(root).unwrap());
        assert_eq!(stats.clones_deduped, 0);
        assert_eq!(stats.shared_bytes_deduped, 0);
    }

    /// Ask btrfs to compress what is written into `dir`, unprivileged.
    fn compress_into(dir: &Path) -> bool {
        let chattr = Command::new("chattr").arg("+c").arg(dir).status();
        if chattr.is_ok_and(|s| s.success()) {
            return true;
        }
        Command::new("btrfs")
            .args(["property", "set"])
            .arg(dir)
            .args(["compression", "zstd"])
            .status()
            .is_ok_and(|s| s.success())
    }

    /// `compsize -b`'s "Disk Usage" for `path`, when compsize is installed
    /// and may run (it needs the same privilege the scanner does).
    fn compsize_disk(path: &Path) -> Option<u64> {
        let out = Command::new("compsize").arg("-b").arg(path).output().ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let total = text.lines().find(|l| l.starts_with("TOTAL"))?;
        total.split_whitespace().nth(2)?.parse().ok()
    }

    /// btrfs compression. `st_blocks` reports a compressed extent at its
    /// uncompressed length, so `du` is high by the compression ratio.
    ///
    /// With `CAP_SYS_ADMIN` the scan reads the real size and agrees with
    /// `compsize` and `df`; without it the scan agrees with `du` and says so.
    /// Both outcomes are asserted, because both are promises.
    #[test]
    fn a_compressed_file_is_charged_its_size_on_disk_where_that_can_be_read() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        let Some(dir) = shared_tempdir("compressed_file") else {
            return;
        };
        if reflink_fs(dir.path()) != Ok("btrfs") {
            eprintln!("SKIPPED compressed_file: only btrfs compresses");
            return;
        }
        let root = dir.path().join("z");
        fs::create_dir(&root).unwrap();
        if !compress_into(&root) {
            could_not_run("compressed_file", "could not switch compression on");
            return;
        }
        let before = df_used(&root);
        let line = b"2026-10-02T12:00:00Z INFO request served path=/api/v1/items status=200\n";
        let mut file = fs::File::create(root.join("log.txt")).unwrap();
        for _ in 0..(50 * MB as usize / line.len()) {
            file.write_all(line).unwrap();
        }
        file.sync_all().unwrap();
        drop(file);
        assert!(reflink(&root.join("log.txt"), &root.join("log-copy.txt")));
        let after = df_used(&root);
        let du = du_bytes(&root).unwrap();

        // Decided from `df`, not from the scan: a skip that read the
        // scanner's own answer would skip exactly when the scanner is wrong.
        let stored = after.saturating_sub(before);
        if stored > du / 4 {
            could_not_run(
                "compressed_file",
                &format!("btrfs stored the data uncompressed ({stored} of {du})"),
            );
            return;
        }

        let (tree, stats) = run(&root, ScanOptions::default());
        eprintln!(
            "btrfs compressed: df used {before} -> {after} (+{stored}), du {du}, alloc {}, \
             compsize {:?}, saved {}, shared {}, inexact {}",
            tree.total_alloc(),
            compsize_disk(&root),
            stats.compressed_bytes_saved,
            stats.shared_bytes_deduped,
            stats.compressed_files_inexact
        );

        if stats.compressed_files_inexact > 0 {
            eprintln!("  no CAP_SYS_ADMIN: checking the du-like fallback");
            assert_eq!(
                tree.total_alloc(),
                du,
                "unmeasured means counted as du counts"
            );
            assert_eq!(stats.compressed_bytes_saved, 0);
            assert_eq!(stats.compressed_files_inexact, 2, "both names say so");
            return;
        }
        assert!(
            tree.total_alloc() < du / 10,
            "a repeated log line compresses far better than ten to one"
        );
        if let Some(disk) = compsize_disk(&root) {
            assert_eq!(tree.total_alloc(), disk, "compsize's Disk Usage, exactly");
        }
        assert!(
            barely_moved(before + tree.total_alloc(), after, DF_SLACK),
            "and df agrees: {} vs {stored}",
            tree.total_alloc(),
        );
        assert_du_gap_is_exactly_the_shared_bytes(&root, &tree, &stats);
    }
}
