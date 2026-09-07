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

// ---------------------------------------------------------- snapshot export

/// The exported file is what the agent puts on the wire, so it has to be
/// openable on its own and carry the original scan's metadata unchanged.
#[test]
fn an_exported_snapshot_is_a_standalone_database() {
    let dir = fixture();
    let (tree, stats) = scan_fixture(&dir);
    let out_dir = tempfile::tempdir().unwrap();
    let out = out_dir.path().join("snap.sqlite");

    let mut store = Store::open(out_dir.path().join("main.sqlite")).unwrap();
    let id = store.save(&tree, &stats, "srv1", Some("nightly")).unwrap();
    let original = store.scan(id).unwrap().unwrap();

    store.export_snapshot(id, &out).unwrap();

    let exported = Store::open(&out).unwrap();
    let scans = exported.list().unwrap();
    assert_eq!(scans.len(), 1, "export must hold exactly the one scan");

    let (loaded, meta) = exported.load(id).unwrap();
    assert_eq!(meta.id, original.id);
    assert_eq!(meta.host, "srv1");
    assert_eq!(meta.label.as_deref(), Some("nightly"));
    assert_eq!(
        meta.started_at, original.started_at,
        "timestamp must survive"
    );
    assert_eq!(meta.duration_ms, original.duration_ms);
    assert_eq!(meta.scanner_version, original.scanner_version);
    assert_eq!(loaded.len(), tree.len());
    assert_eq!(loaded.total_size(), tree.total_size());
    assert_eq!(loaded.total_alloc(), tree.total_alloc());
    for node in tree.iter() {
        assert_eq!(tree.node(node).name, loaded.node(node).name);
        assert_eq!(tree.node(node).size, loaded.node(node).size);
    }
}

#[test]
fn exporting_picks_out_one_scan_from_a_database_holding_many() {
    let dir = fixture();
    let (tree, stats) = scan_fixture(&dir);
    let out_dir = tempfile::tempdir().unwrap();
    let out = out_dir.path().join("snap.sqlite");

    let mut store = Store::open(out_dir.path().join("main.sqlite")).unwrap();
    let first = store.save(&tree, &stats, "srv1", Some("first")).unwrap();
    let second = store.save(&tree, &stats, "srv2", Some("second")).unwrap();
    store.save(&tree, &stats, "srv3", Some("third")).unwrap();

    store.export_snapshot(second, &out).unwrap();

    let exported = Store::open(&out).unwrap();
    let scans = exported.list().unwrap();
    assert_eq!(scans.len(), 1);
    assert_eq!(scans[0].label.as_deref(), Some("second"));
    assert!(exported.scan(first).unwrap().is_none());
}

#[test]
fn exporting_over_an_existing_file_replaces_it_rather_than_merging() {
    let dir = fixture();
    let (tree, stats) = scan_fixture(&dir);
    let out_dir = tempfile::tempdir().unwrap();
    let out = out_dir.path().join("snap.sqlite");

    let mut store = Store::open(out_dir.path().join("main.sqlite")).unwrap();
    let first = store.save(&tree, &stats, "srv1", Some("first")).unwrap();
    let second = store.save(&tree, &stats, "srv2", Some("second")).unwrap();

    store.export_snapshot(first, &out).unwrap();
    store.export_snapshot(second, &out).unwrap();

    let exported = Store::open(&out).unwrap();
    let scans = exported.list().unwrap();
    assert_eq!(scans.len(), 1, "the earlier export must not linger");
    assert_eq!(scans[0].label.as_deref(), Some("second"));
}

#[test]
fn exporting_an_unknown_scan_fails_without_leaving_a_file() {
    let out_dir = tempfile::tempdir().unwrap();
    let out = out_dir.path().join("snap.sqlite");
    let store = Store::open(out_dir.path().join("main.sqlite")).unwrap();

    assert!(store.export_snapshot(999, &out).is_err());
    assert!(
        !out.exists(),
        "a failed export must not leave a stub behind"
    );
}

/// The connection has to survive a failed export, because the agent keeps
/// serving from it afterwards.
#[test]
fn the_store_is_still_usable_after_a_failed_export() {
    let dir = fixture();
    let (tree, stats) = scan_fixture(&dir);
    let out_dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(out_dir.path().join("main.sqlite")).unwrap();
    let id = store.save(&tree, &stats, "srv1", None).unwrap();

    assert!(store
        .export_snapshot(999, &out_dir.path().join("a.sqlite"))
        .is_err());

    let good = out_dir.path().join("b.sqlite");
    store.export_snapshot(id, &good).unwrap();
    assert_eq!(Store::open(&good).unwrap().list().unwrap().len(), 1);
}

// ------------------------------------------------------------ per-target prune

#[test]
fn pruning_one_target_leaves_other_targets_alone() {
    let dir = fixture();
    let (tree, stats) = scan_fixture(&dir);
    let mut store = Store::open_in_memory().unwrap();

    for _ in 0..5 {
        store.save(&tree, &stats, "srv1", None).unwrap();
    }
    for _ in 0..3 {
        store.save(&tree, &stats, "srv2", None).unwrap();
    }
    let root = tree.root_path().to_string_lossy().into_owned();

    let removed = store.prune_target(&root, "srv1", 2).unwrap();
    assert_eq!(removed, 3);

    let remaining = store.list().unwrap();
    assert_eq!(remaining.iter().filter(|s| s.host == "srv1").count(), 2);
    assert_eq!(
        remaining.iter().filter(|s| s.host == "srv2").count(),
        3,
        "the other host's retention policy must not be applied"
    );
}

#[test]
fn pruning_a_target_keeps_the_newest_scans() {
    let dir = fixture();
    let (tree, stats) = scan_fixture(&dir);
    let mut store = Store::open_in_memory().unwrap();

    let mut ids = Vec::new();
    for _ in 0..4 {
        ids.push(store.save(&tree, &stats, "srv1", None).unwrap());
    }
    let root = tree.root_path().to_string_lossy().into_owned();

    store.prune_target(&root, "srv1", 2).unwrap();

    assert!(store.scan(ids[0]).unwrap().is_none());
    assert!(store.scan(ids[1]).unwrap().is_none());
    assert!(store.scan(ids[2]).unwrap().is_some());
    assert!(store.scan(ids[3]).unwrap().is_some());
}

#[test]
fn pruning_a_target_with_fewer_scans_than_the_limit_removes_nothing() {
    let dir = fixture();
    let (tree, stats) = scan_fixture(&dir);
    let mut store = Store::open_in_memory().unwrap();
    store.save(&tree, &stats, "srv1", None).unwrap();
    let root = tree.root_path().to_string_lossy().into_owned();

    assert_eq!(store.prune_target(&root, "srv1", 10).unwrap(), 0);
    assert_eq!(store.list().unwrap().len(), 1);
}

// ---------------------------------------------------------- snapshot import

/// A pushed snapshot must land in the receiver's history looking exactly like a
/// scan it took itself, or diffing against it would compare the wrong things.
#[test]
fn an_imported_snapshot_keeps_its_identity_and_gets_a_local_id() {
    let dir = fixture();
    let (tree, stats) = scan_fixture(&dir);
    let work = tempfile::tempdir().unwrap();

    let mut sender = Store::open(work.path().join("sender.sqlite")).unwrap();
    let sent = sender.save(&tree, &stats, "srv1", Some("nightly")).unwrap();
    let original = sender.scan(sent).unwrap().unwrap();
    let wire = work.path().join("wire.sqlite");
    sender.export_snapshot(sent, &wire).unwrap();

    // Give the receiver a scan of its own first, so ids genuinely collide.
    let mut receiver = Store::open(work.path().join("receiver.sqlite")).unwrap();
    receiver.save(&tree, &stats, "laptop", None).unwrap();

    let imported = receiver.import_snapshot(&wire).unwrap();
    assert_eq!(imported.len(), 1);

    let meta = receiver.scan(imported[0]).unwrap().unwrap();
    assert_eq!(meta.host, "srv1");
    assert_eq!(meta.root, original.root);
    assert_eq!(meta.started_at, original.started_at);
    assert_eq!(meta.label.as_deref(), Some("nightly"));
    assert_eq!(meta.total_size, original.total_size);
    assert_eq!(meta.scanner_version, original.scanner_version);

    // And the tree itself survived the round trip.
    let (loaded, _) = receiver.load(imported[0]).unwrap();
    assert_eq!(loaded.len(), tree.len());
    assert_eq!(loaded.total_size(), tree.total_size());
    for node in tree.iter() {
        assert_eq!(tree.node(node).name, loaded.node(node).name);
        assert_eq!(tree.rel_path(node), loaded.rel_path(node));
    }
    assert_eq!(receiver.list().unwrap().len(), 2);
}

#[test]
fn importing_the_same_snapshot_twice_is_a_no_op() {
    let dir = fixture();
    let (tree, stats) = scan_fixture(&dir);
    let work = tempfile::tempdir().unwrap();

    let mut sender = Store::open(work.path().join("sender.sqlite")).unwrap();
    let sent = sender.save(&tree, &stats, "srv1", None).unwrap();
    let wire = work.path().join("wire.sqlite");
    sender.export_snapshot(sent, &wire).unwrap();

    let mut receiver = Store::open(work.path().join("receiver.sqlite")).unwrap();
    assert_eq!(receiver.import_snapshot(&wire).unwrap().len(), 1);
    assert_eq!(
        receiver.import_snapshot(&wire).unwrap().len(),
        0,
        "a re-push must not duplicate the snapshot"
    );
    assert_eq!(receiver.list().unwrap().len(), 1);
}

/// Two hosts scanning the same path are different targets, not duplicates.
#[test]
fn snapshots_from_different_hosts_are_not_treated_as_duplicates() {
    let dir = fixture();
    let (tree, stats) = scan_fixture(&dir);
    let work = tempfile::tempdir().unwrap();

    let mut receiver = Store::open(work.path().join("receiver.sqlite")).unwrap();
    for host in ["srv1", "srv2"] {
        let mut sender = Store::open(work.path().join(format!("{host}.sqlite"))).unwrap();
        let id = sender.save(&tree, &stats, host, None).unwrap();
        let wire = work.path().join(format!("{host}-wire.sqlite"));
        sender.export_snapshot(id, &wire).unwrap();
        assert_eq!(receiver.import_snapshot(&wire).unwrap().len(), 1);
    }
    assert_eq!(receiver.list().unwrap().len(), 2);
}

#[test]
fn importing_a_missing_file_fails_cleanly() {
    let work = tempfile::tempdir().unwrap();
    let mut receiver = Store::open(work.path().join("receiver.sqlite")).unwrap();
    assert!(receiver
        .import_snapshot(&work.path().join("nope.sqlite"))
        .is_err());
    // The connection must still work afterwards.
    assert_eq!(receiver.list().unwrap().len(), 0);
}

/// The whole point of importing: the receiver can diff a pushed snapshot
/// against a later one from the same machine.
#[test]
fn imported_snapshots_participate_in_target_lookups() {
    let dir = fixture();
    let (tree, stats) = scan_fixture(&dir);
    let work = tempfile::tempdir().unwrap();
    let root = tree.root_path().to_string_lossy().into_owned();

    let mut receiver = Store::open(work.path().join("receiver.sqlite")).unwrap();
    for n in 0..2 {
        if n > 0 {
            // started_at has one-second resolution and is part of the
            // duplicate key, so two scans taken inside the same second are
            // genuinely the same snapshot as far as import is concerned.
            std::thread::sleep(std::time::Duration::from_millis(1100));
        }
        let mut sender = Store::open(work.path().join(format!("s{n}.sqlite"))).unwrap();
        let id = sender.save(&tree, &stats, "srv1", None).unwrap();
        let wire = work.path().join(format!("w{n}.sqlite"));
        sender.export_snapshot(id, &wire).unwrap();
        receiver.import_snapshot(&wire).unwrap();
    }

    let pair = receiver.last_two_for(&root, Some("srv1")).unwrap();
    assert_eq!(pair.len(), 2, "both pushed snapshots must be comparable");
    assert!(pair[0].started_at >= pair[1].started_at);
}

/// The load path is the trust boundary for any snapshot file, including one
/// downloaded from an agent. A tampered arena must be an error, not a panic.
#[test]
fn a_corrupted_snapshot_is_rejected_rather_than_loaded() {
    let dir = fixture();
    let (tree, stats) = scan_fixture(&dir);
    let work = tempfile::tempdir().unwrap();
    let path = work.path().join("db.sqlite");

    let id = {
        let mut store = Store::open(&path).unwrap();
        store.save(&tree, &stats, "srv1", None).unwrap()
    };

    // Point the root's children off the end of the arena, the way a truncated
    // or hand-edited file would.
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute(
        "UPDATE entries SET children_len = 9999 WHERE scan_id = ?1 AND id = 0",
        [id],
    )
    .unwrap();
    drop(conn);

    let store = Store::open(&path).unwrap();
    let err = store.load(id).unwrap_err();
    let message = format!("{err:#}");
    assert!(message.contains("not a usable tree"), "{message}");

    // Metadata still reads, so `scans` can list a snapshot it cannot open.
    assert!(store.scan(id).unwrap().is_some());
}

#[test]
fn a_snapshot_whose_child_points_backwards_is_rejected() {
    let dir = fixture();
    let (tree, stats) = scan_fixture(&dir);
    let work = tempfile::tempdir().unwrap();
    let path = work.path().join("db.sqlite");

    let id = {
        let mut store = Store::open(&path).unwrap();
        store.save(&tree, &stats, "srv1", None).unwrap()
    };

    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute(
        "UPDATE entries SET children_start = 0, children_len = 1 WHERE scan_id = ?1 AND id = 0",
        [id],
    )
    .unwrap();
    drop(conn);

    // Without the check this loops forever walking parent pointers.
    assert!(Store::open(&path).unwrap().load(id).is_err());
}

// ------------------------------------------------------------- migration

/// A database written by a v1 build must keep working, and its old rows must
/// read back as "capacity unknown" rather than as a full disk.
#[test]
fn a_v1_database_migrates_to_v2_without_losing_anything() {
    let work = tempfile::tempdir().unwrap();
    let path = work.path().join("v1.sqlite");

    // Build a v1 database by hand: the v1 schema had no fs_* columns.
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE scans (
                id                INTEGER PRIMARY KEY AUTOINCREMENT,
                host              TEXT    NOT NULL,
                root              TEXT    NOT NULL,
                started_at        INTEGER NOT NULL,
                duration_ms       INTEGER NOT NULL,
                total_size        INTEGER NOT NULL,
                total_alloc       INTEGER NOT NULL,
                files             INTEGER NOT NULL,
                dirs              INTEGER NOT NULL,
                errors            INTEGER NOT NULL,
                hardlinks_deduped INTEGER NOT NULL,
                scanner_version   TEXT    NOT NULL,
                label             TEXT
            );
            CREATE TABLE entries (
                scan_id        INTEGER NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
                id             INTEGER NOT NULL,
                parent_id      INTEGER,
                name           TEXT    NOT NULL,
                kind           INTEGER NOT NULL,
                size           INTEGER NOT NULL,
                alloc          INTEGER NOT NULL,
                mtime          INTEGER NOT NULL,
                nlink          INTEGER NOT NULL,
                files          INTEGER NOT NULL,
                dirs           INTEGER NOT NULL,
                children_start INTEGER NOT NULL,
                children_len   INTEGER NOT NULL,
                PRIMARY KEY (scan_id, id)
            ) WITHOUT ROWID;
            "#,
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO scans (host, root, started_at, duration_ms, total_size, total_alloc,
                                files, dirs, errors, hardlinks_deduped, scanner_version, label)
             VALUES ('oldhost', '/legacy', 1000, 50, 4096, 8192, 2, 1, 0, 0, '0.1.0', 'v1row');
             INSERT INTO entries VALUES (1, 0, NULL, 'legacy', 0, 4096, 8192, 1000, 1, 2, 1, 1, 1);
             INSERT INTO entries VALUES (1, 1, 0, 'a.bin', 1, 4096, 8192, 1000, 1, 1, 0, 0, 0);",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 1i64).unwrap();
    }

    // Opening with the current build migrates it in place.
    let store = Store::open(&path).unwrap();
    let scans = store.list().unwrap();
    assert_eq!(scans.len(), 1, "the v1 row must survive");

    let meta = &scans[0];
    assert_eq!(meta.host, "oldhost");
    assert_eq!(meta.root, "/legacy");
    assert_eq!(meta.started_at, 1000);
    assert_eq!(meta.total_size, 4096);
    assert_eq!(meta.label.as_deref(), Some("v1row"));
    assert!(
        meta.fs_total.is_none() && meta.fs_available.is_none(),
        "a v1 snapshot did not measure capacity; it must not claim to"
    );
    assert!(meta.fs_free_fraction().is_none());

    // The tree still loads.
    let (tree, _) = store.load(meta.id).unwrap();
    assert_eq!(tree.len(), 2);

    // And the file is now v2, so a second open is a no-op.
    let version: i64 = rusqlite::Connection::open(&path)
        .unwrap()
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, 2);
    assert_eq!(Store::open(&path).unwrap().list().unwrap().len(), 1);
}

/// After migrating, a new scan written into the same file records capacity.
#[test]
fn a_migrated_database_records_capacity_for_new_scans() {
    let dir = fixture();
    let (tree, stats) = scan_fixture(&dir);
    let work = tempfile::tempdir().unwrap();
    let path = work.path().join("db.sqlite");

    let mut store = Store::open(&path).unwrap();
    let id = store.save(&tree, &stats, "here", None).unwrap();
    let meta = store.scan(id).unwrap().unwrap();

    // The scan really ran on a real filesystem, so this must be populated.
    assert!(
        meta.fs_total.is_some(),
        "capacity should have been measured"
    );
    let fraction = meta.fs_free_fraction().expect("a fraction");
    assert!(
        (0.0..=1.0).contains(&fraction),
        "implausible fraction {fraction}"
    );
    assert!(meta.fs_available.unwrap() <= meta.fs_total.unwrap());
}

/// A newer database must be refused rather than read incorrectly.
#[test]
fn a_future_schema_is_refused() {
    let work = tempfile::tempdir().unwrap();
    let path = work.path().join("future.sqlite");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", 99i64).unwrap();
    }
    let err = match Store::open(&path) {
        Ok(_) => panic!("a v99 database must not open"),
        Err(err) => err.to_string(),
    };
    assert!(err.contains("newer spacetrace"), "{err}");
}

// ------------------------------------------------- concurrent open vs write

/// Opening a store while another connection is mid-write must not fail.
///
/// The agent and the hub open a connection per request, so a read can easily
/// land while a scan is saving. This reproduced a CI failure where the *writer*
/// was knocked over with SQLITE_BUSY: `Store::open` ran the migration on every
/// open, and both `PRAGMA journal_mode` and `CREATE TABLE IF NOT EXISTS` take a
/// write lock even when there is nothing to change.
#[test]
fn opening_a_store_while_another_writes_does_not_disturb_either() {
    let dir = fixture();
    let (tree, stats) = scan_fixture(&dir);
    let work = tempfile::tempdir().unwrap();
    let path = work.path().join("db.sqlite");

    // Create it once so the schema is already current.
    {
        let mut store = Store::open(&path).unwrap();
        store.save(&tree, &stats, "host", None).unwrap();
    }

    // Hold an open write transaction on one connection...
    let writer = rusqlite::Connection::open(&path).unwrap();
    writer
        .busy_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    writer
        .execute(
            "INSERT INTO scans (host, root, started_at, duration_ms, total_size, total_alloc,
                                files, dirs, errors, hardlinks_deduped, scanner_version, label)
             VALUES ('h2','/r2',1,1,1,1,1,1,0,0,'t',NULL)",
            [],
        )
        .unwrap();

    // ...and open the store for reading at the same time.
    let reader = Store::open(&path).expect("opening while a write is in flight must work");
    assert!(!reader.list().unwrap().is_empty());

    writer.execute_batch("COMMIT").unwrap();
    assert_eq!(Store::open(&path).unwrap().list().unwrap().len(), 2);
}

/// And the mirror image: a write must still succeed while readers keep opening.
#[test]
fn a_write_survives_readers_opening_the_store_repeatedly() {
    let dir = fixture();
    let (tree, stats) = scan_fixture(&dir);
    let work = tempfile::tempdir().unwrap();
    let path = work.path().join("db.sqlite");
    {
        let mut store = Store::open(&path).unwrap();
        store.save(&tree, &stats, "host", None).unwrap();
    }

    let reading = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let stop = std::sync::Arc::clone(&reading);
    let readers = {
        let path = path.clone();
        std::thread::spawn(move || {
            let mut opens = 0u32;
            while stop.load(std::sync::atomic::Ordering::Relaxed) {
                // This is exactly what the agent does per HTTP request.
                Store::open(&path)
                    .expect("a reader open must not fail")
                    .list()
                    .unwrap();
                opens += 1;
            }
            opens
        })
    };

    let mut store = Store::open(&path).unwrap();
    for _ in 0..8 {
        store
            .save(&tree, &stats, "writer", None)
            .expect("a save must not be knocked over by readers opening");
    }

    reading.store(false, std::sync::atomic::Ordering::Relaxed);
    let opens = readers.join().unwrap();
    assert!(
        opens > 0,
        "the reader thread should have opened at least once"
    );
}

/// The case CI actually hit: a brand-new database being opened by a reader
/// while a writer is creating and populating it.
///
/// This is the agent's normal shape — `POST /scans` starts a scan while the
/// client polls `GET /scans` — and on a fresh file both sides run the
/// migration. A writer using a DEFERRED transaction fails here with
/// SQLITE_BUSY no matter how long the busy timeout is, because SQLite does not
/// consult the busy handler for a deferred-to-write upgrade.
#[test]
fn a_writer_survives_readers_racing_it_on_a_brand_new_database() {
    let dir = fixture();
    let (tree, stats) = scan_fixture(&dir);
    let work = tempfile::tempdir().unwrap();
    let path = work.path().join("fresh.sqlite");

    let reading = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let stop = std::sync::Arc::clone(&reading);
    let readers = {
        let path = path.clone();
        std::thread::spawn(move || {
            while stop.load(std::sync::atomic::Ordering::Relaxed) {
                // The file may not exist yet on the first turns, which is
                // exactly what the polling client does.
                if let Ok(store) = Store::open(&path) {
                    let _ = store.list();
                }
            }
        })
    };

    let mut store = Store::open(&path).expect("the writer must be able to create it");
    for _ in 0..6 {
        store
            .save(&tree, &stats, "writer", None)
            .expect("a save must not be knocked over by a racing reader");
    }

    reading.store(false, std::sync::atomic::Ordering::Relaxed);
    readers.join().unwrap();
    assert_eq!(Store::open(&path).unwrap().list().unwrap().len(), 6);
}
