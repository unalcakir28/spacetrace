//! What the store keeps for an incremental rescan — the journal cursor, the
//! record of how a tree was made, and which directories may not be taken
//! over unread — in a side table that belongs to this machine: never
//! exported, never imported, and invisible to a build that predates it.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use rusqlite::Connection;
use spacetrace_scan_core::{
    scan, Fallback, ImportedNode, Node, RescanKind, ScanOptions, ScanProgress, ScanStats, Tree,
};
use spacetrace_store::{ScanId, Store};

/// A tree with a hardlink in one branch and nothing shared in the other, so
/// that both flag values are present.
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("linked/deep")).unwrap();
    fs::create_dir_all(root.join("plain/inner")).unwrap();
    fs::write(root.join("linked/deep/x"), vec![1u8; 9000]).unwrap();
    fs::hard_link(root.join("linked/deep/x"), root.join("linked/y")).unwrap();
    fs::write(root.join("plain/inner/z"), vec![2u8; 300]).unwrap();
    dir
}

fn scan_of(dir: &tempfile::TempDir) -> (Tree, ScanStats) {
    scan(
        dir.path(),
        ScanOptions::default(),
        Arc::new(ScanProgress::default()),
    )
    .unwrap()
}

fn flags_by_path(tree: &Tree) -> HashMap<String, u8> {
    let mut out = HashMap::new();
    tree.for_each_path(None, |id, path| {
        out.insert(path.to_string(), tree.node(id).flags());
    });
    out
}

/// The stored row, read as an older build or an operator would read it.
fn side_row(db: &Path, id: ScanId) -> Option<(Option<String>, String)> {
    let conn = Connection::open(db).unwrap();
    conn.query_row(
        "SELECT journal, rescan FROM rescan_state WHERE scan_id = ?1",
        [id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .ok()
}

fn has_side_table(db: &Path) -> bool {
    Connection::open(db)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'rescan_state'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
        > 0
}

fn base_tree(store: &Store, root: &Path) -> Result<Tree, Fallback> {
    let base = store
        .rescan_base(&root.to_string_lossy(), "here")
        .unwrap()
        .expect("there is a scan of the root");
    (base.load)(&ScanProgress::default())
}

/// A directory's flags come back with the base exactly as the scan computed
/// them; a file's come back as zero, because only the directory above it is
/// ever asked (see `encode_flags`).
#[test]
fn a_base_carries_its_directories_flags() {
    let dir = fixture();
    let (tree, stats) = scan_of(&dir);
    let mut store = Store::open_in_memory().unwrap();
    store.save(&tree, &stats, "here", None).unwrap();

    let loaded = base_tree(&store, tree.root_path()).unwrap();

    let before = flags_by_path(&tree);
    assert_eq!(before.get("linked/y"), Some(&Node::SHARED), "the fixture");
    assert_eq!(before.get("linked"), Some(&Node::SHARED), "the fixture");
    assert_eq!(before.get("plain"), Some(&0), "the fixture");
    let after = flags_by_path(&loaded);
    tree.for_each_path(None, |id, path| {
        let want = if tree.node(id).is_dir() {
            before[path]
        } else {
            0
        };
        assert_eq!(after[path], want, "{path:?}");
    });
}

/// The cursor is stored exactly as the scan handed it over, beside a record
/// saying the tree was read in full — and the scans table itself is exactly
/// what schema v3 holds.
#[test]
fn the_cursor_and_the_record_go_to_the_side_table() {
    let dir = fixture();
    let work = tempfile::tempdir().unwrap();
    let db = work.path().join("db.sqlite");
    let (tree, stats) = scan_of(&dir);
    let id = Store::open(&db)
        .unwrap()
        .save(&tree, &stats, "here", None)
        .unwrap();

    if cfg!(target_os = "macos") {
        let journal = stats.journal.as_deref().expect("a temp dir is on APFS");
        assert!(journal.starts_with("fsevents1 "), "{journal}");
    } else {
        assert_eq!(stats.journal, None, "no journal here yet");
    }
    assert_eq!(
        side_row(&db, id),
        Some((stats.journal.clone(), "full".into()))
    );
    assert_eq!(
        Store::open(&db).unwrap().rescan_of(id).unwrap(),
        Some(RescanKind::Full)
    );
    let version: i64 = Connection::open(&db)
        .unwrap()
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(
        version, 3,
        "no schema change for a feature older builds lack"
    );
}

/// A snapshot that travelled is never the base of an incremental rescan
/// here: its cursor is a position in another machine's journal, and its
/// flags were never checked by this one. The export leaves the side table
/// behind, so the receiver has no row — no cursor.
#[test]
fn a_pushed_snapshot_carries_no_rescan_state() {
    let dir = fixture();
    let work = tempfile::tempdir().unwrap();
    let (tree, stats) = scan_of(&dir);
    let mut sender = Store::open(work.path().join("sender.sqlite")).unwrap();
    let id = sender.save(&tree, &stats, "here", None).unwrap();
    let wire = work.path().join("wire.sqlite");
    sender.export_snapshot(id, &wire).unwrap();
    assert!(!has_side_table(&wire), "the export is exactly schema v3");

    let receiver_db = work.path().join("receiver.sqlite");
    let mut receiver = Store::open(&receiver_db).unwrap();
    let imported = receiver.import_snapshot(&wire).unwrap()[0];

    assert_eq!(side_row(&receiver_db, imported), None);
    assert_eq!(receiver.rescan_of(imported).unwrap(), None);
    let base = receiver
        .rescan_base(&tree.root_path().to_string_lossy(), "here")
        .unwrap()
        .unwrap();
    assert_eq!(base.journal.unwrap_err(), Fallback::NoCursor);
}

/// A reason a newer build added, read by this one: still a fallback, as
/// `fallback:unknown`, and never an error. Played by writing the record as
/// that build would.
#[test]
fn a_reason_from_a_newer_build_reads_as_an_unknown_fallback() {
    let dir = fixture();
    let db = dir.path().join("db.sqlite");
    let (tree, stats) = scan_of(&dir);
    let id = Store::open(&db)
        .unwrap()
        .save(&tree, &stats, "here", None)
        .unwrap();
    Connection::open(&db)
        .unwrap()
        .execute(
            "UPDATE rescan_state SET rescan = 'fallback:from-a-newer-build' WHERE scan_id = ?1",
            [id],
        )
        .unwrap();
    assert_eq!(
        Store::open(&db).unwrap().rescan_of(id).unwrap(),
        Some(RescanKind::Fallback(Fallback::Unknown))
    );
}

/// Another tool's export was never walked here: no row, and the reason a
/// rescan gives is the one a user can act on.
#[test]
fn a_tree_imported_from_another_tool_has_no_rescan_state() {
    let work = tempfile::tempdir().unwrap();
    let db = work.path().join("db.sqlite");
    let tree = Tree::from_nested(
        "/imported".into(),
        ImportedNode {
            children: vec![ImportedNode::file("f", 10, 4096)],
            ..ImportedNode::dir("imported")
        },
    );
    let mut store = Store::open(&db).unwrap();
    let id = store
        .save_import(&tree, &ScanStats::default(), "here", None)
        .unwrap();

    assert_eq!(side_row(&db, id), None);
    assert_eq!(store.rescan_of(id).unwrap(), None);
    let base = store.rescan_base("/imported", "here").unwrap().unwrap();
    assert_eq!(base.journal.unwrap_err(), Fallback::ImportedBase);
}

/// Deleting a scan — through the store, or as an older build does it, with
/// nothing but `DELETE FROM scans` — takes its row with it.
#[test]
fn a_deleted_or_pruned_scan_leaves_no_row_behind() {
    let dir = fixture();
    let work = tempfile::tempdir().unwrap();
    let db = work.path().join("db.sqlite");
    let (tree, stats) = scan_of(&dir);
    let mut store = Store::open(&db).unwrap();
    let ids: Vec<ScanId> = (0..4)
        .map(|_| store.save(&tree, &stats, "here", None).unwrap())
        .collect();

    assert!(store.delete(ids[0]).unwrap());
    store
        .prune_target(&tree.root_path().to_string_lossy(), "here", 2)
        .unwrap();
    // An older build: its own connection, its own `foreign_keys = ON`.
    let older = Connection::open(&db).unwrap();
    older.pragma_update(None, "foreign_keys", "ON").unwrap();
    older
        .execute("DELETE FROM scans WHERE id = ?1", [ids[3]])
        .unwrap();

    let left: Vec<ScanId> = {
        let mut stmt = older
            .prepare("SELECT scan_id FROM rescan_state ORDER BY scan_id")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(left, vec![ids[2]]);
}

/// Invariant 0, with the side table in the file and without it: opening and
/// asking for a base takes no write lock, so neither disturbs a writer — and
/// a reader that knows only schema v3 reads the file as before.
#[test]
fn reading_with_or_without_the_side_table_takes_no_write_lock() {
    let dir = fixture();
    let (tree, stats) = scan_of(&dir);
    let work = tempfile::tempdir().unwrap();
    let root = tree.root_path().to_string_lossy().into_owned();

    // Without: a file written by an older build, as `import_snapshot` leaves
    // it. With: one this build has saved a scan into.
    let without = work.path().join("without.sqlite");
    let with = work.path().join("with.sqlite");
    let id = Store::open(&with)
        .unwrap()
        .save(&tree, &stats, "here", None)
        .unwrap();
    Store::open(&with)
        .unwrap()
        .export_snapshot(id, &without)
        .unwrap();
    assert!(has_side_table(&with) && !has_side_table(&without));
    // In WAL, as every database the store has opened once is: an export is
    // written with a rollback journal, and switching it is a write.
    drop(Store::open(&without).unwrap());

    for db in [&without, &with] {
        let writer = Connection::open(db).unwrap();
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        writer
            .execute(
                "INSERT INTO scans (host, root, started_at, duration_ms, total_size,
                                    total_alloc, files, dirs, errors, hardlinks_deduped,
                                    scanner_version, label)
                 VALUES ('h2','/r2',1,1,1,1,1,1,0,0,'t',NULL)",
                [],
            )
            .unwrap();

        let reader = Store::open(db).expect("opening while a write is in flight");
        let base = reader.rescan_base(&root, "here").unwrap().unwrap();
        if db == &with {
            assert!((base.load)(&ScanProgress::default()).is_ok());
        } else {
            assert_eq!(base.journal.unwrap_err(), Fallback::NoCursor);
        }

        // The older reader: the v3 columns and nothing else.
        let older = Connection::open(db).unwrap();
        let (files, version): (i64, i64) = (
            older
                .query_row("SELECT files FROM scans WHERE id = 1", [], |r| r.get(0))
                .unwrap(),
            older
                .pragma_query_value(None, "user_version", |r| r.get(0))
                .unwrap(),
        );
        assert_eq!((files as u64, version), (stats.files, 3));

        writer.execute_batch("COMMIT").unwrap();
    }
}

/// A flags blob this build did not write is no knowledge at all.
#[test]
fn a_damaged_flags_blob_is_a_damaged_base() {
    let dir = fixture();
    let work = tempfile::tempdir().unwrap();
    let db = work.path().join("db.sqlite");
    let (tree, stats) = scan_of(&dir);
    let id = Store::open(&db)
        .unwrap()
        .save(&tree, &stats, "here", None)
        .unwrap();
    for broken in [
        &b"\x01\x00\x00"[..],
        &b"\x05\x00\x00\x00\x01\x02\x00\x00\x00\x01"[..],
    ] {
        Connection::open(&db)
            .unwrap()
            .execute(
                "UPDATE rescan_state SET flags = ?1 WHERE scan_id = ?2",
                rusqlite::params![broken, id],
            )
            .unwrap();
        let store = Store::open(&db).unwrap();
        assert_eq!(
            base_tree(&store, tree.root_path()).unwrap_err(),
            Fallback::BaseDamaged,
            "{broken:?}"
        );
    }
}

/// Why the newest scan of `root` cannot be a base, if it cannot: what the
/// stored state says before the journal is asked, or what loading it says.
#[cfg(target_os = "macos")]
fn refusal(store: &Store, root: &Path) -> Option<Fallback> {
    let base = store
        .rescan_base(&root.to_string_lossy(), "here")
        .unwrap()
        .expect("there is a scan of the root");
    match base.journal {
        Err(reason) => Some(reason),
        Ok(_) => (base.load)(&ScanProgress::default()).err(),
    }
}

/// The flags and the cursor decide what is copied unread and where the
/// replay starts, and `content_hash` covers neither. Damage that still
/// parses — a directory's flags cleared, a position moved — must read as a
/// damaged base, not as permission to copy a subtree that holds a hardlink.
///
/// macOS only: it needs a cursor, and only a root with a journal gets one —
/// elsewhere every base reads as `no-cursor` before anything is checked.
#[test]
#[cfg(target_os = "macos")]
fn rescan_state_that_still_parses_but_was_altered_is_a_damaged_base() {
    let dir = fixture();
    let work = tempfile::tempdir().unwrap();
    let db = work.path().join("db.sqlite");
    let (tree, stats) = scan_of(&dir);
    let id = Store::open(&db)
        .unwrap()
        .save(&tree, &stats, "here", None)
        .unwrap();
    let root = tree.root_path().to_path_buf();
    assert_eq!(refusal(&Store::open(&db).unwrap(), &root), None, "intact");

    let conn = Connection::open(&db).unwrap();
    let (journal, flags): (String, Vec<u8>) = conn
        .query_row(
            "SELECT journal, flags FROM rescan_state WHERE scan_id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(
        flags.len() >= 5 && flags[4] != 0,
        "the fixture flags a directory"
    );
    let set = |column: &str, value: &dyn rusqlite::ToSql| {
        conn.execute(
            &format!("UPDATE rescan_state SET {column} = ?1 WHERE scan_id = ?2"),
            rusqlite::params![value, id],
        )
        .unwrap();
    };

    // One directory's flags cleared: still whole records, in order.
    let mut cleared = flags.clone();
    cleared[4] = 0;
    set("flags", &cleared);
    assert_eq!(
        refusal(&Store::open(&db).unwrap(), &root),
        Some(Fallback::BaseDamaged),
        "cleared flags"
    );
    set("flags", &flags);

    // The position moved forwards: still a cursor, and a replay from it
    // would skip what happened in between.
    let mut fields: Vec<String> = journal.split(' ').map(str::to_string).collect();
    fields[2] = (fields[2].parse::<u64>().unwrap() + 1000).to_string();
    set("journal", &fields.join(" "));
    assert_eq!(
        refusal(&Store::open(&db).unwrap(), &root),
        Some(Fallback::BaseDamaged),
        "moved cursor"
    );
    set("journal", &journal);
    assert_eq!(refusal(&Store::open(&db).unwrap(), &root), None, "restored");
}
