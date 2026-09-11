//! The live view has to end up agreeing with the tree.
//!
//! It is built by a completely different route — one atomic add per directory
//! from every worker, attributed to an ancestor — while the tree is built by
//! aggregating the arena in a single pass at the end. Two answers arrived at
//! two ways, and the failure this guards against is them drifting apart: a
//! preview that says 4.2 GB while the finished scan says 3.8 GB teaches the
//! reader to distrust both.

use std::fs;
use std::sync::Arc;

use spacetrace_scan_core::{scan, EntryKind, ScanOptions, ScanProgress};

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();

    // A top-level file, so a branch that is not a directory is covered.
    fs::write(dir.path().join("loose.bin"), vec![0u8; 3000]).unwrap();

    // A branch with everything directly inside it.
    fs::create_dir(dir.path().join("flat")).unwrap();
    for i in 0..5 {
        fs::write(dir.path().join(format!("flat/f{i}.bin")), vec![0u8; 1000]).unwrap();
    }

    // A branch whose bytes are several levels down, so attribution has to
    // survive the recursion rather than only the first hop.
    fs::create_dir_all(dir.path().join("deep/a/b/c")).unwrap();
    fs::write(dir.path().join("deep/a/b/c/buried.bin"), vec![0u8; 7000]).unwrap();
    fs::write(dir.path().join("deep/a/beside.bin"), vec![0u8; 500]).unwrap();

    // An empty branch: it must appear, at zero, rather than be missing.
    fs::create_dir(dir.path().join("empty")).unwrap();

    dir
}

#[test]
fn the_live_totals_match_the_finished_tree() {
    let dir = fixture();
    let progress = Arc::new(ScanProgress::default());
    let (tree, _) = scan(dir.path(), ScanOptions::default(), Arc::clone(&progress)).unwrap();

    let live = progress.live.snapshot();
    assert_eq!(live.len(), 4, "one branch per child of the root: {live:?}");

    for entry in &live {
        let node = tree
            .children(tree.root())
            .find(|&id| tree.name(id) == entry.name)
            .unwrap_or_else(|| panic!("{} is in the live view but not the tree", entry.name));
        let node = tree.node(node);

        assert_eq!(
            entry.size, node.size,
            "{}: live logical total disagrees with the tree",
            entry.name
        );
        assert_eq!(
            entry.alloc, node.alloc,
            "{}: live on-disk total disagrees with the tree",
            entry.name
        );
        assert_eq!(
            entry.files, node.files as u64,
            "{}: live file count disagrees with the tree",
            entry.name
        );
        assert_eq!(entry.is_dir, node.kind == EntryKind::Dir);
    }
}

/// Every byte in the tree has to be in exactly one branch. A file attributed
/// to two branches, or to none, is invisible in the per-branch check above —
/// the totals would each be self-consistent and the sum would be wrong.
#[test]
fn the_branches_account_for_the_whole_root() {
    let dir = fixture();
    let progress = Arc::new(ScanProgress::default());
    let (tree, _) = scan(dir.path(), ScanOptions::default(), Arc::clone(&progress)).unwrap();

    let live = progress.live.snapshot();
    let live_size: u64 = live.iter().map(|e| e.size).sum();
    let live_files: u64 = live.iter().map(|e| e.files).sum();

    assert_eq!(live_size, tree.total_size());
    assert_eq!(live_files, tree.node(tree.root()).files as u64);

    // The root's own inode is not in any branch, which is why `alloc` is
    // compared against the sum of the children rather than the root's total.
    let children_alloc: u64 = tree
        .children(tree.root())
        .map(|id| tree.node(id).alloc)
        .sum();
    assert_eq!(live.iter().map(|e| e.alloc).sum::<u64>(), children_alloc);
}

/// An empty folder is a branch too. Dropping it would make a directory that is
/// genuinely empty look like one that has not been read yet.
#[test]
fn an_empty_branch_is_present_at_zero() {
    let dir = fixture();
    let progress = Arc::new(ScanProgress::default());
    scan(dir.path(), ScanOptions::default(), Arc::clone(&progress)).unwrap();

    let live = progress.live.snapshot();
    let empty = live.iter().find(|e| e.name == "empty").expect("the branch");
    assert_eq!(empty.size, 0);
    assert_eq!(empty.files, 0);
    assert!(empty.is_dir);
}

/// Before the root has been listed there is nothing to show, and showing a
/// half-installed list would put a folder on screen and then move it.
#[test]
fn nothing_is_published_before_the_root_is_read() {
    let progress = ScanProgress::default();
    assert!(!progress.live.is_ready());
    assert!(progress.live.snapshot().is_empty());
}

/// A scan of a single file has no branches at all. The view has to cope with
/// that rather than showing the file as its own child.
#[test]
fn scanning_one_file_produces_no_branches() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("alone.bin");
    fs::write(&file, vec![0u8; 100]).unwrap();

    let progress = Arc::new(ScanProgress::default());
    scan(&file, ScanOptions::default(), Arc::clone(&progress)).unwrap();
    assert!(progress.live.snapshot().is_empty());
}

/// Hardlinked bytes are counted once in the tree, so they have to be counted
/// once here too — otherwise the preview overstates a folder and then the
/// finished scan appears to lose bytes.
#[test]
#[cfg(unix)]
fn hardlinks_are_counted_once_in_the_live_view_as_well() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("one")).unwrap();
    fs::create_dir(dir.path().join("two")).unwrap();
    let original = dir.path().join("one/file.bin");
    fs::write(&original, vec![0u8; 4000]).unwrap();
    fs::hard_link(&original, dir.path().join("two/same.bin")).unwrap();

    let progress = Arc::new(ScanProgress::default());
    let (tree, _) = scan(dir.path(), ScanOptions::default(), Arc::clone(&progress)).unwrap();

    let live = progress.live.snapshot();
    assert_eq!(live.iter().map(|e| e.size).sum::<u64>(), tree.total_size());
    // Which of the two names carries the bytes is deliberately unspecified
    // (invariant 3), so the assertion is on the pair, not on a name.
    let mut sizes: Vec<u64> = live.iter().map(|e| e.size).collect();
    sizes.sort_unstable();
    assert_eq!(sizes, vec![0, 4000]);
}
