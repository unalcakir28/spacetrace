//! What an incremental rescan may take over from an older tree, and what it
//! must read again.
//!
//! The arena stores no identity, so a subtree copied from an older scan
//! cannot take part in charging a shared file once. `Node::flags` is the
//! answer: a subtree whose flags are clear holds nothing that interacts with
//! anything outside it, and only such a subtree may be copied.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use spacetrace_scan_core::{scan, Node, ScanOptions, ScanProgress, ScanStats, Tree};

#[cfg(target_os = "macos")]
#[path = "support/disk_image.rs"]
mod disk_image;
#[cfg(target_os = "macos")]
use disk_image::DiskImage;

fn run(path: &Path) -> (Tree, ScanStats) {
    scan(
        path,
        ScanOptions::default(),
        Arc::new(ScanProgress::default()),
    )
    .unwrap()
}

fn flags(tree: &Tree, path: &str) -> u8 {
    let id = tree
        .find(path)
        .unwrap_or_else(|| panic!("{path} is not in the tree"));
    tree.node(id).flags()
}

/// A tree with nothing shared, nothing unreadable and no mount in it must
/// come out with every flag clear — otherwise a rescan could never copy a
/// single directory, and the feature would quietly be a full scan.
#[test]
fn a_plain_tree_is_flagged_nowhere() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("a/b/c")).unwrap();
    fs::write(root.join("a/b/c/leaf"), vec![1u8; 5000]).unwrap();
    fs::write(root.join("a/top"), b"x").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("a/top", root.join("link")).unwrap();

    let (tree, _) = run(root);

    for id in tree.iter() {
        assert_eq!(
            tree.node(id).flags(),
            0,
            "{} is flagged in a tree with nothing to flag",
            tree.rel_path(id)
        );
    }
}

/// Both names of a hardlink are flagged, and so is every directory above
/// them — but a sibling subtree is not.
#[test]
fn a_hardlink_flags_both_names_and_their_ancestors_only() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("linked/deep")).unwrap();
    fs::create_dir_all(root.join("clean/inner")).unwrap();
    fs::write(root.join("linked/deep/x"), vec![2u8; 9000]).unwrap();
    fs::hard_link(root.join("linked/deep/x"), root.join("linked/y")).unwrap();
    fs::write(root.join("clean/inner/z"), vec![3u8; 100]).unwrap();

    let (tree, stats) = run(root);
    assert_eq!(stats.hardlinks_deduped, 1, "the fixture is what it claims");

    assert_eq!(flags(&tree, "linked/deep/x"), Node::SHARED);
    assert_eq!(flags(&tree, "linked/y"), Node::SHARED);
    assert_eq!(flags(&tree, "linked/deep"), Node::SHARED);
    assert_eq!(flags(&tree, "linked"), Node::SHARED);
    assert_eq!(flags(&tree, ""), Node::SHARED);
    assert_eq!(flags(&tree, "clean"), 0);
    assert_eq!(flags(&tree, "clean/inner"), 0);
    assert_eq!(flags(&tree, "clean/inner/z"), 0);
}

/// An APFS clone has its own inode and a link count of one, so it is the
/// sharing hardlink deduplication cannot see — and exactly as able to be
/// charged twice by a rescan.
#[cfg(target_os = "macos")]
#[test]
fn every_member_of_a_clone_family_is_flagged() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("one")).unwrap();
    fs::create_dir_all(root.join("two/deeper")).unwrap();
    fs::create_dir_all(root.join("plain")).unwrap();
    fs::write(root.join("one/original"), vec![4u8; 100_000]).unwrap();
    fs::write(root.join("plain/file"), vec![5u8; 100_000]).unwrap();
    let cloned = std::process::Command::new("cp")
        .arg("-c")
        .arg(root.join("one/original"))
        .arg(root.join("two/deeper/copy"))
        .status()
        .is_ok_and(|s| s.success());
    if !cloned {
        eprintln!("skipped: this filesystem does not clone");
        return;
    }

    let (tree, stats) = run(root);
    assert_eq!(stats.clones_deduped, 1, "the fixture is what it claims");

    assert_eq!(flags(&tree, "one/original"), Node::SHARED);
    assert_eq!(flags(&tree, "two/deeper/copy"), Node::SHARED);
    assert_eq!(flags(&tree, "two/deeper"), Node::SHARED);
    assert_eq!(flags(&tree, "two"), Node::SHARED);
    assert_eq!(flags(&tree, "plain"), 0);
    assert_eq!(flags(&tree, "plain/file"), 0);
}

/// The tree keeps no record of which directory could not be read, so the
/// flag is the only trace — and a subtree with an error in it copied into a
/// later scan would lose that error from its count.
#[cfg(unix)]
#[test]
fn an_unreadable_directory_is_flagged_with_its_ancestors() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipped: root reads everything");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("outer/locked")).unwrap();
    fs::create_dir_all(root.join("fine")).unwrap();
    fs::write(root.join("outer/locked/hidden"), b"x").unwrap();
    fs::write(root.join("fine/f"), b"y").unwrap();
    fs::set_permissions(root.join("outer/locked"), fs::Permissions::from_mode(0o000)).unwrap();

    let (tree, stats) = run(root);
    fs::set_permissions(root.join("outer/locked"), fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(stats.errors, 1);
    assert_eq!(flags(&tree, "outer/locked"), Node::ERRORS);
    assert_eq!(flags(&tree, "outer"), Node::ERRORS);
    assert_eq!(flags(&tree, ""), Node::ERRORS);
    assert_eq!(flags(&tree, "fine"), 0);
}

/// A volume mounted inside the root. Its own changes are not in the root's
/// journal, and mounting or unmounting it swaps a directory's contents
/// without a change inside that directory.
///
/// Built with a disk image, which `hdiutil` attaches without privileges;
/// where it cannot, the test says so and passes.
#[cfg(target_os = "macos")]
#[test]
fn a_mount_point_is_flagged_with_its_ancestors() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    fs::create_dir_all(root.join("holder/mnt")).unwrap();
    fs::create_dir_all(root.join("beside")).unwrap();
    fs::write(root.join("beside/f"), b"z").unwrap();
    let Some(_mounted) = DiskImage::attach(dir.path(), &root.join("holder/mnt")) else {
        eprintln!("skipped: no disk image could be attached here");
        return;
    };
    fs::write(root.join("holder/mnt/inside"), b"on the image").unwrap();

    let (tree, _) = run(&root);

    assert_eq!(flags(&tree, "holder/mnt"), Node::MOUNT | Node::MOUNT_POINT);
    assert_eq!(
        flags(&tree, "holder/mnt/inside"),
        0,
        "below it is one volume"
    );
    assert_eq!(
        flags(&tree, "holder"),
        Node::MOUNT,
        "an ancestor knows a mount is below, not that it is one"
    );
    assert_eq!(flags(&tree, "beside"), 0);
}
