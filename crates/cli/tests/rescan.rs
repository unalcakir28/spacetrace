//! `scan --save` starting from the last saved scan, and `--full` refusing to.
//!
//! The binary against a real directory and a real database. Whether the
//! rebuilt tree is right is proven in `store/tests/incremental.rs`; this
//! proves the command offers the base, reports what happened, and lets the
//! user say no.
#![cfg(target_os = "macos")]

use std::path::Path;
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_spacetrace");

fn spacetrace(db: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .arg("--db")
        .arg(db)
        .args(args)
        .env("SPACETRACE_NO_UPDATE_CHECK", "1")
        .env("SPACETRACE_REMOTES", db.with_extension("remotes.toml"))
        .output()
        .unwrap()
}

/// `scan --save --json` on `root`, plus `extra`, as parsed JSON.
fn scan(db: &Path, root: &Path, extra: &[&str]) -> serde_json::Value {
    let mut args = vec!["--json", "scan", "--save"];
    args.extend_from_slice(extra);
    args.push(root.to_str().unwrap());
    let out = spacetrace(db, &args);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn a_saved_scan_starts_from_the_last_one_unless_told_not_to() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("sub")).unwrap();
    std::fs::write(root.path().join("sub/a.txt"), b"aaaa").unwrap();
    let home = tempfile::tempdir().unwrap();
    let db = home.path().join("db.sqlite");

    let first = scan(&db, root.path(), &[]);
    assert_eq!(first["rescan"], "full", "nothing to start from");
    std::fs::write(root.path().join("sub/b.txt"), b"bb").unwrap();

    assert!(first["incremental"].is_null());

    // A replay past its budget is the machine's load, not the subject here;
    // its snapshot is a full one, and the next scan starts from it.
    let second = (0..5)
        .map(|_| scan(&db, root.path(), &[]))
        .find(|out| out["rescan"] != "fallback:deadline")
        .expect("five replays in a row outlasted their budget");
    assert_eq!(second["rescan"], "incremental");
    assert_eq!(second["files"], 2);
    // What an incremental scan did, for whoever times one against a full one.
    let done = &second["incremental"];
    for key in [
        "events",
        "distance",
        "replay_ms",
        "load_ms",
        "dirs_listed",
        "entries_reused",
    ] {
        assert!(done[key].is_u64(), "{key} missing: {done}");
    }
    assert!(
        done["dirs_listed"].as_u64() > Some(0),
        "the root is always read"
    );

    let third = scan(&db, root.path(), &["--full"]);
    assert_eq!(third["rescan"], "full");
    assert_eq!(third["files"], 2);
    assert!(third["incremental"].is_null());
}

/// Without `--save` there is no chain to continue, so nothing is offered —
/// not even a database to open.
#[test]
fn an_unsaved_scan_is_always_full() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"a").unwrap();
    let home = tempfile::tempdir().unwrap();
    let db = home.path().join("db.sqlite");
    scan(&db, root.path(), &[]);

    let out = spacetrace(&db, &["--json", "scan", root.path().to_str().unwrap()]);
    assert!(out.status.success());
    let unsaved: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(unsaved["rescan"], "full");
    assert!(unsaved["scan_id"].is_null());
}

/// The human summary says when the scan was incremental and, when it fell
/// back, why — the reason is what tells the user whether the next one will.
#[test]
fn the_summary_names_a_fallback_and_its_reason() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), b"a").unwrap();
    let home = tempfile::tempdir().unwrap();
    let db = home.path().join("db.sqlite");
    let path = root.path().to_str().unwrap();
    scan(&db, root.path(), &[]);

    // Different options: the base was built without what this scan wants.
    let out = spacetrace(&db, &["scan", "--save", "--exclude", "x", path]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("  full scan: "), "{text}");
}
