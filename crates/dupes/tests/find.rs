//! Against a real filesystem, like every other test in this workspace.
//!
//! A mocked reader would prove the grouping logic and nothing about the three
//! things that actually go wrong here: a short read splitting a group, a
//! hardlink counted as recoverable space, and a file that changed between the
//! scan and the hash.

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use spacetrace_dupes::{find, CacheKey, Group, Hash, HashCache, NoCache, Options, Sharing};
use spacetrace_scan_core::{scan, ScanOptions, ScanProgress, Tree};

const MIB: usize = 1024 * 1024;

/// Big enough to clear the default minimum and to run past the prefix stage,
/// so the tests exercise all three stages rather than the short-circuit.
fn blob(seed: u8) -> Vec<u8> {
    let mut out = vec![seed; 3 * MIB];
    // A tail that differs, so two blobs with the same first bytes can be made
    // deliberately.
    out[3 * MIB - 1] = seed.wrapping_add(1);
    out
}

fn tree_of(dir: &Path) -> Tree {
    scan(
        dir,
        ScanOptions::default(),
        Arc::new(ScanProgress::default()),
    )
    .unwrap()
    .0
}

fn run(dir: &Path) -> spacetrace_dupes::Report {
    find(&tree_of(dir), &Options::default(), &NoCache)
}

fn names(group: &Group) -> Vec<String> {
    let mut out: Vec<String> = group
        .copies
        .iter()
        .map(|c| c.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

#[test]
fn identical_files_are_one_group_and_different_ones_are_not() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.bin"), blob(1)).unwrap();
    fs::write(dir.path().join("b.bin"), blob(1)).unwrap();
    fs::write(dir.path().join("other.bin"), blob(2)).unwrap();

    let report = run(dir.path());
    assert_eq!(report.groups.len(), 1, "{:?}", report.groups);
    assert_eq!(names(&report.groups[0]), vec!["a.bin", "b.bin"]);
    assert_eq!(report.groups[0].shared, Sharing::Separate);
    assert_eq!(report.reclaimable(), 3 * MIB as u64);
}

/// The case the prefix stage exists for, and the case it must not get wrong:
/// two files that share their first 16 KiB and differ later are *not*
/// duplicates, and a pipeline that stopped at the prefix would say they were.
#[test]
fn files_that_share_a_prefix_but_differ_later_are_not_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let mut first = blob(7);
    let mut second = blob(7);
    second[3 * MIB - 1] = 0xAB;
    first[3 * MIB - 1] = 0xCD;
    fs::write(dir.path().join("a.bin"), &first).unwrap();
    fs::write(dir.path().join("b.bin"), &second).unwrap();

    let report = run(dir.path());
    assert!(report.groups.is_empty(), "{:?}", report.groups);
    assert_eq!(report.hashed, 2, "both had to be read in full to know");
}

/// Two names for one inode already share their bytes. Reporting them as
/// reclaimable would promise space that deleting one cannot return.
#[test]
#[cfg(unix)]
fn hardlinked_names_are_grouped_but_reclaim_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("original.bin");
    fs::write(&original, blob(3)).unwrap();
    fs::hard_link(&original, dir.path().join("same.bin")).unwrap();

    let report = run(dir.path());
    assert_eq!(report.groups.len(), 1);
    assert_eq!(report.groups[0].shared, Sharing::Hardlinked);
    assert_eq!(report.groups[0].copies.len(), 2);
    assert_eq!(report.reclaimable(), 0, "deleting a link frees nothing");
    assert_eq!(report.bytes_read, 0, "and nothing had to be read to know");
}

/// A hardlinked pair must not be hashed twice and must not turn into a
/// three-way "duplicate" with a genuine copy that happens to match.
#[test]
#[cfg(unix)]
fn a_link_and_a_real_copy_are_told_apart() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("original.bin");
    fs::write(&original, blob(5)).unwrap();
    fs::hard_link(&original, dir.path().join("link.bin")).unwrap();
    fs::write(dir.path().join("copy.bin"), blob(5)).unwrap();

    let report = run(dir.path());
    let linked = report
        .groups
        .iter()
        .find(|g| g.shared == Sharing::Hardlinked)
        .expect("the link pair");
    let copied = report
        .groups
        .iter()
        .find(|g| g.shared == Sharing::Separate)
        .expect("the real copy");

    assert_eq!(names(linked), vec!["link.bin", "original.bin"]);
    // The real copy pairs with one representative of the inode, not with both
    // of its names — otherwise the group claims three copies of bytes that
    // exist twice.
    assert_eq!(copied.copies.len(), 2, "{:?}", names(copied));
    assert!(names(copied).contains(&"copy.bin".to_string()));
    assert_eq!(
        report.reclaimable(),
        3 * MIB as u64,
        "one copy's worth, not two"
    );
}

#[test]
fn files_under_the_minimum_are_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), b"identical").unwrap();
    fs::write(dir.path().join("b.txt"), b"identical").unwrap();

    assert!(run(dir.path()).groups.is_empty());

    let smaller = Options {
        min_size: 1,
        ..Options::default()
    };
    let report = find(&tree_of(dir.path()), &smaller, &NoCache);
    assert_eq!(report.groups.len(), 1, "and found when asked for");
}

/// Symlinks are not contents. Following one would count the target twice and
/// report the link as a copy of what it points at.
#[test]
#[cfg(unix)]
fn a_symlink_is_not_a_copy_of_its_target() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("real.bin");
    fs::write(&target, blob(9)).unwrap();
    std::os::unix::fs::symlink(&target, dir.path().join("pointer.bin")).unwrap();

    assert!(run(dir.path()).groups.is_empty());
}

/// The funnel has to actually narrow. Three files of one size where only two
/// match means the third is read once at the prefix and never in full.
#[test]
fn only_what_survives_a_stage_pays_for_the_next() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.bin"), blob(1)).unwrap();
    fs::write(dir.path().join("b.bin"), blob(1)).unwrap();
    // Same length, different from the first byte, so the prefix stage removes
    // it and it is never hashed whole.
    fs::write(dir.path().join("c.bin"), blob(200)).unwrap();
    // A different length entirely: stage one removes it and it is never opened.
    fs::write(dir.path().join("d.bin"), vec![1u8; 2 * MIB]).unwrap();

    let report = run(dir.path());
    assert_eq!(report.groups.len(), 1);
    assert_eq!(
        report.hashed, 2,
        "only the pair that survived the prefix was read whole"
    );
    assert!(
        report.bytes_read < 7 * MIB as u64,
        "read {} bytes for 11 MiB of files",
        report.bytes_read
    );
}

/// A cache that answers avoids the read entirely. The assertion is on bytes
/// read rather than on wall clock, because the point is the I/O and a timing
/// test would be a bet on the machine.
#[derive(Default)]
struct Remembering {
    entries: Mutex<Vec<(CacheKey, Hash)>>,
}

impl HashCache for Remembering {
    fn get(&self, key: &CacheKey) -> Option<Hash> {
        let entries = self.entries.lock().unwrap();
        entries.iter().find(|(k, _)| k == key).map(|(_, h)| *h)
    }
    fn put(&self, key: &CacheKey, hash: Hash) {
        self.entries.lock().unwrap().push((*key, hash));
    }
}

#[test]
fn a_second_run_reads_nothing_it_has_already_hashed() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.bin"), blob(1)).unwrap();
    fs::write(dir.path().join("b.bin"), blob(1)).unwrap();

    let cache = Remembering::default();
    let tree = tree_of(dir.path());

    let first = find(&tree, &Options::default(), &cache);
    assert_eq!(first.groups.len(), 1);
    assert!(first.bytes_read > 0);

    let second = find(&tree, &Options::default(), &cache);
    assert_eq!(second.groups, first.groups, "the same answer");
    assert_eq!(second.hashed, 0, "nothing was hashed again");
    assert!(
        second.bytes_read < first.bytes_read,
        "the whole-file reads were skipped: {} vs {}",
        second.bytes_read,
        first.bytes_read
    );
}

/// A file rewritten since the scan is a different file. Reporting it under its
/// old size would put it in a group it does not belong to — and this is the
/// group somebody deletes from.
#[test]
fn a_file_that_changed_since_the_scan_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.bin"), blob(1)).unwrap();
    fs::write(dir.path().join("b.bin"), blob(1)).unwrap();
    let tree = tree_of(dir.path());

    // Rewrite one of them at a different length, after the tree was built.
    fs::write(dir.path().join("b.bin"), vec![1u8; 5 * MIB]).unwrap();

    let report = find(&tree, &Options::default(), &NoCache);
    assert!(
        report.groups.is_empty(),
        "b.bin is no longer 3 MiB: {:?}",
        report.groups
    );
    assert!(
        report.unreadable.is_empty(),
        "nothing failed; the file simply changed"
    );
}

/// A file deleted between the scan and the run must not fail the whole thing.
#[test]
fn a_file_that_vanished_is_reported_and_not_fatal() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.bin"), blob(1)).unwrap();
    fs::write(dir.path().join("b.bin"), blob(1)).unwrap();
    fs::write(dir.path().join("c.bin"), blob(1)).unwrap();
    let tree = tree_of(dir.path());

    fs::remove_file(dir.path().join("c.bin")).unwrap();

    let report = find(&tree, &Options::default(), &NoCache);
    assert_eq!(report.groups.len(), 1, "the surviving pair is still found");
    assert_eq!(report.unreadable.len(), 1);
    assert!(report.unreadable[0].0.ends_with("c.bin"));
}

/// Two runs over one tree must produce the same list in the same order, or a
/// diff between two reports is unreadable.
#[test]
fn the_order_is_the_same_every_run() {
    let dir = tempfile::tempdir().unwrap();
    for pair in 0..6u8 {
        fs::write(dir.path().join(format!("x{pair}.bin")), blob(pair)).unwrap();
        fs::write(dir.path().join(format!("y{pair}.bin")), blob(pair)).unwrap();
    }
    let tree = tree_of(dir.path());

    let first = find(&tree, &Options::default(), &NoCache);
    assert_eq!(first.groups.len(), 6);
    for _ in 0..4 {
        assert_eq!(
            find(&tree, &Options::default(), &NoCache).groups,
            first.groups
        );
    }
}
