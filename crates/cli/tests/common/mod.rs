//! What more than one test file needs.

use std::path::{Path, PathBuf};
use std::process::Command;

use spacetrace_store::{Integrity, Store};

pub const BIN: &str = env!("CARGO_BIN_EXE_spacetrace");

/// A snapshot database whose arena is broken and whose digest has been
/// recomputed to match — what a sender that wanted to plant one would send.
/// The digest is not authentication, so only the structural check can stop
/// this one; it has to do so before anything is committed.
pub fn forged_snapshot(at: &Path) -> PathBuf {
    let tree = at.join("forged-tree");
    std::fs::create_dir_all(tree.join("sub")).unwrap();
    std::fs::write(tree.join("sub/file"), b"payload").unwrap();
    let forged = at.join("forged.sqlite");
    let out = Command::new(BIN)
        .arg("--db")
        .arg(&forged)
        .args(["scan", "--save"])
        .arg(&tree)
        .env("SPACETRACE_NO_UPDATE_CHECK", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let conn = rusqlite::Connection::open(&forged).unwrap();
    // The root's children now start past the end of the arena.
    let changed = conn
        .execute("UPDATE entries SET children_start = 9999 WHERE id = 0", [])
        .unwrap();
    assert_eq!(changed, 1);
    drop(conn);

    let store = Store::open(&forged).unwrap();
    let id = store.list().unwrap()[0].id;
    if let Integrity::Mismatch { computed, .. } = store.verify(id).unwrap() {
        drop(store);
        let conn = rusqlite::Connection::open(&forged).unwrap();
        conn.execute("UPDATE scans SET content_hash = ?1", [computed])
            .unwrap();
    }
    let store = Store::open(&forged).unwrap();
    assert_eq!(
        store.verify(id).unwrap(),
        Integrity::Intact,
        "the forged digest must pass, or the test proves nothing"
    );
    assert!(
        store.load(id).is_err(),
        "and the arena must really be broken"
    );
    // Closing the last connection checkpoints the WAL into the main file, so
    // a plain copy of that one file carries all of it.
    drop(store);
    forged
}
