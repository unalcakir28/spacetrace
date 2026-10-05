//! The parser against volumes a real NTFS implementation wrote.
//!
//! The images and their manifests come from `scripts/ntfs-fixtures.sh`: each
//! image was populated through an ntfs-3g mount, and the manifest is what
//! that mount reported about every entry — `lstat` sizes, blocks, link
//! counts, times, and the MFT record as the inode number. The tests build the
//! tree through the scanner's own `place` (the code every listing goes
//! through) and compare it with the manifest entry for entry.
//!
//! **Where the oracle is not Windows.** The rules this path follows are the
//! normal Windows walk's (see the module comment in `mod.rs`), and ntfs-3g
//! disagrees with them in three documented places, each handled by name
//! below rather than by loosening the comparison:
//!
//! * a symlink's size: ntfs-3g reports the target's length, the way POSIX
//!   does; Windows reports the unnamed stream, which for these links is 0;
//! * a directory's allocation: ntfs-3g reports its resident `$INDEX_ROOT`;
//!   the manifest carries the `$I30` `$INDEX_ALLOCATION` read by `ntfsinfo`
//!   instead, which is what `FILE_STANDARD_INFO` is taken to report;
//! * the link count of a file with an 8.3 alias: ntfs-3g reports the record
//!   header's count, which includes the alias; Windows does not count it.
//!
//! Whether Windows agrees with those three is exactly what the differential
//! test in `volume.rs` checks on the CI runner.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};

use super::*;
use crate::mounts::Mounts;
use crate::scan::{
    probe_mount, scan_source, scan_with, ScanOptions, ScanProgress, ScanStats, Source,
};
use crate::Tree;

// ------------------------------------------------------------- fixtures

/// The images, unpacked once per test binary.
///
/// Unpacked with the system `tar`, which reads gzip on macOS, Linux and
/// Windows alike (bsdtar ships with Windows 10 and later), because the crate
/// has no decompressor and must not grow one for its tests.
fn fixtures() -> &'static Path {
    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let archive = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ntfs.tar.gz");
        let status = std::process::Command::new("tar")
            .arg("-xzf")
            .arg(&archive)
            .arg("-C")
            .arg(dir.path())
            .status()
            .expect("tar must be on PATH to unpack the NTFS fixtures");
        assert!(
            status.success(),
            "tar could not unpack {}",
            archive.display()
        );
        dir
    })
    .path()
}

fn image(name: &str) -> PathBuf {
    fixtures().join(format!("{name}.img"))
}

/// One line of a manifest.
#[derive(Debug, Clone)]
struct Expected {
    kind: EntryKind,
    size: u64,
    blocks: u64,
    links: u64,
    mtime: i64,
    inode: u64,
    /// For a directory, its `$I30` index allocation per ntfsinfo.
    index_alloc: Option<u64>,
}

fn manifest(name: &str) -> BTreeMap<String, Expected> {
    let text = std::fs::read_to_string(fixtures().join(format!("{name}.manifest"))).unwrap();
    text.lines()
        .map(|line| {
            let cols: Vec<&str> = line.split('\t').collect();
            assert_eq!(cols.len(), 8, "manifest line: {line}");
            let kind = match cols[1] {
                "dir" => EntryKind::Dir,
                "symlink" => EntryKind::Symlink,
                _ => EntryKind::File,
            };
            let expected = Expected {
                kind,
                size: cols[2].parse().unwrap(),
                blocks: cols[3].parse().unwrap(),
                links: cols[4].parse().unwrap(),
                mtime: cols[5].parse().unwrap(),
                inode: cols[6].parse().unwrap(),
                index_alloc: cols[7].parse().ok(),
            };
            (cols[0].to_string(), expected)
        })
        .collect()
}

fn read(name: &str) -> Table {
    let mut file = File::open(image(name)).unwrap();
    let geometry = read_geometry(&mut file).unwrap();
    read_table(&mut file, geometry, |_, _| true).unwrap()
}

/// Build the tree from `table` through the scanner, rooted at `root`.
///
/// The root entry itself comes from a stand-in directory — on Windows it is
/// read with a handle, like the walk's, and is not the table's to answer —
/// so the comparisons below are over everything under it.
fn scan_table(table: Table, root: u64, opts: ScanOptions) -> (Tree, ScanStats) {
    let stand_in = tempfile::tempdir().unwrap();
    scan_source(stand_in.path(), opts, Source::Table(table, root)).unwrap()
}

/// Every entry under the root, by relative path with `/` separators.
fn entries(tree: &Tree) -> BTreeMap<String, crate::NodeId> {
    let mut found = BTreeMap::new();
    let mut stack: Vec<(crate::NodeId, String)> = vec![(tree.root(), String::new())];
    while let Some((id, prefix)) = stack.pop() {
        for child in tree.children(id) {
            let path = if prefix.is_empty() {
                tree.name(child).to_string()
            } else {
                format!("{prefix}/{}", tree.name(child))
            };
            assert!(
                found.insert(path.clone(), child).is_none(),
                "{path} is listed twice"
            );
            stack.push((child, path));
        }
    }
    found
}

/// Files whose 8.3 alias the fixture script set by hand.
const DOS_ALIASED: &[&str] = &["Long Name With Spaces.txt"];

/// Every field of every entry, against the mount. With deduplication off, so
/// that each name carries its own file's bytes and can be compared alone.
fn assert_matches_mount(name: &str) {
    let expected = manifest(name);
    let table = read(name);
    assert_eq!(table.bad_count, 0, "{name}: unreadable records");
    let root = ROOT_RECORD;
    let opts = ScanOptions {
        dedupe_hardlinks: false,
        ..ScanOptions::default()
    };
    let (tree, stats) = scan_table(table, root, opts);
    assert_eq!(stats.errors, 0, "{name}: {:?}", stats.error_samples);

    let found = entries(&tree);
    let missing: Vec<&String> = expected
        .keys()
        .filter(|p| !found.contains_key(*p))
        .collect();
    let extra: Vec<&String> = found
        .keys()
        .filter(|p| !expected.contains_key(*p))
        .collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "{name}: the parsed tree and the mount list different entries\n\
         missing from the parse: {missing:?}\nnot on the mount: {extra:?}"
    );

    for (path, want) in &expected {
        let node = tree.node(found[path]);
        assert_eq!(node.kind, want.kind, "{name}: {path}: kind");
        assert_eq!(node.mtime, want.mtime, "{name}: {path}: mtime");
        match want.kind {
            EntryKind::Dir => {
                assert_eq!(node.own_size, 0, "{name}: {path}: a directory has no size");
                assert_eq!(
                    Some(node.own_alloc),
                    want.index_alloc,
                    "{name}: {path}: a directory's allocation is its index allocation"
                );
            }
            // ntfs-3g reports the link target's length; the unnamed stream
            // of these links — the size Windows reports — is empty.
            EntryKind::Symlink => {
                assert_eq!(node.own_size, 0, "{name}: {path}: a link's stream is empty");
                assert_eq!(node.own_alloc, 0, "{name}: {path}: and allocates nothing");
                assert_eq!(want.blocks, 0, "{name}: {path}: the mount agrees on blocks");
            }
            _ => {
                assert_eq!(node.own_size, want.size, "{name}: {path}: size");
                // The mount counts 512-byte blocks rounded up; the parse has
                // the exact byte count, so it must round to the same number.
                assert_eq!(
                    node.own_alloc.div_ceil(512),
                    want.blocks,
                    "{name}: {path}: allocated {} bytes, the mount says {} blocks",
                    node.own_alloc,
                    want.blocks
                );
            }
        }
    }
}

/// Link counts and hardlink accounting, with deduplication on.
fn assert_hardlinks_counted_once(name: &str) {
    let expected = manifest(name);
    let table = read(name);
    let root = ROOT_RECORD;
    let (tree, stats) = scan_table(table, root, ScanOptions::default());
    let found = entries(&tree);

    let mut groups: HashMap<u64, Vec<&String>> = HashMap::new();
    for (path, want) in &expected {
        let node = tree.node(found[path]);
        if want.kind == EntryKind::Dir {
            // Read only with `one_filesystem`, as on the walk.
            assert_eq!(node.nlink, 1, "{name}: {path}: a directory reports 1");
            continue;
        }
        let aliased = DOS_ALIASED.contains(&path.as_str());
        let links = want.links - u64::from(aliased);
        assert_eq!(u64::from(node.nlink), links, "{name}: {path}: links");
        groups.entry(want.inode).or_default().push(path);
    }

    let mut deduped = 0u64;
    for paths in groups.values().filter(|paths| paths.len() > 1) {
        let charged: Vec<&&String> = paths
            .iter()
            .filter(|path| tree.node(found[**path]).own_size > 0)
            .collect();
        assert_eq!(
            charged.len(),
            1,
            "{name}: exactly one of {paths:?} carries the bytes, got {charged:?}"
        );
        let size = expected[*charged[0]].size;
        let total: u64 = paths
            .iter()
            .map(|path| tree.node(found[*path]).own_size)
            .sum();
        assert_eq!(total, size, "{name}: {paths:?} add up to one copy");
        deduped += paths.len() as u64 - 1;
    }
    assert!(deduped > 0, "{name}: the fixture has hardlinks");
    assert_eq!(stats.hardlinks_deduped, deduped, "{name}: repeat names");
}

// ------------------------------------------------------- the three images

#[test]
fn every_entry_of_the_feature_image_matches_the_mount() {
    assert_matches_mount("features");
}

#[test]
fn every_entry_of_the_fragmented_image_matches_the_mount() {
    assert_matches_mount("fragmented");
}

#[test]
fn every_entry_of_the_big_sector_image_matches_the_mount() {
    assert_matches_mount("bigsector");
}

#[test]
fn hardlinks_are_counted_once_and_report_their_link_count() {
    for name in ["features", "fragmented", "bigsector"] {
        assert_hardlinks_counted_once(name);
    }
}

/// The cases the feature image exists for, stated one by one, so that a
/// manifest regenerated by a changed ntfs-3g cannot quietly drop one.
#[test]
fn the_feature_image_holds_the_cases_it_is_meant_to() {
    let expected = manifest("features");
    let table = read("features");
    let opts = ScanOptions {
        dedupe_hardlinks: false,
        ..ScanOptions::default()
    };
    let (tree, _) = scan_table(table, ROOT_RECORD, opts);
    let found = entries(&tree);
    let node = |path: &str| tree.node(found[path]).clone();

    // A directory larger than one index block, listed whole.
    assert_eq!(node("big").children_len, 2500);
    assert!(expected["big"].index_alloc.unwrap() > 0);
    // Sparse: 4 MiB of hole and one cluster of data, charged for the cluster.
    let sparse = node("sparse.bin");
    assert_eq!((sparse.own_size, sparse.own_alloc), (4_198_400, 4096));
    // Compressed: charged what the compressed units occupy.
    let text = node("comp/text.txt");
    assert_eq!(text.own_size, 200_000);
    assert!(text.own_alloc < 40_000, "compressed to {}", text.own_alloc);
    // An alternate data stream is not the file's size.
    assert_eq!(node("ads.txt").own_size, 12);
    // A file with an 8.3 alias is one entry, not two.
    assert!(!found.keys().any(|path| path.contains('~')));
    // Resident data is charged its length rounded to eight bytes.
    assert_eq!(node("resident300").own_alloc, 304);
    assert_eq!(node("tiny").own_alloc, 8);
    assert_eq!(node("empty").own_alloc, 0);
    // Windows symlinks and junctions are links, never entered.
    assert_eq!(node("link.txt").kind, EntryKind::Symlink);
    assert_eq!(node("junction").kind, EntryKind::Symlink);
    assert_eq!(node("junction").children_len, 0);
    // Names outside the BMP survive as themselves.
    assert!(found
        .keys()
        .any(|path| path.contains("😀😀😀") && path.contains("ııı")));
    // Deleted files are gone, though their records are still on disk.
    assert!(!found
        .keys()
        .any(|path| path.starts_with("doomed") || path.starts_with("gone")));
}

#[test]
fn the_geometry_comes_from_the_boot_sector() {
    let cases = [
        ("features", 4096, 1024),
        ("fragmented", 512, 1024),
        ("bigsector", 65536, 4096),
    ];
    for (name, cluster, record) in cases {
        let geometry = read_geometry(&mut File::open(image(name)).unwrap()).unwrap();
        assert_eq!(geometry.bytes_per_cluster, cluster, "{name}");
        assert_eq!(geometry.bytes_per_record, record, "{name}");
    }
}

/// `striped.bin` needed an `$ATTRIBUTE_LIST`: its `$DATA` is in four records
/// with only the first carrying sizes, and its name is in an extension record.
/// Without extension records it would be missing from the tree altogether;
/// with the wrong instance's sizes it would claim nothing.
#[test]
fn a_file_spread_over_extension_records_is_whole() {
    let expected = manifest("fragmented");
    let table = read("fragmented");
    let opts = ScanOptions {
        dedupe_hardlinks: false,
        ..ScanOptions::default()
    };
    let (tree, _) = scan_table(table, ROOT_RECORD, opts);
    let found = entries(&tree);
    for path in ["striped.bin", "frag/striped-again.bin"] {
        let node = tree.node(found[path]);
        assert_eq!(node.own_size, expected[path].size, "{path}");
        assert_eq!(node.own_alloc, 600 * 512, "{path}: 600 one-cluster stripes");
    }
}

/// A subtree is the table's answer for any directory, not only the root.
#[test]
fn a_directory_below_the_root_lists_exactly_its_own_subtree() {
    let expected = manifest("features");
    let table = read("features");
    let nested = table
        .children(ROOT_RECORD)
        .find(|child| child.name == "nested")
        .unwrap()
        .reference;
    assert!(table.is_directory(nested));
    let (tree, _) = scan_table(table, nested, ScanOptions::default());
    let found: Vec<String> = entries(&tree).into_keys().collect();
    let want: Vec<String> = expected
        .keys()
        .filter_map(|path| path.strip_prefix("nested/"))
        .map(str::to_string)
        .collect();
    assert_eq!(found, want);
}

/// Exclusion and the depth limit are the walk's own rules, applied by the
/// same code: the entry is listed, its contents are not.
#[test]
fn exclusion_and_depth_limit_list_the_directory_but_not_its_contents() {
    let table = read("features");
    let opts = ScanOptions {
        exclude_names: vec!["big".into()],
        max_depth: Some(2),
        ..ScanOptions::default()
    };
    let (tree, _) = scan_table(table, ROOT_RECORD, opts);
    let found = entries(&tree);
    assert!(found.contains_key("big"));
    assert!(!found.keys().any(|path| path.starts_with("big/")));
    assert!(found.contains_key("nested/a"));
    assert!(!found.keys().any(|path| path.starts_with("nested/a/")));
}

/// Invariant 5, on this path: cancelled before it starts, no tree and no
/// counted files.
#[test]
fn a_cancelled_scan_from_the_table_returns_no_tree() {
    let table = read("features");
    let stand_in = tempfile::tempdir().unwrap();
    let progress = Arc::new(ScanProgress::default());
    progress.cancel();
    let result = scan_with(
        stand_in.path(),
        ScanOptions::default(),
        Arc::clone(&progress),
        Mounts::none(),
        probe_mount,
        Source::Table(table, ROOT_RECORD),
    );
    let err = result.expect_err("a cancelled scan has no tree");
    assert_eq!(err.kind(), std::io::ErrorKind::Interrupted);
    assert_eq!(progress.files.load(Ordering::Relaxed), 0);
}

/// The read reports how far it got and stops when told to.
#[test]
fn the_read_reports_progress_and_stops_when_asked() {
    let mut file = File::open(image("features")).unwrap();
    let geometry = read_geometry(&mut file).unwrap();
    let mut calls = Vec::new();
    read_table(&mut file, geometry, |done, total| {
        calls.push((done, total));
        true
    })
    .unwrap();
    let (last, total) = *calls.last().unwrap();
    assert_eq!(last, total, "the last call is the whole table");
    assert!(total > 2500, "one record per file at least: {total}");
    assert!(
        calls.windows(2).all(|w| w[0].0 < w[1].0),
        "and it only moves forward"
    );

    let stopped = read_table(&mut file, geometry, |_, _| false);
    assert_eq!(
        stopped.err().map(|e| e.kind()),
        Some(std::io::ErrorKind::Interrupted)
    );
}

/// A record whose fixups do not check out is a file the scan could not read:
/// counted and named, never parsed as if the bytes were right (invariant 7).
#[test]
fn a_torn_record_is_reported_and_left_out() {
    let expected = manifest("features");
    let mut bytes = std::fs::read(image("features")).unwrap();
    let mut cursor = Cursor::new(&bytes);
    let geometry = read_geometry(&mut cursor).unwrap();
    let record = expected["tiny"].inode;
    let runs = mft_stream(&mut cursor, geometry).unwrap().0.runs;
    // `tiny` is in the table's first run, which mkntfs lays out contiguously.
    let first = runs[0];
    assert!(record * geometry.bytes_per_record < first.len * geometry.bytes_per_cluster);
    let offset = first.lcn.unwrap() * geometry.bytes_per_cluster
        + record * geometry.bytes_per_record
        + FIXUP_STRIDE as u64
        - 1;
    bytes[offset as usize] ^= 0xFF;

    let table = read_table(&mut Cursor::new(&bytes), geometry, |_, _| true).unwrap();
    assert_eq!(table.bad_count, 1);
    assert_eq!(table.bad_records, vec![record]);

    let (tree, stats) = scan_table(table, ROOT_RECORD, ScanOptions::default());
    assert!(!entries(&tree).contains_key("tiny"));
    assert_eq!(stats.errors, 1);
    assert!(
        stats.error_samples[0]
            .0
            .ends_with(format!("<MFT record {record}>")),
        "{:?}",
        stats.error_samples
    );
}

#[test]
fn something_that_is_not_ntfs_is_refused() {
    let zeros = vec![0u8; 1 << 20];
    assert!(read_geometry(&mut Cursor::new(&zeros)).is_err());
}

// ------------------------------------------------------------ unit tests

/// A 1024-byte record with a two-block update sequence: the record carries
/// the sequence number at the end of each block, the array the real bytes.
fn protected_record() -> Vec<u8> {
    let mut record = vec![0u8; 1024];
    record[0..4].copy_from_slice(b"FILE");
    record[4..6].copy_from_slice(&0x30u16.to_le_bytes());
    record[6..8].copy_from_slice(&3u16.to_le_bytes());
    record[0x30..0x32].copy_from_slice(&[0xAB, 0xCD]);
    record[0x32..0x34].copy_from_slice(&[0x11, 0x22]);
    record[0x34..0x36].copy_from_slice(&[0x33, 0x44]);
    record[510..512].copy_from_slice(&[0xAB, 0xCD]);
    record[1022..1024].copy_from_slice(&[0xAB, 0xCD]);
    record
}

#[test]
fn fixups_put_back_the_bytes_the_sequence_number_stood_in_for() {
    let mut record = protected_record();
    assert!(apply_fixups(&mut record));
    assert_eq!(&record[510..512], &[0x11, 0x22]);
    assert_eq!(&record[1022..1024], &[0x33, 0x44]);
}

#[test]
fn a_block_without_the_sequence_number_is_torn() {
    let mut record = protected_record();
    record[1023] = 0;
    assert!(!apply_fixups(&mut record));

    // An array that does not cover the record is not a record either.
    let mut record = protected_record();
    record[6..8].copy_from_slice(&2u16.to_le_bytes());
    assert!(!apply_fixups(&mut record));
}

#[test]
fn run_lists_decode_signed_deltas_and_holes() {
    let bytes = [
        0x21, 0x10, 0x00, 0x01, // 16 clusters at LCN 256
        0x11, 0x08, 0xF0, // 8 clusters at 256 - 16 = 240
        0x01, 0x04, // a hole of 4
        0x11, 0x02, 0x10, // 2 clusters at 240 + 16 = 256: holes do not move the base
        0x00,
    ];
    let runs = decode_runs(&bytes, 0).unwrap();
    let want = [
        Run {
            vcn: 0,
            lcn: Some(256),
            len: 16,
        },
        Run {
            vcn: 16,
            lcn: Some(240),
            len: 8,
        },
        Run {
            vcn: 24,
            lcn: None,
            len: 4,
        },
        Run {
            vcn: 28,
            lcn: Some(256),
            len: 2,
        },
    ];
    assert_eq!(runs, want);
}

#[test]
fn a_malformed_run_list_is_refused_rather_than_followed() {
    // Before cluster 0.
    assert!(decode_runs(&[0x11, 0x01, 0xFF, 0x00], 0).is_none());
    // A run of no clusters.
    assert!(decode_runs(&[0x11, 0x00, 0x05, 0x00], 0).is_none());
    // Cut off mid-run, and with no terminator.
    assert!(decode_runs(&[0x21, 0x10], 0).is_none());
    assert!(decode_runs(&[0x11, 0x01, 0x05], 0).is_none());
}

/// 4 KiB clusters, 1 KiB records: the volume the reparse tests read from.
const REPARSE_GEOMETRY: Geometry = Geometry {
    bytes_per_cluster: 4096,
    bytes_per_record: 1024,
    mft_lcn: 0,
};
const JUNCTION: u32 = 0xA000_0003;
const WOF: u32 = 0x8000_0017;
const RECORD: u64 = 40;

/// An in-use directory record — a junction is a directory — holding one
/// non-resident `$REPARSE_POINT` instance whose data `runs` (a mapping-pairs
/// array) map from VCN `lowest`. `base` is 0 for a base record, else the
/// reference of its base; `length` is only read where `lowest` is 0.
fn reparse_record(seq: u16, base: u64, lowest: u64, length: u64, runs: &[u8]) -> Vec<u8> {
    let mut record = vec![0u8; 1024];
    let put = |record: &mut Vec<u8>, at: usize, bytes: &[u8]| {
        record[at..at + bytes.len()].copy_from_slice(bytes);
    };
    put(&mut record, 0, b"FILE");
    put(&mut record, 0x04, &0x30u16.to_le_bytes());
    put(&mut record, 0x06, &3u16.to_le_bytes());
    put(&mut record, 0x10, &seq.to_le_bytes());
    put(&mut record, 0x14, &0x38u16.to_le_bytes());
    put(
        &mut record,
        0x16,
        &(RECORD_IN_USE | RECORD_IS_DIRECTORY).to_le_bytes(),
    );
    put(&mut record, 0x20, &base.to_le_bytes());

    let attr = 0x38;
    let attr_len = (0x40 + runs.len() + 7) & !7;
    put(&mut record, attr, &ATTR_REPARSE_POINT.to_le_bytes());
    put(&mut record, attr + 0x04, &(attr_len as u32).to_le_bytes());
    record[attr + 0x08] = 1;
    put(&mut record, attr + 0x0A, &0x40u16.to_le_bytes());
    put(&mut record, attr + 0x10, &lowest.to_le_bytes());
    put(&mut record, attr + 0x20, &0x40u16.to_le_bytes());
    put(&mut record, attr + 0x30, &length.to_le_bytes());
    put(&mut record, attr + 0x40, runs);
    let end = attr + attr_len;
    put(&mut record, end, &ATTR_END.to_le_bytes());
    put(&mut record, 0x18, &((end + 8) as u32).to_le_bytes());
    put(&mut record, 0x1C, &1024u32.to_le_bytes());

    // Update sequence 1 at the end of both blocks; the bytes it stands in
    // for are zeros, already in the array.
    put(&mut record, 0x30, &[1, 0]);
    put(&mut record, 510, &[1, 0]);
    put(&mut record, 1022, &[1, 0]);
    record
}

/// Eight clusters of zeros, with `tags` written at the start of clusters.
fn reparse_volume(tags: &[(u64, u32)]) -> Vec<u8> {
    let mut volume = vec![0u8; 8 * 4096];
    for &(lcn, tag) in tags {
        let at = (lcn * 4096) as usize;
        volume[at..at + 4].copy_from_slice(&tag.to_le_bytes());
    }
    volume
}

/// Parse `records` and resolve their out-of-line reparse points.
fn resolve(records: Vec<(u64, Vec<u8>)>, volume: &[u8]) -> Table {
    let mut builder = Builder::with_records(64);
    for (number, mut record) in records {
        assert!(matches!(builder.record(number, &mut record), Parsed::Done));
    }
    resolve_reparse(&mut builder, &mut Cursor::new(volume), REPARSE_GEOMETRY);
    builder.finish()
}

/// One cluster at LCN 2.
const AT_LCN_2: &[u8] = &[0x11, 0x01, 0x02, 0x00];

#[test]
fn a_reparse_point_outside_its_record_is_read_for_its_tag() {
    let volume = reparse_volume(&[(2, JUNCTION)]);
    let table = resolve(
        vec![(RECORD, reparse_record(1, 0, 0, 64, AT_LCN_2))],
        &volume,
    );
    assert!(table.slots[RECORD as usize].is_surrogate());
    assert_eq!(table.bad_count, 0);

    let volume = reparse_volume(&[(2, WOF)]);
    let table = resolve(
        vec![(RECORD, reparse_record(1, 0, 0, 64, AT_LCN_2))],
        &volume,
    );
    assert!(!table.slots[RECORD as usize].is_surrogate());
    assert_eq!(table.bad_count, 0);
}

/// Split across a base and an extension record, the data's start is in the
/// base's instance: the extension's first run is the middle of the data, and
/// whatever its cluster begins with is not the tag.
#[test]
fn the_tag_is_read_where_the_data_starts_not_where_an_instance_does() {
    let seq = 7;
    let volume = reparse_volume(&[(2, WOF), (3, JUNCTION)]);
    let base = reparse_record(seq, 0, 0, 8192, AT_LCN_2);
    let extension = reparse_record(
        1,
        RECORD | u64::from(seq) << 48,
        1,
        0,
        &[0x11, 0x01, 0x03, 0x00],
    );
    let table = resolve(vec![(RECORD, base), (RECORD + 1, extension)], &volume);
    assert!(!table.slots[RECORD as usize].is_surrogate());
    assert_eq!(table.bad_count, 0);
}

/// A tag that cannot be read is one bad record, never a failed table, and the
/// entry is kept as a link rather than entered: taken for a directory, a
/// junction elsewhere on the volume would be counted twice.
#[test]
fn a_reparse_point_whose_tag_cannot_be_read_is_bad_and_not_entered() {
    let volume = reparse_volume(&[(2, JUNCTION)]);
    let cases: [(&str, &[u8]); 3] = [
        // A hole, then the cluster holding what would be the tag.
        (
            "a leading sparse run",
            &[0x01, 0x01, 0x11, 0x01, 0x02, 0x00],
        ),
        // LCN 100, past the end of an eight-cluster volume.
        ("a read error", &[0x11, 0x01, 0x64, 0x00]),
        // Only an instance from VCN 1: nothing says where the data starts.
        ("no instance starting the data", &[]),
    ];
    for (case, runs) in cases {
        let record = if runs.is_empty() {
            reparse_record(1, 0, 1, 0, AT_LCN_2)
        } else {
            reparse_record(1, 0, 0, 64, runs)
        };
        let table = resolve(vec![(RECORD, record)], &volume);
        assert!(table.slots[RECORD as usize].is_surrogate(), "{case}");
        assert_eq!(table.bad_count, 1, "{case}");
        assert_eq!(table.bad_records, vec![RECORD], "{case}");
    }
}

fn boot(bytes_per_sector: u16, sectors_per_cluster: u8, per_record: i8) -> Vec<u8> {
    let mut boot = vec![0u8; 512];
    boot[3..11].copy_from_slice(b"NTFS    ");
    boot[0x0B..0x0D].copy_from_slice(&bytes_per_sector.to_le_bytes());
    boot[0x0D] = sectors_per_cluster;
    boot[0x30..0x38].copy_from_slice(&4u64.to_le_bytes());
    boot[0x40] = per_record as u8;
    boot[510..512].copy_from_slice(&[0x55, 0xAA]);
    boot
}

#[test]
fn every_way_a_boot_sector_writes_its_sizes_is_read() {
    // The usual: 4 KiB clusters, a record written as 2^10.
    let g = Geometry::from_boot_sector(&boot(512, 8, -10)).unwrap();
    assert_eq!(
        (g.bytes_per_cluster, g.bytes_per_record, g.mft_lcn),
        (4096, 1024, 4)
    );
    // Clusters smaller than a record: written as a cluster count.
    let g = Geometry::from_boot_sector(&boot(512, 1, 2)).unwrap();
    assert_eq!((g.bytes_per_cluster, g.bytes_per_record), (512, 1024));
    // Clusters past 64 KiB: sectors per cluster as a negative power of two.
    let g = Geometry::from_boot_sector(&boot(512, 0xF4, -10)).unwrap();
    assert_eq!(g.bytes_per_cluster, 2 << 20);
    // And refusals.
    assert!(Geometry::from_boot_sector(&boot(512, 0, -10)).is_err());
    assert!(Geometry::from_boot_sector(&boot(500, 8, -10)).is_err());
    assert!(Geometry::from_boot_sector(&boot(512, 8, -8)).is_err());
}

#[test]
fn file_times_convert_exactly_as_the_walk_converts_them() {
    // 2026-10-05 07:46:30 UTC, and one tick short of the next second.
    let at = (1_791_186_390 + 11_644_473_600) * 10_000_000u64;
    assert_eq!(filetime_to_unix(at), 1_791_186_390);
    assert_eq!(filetime_to_unix(at + 9_999_999), 1_791_186_390);
}

/// Not a check: how long reading and building take on an image of your
/// choosing, five rounds — the parser's own cost, without a disk's.
///
/// `SPACETRACE_NTFS_IMAGE=/path/to.img cargo test --release -p
/// spacetrace-scan-core measure_an_image -- --ignored --nocapture`
#[test]
#[ignore]
fn measure_an_image() {
    let Ok(path) = std::env::var("SPACETRACE_NTFS_IMAGE") else {
        eprintln!("set SPACETRACE_NTFS_IMAGE to an NTFS image");
        return;
    };
    for round in 0..5 {
        let started = std::time::Instant::now();
        let mut file = File::open(&path).unwrap();
        let geometry = read_geometry(&mut file).unwrap();
        let table = read_table(&mut file, geometry, |_, _| true).unwrap();
        let read = started.elapsed();
        let records = table.capacity();
        let started = std::time::Instant::now();
        let (_, stats) = scan_table(table, ROOT_RECORD, ScanOptions::default());
        let built = started.elapsed();
        eprintln!(
            "round {round}: {records} records read and parsed in {} ms; \
             {} files and {} dirs built into a tree in {} ms",
            read.as_millis(),
            stats.files,
            stats.dirs,
            built.as_millis()
        );
    }
}
