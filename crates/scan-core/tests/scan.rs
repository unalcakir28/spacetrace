use std::fs;
use std::sync::Arc;

use spacetrace_scan_core::{
    scan, EntryKind, Phase, ScanOptions, ScanProgress, SizeBasis, StallWatch, STALL_GRACE,
};

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

    let by_size = tree.children_by(tree.root(), SizeBasis::Logical);
    let sizes: Vec<u64> = by_size.iter().map(|&c| tree.node(c).size).collect();
    assert!(
        sizes.windows(2).all(|w| w[0] >= w[1]),
        "descending: {sizes:?}"
    );
    assert_eq!(tree.name(by_size[0]), "node_modules");
}

#[test]
fn largest_files_are_ranked() {
    let dir = fixture();
    let (tree, _) = run(dir.path(), ScanOptions::default());

    let top = tree.largest(2, Some(EntryKind::File));
    assert_eq!(tree.name(top[0]), "junk");
    assert_eq!(tree.name(top[1]), "b.bin");
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

/// 40 levels deep, 40 files each — 1600 files that a walk has to work through.
fn deep_tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let mut path = dir.path().to_path_buf();
    for level in 0..40 {
        path.push(format!("level{level}"));
        fs::create_dir(&path).unwrap();
        for file in 0..40 {
            fs::write(path.join(format!("f{file}")), b"x").unwrap();
        }
    }
    dir
}

#[test]
fn a_scan_cancelled_before_it_starts_reads_nothing_at_all() {
    let dir = deep_tree();
    let progress = Arc::new(ScanProgress::default());
    progress.cancel();

    let err = scan(dir.path(), ScanOptions::default(), Arc::clone(&progress)).unwrap_err();

    // The danger a cancelled scan poses is not failing — it is succeeding with
    // a tree that looks whole and is not. `Interrupted` is what tells the
    // caller the difference.
    assert_eq!(err.kind(), std::io::ErrorKind::Interrupted, "{err}");
    assert!(progress.is_cancelled());

    // This is the assertion that proves cancellation is checked *during* the
    // walk rather than noticed at the end. Without the check at the top of
    // every directory, all 1600 files below would have been visited before
    // anyone looked at the flag — and the count says none were.
    //
    // It lives here, on a scan cancelled before it starts, because that is the
    // only way to test it without a race: any test that cancels from another
    // thread is betting the walk is slower than the scheduler, and on a fast
    // machine that bet loses (it lost on macOS CI).
    assert_eq!(
        progress.files.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "a cancelled walk must not read a single directory"
    );
}

#[test]
fn cancelling_from_another_thread_mid_walk_yields_no_tree() {
    let dir = deep_tree();
    let progress = Arc::new(ScanProgress::default());
    let watcher = Arc::clone(&progress);

    // The scan does not start until the watcher is already spinning. Without
    // this the test was really measuring thread-spawn latency against the
    // whole walk, and on a two-core CI runner with eight rayon threads the
    // watcher could fail to be scheduled at all before the walk was over.
    let ready = Arc::new(std::sync::Barrier::new(2));
    let their_turn = Arc::clone(&ready);
    let stopper = std::thread::spawn(move || {
        their_turn.wait();
        while watcher.files.load(std::sync::atomic::Ordering::Relaxed) < 40 {
            std::thread::yield_now();
        }
        watcher.cancel();
    });
    ready.wait();

    let outcome = scan(dir.path(), ScanOptions::default(), Arc::clone(&progress));
    stopper.join().unwrap();

    // Whether the cancel lands before the walk ends is a property of the
    // machine, not of this code — this tree is small and the macOS walk got
    // 2.3× faster in B5, so a fast machine now often finishes first. What must
    // hold either way is the thing worth testing: **never a partial tree.**
    // Cancelled means no tree at all (invariant #5); not cancelled in time
    // means the whole tree. There is no third answer, and a scan that returned
    // a truncated tree with a confident total would be the worst outcome of
    // the three.
    match outcome {
        Err(err) => assert_eq!(err.kind(), std::io::ErrorKind::Interrupted, "{err}"),
        Ok((tree, _)) => assert_eq!(
            tree.len(),
            1 + 40 + 40 * 40,
            "the walk beat the cancel, so it owes a complete tree"
        ),
    }
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
        let keep_name = tree.name(keep).to_string();
        let keep_size = tree.node(keep).size;

        tree.remove_subtree(tree.find("sub").unwrap()).unwrap();

        assert_eq!(tree.name(keep), keep_name);
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

        // Round-trip it the way a snapshot would be: rebuilding through the
        // assembler runs exactly the checks a loaded tree gets.
        let mut asm = spacetrace_scan_core::TreeAssembler::with_capacity(tree.len());
        for id in tree.iter() {
            let n = tree.node(id);
            asm.push(spacetrace_scan_core::StoredNode {
                parent: n.parent,
                name: tree.name(id),
                kind: n.kind,
                size: n.size,
                alloc: n.alloc,
                own_size: n.own_size,
                own_alloc: n.own_alloc,
                mtime: n.mtime,
                nlink: n.nlink,
                files: n.files,
                dirs: n.dirs,
                children_start: n.children_start,
                children_len: n.children_len,
            });
        }
        asm.finish(tree.root_path().to_path_buf())
            .expect("the edited arena is still well formed");
    }
}

// ------------------------------------------------- untrusted snapshot loading

/// A snapshot can arrive from another machine, so the arena invariants have to
/// be checked rather than assumed. Each case here would otherwise panic on an
/// out-of-range index or loop forever.
mod untrusted {
    use spacetrace_scan_core::{EntryKind, StoredNode, Tree, TreeAssembler, TreeError};
    use std::path::PathBuf;

    struct Row {
        parent: u32,
        children_start: u32,
        children_len: u32,
    }

    fn node(parent: u32, children_start: u32, children_len: u32) -> Row {
        Row {
            parent,
            children_start,
            children_len,
        }
    }

    fn root(children_start: u32, children_len: u32) -> Row {
        node(Tree::NO_PARENT, children_start, children_len)
    }

    /// Push the rows through the assembler, which is the only way a loaded
    /// snapshot becomes a tree and therefore the only place worth testing.
    fn build(rows: Vec<Row>) -> Result<Tree, TreeError> {
        let mut asm = TreeAssembler::with_capacity(rows.len());
        for r in rows {
            asm.push(StoredNode {
                parent: r.parent,
                name: "n",
                kind: EntryKind::Dir,
                size: 0,
                alloc: 0,
                own_size: 0,
                own_alloc: 0,
                mtime: 0,
                nlink: 1,
                files: 0,
                dirs: 0,
                children_start: r.children_start,
                children_len: r.children_len,
            });
        }
        asm.finish(PathBuf::from("/x"))
    }

    #[test]
    fn a_well_formed_arena_is_accepted() {
        let nodes = vec![root(1, 2), node(0, 0, 0), node(0, 0, 0)];
        let tree = build(nodes).unwrap();
        assert_eq!(tree.len(), 3);
        assert_eq!(tree.children(tree.root()).count(), 2);
    }

    #[test]
    fn an_empty_arena_is_rejected() {
        assert_eq!(build(vec![]).unwrap_err(), TreeError::Empty);
    }

    #[test]
    fn children_past_the_end_are_rejected() {
        // Would index out of bounds on the first traversal.
        let nodes = vec![root(1, 9), node(0, 0, 0)];
        assert!(matches!(
            build(nodes).unwrap_err(),
            TreeError::ChildrenOutOfBounds { .. }
        ));
    }

    #[test]
    fn children_pointing_backwards_are_rejected() {
        // Would make a cycle: node 1's children include node 1.
        let nodes = vec![root(1, 1), node(0, 1, 1)];
        assert!(matches!(
            build(nodes).unwrap_err(),
            TreeError::ChildrenNotAfterParent { .. }
        ));
    }

    #[test]
    fn a_node_pointing_at_itself_as_a_child_is_rejected() {
        let nodes = vec![root(0, 1)];
        assert!(matches!(
            build(nodes).unwrap_err(),
            TreeError::ChildrenNotAfterParent { .. }
        ));
    }

    #[test]
    fn a_parent_pointer_that_is_not_before_its_child_is_rejected() {
        // rel_path walks parent pointers upward; this would never terminate.
        let nodes = vec![root(1, 1), node(2, 0, 0), node(1, 0, 0)];
        assert!(matches!(
            build(nodes).unwrap_err(),
            TreeError::ParentNotBeforeChild { .. }
        ));
    }

    #[test]
    fn a_root_claiming_a_parent_is_rejected() {
        let nodes = vec![node(0, 0, 0)];
        assert_eq!(build(nodes).unwrap_err(), TreeError::RootHasParent);
    }

    #[test]
    fn a_childless_node_may_leave_children_start_at_zero() {
        // children_len == 0 means children_start is meaningless, and the
        // scanner does leave it at 0 — this must not be mistaken for a cycle.
        let nodes = vec![root(1, 1), node(0, 0, 0)];
        assert!(build(nodes).is_ok());
    }
}

// ------------------------------------------------------------- size basis

/// Ranking by the two measures, on a real sparse file.
///
/// Every consumer that says "biggest first" sorts through `children_by`, so if
/// the basis does not reach the ordering, the figures in a list and the order
/// of that list disagree — and the entry the user is looking for is the one
/// most likely to be in the wrong place.
mod basis {
    use std::fs;
    // Only the sparse-file test writes, and that one does not build on Windows.
    #[cfg(not(windows))]
    use std::io::Write;
    use std::sync::Arc;

    use spacetrace_scan_core::{scan, ScanOptions, ScanProgress, SizeBasis};

    // Not on Windows: `alloc` there is still the logical length (`TODO(win)` in
    // `meta.rs` — it needs `GetFileInformationByHandleEx`), so the on-disk half
    // of this assertion is testing the platform gap rather than the basis. The
    // ordering itself is covered on the platforms that report real blocks.
    #[cfg(not(windows))]
    #[test]
    fn a_sparse_file_outranks_a_dense_one_logically_and_loses_on_disk() {
        let dir = tempfile::tempdir().unwrap();

        let mut f = fs::File::create(dir.path().join("sparse.img")).unwrap();
        f.set_len(1 << 30).unwrap();
        f.write_all(&vec![0xAB; 64 * 1024]).unwrap();
        f.sync_all().unwrap();
        drop(f);
        fs::write(dir.path().join("dense.bin"), vec![0u8; 4 * 1024 * 1024]).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let md = fs::metadata(dir.path().join("sparse.img")).unwrap();
            if md.blocks() * 512 > (1 << 30) / 2 {
                eprintln!("skipping: this filesystem does not do sparse files");
                return;
            }
        }

        let (tree, _) = scan(
            dir.path(),
            ScanOptions::default(),
            Arc::new(ScanProgress::default()),
        )
        .unwrap();

        let name_of = |id| tree.name(id).to_string();

        let logical = tree.children_by(tree.root(), SizeBasis::Logical);
        assert_eq!(
            name_of(logical[0]),
            "sparse.img",
            "logically the claim wins: 1 GiB against 4 MiB"
        );

        let on_disk = tree.children_by(tree.root(), SizeBasis::OnDisk);
        assert_eq!(
            name_of(on_disk[0]),
            "dense.bin",
            "on disk the blocks win: 4 MiB against 64 KiB"
        );
    }

    #[test]
    fn measure_returns_the_field_the_basis_names() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.bin"), vec![0u8; 8192]).unwrap();
        let (tree, _) = scan(
            dir.path(),
            ScanOptions::default(),
            Arc::new(ScanProgress::default()),
        )
        .unwrap();

        let root = tree.node(tree.root());
        assert_eq!(root.measure(SizeBasis::Logical), root.size);
        assert_eq!(root.measure(SizeBasis::OnDisk), root.alloc);
    }

    /// A one-byte file allocates a whole block, so it is *bigger* on disk than
    /// its length. The divergence runs both ways and neither side is a bug.
    ///
    /// Windows is excluded for the same reason as above: `alloc` equals the
    /// length there, so one byte reports one byte and there is no block to see.
    #[cfg(not(windows))]
    #[test]
    fn a_tiny_file_is_larger_on_disk_than_its_length() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("tiny.txt"), b"x").unwrap();
        let (tree, _) = scan(
            dir.path(),
            ScanOptions::default(),
            Arc::new(ScanProgress::default()),
        )
        .unwrap();

        let tiny = tree.children_by(tree.root(), SizeBasis::Logical)[0];
        let node = tree.node(tiny);
        assert_eq!(node.measure(SizeBasis::Logical), 1);
        assert!(
            node.measure(SizeBasis::OnDisk) >= 512,
            "one byte still costs a block, got {}",
            node.measure(SizeBasis::OnDisk)
        );
    }
}

/// Thread count is a performance knob, not a semantic one.
///
/// With no hardlinks in the tree there is nothing left to race over, so the
/// result must be identical down to each node's own size — a width that
/// changed the interleaving must not change the arena, the order of children
/// or any total.
#[test]
fn the_thread_count_does_not_change_the_answer() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // Wide and deep enough that eight threads genuinely interleave.
    for d in 0..24 {
        let sub = root.join(format!("d{d}"));
        fs::create_dir(&sub).unwrap();
        for f in 0..12 {
            fs::write(sub.join(format!("f{f}")), vec![b'x'; 100 + f * 7]).unwrap();
        }
        fs::create_dir(sub.join("deep")).unwrap();
        fs::write(sub.join("deep/leaf"), vec![b'y'; 512]).unwrap();
    }

    let shape = |threads: usize| {
        let opts = ScanOptions {
            threads: Some(threads),
            ..ScanOptions::default()
        };
        let (tree, stats) = run(root, opts);
        let nodes: Vec<(String, u64, u64)> = (0..tree.len() as u32)
            .map(|id| {
                let n = tree.node(id);
                (tree.name(id).to_string(), n.size, n.files as u64)
            })
            .collect();
        (
            tree.total_size(),
            tree.total_alloc(),
            tree.len(),
            stats.files,
            stats.dirs,
            stats.errors,
            nodes,
        )
    };

    let one = shape(1);
    for threads in [2, 8, 16] {
        assert_eq!(
            shape(threads),
            one,
            "walking with {threads} threads gave a different tree than with 1"
        );
    }
}

/// A hardlink is the one thing the width genuinely changes, and the guarantee
/// is narrower than it looks.
///
/// Invariant 3: the bytes are counted **once**, not "at the first path". The
/// thread that claims the inode wins, so which of the two names carries the
/// bytes varies with the width — and so does the subtree total of whichever
/// directory that name sits in. What may not vary is the root total and the
/// fact that exactly one copy was charged.
#[test]
fn a_hardlink_is_counted_once_at_every_width() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for d in 0..24 {
        let sub = root.join(format!("d{d}"));
        fs::create_dir(&sub).unwrap();
        for f in 0..12 {
            fs::write(sub.join(format!("f{f}")), vec![b'x'; 100]).unwrap();
        }
    }
    fs::hard_link(root.join("d0/f0"), root.join("d1/linked")).unwrap();

    for threads in [1, 2, 8, 16] {
        let opts = ScanOptions {
            threads: Some(threads),
            ..ScanOptions::default()
        };
        let (tree, stats) = run(root, opts);
        assert_eq!(
            stats.hardlinks_deduped, 1,
            "one link, one dedupe, whatever the width ({threads} threads)"
        );
        assert_eq!(
            tree.total_size(),
            24 * 12 * 100,
            "the linked copy must add nothing at {threads} threads"
        );
        // Both names are present; one of them carries nothing.
        let charged: Vec<u64> = (0..tree.len() as u32)
            .filter(|id| matches!(tree.name(*id), "f0" | "linked"))
            .map(|id| tree.node(id).size)
            .collect();
        assert_eq!(charged.iter().filter(|s| **s == 0).count(), 1);
    }
}

/// Every path that goes into the in-flight list has to come out, on every way
/// the listing can end — otherwise a finished scan reports itself as stuck on
/// a directory it read minutes ago.
#[test]
fn nothing_is_left_in_flight_after_a_scan() {
    let dir = fixture();
    let progress = Arc::new(ScanProgress::default());
    let (tree, _) = scan(dir.path(), ScanOptions::default(), Arc::clone(&progress)).unwrap();
    assert!(tree.len() > 1);
    assert!(
        progress.reading_now().is_empty(),
        "still listed as reading: {:?}",
        progress.reading_now()
    );
}

/// The unreadable-directory path returns early, and an early return is exactly
/// where a hand-written "remove it afterwards" would have been forgotten.
#[cfg(unix)]
#[test]
fn an_unreadable_directory_is_not_left_in_flight() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let locked = dir.path().join("locked");
    fs::create_dir(&locked).unwrap();
    fs::write(locked.join("hidden"), b"x").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

    let progress = Arc::new(ScanProgress::default());
    let (_, stats) = scan(dir.path(), ScanOptions::default(), Arc::clone(&progress)).unwrap();

    // Restore before the assertions so a failure still leaves a deletable dir.
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

    assert!(stats.errors > 0, "the locked directory should have counted");
    assert!(
        progress.reading_now().is_empty(),
        "an unreadable directory stayed in the in-flight list: {:?}",
        progress.reading_now()
    );
}

/// A cancelled scan is the other early return.
#[test]
fn a_cancelled_scan_leaves_nothing_in_flight() {
    let dir = fixture();
    let progress = Arc::new(ScanProgress::default());
    progress.cancel();
    let refused = scan(dir.path(), ScanOptions::default(), Arc::clone(&progress));
    assert!(refused.is_err());
    assert!(progress.reading_now().is_empty());
}

/// The clone probe is the one phase that moves no other counter, so its own
/// counter is what tells a watcher "still working" from "stuck". If it stops
/// being incremented, a healthy scan starts looking hung — which is why this
/// asserts the counter moved rather than only that the dedupe worked.
#[test]
fn the_clone_probe_reports_what_it_checked() {
    let dir = tempfile::tempdir().unwrap();
    // Same size and over the 64 KiB floor, so both are candidates. Whether
    // they turn out to be clones is beside the point here.
    for name in ["one.bin", "two.bin"] {
        fs::write(dir.path().join(name), vec![b'z'; 128 * 1024]).unwrap();
    }

    let progress = Arc::new(ScanProgress::default());
    let opts = ScanOptions {
        dedupe_clones: true,
        ..ScanOptions::default()
    };
    scan(dir.path(), opts, Arc::clone(&progress)).unwrap();

    assert!(
        progress
            .clones_probed
            .load(std::sync::atomic::Ordering::Relaxed)
            >= 2,
        "both candidates should have been counted, got {}",
        progress
            .clones_probed
            .load(std::sync::atomic::Ordering::Relaxed)
    );
}

/// A progress line that still says "scanning…" after the walk is finished is
/// telling the reader something untrue, so the phase has to be marked.
#[test]
fn the_phase_moves_on_when_the_walk_is_done() {
    let dir = fixture();
    let progress = Arc::new(ScanProgress::default());
    assert_eq!(progress.phase(), Phase::Walking, "before anything happens");
    scan(dir.path(), ScanOptions::default(), Arc::clone(&progress)).unwrap();
    assert_eq!(progress.phase(), Phase::Finishing);
}

// ------------------------------------------------- the stall watch

/// A scan that is moving is never a stall, however long it runs.
#[test]
fn a_moving_scan_is_never_stalled() {
    use std::time::{Duration, Instant};
    let progress = ScanProgress::default();
    let start = Instant::now();
    let mut watch = StallWatch::new(start, STALL_GRACE);
    for step in 1..50u64 {
        progress
            .files
            .store(step, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            watch.observe(&progress, start + Duration::from_secs(step * 60)),
            None,
            "the counters moved, so an hour of wall clock is still not a stall"
        );
    }
}

#[test]
fn standing_still_becomes_a_stall_only_after_the_grace_period() {
    use std::time::{Duration, Instant};
    let progress = ScanProgress::default();
    progress
        .files
        .store(7, std::sync::atomic::Ordering::Relaxed);
    let start = Instant::now();
    let mut watch = StallWatch::new(start, STALL_GRACE);

    assert_eq!(watch.observe(&progress, start), None, "first sighting");
    assert_eq!(
        watch.observe(&progress, start + STALL_GRACE - Duration::from_millis(1)),
        None,
        "one millisecond short is not a stall"
    );
    assert_eq!(
        watch.observe(&progress, start + STALL_GRACE),
        Some(STALL_GRACE),
        "the boundary itself counts, or the message never appears"
    );
}

/// Measured from when movement stopped, not from when anyone last looked: a
/// watcher polling every 120 ms would otherwise under-report by that much, and
/// the boundary case would never fire at all.
#[test]
fn the_wait_is_measured_from_when_movement_stopped() {
    use std::time::{Duration, Instant};
    let progress = ScanProgress::default();
    let start = Instant::now();
    let mut watch = StallWatch::new(start, STALL_GRACE);
    watch.observe(&progress, start);
    let waited = watch
        .observe(&progress, start + Duration::from_secs(90))
        .expect("ninety seconds of nothing is a stall");
    assert_eq!(waited, Duration::from_secs(90));
}

#[test]
fn movement_after_a_stall_clears_it() {
    use std::time::{Duration, Instant};
    let progress = ScanProgress::default();
    let start = Instant::now();
    let mut watch = StallWatch::new(start, STALL_GRACE);
    watch.observe(&progress, start);
    assert!(watch.observe(&progress, start + STALL_GRACE).is_some());

    progress
        .files
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(
        watch.observe(&progress, start + STALL_GRACE + Duration::from_secs(1)),
        None,
        "one entry read is enough to say the scan is alive again"
    );
}

/// Invariant 8 in practice: the phase after the walk moves only
/// `clones_probed`, and a watcher that ignored it would call that phase a
/// stall. This is the test that fails if a future counter is left out.
#[test]
fn a_phase_that_only_probes_clones_is_not_a_stall() {
    use std::time::{Duration, Instant};
    let progress = ScanProgress::default();
    let start = Instant::now();
    let mut watch = StallWatch::new(start, STALL_GRACE);
    watch.observe(&progress, start);

    progress
        .clones_probed
        .store(1, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(
        watch.observe(&progress, start + Duration::from_secs(60)),
        None,
        "the clone probe is work, not a stall"
    );
}
