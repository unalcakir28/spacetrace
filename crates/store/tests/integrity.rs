//! A snapshot that changed on the way here must be refused, not believed.
//!
//! The structural check (`TreeAssembler::finish`) already stops a snapshot
//! that would panic or loop. These tests are about the other kind of damage:
//! a single value that changed and left a perfectly valid tree behind. That
//! one is worse, because it produces a number instead of an error.

use std::fs;
use std::sync::Arc;

use rusqlite::Connection;
use spacetrace_scan_core::{scan, ScanOptions, ScanProgress, Tree};
use spacetrace_store::{Integrity, ScanId, Store};

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

/// A store on disk holding one scan of `dir`, plus that scan's id.
fn stored(dir: &tempfile::TempDir, at: &std::path::Path) -> (Store, ScanId) {
    let (tree, stats) = scan_fixture(dir);
    let mut store = Store::open(at).unwrap();
    let id = store.save(&tree, &stats, "sender", Some("first")).unwrap();
    (store, id)
}

#[test]
fn a_saved_scan_matches_its_own_digest() {
    let dir = fixture();
    let work = tempfile::tempdir().unwrap();
    let (store, id) = stored(&dir, &work.path().join("db.sqlite"));

    assert_eq!(store.verify(id).unwrap(), Integrity::Intact);
}

#[test]
fn the_digest_survives_export_and_import() {
    let dir = fixture();
    let work = tempfile::tempdir().unwrap();
    let (sender, id) = stored(&dir, &work.path().join("sender.sqlite"));

    let wire = work.path().join("wire.sqlite");
    sender.export_snapshot(id, &wire).unwrap();

    let mut receiver = Store::open(work.path().join("receiver.sqlite")).unwrap();
    let imported = receiver.import_snapshot(&wire).unwrap();
    assert_eq!(imported.len(), 1);

    assert_eq!(
        receiver.verify(imported[0]).unwrap(),
        Integrity::Intact,
        "the digest describes the content, so re-packing must not disturb it"
    );
}

/// The point of the whole exercise: ids are reassigned on arrival, and a
/// digest that included the id would fail on every single import.
#[test]
fn the_digest_ignores_the_scan_id() {
    let dir = fixture();
    let work = tempfile::tempdir().unwrap();
    let (sender, id) = stored(&dir, &work.path().join("sender.sqlite"));

    // Give the receiver some history of its own, so the incoming scan cannot
    // land on the id it had at the sender.
    let other = fixture();
    let (other_tree, other_stats) = scan_fixture(&other);
    let mut receiver = Store::open(work.path().join("receiver.sqlite")).unwrap();
    for _ in 0..3 {
        receiver
            .save(&other_tree, &other_stats, "receiver", None)
            .unwrap();
    }

    let wire = work.path().join("wire.sqlite");
    sender.export_snapshot(id, &wire).unwrap();
    let imported = receiver.import_snapshot(&wire).unwrap();

    assert_ne!(imported[0], id, "the test is pointless if the id survived");
    assert_eq!(receiver.verify(imported[0]).unwrap(), Integrity::Intact);
}

/// One byte of one entry. The tree is still structurally perfect afterwards —
/// which is exactly why the structural check cannot see this.
#[test]
fn a_changed_entry_is_refused_on_import() {
    let dir = fixture();
    let work = tempfile::tempdir().unwrap();
    let (sender, id) = stored(&dir, &work.path().join("sender.sqlite"));

    let wire = work.path().join("wire.sqlite");
    sender.export_snapshot(id, &wire).unwrap();
    corrupt(
        &wire,
        "UPDATE entries SET size = size + 1 WHERE name = 'a.txt'",
    );

    let mut receiver = Store::open(work.path().join("receiver.sqlite")).unwrap();
    let refused = receiver.import_snapshot(&wire).unwrap_err();
    assert!(
        format!("{refused:#}").contains("does not match its digest"),
        "the error must say what went wrong: {refused:#}"
    );
    assert!(
        receiver.list().unwrap().is_empty(),
        "a refused import must leave nothing behind"
    );
}

/// The metadata row travels too, and a wrong total is just as damaging as a
/// wrong entry — more so, since it is the number people read first.
#[test]
fn a_changed_total_is_refused_on_import() {
    let dir = fixture();
    let work = tempfile::tempdir().unwrap();
    let (sender, id) = stored(&dir, &work.path().join("sender.sqlite"));

    let wire = work.path().join("wire.sqlite");
    sender.export_snapshot(id, &wire).unwrap();
    corrupt(&wire, "UPDATE scans SET total_size = total_size * 2");

    let mut receiver = Store::open(work.path().join("receiver.sqlite")).unwrap();
    assert!(receiver.import_snapshot(&wire).is_err());
    assert!(receiver.list().unwrap().is_empty());
}

/// Renaming a file changes nothing about its size, so a check that only
/// covered the numbers would pass this.
#[test]
fn a_changed_name_is_refused_on_import() {
    let dir = fixture();
    let work = tempfile::tempdir().unwrap();
    let (sender, id) = stored(&dir, &work.path().join("sender.sqlite"));

    let wire = work.path().join("wire.sqlite");
    sender.export_snapshot(id, &wire).unwrap();
    corrupt(
        &wire,
        "UPDATE entries SET name = 'b.txt' WHERE name = 'a.txt'",
    );

    let mut receiver = Store::open(work.path().join("receiver.sqlite")).unwrap();
    assert!(receiver.import_snapshot(&wire).is_err());
}

/// Dropping a row leaves a shorter stream, which is a *prefix* of the real
/// one. Prefixes are why the entry count is hashed as well as the entries.
#[test]
fn a_missing_entry_is_refused_on_import() {
    let dir = fixture();
    let work = tempfile::tempdir().unwrap();
    let (sender, id) = stored(&dir, &work.path().join("sender.sqlite"));

    let wire = work.path().join("wire.sqlite");
    sender.export_snapshot(id, &wire).unwrap();
    // The last entry, so no parent is left pointing at a hole and the tree
    // that arrives is still structurally valid.
    corrupt(
        &wire,
        "DELETE FROM entries WHERE id = (SELECT MAX(id) FROM entries)",
    );

    let mut receiver = Store::open(work.path().join("receiver.sqlite")).unwrap();
    assert!(receiver.import_snapshot(&wire).is_err());
}

/// Snapshots written before schema v3 have no digest. That is "nothing to
/// check", not "failed the check" — refusing them would make an upgrade throw
/// away every snapshot anyone already had.
#[test]
fn a_snapshot_without_a_digest_still_imports() {
    let dir = fixture();
    let work = tempfile::tempdir().unwrap();
    let (sender, id) = stored(&dir, &work.path().join("sender.sqlite"));

    let wire = work.path().join("wire.sqlite");
    sender.export_snapshot(id, &wire).unwrap();
    corrupt(&wire, "UPDATE scans SET content_hash = NULL");

    let mut receiver = Store::open(work.path().join("receiver.sqlite")).unwrap();
    let imported = receiver.import_snapshot(&wire).unwrap();
    assert_eq!(imported.len(), 1);
    assert_eq!(
        receiver.verify(imported[0]).unwrap(),
        Integrity::Unknown,
        "and it must keep saying it does not know, rather than claiming to be intact"
    );
}

/// Damage found before sending is the sender's problem. Shipping it anyway
/// would have the receiver blame the network for a database that was already
/// broken.
#[test]
fn a_damaged_scan_is_not_exported() {
    let dir = fixture();
    let work = tempfile::tempdir().unwrap();
    let db = work.path().join("sender.sqlite");
    let (sender, id) = stored(&dir, &db);
    drop(sender);

    corrupt(&db, "UPDATE entries SET alloc = alloc + 4096 WHERE id = 1");

    let sender = Store::open(&db).unwrap();
    assert!(matches!(
        sender.verify(id).unwrap(),
        Integrity::Mismatch { .. }
    ));

    let wire = work.path().join("wire.sqlite");
    let refused = sender.export_snapshot(id, &wire).unwrap_err();
    assert!(
        format!("{refused:#}").contains("does not match its own digest"),
        "{refused:#}"
    );
}

/// The `SELECT *` trap. `export_snapshot` copies the metadata row with
/// `INSERT INTO snap.scans SELECT * FROM main.scans`: `snap` is built by
/// `create_tables`, while `main` may have arrived at the same shape through
/// `ALTER TABLE ADD COLUMN`, which can only append. If the two ever list their
/// columns in a different order the copy silently writes each value into the
/// wrong column — so a database that came up through the migrations has to
/// make the same round trip as one created fresh.
#[test]
fn a_migrated_database_exports_with_its_columns_in_the_right_places() {
    let dir = fixture();
    let work = tempfile::tempdir().unwrap();
    let db = work.path().join("legacy.sqlite");
    write_v1_database(&db);

    // Opening migrates it: fs_total, fs_available and content_hash are all
    // appended here, in that order.
    let (tree, stats) = scan_fixture(&dir);
    let mut sender = Store::open(&db).unwrap();
    let id = sender.save(&tree, &stats, "sender", Some("after")).unwrap();

    let wire = work.path().join("wire.sqlite");
    sender.export_snapshot(id, &wire).unwrap();

    let mut receiver = Store::open(work.path().join("receiver.sqlite")).unwrap();
    let imported = receiver.import_snapshot(&wire).unwrap();
    let meta = receiver.scan(imported[0]).unwrap().unwrap();

    // Every field a shifted column would scramble.
    assert_eq!(meta.host, "sender");
    assert_eq!(meta.label.as_deref(), Some("after"));
    assert_eq!(meta.root, tree.root_path().to_string_lossy());
    assert_eq!(meta.scanner_version, spacetrace_store::SCANNER_VERSION);
    assert_eq!(
        receiver.verify(imported[0]).unwrap(),
        Integrity::Intact,
        "and the digest would not survive a shift either"
    );
}

/// A characterisation test, and deliberately brittle.
///
/// Changing the encoding invalidates every digest already written — every
/// snapshot in the wild starts reading as corrupt. That is allowed, but it has
/// to be a decision: bump `FORMAT` in digest.rs and change this number in the
/// same commit. A test that merely recomputed both sides would let the change
/// through in silence.
///
/// The digest is read out of the `Mismatch` this produces, since a wrong
/// stored value is the only way to see the computed one from outside.
#[test]
fn the_encoding_is_pinned() {
    let work = tempfile::tempdir().unwrap();
    let db = work.path().join("fixed.sqlite");
    // Every field fixed by hand: a scan of a real directory would fold in a
    // temporary path and a duration, and pin nothing.
    let store = Store::open(&db).unwrap();
    drop(store);
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch(
        "INSERT INTO scans (id, host, root, started_at, duration_ms, total_size, total_alloc,
                            files, dirs, errors, hardlinks_deduped, scanner_version, label,
                            fs_total, fs_available, content_hash)
         VALUES (1, 'pinned', '/fixed', 1000, 50, 5096, 12288, 2, 1, 0, 0, '0.0.0', NULL,
                 NULL, NULL, 'not the right digest');
         INSERT INTO entries VALUES (1, 0, NULL, 'fixed', 0, 5096, 12288, 1000, 1, 2, 1, 1, 2);
         INSERT INTO entries VALUES (1, 1, 0, 'a.txt', 1, 1000, 4096, 1000, 1, 1, 0, 0, 0);
         INSERT INTO entries VALUES (1, 2, 0, 'b.bin', 1, 4096, 8192, 1000, 1, 1, 0, 0, 0);",
    )
    .unwrap();
    drop(conn);

    let store = Store::open(&db).unwrap();
    let Integrity::Mismatch { computed, .. } = store.verify(1).unwrap() else {
        panic!("the stored digest was deliberately wrong, so this must not match");
    };
    assert_eq!(
        computed, "85f9b7eed329d35929214dda7dcc041394d8ab2497907f24e039c8e744d541fd",
        "the encoding changed; bump FORMAT in digest.rs if that was intended"
    );
}

/// The digest proves the body is what the sender wrote, not that the sender
/// wrote a tree. Somebody who can change the body can recompute the digest, so
/// a broken arena with a correct digest has to be stopped by the structural
/// check — before the import commits, or the receiving database holds a
/// snapshot that every later read of it fails on.
#[test]
fn a_broken_arena_with_a_recomputed_digest_imports_nothing() {
    let dir = fixture();
    let work = tempfile::tempdir().unwrap();
    let (sender, id) = stored(&dir, &work.path().join("sender.sqlite"));
    let wire = work.path().join("wire.sqlite");
    sender.export_snapshot(id, &wire).unwrap();

    // The root's children now start past the end of the arena.
    corrupt(
        &wire,
        "UPDATE entries SET children_start = 9999 WHERE id = 0",
    );
    reseal(&wire, id);
    assert_eq!(
        Store::open(&wire).unwrap().verify(id).unwrap(),
        Integrity::Intact,
        "the forged digest must pass, or this test proves nothing"
    );

    // A receiver that already holds something, so "unchanged" is a claim
    // about real content and not about an empty file.
    // Another host, so the import is not skipped as a duplicate of it.
    let mut receiver = Store::open(work.path().join("receiver.sqlite")).unwrap();
    let (tree, stats) = scan_fixture(&dir);
    let existing = receiver.save(&tree, &stats, "receiver", None).unwrap();
    let before = receiver.list().unwrap();

    let refused = receiver.import_snapshot(&wire).unwrap_err();
    assert!(
        format!("{refused:#}").contains("not a usable tree"),
        "the error must say what went wrong: {refused:#}"
    );
    let after = receiver.list().unwrap();
    assert_eq!(after.len(), before.len());
    assert_eq!(after[0].id, existing);
    receiver.load(existing).expect("what was there still loads");
}

/// The entry count in a snapshot is a memory hint read from a file that may
/// have crossed a network. An absurd one must not abort the process.
#[test]
fn an_absurd_entry_count_is_only_a_hint() {
    let dir = fixture();
    let work = tempfile::tempdir().unwrap();
    let path = work.path().join("db.sqlite");
    let (store, id) = stored(&dir, &path);
    drop(store);
    corrupt(&path, "UPDATE scans SET files = 1099511627776");

    let (tree, meta) = Store::open(&path).unwrap().load(id).unwrap();
    assert_eq!(meta.files, 1 << 40);
    assert_eq!(tree.total_size(), 1000 + 4096);
}

/// The sender writes the file and its DDL. A copy of `wire` in tables with no
/// key and no column affinity, the way a forger is free to write it; no
/// digest, so the snapshot passes as one from before digests existed and only
/// the other checks stand between it and the database.
fn untyped_copy(wire: &std::path::Path, at: &std::path::Path) {
    let conn = Connection::open(at).unwrap();
    conn.execute("ATTACH DATABASE ?1 AS src", [wire.to_string_lossy()])
        .unwrap();
    conn.execute_batch(
        "CREATE TABLE scans (id, host, root, started_at, duration_ms, total_size,
             total_alloc, files, dirs, errors, hardlinks_deduped, scanner_version,
             label, fs_total, fs_available, content_hash);
         INSERT INTO scans SELECT * FROM src.scans;
         UPDATE scans SET content_hash = NULL;
         CREATE TABLE entries (scan_id, id, parent_id, name, kind, size, alloc, mtime,
             nlink, files, dirs, children_start, children_len);
         INSERT INTO entries SELECT * FROM src.entries;
         DETACH DATABASE src;",
    )
    .unwrap();
}

/// A receiver holding one scan of its own, and what it lists.
fn receiver_with_a_scan(
    dir: &tempfile::TempDir,
    at: &std::path::Path,
) -> (Store, Vec<spacetrace_store::ScanMeta>) {
    let mut receiver = Store::open(at).unwrap();
    let (tree, stats) = scan_fixture(dir);
    receiver.save(&tree, &stats, "receiver", None).unwrap();
    let listed = receiver.list().unwrap();
    (receiver, listed)
}

fn same_listing(a: &[spacetrace_store::ScanMeta], b: &[spacetrace_store::ScanMeta]) {
    let ids = |l: &[spacetrace_store::ScanMeta]| l.iter().map(|m| m.id).collect::<Vec<_>>();
    assert_eq!(ids(a), ids(b), "the receiver must hold exactly what it did");
}

/// Two scan rows sharing an id. Each matched the `WHERE id = ?` that copied
/// one of them, so both were inserted, the entries went to the second, and
/// the first stayed behind with none — a snapshot `load` refuses forever.
#[test]
fn two_incoming_scans_with_one_id_import_nothing() {
    let dir = fixture();
    let work = tempfile::tempdir().unwrap();
    let (sender, id) = stored(&dir, &work.path().join("sender.sqlite"));
    let wire = work.path().join("wire.sqlite");
    sender.export_snapshot(id, &wire).unwrap();
    let forged = work.path().join("forged.sqlite");
    untyped_copy(&wire, &forged);
    corrupt(&forged, "INSERT INTO scans SELECT * FROM scans");

    let (mut receiver, before) = receiver_with_a_scan(&dir, &work.path().join("r.sqlite"));
    let refused = receiver.import_snapshot(&forged).unwrap_err();
    assert!(
        format!("{refused:#}").contains("share an id"),
        "{refused:#}"
    );
    same_listing(&receiver.list().unwrap(), &before);
    for meta in receiver.list().unwrap() {
        receiver
            .load(meta.id)
            .expect("everything stored still loads");
    }
}

/// An entry id stored as text. In the sender's untyped column `'-1'` sorts
/// after every integer, so the tree checked there is valid; `main` stores it
/// as the integer -1, which sorts first, and `load` then finds no root. What
/// was checked has to be what is stored.
#[test]
fn an_entry_id_stored_as_text_imports_nothing() {
    let dir = fixture();
    let work = tempfile::tempdir().unwrap();
    let (sender, id) = stored(&dir, &work.path().join("sender.sqlite"));
    let wire = work.path().join("wire.sqlite");
    sender.export_snapshot(id, &wire).unwrap();
    let forged = work.path().join("forged.sqlite");
    untyped_copy(&wire, &forged);
    corrupt(
        &forged,
        "UPDATE entries SET id = '-1' WHERE id = (SELECT max(id) FROM entries)",
    );

    let (mut receiver, before) = receiver_with_a_scan(&dir, &work.path().join("r.sqlite"));
    let refused = receiver.import_snapshot(&forged).unwrap_err();
    assert!(
        format!("{refused:#}").contains("entries.id holds a value of the wrong type"),
        "{refused:#}"
    );
    same_listing(&receiver.list().unwrap(), &before);
}

/// Untyped and keyless is not wrong in itself: a file with honest values in
/// such tables imports, and loads as the tree it was. The checks are about
/// what the values are, not how the sender declared its columns.
#[test]
fn an_untyped_but_honest_snapshot_still_imports() {
    let dir = fixture();
    let work = tempfile::tempdir().unwrap();
    let (sender, id) = stored(&dir, &work.path().join("sender.sqlite"));
    let wire = work.path().join("wire.sqlite");
    sender.export_snapshot(id, &wire).unwrap();
    let plain = work.path().join("plain.sqlite");
    untyped_copy(&wire, &plain);

    let mut receiver = Store::open(work.path().join("r.sqlite")).unwrap();
    let imported = receiver.import_snapshot(&plain).unwrap();
    assert_eq!(imported.len(), 1);
    let (tree, _) = receiver.load(imported[0]).unwrap();
    assert_eq!(tree.total_size(), 1000 + 4096);
}

/// A scan dated years ahead would be the newest of its target for good:
/// every diff, every retention pass and the agent's sizing of the next scan
/// would go by it. A sender's clock wrong by less than a day is ordinary
/// misconfiguration and still imports.
#[test]
fn a_snapshot_dated_far_ahead_is_refused_and_a_day_of_skew_is_not() {
    let dir = fixture();
    let work = tempfile::tempdir().unwrap();
    let (sender, id) = stored(&dir, &work.path().join("sender.sqlite"));
    drop(sender);

    for (ahead_hours, accepted) in [(14, true), (24 * 365 * 10, false)] {
        let wire = work.path().join(format!("wire-{ahead_hours}.sqlite"));
        Store::open(work.path().join("sender.sqlite"))
            .unwrap()
            .export_snapshot(id, &wire)
            .unwrap();
        corrupt(
            &wire,
            &format!(
                "UPDATE scans SET started_at = started_at + {}",
                ahead_hours * 3600
            ),
        );
        reseal(&wire, id);

        let (mut receiver, before) =
            receiver_with_a_scan(&dir, &work.path().join(format!("r-{ahead_hours}.sqlite")));
        let result = receiver.import_snapshot(&wire);
        if accepted {
            assert_eq!(result.unwrap().len(), 1, "{ahead_hours} h ahead");
            continue;
        }
        let refused = result.unwrap_err();
        assert!(
            format!("{refused:#}").contains("in the future"),
            "{refused:#}"
        );
        same_listing(&receiver.list().unwrap(), &before);
    }
}

/// The checks that read the whole snapshot run before the write lock, so a
/// receiver busy writing is not made to wait for them, and a bad snapshot is
/// refused at once rather than after the 30-second busy timeout. Before, this
/// sat out the timeout and failed as "database is locked" — an error the
/// agent returns to the sender as its own mistake.
#[test]
fn a_bad_snapshot_is_refused_without_waiting_for_the_write_lock() {
    let dir = fixture();
    let work = tempfile::tempdir().unwrap();
    let (sender, id) = stored(&dir, &work.path().join("sender.sqlite"));
    let wire = work.path().join("wire.sqlite");
    sender.export_snapshot(id, &wire).unwrap();
    corrupt(
        &wire,
        "UPDATE entries SET size = size + 1 WHERE name = 'a.txt'",
    );

    let receiver_path = work.path().join("r.sqlite");
    let mut receiver = Store::open(&receiver_path).unwrap();
    // Another writer, mid-transaction.
    let writer = Connection::open(&receiver_path).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();

    let started = std::time::Instant::now();
    let refused = receiver.import_snapshot(&wire).unwrap_err();
    assert!(
        format!("{refused:#}").contains("does not match its digest"),
        "{refused:#}"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "took {:?}: the check waited for the lock",
        started.elapsed()
    );
    writer.execute_batch("ROLLBACK").unwrap();
}

/// Write the digest the current content hashes to, as a forger would.
fn reseal(path: &std::path::Path, id: ScanId) {
    let store = Store::open(path).unwrap();
    let Integrity::Mismatch { computed, .. } = store.verify(id).unwrap() else {
        return;
    };
    drop(store);
    corrupt(
        path,
        &format!("UPDATE scans SET content_hash = '{computed}' WHERE id = {id}"),
    );
}

/// Reach past the API to damage a snapshot the way a bad cable would.
fn corrupt(path: &std::path::Path, sql: &str) {
    let conn = Connection::open(path).unwrap();
    let changed = conn.execute(sql, []).unwrap();
    assert!(changed > 0, "the corruption did not apply: {sql}");
}

/// A database as an early build wrote it: no capacity columns, no digest.
fn write_v1_database(path: &std::path::Path) {
    let conn = Connection::open(path).unwrap();
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
    conn.pragma_update(None, "user_version", 1i64).unwrap();
}
