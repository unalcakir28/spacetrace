//! Windows counterpart of `du_equivalence.rs`.
//!
//! Windows ships no `du`, so there is no external oracle to compare against.
//! What replaces it is construction: build files whose allocated size cannot
//! possibly equal their logical size, and assert the scanner reports the
//! difference. That is enough to catch the regression that matters — falling
//! back to the logical size, which is what this platform did until the
//! allocated size and file identity were read for real.
//!
//! Every assertion here is one that holds on any NTFS volume without knowing
//! its cluster size, because the test cannot ask for one portably. The trick is
//! to pick a length that is not a multiple of 512: NTFS clusters are at least
//! that big, so a genuine allocation figure can never equal such a length.

#![cfg(windows)]

use std::fs;
use std::sync::Arc;

use spacetrace_scan_core::{scan, ScanOptions, ScanProgress};

/// Not a multiple of 512, so no cluster size can round it to itself.
const ODD_LENGTH: u64 = 100_001;

/// The largest cluster NTFS is formatted with by default. Used only as a loose
/// upper bound, so that "rounded up" cannot pass by being wildly wrong.
const MAX_PLAUSIBLE_CLUSTER: u64 = 2 * 1024 * 1024;

fn run(
    root: &std::path::Path,
    opts: ScanOptions,
) -> (spacetrace_scan_core::Tree, spacetrace_scan_core::ScanStats) {
    scan(root, opts, Arc::new(ScanProgress::default())).unwrap()
}

/// Bytes that NTFS compression cannot shrink, so that "allocated" stays at
/// least as large as the length even on a compressed volume. A file of zeros
/// would compress to almost nothing and fail the lower bound below for a reason
/// that has nothing to do with the code under test.
fn incompressible(len: usize) -> Vec<u8> {
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

#[test]
fn allocated_size_is_not_the_logical_size() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("odd.bin"),
        incompressible(ODD_LENGTH as usize),
    )
    .unwrap();

    let (tree, stats) = run(dir.path(), ScanOptions::default());
    assert_eq!(stats.errors, 0, "reading attributes must not fail here");

    let node = tree.find("odd.bin").expect("odd.bin was scanned");
    let node = tree.node(node);

    assert_eq!(node.size, ODD_LENGTH, "the logical size is the length");
    // The heart of the test: any real allocation is a whole number of clusters,
    // and no cluster size divides ODD_LENGTH. Equality here would mean the
    // logical size was reported as disk usage.
    assert_ne!(
        node.alloc, ODD_LENGTH,
        "alloc equals the logical size, so it was never actually read"
    );
    assert!(
        node.alloc > ODD_LENGTH,
        "a file occupies at least its own length: {} vs {}",
        node.alloc,
        ODD_LENGTH
    );
    assert!(
        node.alloc < ODD_LENGTH + MAX_PLAUSIBLE_CLUSTER,
        "rounded up by more than one cluster: {}",
        node.alloc
    );
    assert_eq!(
        node.alloc % 512,
        0,
        "allocation is a whole number of sectors"
    );
}

#[test]
fn hardlinked_files_are_counted_once() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("original.dat");
    fs::write(&original, vec![b'x'; 4096]).unwrap();
    fs::create_dir(dir.path().join("sub")).unwrap();
    fs::hard_link(&original, dir.path().join("sub/second-name.dat")).unwrap();

    let (tree, stats) = run(dir.path(), ScanOptions::default());

    assert_eq!(
        stats.hardlinks_deduped, 1,
        "the second name must be recognised as the same file"
    );

    // *Which* name carries the bytes is unspecified: the walk is parallel, so
    // whichever thread claims the inode first wins. Both names stay listed —
    // they are real directory entries — and exactly one contributes, which is
    // what keeps a parent's total honest (invariant #3).
    let one = tree.find("original.dat").expect("the first name is listed");
    let other = tree
        .find("sub/second-name.dat")
        .expect("the second name is listed");

    let sizes = [tree.node(one).size, tree.node(other).size];
    assert!(
        sizes.contains(&0),
        "one of the two names must contribute nothing: {sizes:?}"
    );
    assert_eq!(
        sizes[0] + sizes[1],
        4096,
        "and together they must add up to exactly one copy"
    );
}

/// The same tree with deduplication off must charge the file twice. Without
/// this, a dedup that silently did nothing would look identical to one that
/// worked, since both leave the total unchanged when nothing is linked.
#[test]
fn without_dedupe_a_hardlink_is_charged_twice() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("original.dat");
    fs::write(&original, vec![b'x'; 4096]).unwrap();
    fs::hard_link(&original, dir.path().join("second-name.dat")).unwrap();

    let deduped_tree = run(dir.path(), ScanOptions::default()).0;
    let deduped = deduped_tree.total_alloc();
    // Summed rather than read off one name, because which of the two the
    // parallel walk charges is unspecified — the other one is zero, so the sum
    // is exactly one copy either way.
    let one_copy: u64 = ["original.dat", "second-name.dat"]
        .iter()
        .map(|name| {
            deduped_tree
                .find(name)
                .map(|id| deduped_tree.node(id).alloc)
                .expect("both names are listed")
        })
        .sum();

    let opts = ScanOptions {
        dedupe_hardlinks: false,
        ..ScanOptions::default()
    };
    let raw = run(dir.path(), opts).0.total_alloc();

    // Measured against the file's own allocation rather than half the total, so
    // that directory overhead — which differs by platform — cannot skew it.
    assert_eq!(
        raw - deduped,
        one_copy,
        "the gap should be exactly the one inode charged a second time"
    );
}

/// A sparse file is the case where reporting the logical size is worst — a
/// length with nothing behind it. Windows does not make a file sparse just
/// because it was extended, so the flag is set explicitly; if that is not
/// possible the assertion is skipped rather than guessed at.
#[test]
fn a_sparse_file_reports_fewer_allocated_bytes_than_its_length() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sparse.img");
    let file = fs::File::create(&path).unwrap();
    drop(file);

    let marked = std::process::Command::new("fsutil")
        .args(["sparse", "setflag"])
        .arg(&path)
        .status();
    let marked = matches!(marked, Ok(status) if status.success());
    if !marked {
        eprintln!("SKIPPED: `fsutil sparse setflag` did not run, so no hole could be made");
        return;
    }

    let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_len(4 * 1024 * 1024).unwrap();
    drop(file);

    let (tree, _) = run(dir.path(), ScanOptions::default());
    let node = tree.find("sparse.img").expect("sparse.img was scanned");
    let node = tree.node(node);

    assert_eq!(
        node.size,
        4 * 1024 * 1024,
        "the claimed length is the length"
    );
    assert!(
        node.alloc < node.size,
        "a hole must survive into the tree: alloc {} vs size {}",
        node.alloc,
        node.size
    );
}

/// Directories are queried like files, so that `alloc` means the same thing on
/// both platforms instead of quietly excluding directory overhead on one.
///
/// What NTFS reports for a directory is not asserted to a fixed number: a
/// directory's index lives in `$INDEX_ROOT`/`$INDEX_ALLOCATION` rather than the
/// unnamed data stream, so `AllocationSize` may legitimately be 0. Asserting a
/// guess would make this test a statement about the filesystem rather than
/// about our code. What *is* asserted is that the number is well formed, and it
/// is printed so the real value is on the record.
#[test]
fn directory_allocation_is_whatever_the_filesystem_reports() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("empty")).unwrap();
    fs::write(dir.path().join("empty/inside.txt"), b"hello").unwrap();

    let (tree, stats) = run(dir.path(), ScanOptions::default());
    assert_eq!(stats.errors, 0, "opening a directory handle must not fail");

    // `own_alloc`, not `alloc`: the latter is the rolled-up subtree total and
    // would be dominated by the file inside.
    let empty = tree.find("empty").expect("the directory was scanned");
    let own = tree.node(empty).own_alloc;
    eprintln!("NTFS reports AllocationSize {own} for a directory holding one file");

    assert_eq!(
        own % 512,
        0,
        "an allocation is a whole number of sectors, got {own}"
    );
}

/// `one_filesystem` was a no-op here before the volume serial number was read:
/// every entry reported volume 0, so nothing ever looked like a different
/// filesystem. It should now behave like the plain walk within one volume.
#[test]
fn one_filesystem_keeps_a_single_volume_intact() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("sub")).unwrap();
    fs::write(dir.path().join("sub/leaf.txt"), vec![b'a'; 1000]).unwrap();

    let plain = run(dir.path(), ScanOptions::default()).0;
    let opts = ScanOptions {
        one_filesystem: true,
        ..ScanOptions::default()
    };
    let bounded = run(dir.path(), opts).0;

    assert_eq!(
        plain.total_size(),
        bounded.total_size(),
        "one volume, so restricting to it must not drop anything"
    );
    assert!(
        bounded.find("sub/leaf.txt").is_some(),
        "the walk must still have descended"
    );
}
