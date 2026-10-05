//! An incremental rescan must produce the snapshot a full scan of the same
//! filesystem would — byte for byte, as far as `content_hash` can tell.
//!
//! That is the only assertion strong enough to be worth having here (TODO.md
//! B7): a rebuilt tree that is merely plausible would report wrong numbers
//! with full confidence. Two things in a snapshot legitimately differ between
//! two scans of one unchanged disk, and both are taken out before comparing:
//!
//! * **the layout** — node ids follow the order directories finished in
//!   (invariant 2), so each tree is rebuilt in one canonical order, children
//!   sorted by name, before it is hashed;
//! * **when it ran** — `started_at`, `duration_ms` and the free space beside
//!   them are measurements of the moment, and are fixed before hashing.
//!
//! Everything else — every entry's name, kind, size, blocks, mtime, link
//! count and subtree totals, and the scan's file, directory, error and
//! hardlink counts — goes into the digest exactly as `Store::save` writes it.
//! A per-path comparison beside it names the entry when the digests differ,
//! and checks the flags the next rescan will rely on, which the digest does
//! not cover.
//!
//! macOS only: FSEvents on APFS is the one journal read so far.

#![cfg(target_os = "macos")]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

// The scanner's tests make mount points the same way; one copy of it.
#[path = "../../scan-core/tests/support/disk_image.rs"]
mod disk_image;
use disk_image::DiskImage;

use rusqlite::Connection;
use spacetrace_scan_core::{
    rescan, scan, Fallback, ImportedNode, Rescan, RescanKind, ScanOptions, ScanProgress, ScanStats,
    Tree,
};
use spacetrace_store::{Integrity, ScanId, Store};

const HOST: &str = "here";

/// A root under test, the snapshot database beside it, and the scans so far.
struct Bench {
    _dir: tempfile::TempDir,
    /// Outside the root, on the same volume: somewhere to move things in from
    /// and out to.
    outside: PathBuf,
    root: PathBuf,
    db: PathBuf,
    opts: ScanOptions,
}

impl Bench {
    /// A root built by `build`, scanned once in full and saved as the base.
    fn new(build: impl FnOnce(&Path)) -> Bench {
        Bench::with_options(ScanOptions::default(), build)
    }

    fn with_options(opts: ScanOptions, build: impl FnOnce(&Path)) -> Bench {
        Bench::in_dir(tempfile::tempdir().unwrap(), opts, build)
    }

    /// [`Bench::with_options`], in `dir` — on a volume of the test's choosing.
    fn in_dir(dir: tempfile::TempDir, opts: ScanOptions, build: impl FnOnce(&Path)) -> Bench {
        let top = dir.path().canonicalize().unwrap();
        let root = top.join("root");
        let outside = top.join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        build(&root);
        let bench = Bench {
            db: top.join("snapshots.sqlite"),
            _dir: dir,
            outside,
            root,
            opts,
        };
        let (tree, stats) = bench.full();
        assert_eq!(stats.rescan, Rescan::Full);
        bench.save(&tree, &stats);
        bench
    }

    fn store(&self) -> Store {
        Store::open(&self.db).unwrap()
    }

    fn full(&self) -> (Tree, ScanStats) {
        scan(
            &self.root,
            self.opts.clone(),
            Arc::new(ScanProgress::default()),
        )
        .unwrap()
    }

    fn save(&self, tree: &Tree, stats: &ScanStats) -> ScanId {
        self.store().save(tree, stats, HOST, None).unwrap()
    }

    /// Rescan from the newest snapshot, save the result, and hand it back.
    fn rescan(&self) -> (Tree, ScanStats, ScanId) {
        self.rescan_with(self.opts.clone())
    }

    /// A replay that outlasts its budget says how busy this machine is, not
    /// whether the rescan is right, and has its own test (`rescan::tests` in
    /// scan-core). Seen at a load average of 21 with the 500 ms floor, where
    /// FSEvents answers in steps of about 130 ms. Such a scan is asked again
    /// rather than saved, so each test's chain of snapshots stays the one it
    /// describes.
    fn rescan_with(&self, opts: ScanOptions) -> (Tree, ScanStats, ScanId) {
        for _ in 0..5 {
            let store = self.store();
            let base = store
                .rescan_base(&self.root.to_string_lossy(), HOST)
                .unwrap();
            let (tree, stats) = rescan(
                &self.root,
                opts.clone(),
                Arc::new(ScanProgress::default()),
                base,
            )
            .unwrap();
            drop(store);
            if stats.rescan == Rescan::Fallback(Fallback::Deadline) {
                continue;
            }
            let id = self.save(&tree, &stats);
            return (tree, stats, id);
        }
        panic!("five replays in a row outlasted their budget");
    }

    /// Rescan, and require both that it was incremental and that it is the
    /// snapshot a full scan of the same state gives.
    fn rescan_matches_full(&self) -> spacetrace_scan_core::Incremental {
        let (tree, stats, id) = self.rescan();
        let Rescan::Incremental(report) = stats.rescan.clone() else {
            panic!("expected an incremental rescan, got {:?}", stats.rescan);
        };
        assert_eq!(
            self.store().rescan_of(id).unwrap(),
            Some(RescanKind::Incremental),
            "the snapshot says how it was made"
        );
        let (full_tree, full_stats) = self.full();
        assert_same(&tree, &stats, &full_tree, &full_stats);
        report
    }

    fn newest(&self) -> ScanId {
        self.store()
            .latest_for(&self.root.to_string_lossy(), Some(HOST))
            .unwrap()
            .unwrap()
            .id
    }
}

/// Every value a snapshot keeps for one entry, plus its flags.
type Row = (u8, u64, u64, u64, u64, i64, u32, u32, u32, u8);

fn rows(tree: &Tree) -> BTreeMap<String, Row> {
    let mut out = BTreeMap::new();
    tree.for_each_path(None, |id, path| {
        let n = tree.node(id);
        out.insert(
            path.to_string(),
            (
                n.kind as u8,
                n.size,
                n.alloc,
                n.own_size,
                n.own_alloc,
                n.mtime,
                n.nlink,
                n.files,
                n.dirs,
                n.flags(),
            ),
        );
    });
    out
}

/// The headline claim, and the diagnostic beside it.
fn assert_same(tree: &Tree, stats: &ScanStats, full: &Tree, full_stats: &ScanStats) {
    let (got, want) = (rows(tree), rows(full));
    if got != want {
        let mut diffs = Vec::new();
        for path in got.keys().chain(want.keys()) {
            if got.get(path) != want.get(path) && !diffs.contains(path) {
                diffs.push(path.clone());
            }
        }
        let shown: Vec<String> = diffs
            .iter()
            .take(12)
            .map(|p| format!("  {p:?}: rescan {:?} full {:?}", got.get(p), want.get(p)))
            .collect();
        panic!(
            "{} entries differ from a full scan:\n{}",
            diffs.len(),
            shown.join("\n")
        );
    }
    assert_eq!(
        (
            stats.files,
            stats.dirs,
            stats.errors,
            stats.hardlinks_deduped
        ),
        (
            full_stats.files,
            full_stats.dirs,
            full_stats.errors,
            full_stats.hardlinks_deduped
        ),
        "the scan's own counts"
    );
    assert_eq!(
        (stats.clones_deduped, stats.shared_bytes_deduped),
        (full_stats.clones_deduped, full_stats.shared_bytes_deduped)
    );
    assert_eq!(
        canonical_hash(tree, stats),
        canonical_hash(full, full_stats),
        "the content hash of the rebuilt tree is not the full scan's"
    );
}

/// `content_hash` of `tree`, laid out in one canonical order and stored with
/// the moment of the scan fixed.
fn canonical_hash(tree: &Tree, stats: &ScanStats) -> String {
    let canonical = Tree::from_nested(tree.root_path().to_path_buf(), nested(tree, tree.root()));
    let work = tempfile::tempdir().unwrap();
    let db = work.path().join("canonical.sqlite");
    let fixed = ScanStats {
        duration_ms: 0,
        capacity: None,
        journal: None,
        rescan: Rescan::Full,
        ..stats.clone()
    };
    let id = Store::open(&db)
        .unwrap()
        .save(&canonical, &fixed, HOST, None)
        .unwrap();
    // `started_at` is the wall clock at save time; two saves a second apart
    // would differ there and nowhere else.
    Connection::open(&db)
        .unwrap()
        .execute("UPDATE scans SET started_at = 0 WHERE id = ?1", [id])
        .unwrap();
    match Store::open(&db).unwrap().verify(id).unwrap() {
        Integrity::Mismatch { computed, .. } => computed,
        other => panic!("the stored digest was made stale on purpose, got {other:?}"),
    }
}

fn nested(tree: &Tree, id: u32) -> ImportedNode {
    let n = tree.node(id);
    let mut children: Vec<ImportedNode> = tree.children(id).map(|c| nested(tree, c)).collect();
    children.sort_by(|a, b| a.name.cmp(&b.name));
    ImportedNode {
        name: tree.name(id).to_string(),
        kind: n.kind,
        size: n.own_size,
        alloc: n.own_alloc,
        mtime: n.mtime,
        nlink: n.nlink,
        children,
    }
}

/// A tree wide and deep enough that most of it is left alone by any one
/// change, so a rescan that copies nothing would show up as such.
fn forest(root: &Path) {
    for top in ["alpha", "beta", "gamma", "delta"] {
        for mid in 0..5 {
            let dir = root.join(top).join(format!("m{mid}")).join("leaf");
            fs::create_dir_all(&dir).unwrap();
            for f in 0..4 {
                fs::write(dir.join(format!("f{f}.bin")), vec![f as u8; 1000 * (f + 1)]).unwrap();
            }
            fs::write(
                root.join(top).join(format!("m{mid}")).join("note.txt"),
                b"x",
            )
            .unwrap();
        }
    }
    fs::write(root.join("top.txt"), b"top").unwrap();
}

fn cp_clone(from: &Path, to: &Path) {
    let ok = Command::new("cp")
        .arg("-c")
        .arg(from)
        .arg(to)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok, "cp -c {} {}", from.display(), to.display());
}

// --------------------------------------------------------------- mutations

#[test]
fn a_file_grown_in_place() {
    let bench = Bench::new(forest);
    fs::write(bench.root.join("beta/m3/leaf/f1.bin"), vec![7u8; 250_000]).unwrap();
    let report = bench.rescan_matches_full();
    assert!(
        report.entries_reused > 0,
        "nothing was copied, so nothing was tested: {report:?}"
    );
    assert!(report.events >= 1, "{report:?}");
}

#[test]
fn a_file_added_and_one_removed() {
    let bench = Bench::new(forest);
    fs::write(bench.root.join("gamma/m1/leaf/new.bin"), vec![1u8; 33_000]).unwrap();
    fs::remove_file(bench.root.join("alpha/m4/leaf/f0.bin")).unwrap();
    let report = bench.rescan_matches_full();
    assert!(report.entries_reused > 0, "{report:?}");
}

/// A directory moved in arrives with everything in it and one event for
/// itself; one moved out leaves with no event below it either.
#[test]
fn a_directory_renamed_into_and_out_of_the_root() {
    let bench = Bench::new(forest);
    let incoming = bench.outside.join("incoming/deep/deeper");
    fs::create_dir_all(&incoming).unwrap();
    fs::write(incoming.join("cargo.bin"), vec![3u8; 70_000]).unwrap();
    fs::rename(
        bench.outside.join("incoming"),
        bench.root.join("delta/m2/incoming"),
    )
    .unwrap();
    fs::rename(bench.root.join("alpha/m1"), bench.outside.join("left")).unwrap();
    bench.rescan_matches_full();

    // And back again, replacing a directory the base held with another one
    // of the same name.
    fs::rename(
        bench.root.join("delta/m2/incoming"),
        bench.outside.join("incoming2"),
    )
    .unwrap();
    fs::rename(bench.outside.join("left"), bench.root.join("alpha/m1")).unwrap();
    fs::rename(bench.root.join("beta/m0"), bench.outside.join("beta-m0")).unwrap();
    fs::rename(bench.outside.join("incoming2"), bench.root.join("beta/m0")).unwrap();
    bench.rescan_matches_full();
}

/// Two directories swapped by renames alone: every path below them exists
/// before and after, no file inside is touched, and the journal has one
/// event per directory. Only reading each of them in full gets this right;
/// relisting just the two would match `m0/leaf` with the base's `m0/leaf`
/// by name and copy the wrong one.
#[test]
fn two_directories_swapped_by_renames_alone() {
    let bench = Bench::new(|root| {
        forest(root);
        fs::write(root.join("gamma/m0/leaf/f0.bin"), vec![8u8; 77_777]).unwrap();
    });
    fs::rename(bench.root.join("gamma/m0"), bench.outside.join("swap")).unwrap();
    fs::rename(bench.root.join("gamma/m1"), bench.root.join("gamma/m0")).unwrap();
    fs::rename(bench.outside.join("swap"), bench.root.join("gamma/m1")).unwrap();
    let report = bench.rescan_matches_full();
    assert!(report.entries_reused > 0, "{report:?}");
}

/// A new hardlink to a file in a subtree the base could copy. The file's
/// own directory changes nothing — but its link count does, and a copied
/// subtree would carry it at its old charge while the new name claims it
/// again: charged twice. FSEvents reports the source's directory (measured),
/// which is what makes it be read again.
///
/// The new name sits above the old one so that which name is charged comes
/// out the same in every scan (the parent is listed first), which is what
/// lets the comparison be exact; invariant 3 promises only "once".
#[test]
fn a_hardlink_made_across_the_boundary() {
    let bench = Bench::new(forest);
    fs::hard_link(
        bench.root.join("gamma/m2/leaf/f3.bin"),
        bench.root.join("gamma/linked.bin"),
    )
    .unwrap();
    let report = bench.rescan_matches_full();
    assert!(report.entries_reused > 0, "{report:?}");
    let (tree, stats) = bench.full();
    assert_eq!(stats.hardlinks_deduped, 1, "the fixture is what it claims");
    assert_ne!(tree.node(tree.find("gamma/m2").unwrap()).flags(), 0);
}

/// A hardlink pair already in the base, one name at the top and one deep in
/// a branch, with a change in a third place: both names must be read again
/// for the pair to be charged once.
#[test]
fn a_hardlink_pair_the_base_already_held() {
    let bench = Bench::new(|root| {
        forest(root);
        fs::hard_link(root.join("delta/m4/leaf/f2.bin"), root.join("pair.bin")).unwrap();
    });
    fs::write(bench.root.join("alpha/m0/leaf/f0.bin"), vec![9u8; 12_345]).unwrap();
    bench.rescan_matches_full();
    // And one name removed: the other is charged in full again.
    fs::remove_file(bench.root.join("pair.bin")).unwrap();
    bench.rescan_matches_full();
}

/// An APFS clone has its own inode and a link count of one: a copy-on-write
/// sibling hardlink deduplication cannot see. `cp -c` from a copyable subtree
/// into another one; FSEvents reports the source as `ItemCloned` (measured).
#[test]
fn an_apfs_clone_made_across_the_boundary() {
    let bench = Bench::new(|root| {
        forest(root);
        fs::write(root.join("alpha/m2/leaf/big.bin"), vec![5u8; 300_000]).unwrap();
    });
    cp_clone(
        &bench.root.join("alpha/m2/leaf/big.bin"),
        &bench.root.join("delta/m0/leaf/copy.bin"),
    );
    let report = bench.rescan_matches_full();
    assert!(report.entries_reused > 0, "{report:?}");
    let (_, stats) = bench.full();
    assert_eq!(stats.clones_deduped, 1, "the fixture is what it claims");

    // And with the family already in the base, a change elsewhere.
    fs::write(bench.root.join("beta/m1/note.txt"), b"changed").unwrap();
    bench.rescan_matches_full();
}

/// The two sharing cases above, end to end on a fresh APFS volume of the
/// test's own, rather than on whatever the Data volume's history holds.
///
/// Whether either is right rests on FSEvents reporting the *source* of the
/// new name, which nothing documents. Measured, on the Data volume and on
/// such an image alike: `ln a/f b/f2` reports `b/f2` (created, hardlink)
/// and the directory `a` (created, inode metadata, xattr), so `a` is read
/// in full and `f`'s new link count is seen; `cp -c c/g b/g2` reports
/// `b/g2` and `c/g` itself as `ItemCloned`, so `c` is read and `g` joins
/// the family. Without either report the copied side would keep its old
/// charge and the new name claim the bytes again — counted twice. This test
/// is what would notice FSEvents stop saying so.
#[test]
fn sharing_made_across_the_boundary_on_a_fresh_apfs_volume() {
    let scratch = tempfile::tempdir().unwrap();
    let mountpoint = scratch.path().join("mnt");
    fs::create_dir_all(&mountpoint).unwrap();
    let Some(_image) = DiskImage::attach_apfs(scratch.path(), &mountpoint) else {
        eprintln!("skipped: no APFS image could be attached here");
        return;
    };
    // fseventsd starts the volume's history with its first change; until
    // then there is no journal identity and so no cursor to start from.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        fs::write(mountpoint.join("wake"), b"x").unwrap();
        let (_, stats) = scan(
            &mountpoint,
            ScanOptions::default(),
            Arc::new(ScanProgress::default()),
        )
        .unwrap();
        if stats.journal.is_some() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the image never got an FSEvents history"
        );
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    let bench = Bench::in_dir(
        tempfile::tempdir_in(&mountpoint).unwrap(),
        ScanOptions::default(),
        |root| {
            forest(root);
            fs::write(root.join("alpha/m2/leaf/big.bin"), vec![5u8; 300_000]).unwrap();
            // FSEvents numbers records as fseventsd takes them, which can
            // be after the base's cursor is read: the fixture's own records
            // would then mark everything changed and hide what is tested.
            std::thread::sleep(std::time::Duration::from_secs(3));
        },
    );
    fs::hard_link(
        bench.root.join("gamma/m2/leaf/f3.bin"),
        bench.root.join("gamma/linked.bin"),
    )
    .unwrap();
    cp_clone(
        &bench.root.join("alpha/m2/leaf/big.bin"),
        &bench.root.join("delta/m0/leaf/copy.bin"),
    );
    let report = bench.rescan_matches_full();
    assert!(report.entries_reused > 0, "{report:?}");
    let (_, stats) = bench.full();
    assert_eq!(stats.hardlinks_deduped, 1, "the fixture is what it claims");
    assert_eq!(stats.clones_deduped, 1, "the fixture is what it claims");
}

/// Changes spread over many directories at several depths at once.
#[test]
fn deep_and_wide_changes() {
    let bench = Bench::new(forest);
    for (i, top) in ["alpha", "beta", "gamma", "delta"].iter().enumerate() {
        for mid in 0..5 {
            if (mid + i) % 2 == 0 {
                fs::write(
                    bench.root.join(top).join(format!("m{mid}/leaf/f2.bin")),
                    vec![1u8; 100 + mid * 77],
                )
                .unwrap();
            }
        }
    }
    let deep = bench.root.join("beta/m4/leaf/a/b/c/d/e/f/g");
    fs::create_dir_all(&deep).unwrap();
    fs::write(deep.join("bottom.bin"), vec![2u8; 4321]).unwrap();
    fs::remove_dir_all(bench.root.join("gamma/m3")).unwrap();
    bench.rescan_matches_full();
}

/// A directory replaced by a file of the same name, and a file by a
/// directory.
#[test]
fn an_entry_that_changed_kind() {
    let bench = Bench::new(forest);
    fs::remove_dir_all(bench.root.join("beta/m2/leaf")).unwrap();
    fs::write(bench.root.join("beta/m2/leaf"), b"now a file").unwrap();
    fs::remove_file(bench.root.join("delta/m1/note.txt")).unwrap();
    fs::create_dir_all(bench.root.join("delta/m1/note.txt/inside")).unwrap();
    fs::write(bench.root.join("delta/m1/note.txt/inside/x"), b"x").unwrap();
    bench.rescan_matches_full();
}

/// On a case-insensitive volume a rename that changes only case keeps the
/// entry and changes its name.
#[test]
fn a_rename_that_changes_only_case() {
    let bench = Bench::new(forest);
    fs::rename(bench.root.join("gamma/m4"), bench.root.join("gamma/M4")).unwrap();
    fs::rename(
        bench.root.join("alpha/m3/leaf/f1.bin"),
        bench.root.join("alpha/m3/leaf/F1.BIN"),
    )
    .unwrap();
    bench.rescan_matches_full();
}

/// APFS keeps a name in the form it was created in and finds it from either
/// Unicode normal form. A change made through the other spelling could be
/// reported in that spelling, which no listing shows, and this code has no
/// table to fold the two together.
///
/// Measured on macOS 27: FSEvents reports the stored spelling, so this passes
/// even with the escalation for an unplaceable name switched off. It stays as
/// the end-to-end check; the escalation itself is proven by
/// `a_change_under_a_name_the_listing_does_not_show_copies_nothing` in
/// `scan-core/src/rescan.rs`.
#[test]
fn a_change_made_through_the_other_unicode_spelling() {
    let composed = "caf\u{e9}";
    let decomposed = "cafe\u{301}";
    let bench = Bench::new(|root| {
        forest(root);
        fs::create_dir_all(root.join("beta").join(composed).join("inner")).unwrap();
        fs::write(
            root.join("beta").join(composed).join("inner/menu.txt"),
            b"soup",
        )
        .unwrap();
    });
    fs::write(
        bench
            .root
            .join("beta")
            .join(decomposed)
            .join("inner/menu.txt"),
        vec![b'x'; 40_000],
    )
    .unwrap();
    bench.rescan_matches_full();
}

/// A directory that cannot be read is counted as an error every scan, so a
/// rescan reads it again rather than copying an empty one silently.
#[test]
fn a_directory_that_stops_and_starts_being_readable() {
    use std::os::unix::fs::PermissionsExt;
    let bench = Bench::new(forest);
    let locked = bench.root.join("delta/m3/leaf");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    bench.rescan_matches_full();
    // A second rescan from a base that holds the error.
    fs::write(bench.root.join("top.txt"), b"touched").unwrap();
    bench.rescan_matches_full();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    bench.rescan_matches_full();
}

/// Rescans in a row, each built on the last, each checked.
#[test]
fn a_chain_of_rescans_stays_exact() {
    let bench = Bench::new(forest);
    for round in 0..5u8 {
        fs::write(
            bench
                .root
                .join(format!("alpha/m{round}/leaf/f{}.bin", round % 4)),
            vec![round; 5000 + round as usize * 1000],
        )
        .unwrap();
        fs::create_dir_all(bench.root.join(format!("beta/new{round}"))).unwrap();
        bench.rescan_matches_full();
    }
}

/// Changes made while a rescan is walking may or may not be in what it
/// produces — a scan is not a snapshot of one instant — but they must be in
/// the next one. The cursor taken before the walk is what promises that.
#[test]
fn changes_during_the_walk_are_caught_by_the_next_rescan() {
    let bench = Bench::new(|root| {
        forest(root);
        for i in 0..40 {
            let dir = root.join(format!("wide/d{i}"));
            fs::create_dir_all(&dir).unwrap();
            for f in 0..20 {
                fs::write(dir.join(format!("{f}")), b"y").unwrap();
            }
        }
    });
    // Something to read again, so the walk takes long enough to overlap.
    fs::remove_dir_all(bench.root.join("wide")).unwrap();
    let root = bench.root.clone();
    let writer = std::thread::spawn(move || {
        for i in 0..200 {
            let dir = root.join(format!("gamma/m{}/during", i % 5));
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(format!("{i}")), vec![1u8; i]).unwrap();
        }
    });
    let (_, during, _) = bench.rescan();
    writer.join().unwrap();
    assert!(during.rescan.is_incremental(), "{:?}", during.rescan);
    bench.rescan_matches_full();
}

/// A volume mounted inside the root between two scans, then unmounted. Its
/// contents are not in this volume's journal, so it is read in full every
/// time, and the directory it covered is read again when it goes.
#[test]
fn a_volume_mounted_and_unmounted_under_the_root() {
    let bench = Bench::new(|root| {
        forest(root);
        fs::create_dir_all(root.join("beta/m1/mnt")).unwrap();
        fs::write(root.join("beta/m1/mnt/underneath.bin"), vec![4u8; 20_000]).unwrap();
    });
    let image = DiskImage::attach(&bench.outside, &bench.root.join("beta/m1/mnt"));
    let Some(image) = image else {
        eprintln!("skipped: no disk image could be attached here");
        return;
    };
    fs::write(
        bench.root.join("beta/m1/mnt/on-the-image.bin"),
        vec![6u8; 9000],
    )
    .unwrap();
    let (tree, stats, _) = bench.rescan();
    let (full, full_stats) = bench.full();
    assert_same(&tree, &stats, &full, &full_stats);
    eprintln!("mounted: {:?}", stats.rescan);

    // A rescan from a base with the mount in it, a change elsewhere, and
    // one on the image.
    fs::write(bench.root.join("alpha/m0/note.txt"), b"elsewhere").unwrap();
    fs::write(
        bench.root.join("beta/m1/mnt/on-the-image.bin"),
        vec![6u8; 19_000],
    )
    .unwrap();
    let (tree, stats, _) = bench.rescan();
    let (full, full_stats) = bench.full();
    assert_same(&tree, &stats, &full, &full_stats);
    eprintln!("with the mount in the base: {:?}", stats.rescan);

    drop(image);
    let (tree, stats, _) = bench.rescan();
    let (full, full_stats) = bench.full();
    assert_same(&tree, &stats, &full, &full_stats);
    assert!(
        full.find("beta/m1/mnt/underneath.bin").is_some(),
        "the covered directory is back"
    );
    eprintln!("unmounted: {:?}", stats.rescan);
}

// --------------------------------------------------------------- fallbacks

/// Rescan and require a full scan for `reason`, still equal to a full scan.
fn falls_back(bench: &Bench, reason: Fallback) {
    let (tree, stats, id) = bench.rescan();
    assert_eq!(stats.rescan, Rescan::Fallback(reason));
    assert_eq!(
        bench.store().rescan_of(id).unwrap(),
        Some(RescanKind::Fallback(reason)),
        "a fallback is silent, but recorded"
    );
    let (full, full_stats) = bench.full();
    assert_same(&tree, &stats, &full, &full_stats);
}

#[test]
fn other_scan_options_than_the_base_s() {
    let bench = Bench::new(forest);
    let (_, stats, _) = bench.rescan_with(ScanOptions {
        exclude_names: vec!["leaf".into()],
        ..ScanOptions::default()
    });
    assert_eq!(stats.rescan, Rescan::Fallback(Fallback::OptionsChanged));
}

/// The root directory replaced by another one at the same path: no event
/// below it need say so, and its inode does.
#[test]
fn a_root_replaced_by_another_directory() {
    let bench = Bench::new(forest);
    fs::rename(&bench.root, bench.outside.join("old-root")).unwrap();
    fs::create_dir_all(bench.root.join("alpha/m0/leaf")).unwrap();
    fs::write(bench.root.join("alpha/m0/leaf/f0.bin"), b"replaced").unwrap();
    falls_back(&bench, Fallback::RootReplaced);
}

/// A base whose digest no longer matches is not built on: the rescan would
/// carry the damage into a snapshot with a fresh, valid digest.
#[test]
fn a_damaged_base_is_not_built_on() {
    let bench = Bench::new(forest);
    let id = bench.newest();
    let changed = Connection::open(&bench.db)
        .unwrap()
        .execute(
            "UPDATE entries SET size = size + 1 WHERE scan_id = ?1 AND kind = 1 AND id = \
             (SELECT max(id) FROM entries WHERE scan_id = ?1 AND kind = 1)",
            [id],
        )
        .unwrap();
    assert_eq!(changed, 1);
    falls_back(&bench, Fallback::BaseDamaged);
}

/// A base dated ahead of the clock: "since then" no longer means anything.
#[test]
fn a_base_dated_in_the_future() {
    let bench = Bench::new(forest);
    Connection::open(&bench.db)
        .unwrap()
        .execute(
            "UPDATE scans SET started_at = started_at + 10 * 86400 WHERE id = ?1",
            [bench.newest()],
        )
        .unwrap();
    falls_back(&bench, Fallback::FutureBase);
}

/// The newest snapshot of the root came from another tool's export.
#[test]
fn an_imported_base_is_never_built_on() {
    let bench = Bench::new(forest);
    let imported = Tree::from_nested(
        bench.root.clone(),
        ImportedNode {
            children: vec![ImportedNode::file("only.bin", 1, 4096)],
            ..ImportedNode::dir("root")
        },
    );
    bench
        .store()
        .save_import(&imported, &ScanStats::default(), HOST, None)
        .unwrap();
    // Ordered by start time; an import saved this second ties with the base,
    // and the larger id wins the tie.
    falls_back(&bench, Fallback::ImportedBase);
}

/// The newest snapshot of the root arrived by push: the import leaves its
/// cursor behind, so there is nothing to replay from.
#[test]
fn a_pushed_snapshot_has_no_cursor_to_build_on() {
    let bench = Bench::new(forest);
    let wire = bench.outside.join("wire.sqlite");
    let local = bench.newest();
    bench.store().export_snapshot(local, &wire).unwrap();
    // The same scan coming back would be skipped as a duplicate, so the
    // local copy is moved a few seconds into the past; the pushed one is then
    // the newest scan of the root.
    Connection::open(&bench.db)
        .unwrap()
        .execute(
            "UPDATE scans SET started_at = started_at - 5 WHERE id = ?1",
            [local],
        )
        .unwrap();
    let pushed = bench.store().import_snapshot(&wire).unwrap();
    assert_eq!(pushed.len(), 1);
    assert_eq!(bench.newest(), pushed[0]);
    falls_back(&bench, Fallback::NoCursor);
}

/// Invariant 5 holds for a rescan as for a scan: cancelled before it starts,
/// it returns no tree and has counted nothing.
#[test]
fn a_cancelled_rescan_returns_no_tree() {
    let bench = Bench::new(forest);
    let progress = Arc::new(ScanProgress::default());
    progress.cancel();
    let store = bench.store();
    let base = store
        .rescan_base(&bench.root.to_string_lossy(), HOST)
        .unwrap();
    let err = rescan(
        &bench.root,
        ScanOptions::default(),
        Arc::clone(&progress),
        base,
    )
    .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::Interrupted);
    assert_eq!(progress.files.load(std::sync::atomic::Ordering::Relaxed), 0);
}
