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
