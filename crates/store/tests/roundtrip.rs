use std::fs;
use std::sync::Arc;

use spacetrace_scan_core::{scan, ScanOptions, ScanProgress, Tree};
use spacetrace_store::{export_ncdu, Store};

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), vec![b'a'; 1000]).unwrap();
    fs::create_dir(dir.path().join("sub")).unwrap();
    fs::write(dir.path().join("sub/b.bin"), vec![0u8; 4096]).unwrap();
    dir
}

fn scan_fixture(dir: &tempfile::TempDir) -> (Tree, spacetrace_scan_core::ScanStats) {
    scan(
        dir.path(),
        ScanOptions::default(),
        Arc::new(ScanProgress::default()),
    )
    .unwrap()
}

#[test]
fn a_saved_tree_loads_back_identically() {
    let dir = fixture();
    let (tree, stats) = scan_fixture(&dir);
    let mut store = Store::open_in_memory().unwrap();

    let id = store
        .save(&tree, &stats, "testhost", Some("nightly"))
        .unwrap();
    let (loaded, meta) = store.load(id).unwrap();

    assert_eq!(meta.host, "testhost");
    assert_eq!(meta.label.as_deref(), Some("nightly"));
    assert_eq!(meta.total_size, tree.total_size());
    assert_eq!(meta.files, stats.files);
    assert_eq!(loaded.len(), tree.len());
    assert_eq!(loaded.total_size(), tree.total_size());
    assert_eq!(loaded.total_alloc(), tree.total_alloc());
    assert_eq!(loaded.root_path(), tree.root_path());

    for id in tree.iter() {
        let (a, b) = (tree.node(id), loaded.node(id));
        assert_eq!(a.name, b.name, "node {id}");
        assert_eq!(a.size, b.size);
        assert_eq!(a.alloc, b.alloc);
        assert_eq!(a.kind, b.kind);
        assert_eq!(a.files, b.files);
        assert_eq!(a.children_len, b.children_len);
        assert_eq!(tree.rel_path(id), loaded.rel_path(id));
    }
}

#[test]
fn scans_are_listed_newest_first_and_can_be_looked_up() {
    let dir = fixture();
    let (tree, stats) = scan_fixture(&dir);
    let mut store = Store::open_in_memory().unwrap();

    let first = store.save(&tree, &stats, "h1", None).unwrap();
    let second = store.save(&tree, &stats, "h1", None).unwrap();

    let all = store.list().unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].id, second, "newest first");

    let root = tree.root_path().to_string_lossy().to_string();
    assert_eq!(
        store.latest_for(&root, Some("h1")).unwrap().unwrap().id,
        second
    );
    assert!(store.latest_for(&root, Some("nope")).unwrap().is_none());

    let pair = store.last_two_for(&root, None).unwrap();
    assert_eq!(pair.len(), 2);
    assert_eq!((pair[0].id, pair[1].id), (second, first));

    assert!(store.delete(first).unwrap());
    assert_eq!(store.list().unwrap().len(), 1);
    assert!(store.scan(first).unwrap().is_none());
}

#[test]
fn deleting_a_scan_removes_its_entries() {
    let dir = fixture();
    let (tree, stats) = scan_fixture(&dir);
    let path = dir.path().join("snapshots.sqlite");
    let mut store = Store::open(&path).unwrap();

    let id = store.save(&tree, &stats, "h", None).unwrap();
    store.delete(id).unwrap();

    // Reopening proves the cascade really hit the table, not just a cache.
    let store = Store::open(&path).unwrap();
    assert!(store.load(id).is_err());
}

#[test]
fn prune_keeps_the_newest_scans_per_target() {
    let dir = fixture();
    let (tree, stats) = scan_fixture(&dir);
    let mut store = Store::open_in_memory().unwrap();

    let mut ids = Vec::new();
    for _ in 0..4 {
        ids.push(store.save(&tree, &stats, "h1", None).unwrap());
    }
    let other = store.save(&tree, &stats, "h2", None).unwrap();

    let removed = store.prune(2).unwrap();
    assert_eq!(removed, 2, "two of h1's four scans");

    let kept: Vec<i64> = store.list().unwrap().iter().map(|s| s.id).collect();
    assert!(kept.contains(&ids[3]) && kept.contains(&ids[2]));
    assert!(kept.contains(&other), "the other host is untouched");
    assert_eq!(kept.len(), 3);
}

#[test]
fn ncdu_export_is_valid_json_with_the_expected_shape() {
    let dir = fixture();
    let (tree, _) = scan_fixture(&dir);

    let mut buf = Vec::new();
    export_ncdu(&tree, &mut buf).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&buf).expect("valid JSON");

    assert_eq!(v[0], 1, "major format version");
    assert_eq!(v[2]["progname"], "spacetrace");

    let root = &v[3];
    assert!(root.is_array(), "a directory is an array");
    assert_eq!(root[0]["name"], tree.root_path().to_string_lossy().as_ref());

    // a.txt and the sub/ directory
    let names: Vec<String> = root
        .as_array()
        .unwrap()
        .iter()
        .skip(1)
        .map(|child| {
            let info = if child.is_array() { &child[0] } else { child };
            info["name"].as_str().unwrap().to_string()
        })
        .collect();
    assert!(names.contains(&"a.txt".to_string()));
    assert!(names.contains(&"sub".to_string()));

    let a = root
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "a.txt")
        .unwrap();
    assert_eq!(a["asize"], 1000);
}
