//! The binary against a reader that stops reading, and `pkgs` against a
//! snapshot it cannot vouch for — both through the real executable, since
//! what is being tested is what a shell sees.

use std::path::Path;
use std::process::{Command, Output, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_spacetrace");

fn command(db: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(BIN);
    command
        .arg("--db")
        .arg(db)
        .args(args)
        .env("SPACETRACE_NO_UPDATE_CHECK", "1");
    command
}

/// Run with stdout a pipe whose reading end is already closed — `| head -1`
/// at its most abrupt, and without the race of hoping `head` exits before the
/// output is written.
fn with_closed_stdout(db: &Path, args: &[&str]) -> Output {
    let mut child = command(db, args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    child.wait_with_output().unwrap()
}

fn assert_quiet_exit(out: &Output, what: &str) {
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "{what}: {:?}, stderr: {stderr}",
        out.status
    );
    assert!(stderr.is_empty(), "{what} said: {stderr}");
}

/// Every command used to end in "failed printing to stdout: Broken pipe" and
/// a panic exit of 101 when its reader went away. A reader that stopped had
/// what it wanted, so this is a clean exit, 0, and nothing on stderr.
#[test]
fn a_closed_stdout_ends_a_command_quietly() {
    let work = tempfile::tempdir().unwrap();
    let db = work.path().join("db.sqlite");
    let tree = work.path().join("tree");
    for i in 0..50 {
        std::fs::create_dir_all(tree.join(format!("d{i}"))).unwrap();
        std::fs::write(tree.join(format!("d{i}/f")), vec![b'x'; i * 10]).unwrap();
    }
    let saved = command(&db, &["scan", "--save"])
        .arg(&tree)
        .output()
        .unwrap();
    assert!(saved.status.success());

    let tree_arg = tree.to_string_lossy();
    for args in [
        vec!["scans"],
        vec!["ls", &tree_arg, "--top", "50"],
        vec!["export", "--scan", "1"],
        vec!["export", "--scan", "1", "--format", "csv"],
    ] {
        let out = with_closed_stdout(&db, &args);
        assert_quiet_exit(&out, &args.join(" "));
    }
}

/// The same through a shell, as a user types it.
///
/// Unix only: `pipefail` is a POSIX-shell question, and the Windows runner
/// would answer it through Git Bash, an emulation layer between the binary
/// and the pipe. There it ended in exit 1 with nothing on stderr (CI,
/// 5 October 2026) while the closed-pipe test above, which talks to the
/// binary directly, passed — so Windows' own answer is the one above.
#[cfg(unix)]
#[test]
fn piping_into_head_exits_zero_even_under_pipefail() {
    let work = tempfile::tempdir().unwrap();
    let db = work.path().join("db.sqlite");
    let tree = work.path().join("tree");
    for i in 0..2000 {
        std::fs::create_dir_all(tree.join(format!("{i:04}"))).unwrap();
    }
    let script = format!(
        "set -o pipefail 2>/dev/null; SPACETRACE_NO_UPDATE_CHECK=1 '{BIN}' --db '{}' ls '{}' \
         --top 2000 | head -1 >/dev/null",
        db.display(),
        tree.display()
    );
    let out = Command::new("bash").args(["-c", &script]).output().unwrap();
    assert_quiet_exit(&out, "ls | head -1");
}

/// `import` files an export under this machine unless told otherwise, so the
/// host name said nothing and `pkgs --scan` accepted another machine's
/// export. Refused before any package database is read — which is also why
/// this runs on a machine that has none.
#[test]
fn pkgs_refuses_an_imported_snapshot_filed_under_this_host() {
    let work = tempfile::tempdir().unwrap();
    let db = work.path().join("db.sqlite");
    let export = work.path().join("other.json");
    std::fs::write(
        &export,
        r#"[1,2,{"progname":"ncdu","progver":"2.4","timestamp":1700000000},
           [{"name":"/srv"},{"name":"data.bin","asize":4096,"dsize":4096}]]"#,
    )
    .unwrap();
    let imported = command(&db, &["import"]).arg(&export).output().unwrap();
    assert!(
        imported.status.success(),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );

    let out = command(&db, &["pkgs", "--scan", "1"]).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("imported from an export"), "{stderr}");
}
