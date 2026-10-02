//! `spacetrace watch` end to end: the real binary, the platform's real event
//! backend (FSEvents, inotify, ReadDirectoryChangesW), real files.
//!
//! Everything here depends on events arriving, so nothing sleeps a fixed
//! time and hopes. Each test reads the JSON lines the command prints, one per
//! tick, and waits — generously — for the frame that shows the expected
//! state. A slow CI runner makes a test slower, not red. The accounting itself
//! is tested deterministically, without events, beside the model.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::Value;

/// How long a frame may take to show what was done. Far longer than any
/// backend needs; it only bounds how long a broken build takes to fail.
const PATIENCE: Duration = Duration::from_secs(60);

struct Watch {
    child: Child,
    frames: Receiver<Value>,
    seen: Vec<Value>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Watch {
    fn start(root: &Path, extra: &[&str]) -> Watch {
        let mut child = Command::new(env!("CARGO_BIN_EXE_spacetrace"))
            .arg("watch")
            .arg(root)
            .args(["--json", "--interval", "0.2", "--top", "100"])
            .args(extra)
            .env("SPACETRACE_HOME", root.join(".spacetrace-home-unused"))
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("the binary runs");
        let stdout = child.stdout.take().unwrap();
        let (tx, frames) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let frame: Value = serde_json::from_str(&line).expect("every line is JSON");
                if tx.send(frame).is_err() {
                    break;
                }
            }
        });
        let mut watch = Watch {
            child,
            frames,
            seen: Vec::new(),
        };
        // The first frame comes after the first scan, and with the watcher
        // already running: from here on every change is seen.
        watch.until("the first frame", |_| true);
        watch
    }

    /// The first frame that satisfies `ok`, waiting as long as it takes.
    fn until(&mut self, what: &str, ok: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.frames.recv_timeout(left) {
                Ok(frame) => {
                    self.seen.push(frame.clone());
                    if ok(&frame) {
                        return frame;
                    }
                }
                Err(_) => panic!(
                    "no frame showed {what} within {PATIENCE:?}; the last one was {}",
                    self.seen
                        .last()
                        .map_or("none".to_string(), |f| f.to_string())
                ),
            }
        }
    }
}

/// A root under the target directory rather than the system temp dir, which
/// on macOS sits behind a symlink and on some CI images on a filesystem whose
/// events are not delivered.
fn root() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let path = dir.path().canonicalize().unwrap();
    (dir, path)
}

fn write(path: &Path, bytes: usize) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, vec![1u8; bytes]).unwrap();
}

fn delta(frame: &Value) -> i64 {
    frame["delta"].as_i64().unwrap()
}

/// The row for `path`, as `(kind, delta)`.
fn row(frame: &Value, path: &str) -> Option<(String, i64)> {
    frame["changes"].as_array()?.iter().find_map(|c| {
        (c["path"] == path).then(|| {
            (
                c["kind"].as_str().unwrap().to_string(),
                c["delta"].as_i64().unwrap(),
            )
        })
    })
}

#[test]
fn a_file_that_grows_shows_up_under_its_folder() {
    let (_dir, root) = root();
    write(&root.join("var/log/app.log"), 1_000);
    write(&root.join("home/notes"), 10);
    let mut watch = Watch::start(&root, &[]);

    write(&root.join("var/log/app.log"), 301_000);

    let frame = watch.until("+300 000 under var/log", |f| {
        row(f, "var/log") == Some(("grown".into(), 300_000))
    });
    assert_eq!(delta(&frame), 300_000);
}

#[test]
fn a_deleted_file_shows_up_as_a_negative_change() {
    let (_dir, root) = root();
    write(&root.join("cache/blob"), 200_000);
    write(&root.join("cache/keep"), 10);
    let mut watch = Watch::start(&root, &[]);

    std::fs::remove_file(root.join("cache/blob")).unwrap();

    let frame = watch.until("-200 000 under cache", |f| {
        row(f, "cache") == Some(("shrunk".into(), -200_000))
    });
    assert_eq!(delta(&frame), -200_000);
}

#[test]
fn a_new_folder_full_of_files_is_picked_up_whole() {
    let (_dir, root) = root();
    write(&root.join("existing/f"), 10);
    let mut watch = Watch::start(&root, &[]);

    // Nested and filled in one go: most of this lands before anything can
    // be watching inside it, which is the case the whole-subtree scan is for.
    for i in 0..20 {
        write(&root.join(format!("build/out/obj{}/part.o", i % 4)), 10_000);
        write(&root.join(format!("build/out/{i}.bin")), 5_000);
    }

    let frame = watch.until("build/ added with all 140 000 bytes", |f| {
        row(f, "build") == Some(("added".into(), 4 * 10_000 + 20 * 5_000))
    });
    assert_eq!(delta(&frame), 140_000);
}

#[test]
fn a_file_renamed_across_folders_moves_its_bytes_and_not_the_total() {
    let (_dir, root) = root();
    write(&root.join("inbox/report.pdf"), 150_000);
    std::fs::create_dir_all(root.join("archive")).unwrap();
    let mut watch = Watch::start(&root, &[]);

    std::fs::rename(
        root.join("inbox/report.pdf"),
        root.join("archive/report.pdf"),
    )
    .unwrap();

    let frame = watch.until("the bytes leave inbox and arrive in archive", |f| {
        row(f, "inbox") == Some(("shrunk".into(), -150_000))
            && row(f, "archive") == Some(("grown".into(), 150_000))
    });
    assert_eq!(delta(&frame), 0, "a rename frees nothing and costs nothing");
}

/// Invariant 3, live: a second name for bytes already on disk adds nothing,
/// and no frame may say otherwise — not even briefly, before the full rescan
/// that counts it once.
#[test]
fn a_hardlink_to_an_existing_file_adds_nothing() {
    let (_dir, root) = root();
    write(&root.join("originals/disk.img"), 400_000);
    std::fs::create_dir_all(root.join("links")).unwrap();
    let mut watch = Watch::start(&root, &[]);

    std::fs::hard_link(root.join("originals/disk.img"), root.join("links/disk.img")).unwrap();
    // Something to wait for that is certain to come after the link.
    write(&root.join("marker/m"), 1_000);

    let frame = watch.until("the marker, after the link was handled", |f| {
        row(f, "marker").is_some() && f["pending_rescan"].is_null()
    });
    assert_eq!(delta(&frame), 1_000, "the link is not growth: {frame}");
    let wrong: Vec<_> = watch.seen.iter().filter(|f| delta(f) >= 400_000).collect();
    assert!(wrong.is_empty(), "a frame counted the link: {wrong:?}");
}

#[test]
fn an_excluded_folder_never_shows_its_growth() {
    let (_dir, root) = root();
    write(&root.join("app/node_modules/pkg/index.js"), 100);
    write(&root.join("app/src/main.js"), 100);
    let mut watch = Watch::start(&root, &["--exclude", "node_modules"]);

    write(&root.join("app/node_modules/pkg/huge.wasm"), 2_000_000);
    write(&root.join("app/node_modules/other/more.js"), 500_000);
    write(&root.join("app/src/marker"), 1_000);

    let frame = watch.until("the marker under app/src", |f| row(f, "app/src").is_some());
    assert_eq!(delta(&frame), 1_000, "only the marker counts: {frame}");
    // The writes went in first, so later frames had every chance to count them.
    let then = frame["elapsed_ms"].as_u64().unwrap();
    let later = watch.until("a second more of ticks", |f| {
        f["elapsed_ms"].as_u64().unwrap() >= then + 1_000
    });
    assert_eq!(delta(&later), 1_000, "{later}");
    assert!(
        !later.to_string().contains("node_modules"),
        "an excluded folder is never a row: {later}"
    );
}

#[test]
fn without_a_terminal_the_output_is_appended_lines() {
    let (_dir, root) = root();
    write(&root.join("a/f"), 10);
    let mut child = Command::new(env!("CARGO_BIN_EXE_spacetrace"))
        .arg("watch")
        .arg(&root)
        .args(["--interval", "0.2"])
        .env("SPACETRACE_HOME", root.join(".spacetrace-home-unused"))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let first = lines.next().unwrap().unwrap();
    let second = lines.next().unwrap().unwrap();
    let _ = child.kill();
    let _ = child.wait();

    assert!(first.starts_with("watching "), "{first}");
    assert!(
        !first.contains('\x1b'),
        "no escape codes into a pipe: {first:?}"
    );
    assert!(second.contains("total +0 B"), "{second}");
}
