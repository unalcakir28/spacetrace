use std::fs;
use std::path::Path;
use std::sync::Arc;

use spacetrace_diff::{diff, ChangeKind, DiffOptions};
use spacetrace_scan_core::{scan, ScanOptions, ScanProgress, Tree};

const MB: usize = 1024 * 1024;

fn tree_of(path: &Path) -> Tree {
    scan(
        path,
        ScanOptions::default(),
        Arc::new(ScanProgress::default()),
    )
    .unwrap()
    .0
}

fn write(path: &Path, mb: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![0u8; mb * MB]).unwrap();
}

/// A layout where one deep folder is the only thing that changes.
fn build_base(root: &Path) {
    write(&root.join("logs/app.log"), 2);
    write(&root.join("cache/deep/blobs/data.bin"), 5);
    write(&root.join("docs/readme.md"), 1);
}

#[test]
fn an_unchanged_tree_reports_nothing() {
    let dir = tempfile::tempdir().unwrap();
    build_base(dir.path());
    let a = tree_of(dir.path());
    let b = tree_of(dir.path());

    let report = diff(&a, &b, &DiffOptions::default());
    assert!(report.is_unchanged());
    assert_eq!(report.delta(), 0);
}

#[test]
fn blame_lands_on_the_deepest_folder_that_actually_grew() {
    let old_dir = tempfile::tempdir().unwrap();
    build_base(old_dir.path());
    let old = tree_of(old_dir.path());

    let new_dir = tempfile::tempdir().unwrap();
    build_base(new_dir.path());
    write(&new_dir.path().join("cache/deep/blobs/data.bin"), 25);
    let new = tree_of(new_dir.path());

    let report = diff(&old, &new, &DiffOptions::default());
    assert_eq!(report.delta(), 20 * MB as i64);

    let top = &report.changes[0];
    assert_eq!(
        top.path, "cache/deep/blobs",
        "cache/ and cache/deep/ are pass-through, the change is in blobs/"
    );
    assert_eq!(top.kind, ChangeKind::Grown);
    assert_eq!(top.delta(), 20 * MB as i64);

    // The ancestors that merely passed the change through are not repeated.
    assert!(!report.changes.iter().any(|c| c.path == "cache"));
    assert!(!report.changes.iter().any(|c| c.path == "cache/deep"));
}

#[test]
fn a_directory_whose_change_is_spread_out_is_reported_at_that_level() {
    let old_dir = tempfile::tempdir().unwrap();
    write(&old_dir.path().join("shared/a/f.bin"), 1);
    write(&old_dir.path().join("shared/b/f.bin"), 1);
    let old = tree_of(old_dir.path());

    let new_dir = tempfile::tempdir().unwrap();
    write(&new_dir.path().join("shared/a/f.bin"), 11);
    write(&new_dir.path().join("shared/b/f.bin"), 11);
    let new = tree_of(new_dir.path());

    let report = diff(&old, &new, &DiffOptions::default());
    let top = &report.changes[0];
    assert_eq!(top.path, "shared", "neither child explains it alone");
    assert_eq!(top.delta(), 20 * MB as i64);
}

#[test]
fn new_and_deleted_folders_are_reported_once_as_a_whole() {
    let old_dir = tempfile::tempdir().unwrap();
    build_base(old_dir.path());
    let old = tree_of(old_dir.path());

    let new_dir = tempfile::tempdir().unwrap();
    build_base(new_dir.path());
    fs::remove_dir_all(new_dir.path().join("logs")).unwrap();
    write(&new_dir.path().join("uploads/2026/big.zip"), 30);
    let new = tree_of(new_dir.path());

    let report = diff(&old, &new, &DiffOptions::default());

    let added: Vec<&str> = report
        .grown()
        .filter(|c| c.kind == ChangeKind::Added)
        .map(|c| c.path.as_str())
        .collect();
    assert_eq!(added, vec!["uploads"], "reported at its top, not per level");

    let removed: Vec<&str> = report
        .shrunk()
        .filter(|c| c.kind == ChangeKind::Removed)
        .map(|c| c.path.as_str())
        .collect();
    assert_eq!(removed, vec!["logs"]);
    assert_eq!(report.delta(), 30 * MB as i64 - 2 * MB as i64);
}

#[test]
fn changes_below_the_threshold_are_ignored() {
    let old_dir = tempfile::tempdir().unwrap();
    build_base(old_dir.path());
    let old = tree_of(old_dir.path());

    let new_dir = tempfile::tempdir().unwrap();
    build_base(new_dir.path());
    fs::write(new_dir.path().join("docs/tiny.txt"), vec![0u8; 4096]).unwrap();
    let new = tree_of(new_dir.path());

    let report = diff(&old, &new, &DiffOptions::default());
    assert!(report.changes.is_empty(), "4 KB is below the 1 MB default");
    assert_ne!(report.delta(), 0, "the total still reflects it");

    let sensitive = DiffOptions {
        min_delta: 1024,
        ..Default::default()
    };
    let report = diff(&old, &new, &sensitive);
    assert_eq!(report.changes[0].path, "docs");
}

#[test]
fn files_are_included_on_request() {
    let old_dir = tempfile::tempdir().unwrap();
    build_base(old_dir.path());
    let old = tree_of(old_dir.path());

    let new_dir = tempfile::tempdir().unwrap();
    build_base(new_dir.path());
    write(&new_dir.path().join("logs/app.log"), 12);
    let new = tree_of(new_dir.path());

    let dirs_only = diff(&old, &new, &DiffOptions::default());
    assert!(dirs_only
        .changes
        .iter()
        .all(|c| !c.path.ends_with("app.log")));

    let with_files = DiffOptions {
        include_files: true,
        ..Default::default()
    };
    let report = diff(&old, &new, &with_files);
    assert!(
        report.changes.iter().any(|c| c.path == "logs/app.log"),
        "the file itself should be named: {:?}",
        report.changes.iter().map(|c| &c.path).collect::<Vec<_>>()
    );
}

#[test]
fn shrinking_is_tracked_with_a_negative_delta() {
    let old_dir = tempfile::tempdir().unwrap();
    build_base(old_dir.path());
    write(&old_dir.path().join("cache/deep/blobs/data.bin"), 40);
    let old = tree_of(old_dir.path());

    let new_dir = tempfile::tempdir().unwrap();
    build_base(new_dir.path());
    let new = tree_of(new_dir.path());

    let report = diff(&old, &new, &DiffOptions::default());
    assert!(report.delta() < 0);
    let top = &report.changes[0];
    assert_eq!(top.kind, ChangeKind::Shrunk);
    assert_eq!(top.path, "cache/deep/blobs");
    assert_eq!(top.delta(), -35 * MB as i64);
}

#[test]
fn max_depth_stops_the_blame_from_going_deeper() {
    let old_dir = tempfile::tempdir().unwrap();
    build_base(old_dir.path());
    let old = tree_of(old_dir.path());

    let new_dir = tempfile::tempdir().unwrap();
    build_base(new_dir.path());
    write(&new_dir.path().join("cache/deep/blobs/data.bin"), 25);
    let new = tree_of(new_dir.path());

    let opts = DiffOptions {
        max_depth: Some(1),
        ..Default::default()
    };
    let report = diff(&old, &new, &opts);
    assert_eq!(report.changes[0].path, "cache");
    assert_eq!(report.changes[0].depth, 1);
}
