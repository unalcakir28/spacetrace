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
//! Windows ships no `du`. Its counterpart is a construction-based test that
//! arrives with the native metadata backend, since there is nothing correct to
//! compare against until `alloc` stops being the logical size there.

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

    // The second name stays in the tree — it is a real directory entry — but it
    // contributes nothing, which is what keeps parent totals honest.
    let second = tree
        .find("nested/linked-again.dat")
        .expect("the second name is still listed");
    assert_eq!(tree.node(second).size, 0);
    assert_eq!(tree.node(second).alloc, 0);
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
