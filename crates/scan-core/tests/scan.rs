use std::fs;
use std::sync::Arc;

use spacetrace_scan_core::{scan, EntryKind, ScanOptions, ScanProgress};

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("a.txt"), vec![b'a'; 1000]).unwrap();
    fs::create_dir(root.join("sub")).unwrap();
    fs::write(root.join("sub/b.bin"), vec![0u8; 4096]).unwrap();
    fs::create_dir(root.join("sub/deep")).unwrap();
    fs::write(root.join("sub/deep/c.log"), vec![b'c'; 10]).unwrap();
    fs::create_dir(root.join("node_modules")).unwrap();
    fs::write(root.join("node_modules/junk"), vec![0u8; 50_000]).unwrap();
    dir
}

fn run(
    path: &std::path::Path,
    opts: ScanOptions,
) -> (spacetrace_scan_core::Tree, spacetrace_scan_core::ScanStats) {
    scan(path, opts, Arc::new(ScanProgress::default())).unwrap()
}

#[test]
fn totals_match_the_files_on_disk() {
    let dir = fixture();
    let (tree, stats) = run(dir.path(), ScanOptions::default());

    assert_eq!(stats.errors, 0);
    assert_eq!(stats.files, 4, "a.txt, b.bin, c.log, junk");
    assert_eq!(stats.dirs, 4, "root, sub, sub/deep, node_modules");

    // Logical sizes count file bytes only, so they are exact. Allocated sizes
    // include directory blocks and depend on the filesystem.
    assert_eq!(tree.total_size(), 1000 + 4096 + 10 + 50_000);
    assert!(tree.total_alloc() >= 4096);

    let root = tree.node(tree.root());
    assert_eq!(root.files, 4);
    assert_eq!(root.dirs, 3, "subdirectories, excluding the root itself");
}

#[test]
fn subtree_totals_roll_up() {
    let dir = fixture();
    let (tree, _) = run(dir.path(), ScanOptions::default());

    let sub = tree.find("sub").expect("sub exists");
    assert_eq!(tree.node(sub).size, 4096 + 10);
    assert_eq!(tree.node(sub).files, 2);

    let deep = tree.find("sub/deep").expect("sub/deep exists");
    assert_eq!(tree.node(deep).size, 10);
    assert_eq!(tree.rel_path(deep), "sub/deep");
    assert_eq!(
        tree.path(deep),
        dir.path().canonicalize().unwrap().join("sub/deep")
    );
}

#[test]
fn excluded_directories_are_not_descended_into() {
    let dir = fixture();
    let opts = ScanOptions {
        exclude_names: vec!["node_modules".to_string()],
        ..Default::default()
    };
    let (tree, stats) = run(dir.path(), opts);

    assert_eq!(tree.total_size(), 1000 + 4096 + 10);
    assert_eq!(stats.files, 3);
    let nm = tree
        .find("node_modules")
        .expect("still listed, just not opened");
    assert_eq!(tree.node(nm).size, 0);
    assert_eq!(tree.node(nm).children_len, 0);
}

#[test]
fn max_depth_stops_the_walk() {
    let dir = fixture();
    let opts = ScanOptions {
        max_depth: Some(1),
        ..Default::default()
    };
    let (tree, _) = run(dir.path(), opts);

    assert_eq!(
        tree.total_size(),
        1000,
        "only entries directly under the root"
    );
    assert!(tree.find("sub").is_some());
    assert!(tree.find("sub/b.bin").is_none());
}

#[test]
fn children_are_contiguous_and_sorted_by_size_on_demand() {
    let dir = fixture();
    let (tree, _) = run(dir.path(), ScanOptions::default());

    for id in tree.iter() {
        let n = tree.node(id);
        for child in tree.children(id) {
            assert_eq!(tree.node(child).parent, id);
            assert!(child > id, "BFS layout puts children after their parent");
        }
        assert_eq!(tree.children(id).count(), n.children_len as usize);
    }

    let by_size = tree.children_by_size(tree.root());
    let sizes: Vec<u64> = by_size.iter().map(|&c| tree.node(c).size).collect();
    assert!(
        sizes.windows(2).all(|w| w[0] >= w[1]),
        "descending: {sizes:?}"
    );
    assert_eq!(tree.node(by_size[0]).name, "node_modules");
}

#[test]
fn largest_files_are_ranked() {
    let dir = fixture();
    let (tree, _) = run(dir.path(), ScanOptions::default());

    let top = tree.largest(2, Some(EntryKind::File));
    assert_eq!(tree.node(top[0]).name, "junk");
    assert_eq!(tree.node(top[1]).name, "b.bin");
}

#[test]
fn scanning_a_single_file_yields_a_one_node_tree() {
    let dir = fixture();
    let (tree, stats) = run(&dir.path().join("a.txt"), ScanOptions::default());
    assert_eq!(tree.len(), 1);
    assert_eq!(tree.total_size(), 1000);
    assert_eq!(stats.files, 1);
}

#[cfg(unix)]
#[test]
fn hardlinked_files_are_counted_once() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("original"), vec![0u8; 8192]).unwrap();
    fs::create_dir(root.join("elsewhere")).unwrap();
    fs::hard_link(root.join("original"), root.join("elsewhere/linked")).unwrap();

    let (tree, stats) = run(root, ScanOptions::default());
    assert_eq!(tree.total_size(), 8192, "the second link adds no bytes");
    assert_eq!(stats.hardlinks_deduped, 1);

    let opts = ScanOptions {
        dedupe_hardlinks: false,
        ..Default::default()
    };
    let (tree, stats) = run(root, opts);
    assert_eq!(tree.total_size(), 16384, "without dedup both links count");
    assert_eq!(stats.hardlinks_deduped, 0);
}

#[cfg(unix)]
#[test]
fn symlinks_are_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir(root.join("real")).unwrap();
    fs::write(root.join("real/big"), vec![0u8; 100_000]).unwrap();
    std::os::unix::fs::symlink(root.join("real"), root.join("loop")).unwrap();

    let (tree, _) = run(root, ScanOptions::default());
    let link = tree.find("loop").unwrap();
    assert_eq!(
        tree.total_size(),
        100_000 + tree.node(link).size,
        "the symlink adds its own size, never its target's"
    );
    assert_eq!(tree.node(link).kind, EntryKind::Symlink);
    assert_eq!(tree.node(link).children_len, 0);
}

#[test]
fn unreadable_directories_are_reported_not_fatal() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("visible"), vec![0u8; 64]).unwrap();
    fs::create_dir(root.join("locked")).unwrap();
    fs::write(root.join("locked/hidden"), vec![0u8; 4096]).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
    }

    let (tree, stats) = run(root, ScanOptions::default());

    #[cfg(unix)]
    if !nix_running_as_root() {
        assert!(stats.errors >= 1, "the locked directory should be reported");
        assert!(!stats.error_samples.is_empty());
        assert_eq!(
            tree.total_size(),
            64,
            "unreadable contents contribute nothing"
        );
        // restore so the tempdir can be cleaned up
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }

    let _ = (tree, stats);
}

#[cfg(unix)]
fn nix_running_as_root() -> bool {
    // root ignores permission bits, so the test above cannot provoke an error.
    unsafe { libc_geteuid() == 0 }
}

#[cfg(unix)]
extern "C" {
    #[link_name = "geteuid"]
    fn libc_geteuid() -> u32;
}

// ------------------------------------------------------------- cancellation

#[test]
fn a_scan_cancelled_before_it_starts_fails_rather_than_returning_a_stub() {
    let dir = fixture();
    let progress = Arc::new(ScanProgress::default());
    progress.cancel();

    let err = scan(dir.path(), ScanOptions::default(), Arc::clone(&progress)).unwrap_err();

    // The danger a cancelled scan poses is not failing — it is succeeding with
    // a tree that looks whole and is not. `Interrupted` is what tells the
    // caller the difference.
    assert_eq!(err.kind(), std::io::ErrorKind::Interrupted, "{err}");
    assert!(progress.is_cancelled());
}

#[test]
fn cancelling_stops_the_walk_partway_through() {
    // Deep enough that the cancel lands mid-walk rather than after it.
    let dir = tempfile::tempdir().unwrap();
    let mut path = dir.path().to_path_buf();
    for level in 0..40 {
        path.push(format!("level{level}"));
        fs::create_dir(&path).unwrap();
        for file in 0..40 {
            fs::write(path.join(format!("f{file}")), b"x").unwrap();
        }
    }

    let progress = Arc::new(ScanProgress::default());
    let watcher = Arc::clone(&progress);
    // Cancel as soon as the walk is demonstrably running, so the test does not
    // depend on how fast the machine is.
    let stopper = std::thread::spawn(move || {
        while watcher.files.load(std::sync::atomic::Ordering::Relaxed) < 40 {
            std::thread::yield_now();
        }
        watcher.cancel();
    });

    let err = scan(dir.path(), ScanOptions::default(), Arc::clone(&progress)).unwrap_err();
    stopper.join().unwrap();

    assert_eq!(err.kind(), std::io::ErrorKind::Interrupted, "{err}");
    let seen = progress.files.load(std::sync::atomic::Ordering::Relaxed);
    assert!(seen >= 40, "the walk should have started: {seen}");
    assert!(
        seen < 40 * 40,
        "the walk should not have finished all 1600 files: {seen}"
    );
}

#[test]
fn a_scan_that_is_never_cancelled_is_unaffected() {
    let dir = fixture();
    let progress = Arc::new(ScanProgress::default());
    let (tree, stats) = scan(dir.path(), ScanOptions::default(), Arc::clone(&progress)).unwrap();

    assert!(!progress.is_cancelled());
    assert_eq!(stats.files, 4);
    assert_eq!(tree.total_size(), 1000 + 4096 + 10 + 50_000);
}

// ------------------------------------------------------- removing an entry

/// Deleting a file must not cost a rescan, because a rescan renumbers every
/// node and a UI holding ids then has to forget everything it knows.
mod removal {
    use super::*;

    #[test]
    fn removing_a_file_corrects_every_total_above_it() {
        let dir = fixture();
        let (mut tree, _) = run(dir.path(), ScanOptions::default());

        let before = tree.total_size();
        let target = tree.find("sub/b.bin").expect("b.bin is in the fixture");
        let sub = tree.find("sub").unwrap();
        let sub_before = tree.node(sub).size;

        let removed = tree.remove_subtree(target).expect("a file can be removed");

        assert_eq!(removed.size, 4096);
        assert_eq!(removed.files, 1);
        assert_eq!(removed.dirs, 0, "a file is not a directory");
        assert_eq!(removed.nodes, vec![target]);

        assert_eq!(tree.total_size(), before - 4096);
        assert_eq!(tree.node(sub).size, sub_before - 4096);
        assert_eq!(tree.node(target).size, 0);
    }

    #[test]
    fn removing_a_directory_takes_its_whole_subtree_with_it() {
        let dir = fixture();
        let (mut tree, _) = run(dir.path(), ScanOptions::default());

        let root_files = tree.node(tree.root()).files;
        let root_dirs = tree.node(tree.root()).dirs;
        let sub = tree.find("sub").unwrap();
        let sub_size = tree.node(sub).size;

        let removed = tree.remove_subtree(sub).unwrap();

        // sub, sub/b.bin, sub/deep, sub/deep/c.log
        assert_eq!(removed.nodes.len(), 4, "{:?}", removed.nodes);
        assert_eq!(removed.size, sub_size);
        assert_eq!(removed.files, 2, "b.bin and c.log");
        assert_eq!(removed.dirs, 2, "sub itself plus sub/deep");

        let root = tree.node(tree.root());
        assert_eq!(root.files, root_files - 2);
        assert_eq!(root.dirs, root_dirs - 2);

        // Nothing below it is reachable or counted any more.
        assert_eq!(tree.children(sub).count(), 0);
        for gone in removed.nodes {
            assert_eq!(tree.node(gone).size, 0);
            assert_eq!(tree.node(gone).files, 0);
        }
    }

    #[test]
    fn ids_outside_the_removed_subtree_still_mean_what_they_meant() {
        // The whole reason for editing in place rather than rescanning.
        let dir = fixture();
        let (mut tree, _) = run(dir.path(), ScanOptions::default());

        let keep = tree.find("a.txt").unwrap();
        let keep_name = tree.node(keep).name.clone();
        let keep_size = tree.node(keep).size;

        tree.remove_subtree(tree.find("sub").unwrap()).unwrap();

        assert_eq!(tree.node(keep).name, keep_name);
        assert_eq!(tree.node(keep).size, keep_size);
        assert_eq!(tree.rel_path(keep), "a.txt");
    }

    #[test]
    fn removing_twice_takes_the_bytes_away_once() {
        let dir = fixture();
        let (mut tree, _) = run(dir.path(), ScanOptions::default());
        let target = tree.find("sub/b.bin").unwrap();
        let before = tree.total_size();

        tree.remove_subtree(target).unwrap();
        let after_first = tree.total_size();
        let second = tree.remove_subtree(target).unwrap();

        assert_eq!(second.size, 0, "it no longer accounts for anything");
        assert_eq!(tree.total_size(), after_first);
        assert_eq!(after_first, before - 4096);
    }

    #[test]
    fn the_root_cannot_be_removed_from_its_own_tree() {
        let dir = fixture();
        let (mut tree, _) = run(dir.path(), ScanOptions::default());
        assert!(tree.remove_subtree(tree.root()).is_none());
        assert!(tree.total_size() > 0, "and nothing was changed");
    }

    #[test]
    fn an_id_that_does_not_exist_is_refused() {
        let dir = fixture();
        let (mut tree, _) = run(dir.path(), ScanOptions::default());
        let past_the_end = tree.len() as u32;
        assert!(tree.remove_subtree(past_the_end).is_none());
    }

    #[test]
    fn the_arena_still_verifies_after_a_removal() {
        // Removal must not break the invariants that make loading safe, or a
        // tree edited here could not be trusted afterwards.
        let dir = fixture();
        let (mut tree, _) = run(dir.path(), ScanOptions::default());
        tree.remove_subtree(tree.find("sub").unwrap()).unwrap();

        let nodes = tree.nodes().to_vec();
        let path = tree.root_path().to_path_buf();
        spacetrace_scan_core::Tree::from_parts_checked(nodes, path)
            .expect("the edited arena is still well formed");
    }
}

// ------------------------------------------------- untrusted snapshot loading

/// A snapshot can arrive from another machine, so the arena invariants have to
/// be checked rather than assumed. Each case here would otherwise panic on an
/// out-of-range index or loop forever.
mod untrusted {
    use spacetrace_scan_core::{EntryKind, Node, Tree, TreeError};
    use std::path::PathBuf;

    fn node(parent: u32, children_start: u32, children_len: u32) -> Node {
        Node {
            parent,
            name: "n".into(),
            kind: EntryKind::Dir,
            size: 0,
            alloc: 0,
            own_size: 0,
            own_alloc: 0,
            mtime: 0,
            nlink: 1,
            files: 0,
            dirs: 0,
            children_start,
            children_len,
        }
    }

    fn root(children_start: u32, children_len: u32) -> Node {
        node(Tree::NO_PARENT, children_start, children_len)
    }

    #[test]
    fn a_well_formed_arena_is_accepted() {
        let nodes = vec![root(1, 2), node(0, 0, 0), node(0, 0, 0)];
        let tree = Tree::from_parts_checked(nodes, PathBuf::from("/x")).unwrap();
        assert_eq!(tree.len(), 3);
        assert_eq!(tree.children(tree.root()).count(), 2);
    }

    #[test]
    fn an_empty_arena_is_rejected() {
        assert_eq!(
            Tree::from_parts_checked(vec![], PathBuf::from("/x")).unwrap_err(),
            TreeError::Empty
        );
    }

    #[test]
    fn children_past_the_end_are_rejected() {
        // Would index out of bounds on the first traversal.
        let nodes = vec![root(1, 9), node(0, 0, 0)];
        assert!(matches!(
            Tree::from_parts_checked(nodes, PathBuf::from("/x")).unwrap_err(),
            TreeError::ChildrenOutOfBounds { .. }
        ));
    }

    #[test]
    fn children_pointing_backwards_are_rejected() {
        // Would make a cycle: node 1's children include node 1.
        let nodes = vec![root(1, 1), node(0, 1, 1)];
        assert!(matches!(
            Tree::from_parts_checked(nodes, PathBuf::from("/x")).unwrap_err(),
            TreeError::ChildrenNotAfterParent { .. }
        ));
    }

    #[test]
    fn a_node_pointing_at_itself_as_a_child_is_rejected() {
        let nodes = vec![root(0, 1)];
        assert!(matches!(
            Tree::from_parts_checked(nodes, PathBuf::from("/x")).unwrap_err(),
            TreeError::ChildrenNotAfterParent { .. }
        ));
    }

    #[test]
    fn a_parent_pointer_that_is_not_before_its_child_is_rejected() {
        // rel_path walks parent pointers upward; this would never terminate.
        let nodes = vec![root(1, 1), node(2, 0, 0), node(1, 0, 0)];
        assert!(matches!(
            Tree::from_parts_checked(nodes, PathBuf::from("/x")).unwrap_err(),
            TreeError::ParentNotBeforeChild { .. }
        ));
    }

    #[test]
    fn a_root_claiming_a_parent_is_rejected() {
        let nodes = vec![node(0, 0, 0)];
        assert_eq!(
            Tree::from_parts_checked(nodes, PathBuf::from("/x")).unwrap_err(),
            TreeError::RootHasParent
        );
    }

    #[test]
    fn a_childless_node_may_leave_children_start_at_zero() {
        // children_len == 0 means children_start is meaningless, and the
        // scanner does leave it at 0 — this must not be mistaken for a cycle.
        let nodes = vec![root(1, 1), node(0, 0, 0)];
        assert!(Tree::from_parts_checked(nodes, PathBuf::from("/x")).is_ok());
    }
}
