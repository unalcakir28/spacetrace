//! A tree read out of a walk that is still running has to be a tree.
//!
//! The unit tests next to `PartialTree` drive the arena by hand, which settles
//! the aggregation but not the thing that actually goes wrong here: real
//! workers pushing directories in whatever order they finish, while a reader
//! copies the arena underneath them. What this guards is the contract a caller
//! draws its window from — every snapshot is a valid tree, its totals only ever
//! climb, and an id keeps naming the same entry all the way into the finished
//! scan. Ids that moved would be the dangerous failure: nothing would crash,
//! the window would simply attribute one folder's bytes to another.

use std::collections::HashMap;
use std::fs;
use std::sync::Arc;

use spacetrace_scan_core::{scan, NodeId, ScanOptions, ScanProgress, Tree};

/// Wide and deep enough that the walk cannot finish between two reads.
///
/// Small files on purpose: this test is about the shape of the arena while it
/// fills, and bytes on disk would only make it slow.
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for branch in 0..12 {
        let mut path = dir.path().join(format!("branch{branch}"));
        fs::create_dir(&path).unwrap();
        for depth in 0..4 {
            path = path.join(format!("level{depth}"));
            fs::create_dir(&path).unwrap();
            for file in 0..24 {
                fs::write(path.join(format!("f{file}.bin")), vec![0u8; 64 + file]).unwrap();
            }
        }
    }
    dir
}

/// Name and totals of every entry, keyed by id.
fn by_id(tree: &Tree) -> HashMap<NodeId, (String, u64, u32)> {
    (0..tree.len() as NodeId)
        .map(|id| {
            let node = tree.node(id);
            (id, (tree.name(id).to_string(), node.alloc, node.files))
        })
        .collect()
}

#[test]
fn snapshots_taken_during_a_walk_agree_with_the_scan_they_become() {
    let dir = fixture();
    let progress = Arc::new(ScanProgress::default());

    let scanning = {
        let progress = Arc::clone(&progress);
        let root = dir.path().to_path_buf();
        std::thread::spawn(move || scan(root, ScanOptions::default(), progress).unwrap())
    };

    // Spun rather than slept: the window this is trying to land in is the walk
    // itself, and a sleep long enough to be reliable would be longer than the
    // walk on the machine where it matters.
    let mut seen: Vec<HashMap<NodeId, (String, u64, u32)>> = Vec::new();
    while !scanning.is_finished() {
        if let Some(partial) = progress.partial.snapshot() {
            seen.push(by_id(&partial));
        }
    }
    let (tree, _stats) = scanning.join().unwrap();
    let finished = by_id(&tree);

    assert!(
        !seen.is_empty(),
        "no snapshot landed inside the walk, so this test proved nothing"
    );

    // Every snapshot against the finished scan: an id means one entry, and no
    // partial total ever claims more than the real one.
    for (index, partial) in seen.iter().enumerate() {
        for (id, (name, alloc, files)) in partial {
            let (real_name, real_alloc, real_files) = finished
                .get(id)
                .unwrap_or_else(|| panic!("snapshot {index}: id {id} is not in the finished tree"));
            assert_eq!(
                name, real_name,
                "snapshot {index}: id {id} named {name} then and {real_name} now"
            );
            assert!(
                alloc <= real_alloc,
                "snapshot {index}: {name} claimed {alloc} bytes, the scan found {real_alloc}"
            );
            assert!(
                files <= real_files,
                "snapshot {index}: {name} claimed {files} files, the scan found {real_files}"
            );
        }
    }

    // And against each other: a tree that is being read cannot shrink, so a
    // figure that goes down is a reader seeing a half-written arena.
    for pair in seen.windows(2) {
        let (before, after) = (&pair[0], &pair[1]);
        assert!(
            after.len() >= before.len(),
            "the arena lost entries between two reads: {} then {}",
            before.len(),
            after.len()
        );
        for (id, (name, alloc, _)) in before {
            let Some((_, later, _)) = after.get(id) else {
                panic!("{name} was in one snapshot and gone from the next");
            };
            assert!(
                later >= alloc,
                "{name} went from {alloc} bytes down to {later}"
            );
        }
    }

    // The last one is a whole tree rather than a prefix of one: its root is
    // the sum of what it holds, the same claim a finished scan makes.
    let last = progress.partial.snapshot();
    assert!(
        last.is_none(),
        "the walk has handed its arena over; there is nothing left to read"
    );
}

/// The root of any snapshot must account for exactly what that snapshot holds.
/// A prefix of an arena read the wrong way would still look plausible per
/// entry and only fail this.
#[test]
fn a_snapshots_root_is_the_sum_of_its_own_entries() {
    let dir = fixture();
    let progress = Arc::new(ScanProgress::default());

    let scanning = {
        let progress = Arc::clone(&progress);
        let root = dir.path().to_path_buf();
        std::thread::spawn(move || scan(root, ScanOptions::default(), progress).unwrap())
    };

    let mut checked = 0;
    while !scanning.is_finished() {
        let Some(partial) = progress.partial.snapshot() else {
            continue;
        };
        let leaves: u64 = (0..partial.len() as NodeId)
            .map(|id| partial.node(id).own_alloc)
            .sum();
        assert_eq!(
            partial.node(partial.root()).alloc,
            leaves,
            "the root's total is not what the snapshot's own entries add up to"
        );
        checked += 1;
    }
    scanning.join().unwrap();
    assert!(checked > 0, "no snapshot landed inside the walk");
}
