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

/// The watch's own check, a full rescan, runs at most once a minute
/// (`VERIFY_FLOOR`); a frame from before then shows what the events alone
/// found.
const BEFORE_THE_CHECK: Duration = Duration::from_secs(50);

impl Watch {
    /// [`Watch::until`], for a change the events must find on their own. The
    /// periodic full rescan would put a missed one right within a minute,
    /// inside [`PATIENCE`], and a test that waits that long proves nothing.
    fn until_seen_by_events(&mut self, what: &str, ok: impl Fn(&Value) -> bool) -> Value {
        let frame = self.until(what, ok);
        let at = Duration::from_millis(frame["elapsed_ms"].as_u64().unwrap());
        assert!(
            at < BEFORE_THE_CHECK,
            "{what} showed only after {at:?}, which is the periodic rescan's doing, not an event's"
        );
        frame
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

/// Logical bytes under `root`, each file once however many names it has —
/// what a scan counts (invariant 3), read here without the scanner.
#[cfg(unix)]
fn disk_total(root: &Path) -> i64 {
    use std::os::unix::fs::MetadataExt;
    let mut seen = std::collections::HashSet::new();
    let mut total = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let md = entry.metadata().unwrap();
            if md.is_dir() {
                stack.push(entry.path());
            } else if seen.insert((md.dev(), md.ino())) {
                total += md.len() as i64;
            }
        }
    }
    total
}

/// A build tree is where hardlinks are made by the hundred: cargo links each
/// object file into an incremental session folder and the binary to its
/// uplifted name. Each of those once sent the watch into a full rescan; now
/// every frame keeps up through listings alone, and the total is the disk's,
/// each file once.
#[cfg(unix)]
#[test]
fn a_build_tree_full_of_hardlinks_is_followed_without_a_full_rescan() {
    let (_dir, root) = root();
    let debug = root.join("proj/target/debug");
    let deps = debug.join("deps");
    let build = |n: u32, size: usize| {
        let session = debug.join(format!("incremental/app/s-{n}"));
        std::fs::create_dir_all(&session).unwrap();
        for name in ["app-h", "app-h.a.o", "app-h.b.o"] {
            let _ = std::fs::remove_file(deps.join(name));
            write(&deps.join(name), size + name.len());
        }
        for (from, to) in [("app-h.a.o", "a.o"), ("app-h.b.o", "b.o")] {
            std::fs::hard_link(deps.join(from), session.join(to)).unwrap();
        }
        if n > 1 {
            let old = debug.join(format!("incremental/app/s-{}", n - 1));
            std::fs::remove_dir_all(old).unwrap();
        }
        let _ = std::fs::remove_file(debug.join("app"));
        std::fs::hard_link(deps.join("app-h"), debug.join("app")).unwrap();
    };
    write(&root.join("proj/src/main.rs"), 1_000);
    build(1, 100_000);
    let start = disk_total(&root);
    let mut watch = Watch::start(&root, &[]);

    for (n, size) in [(2, 300_000), (3, 50_000), (4, 700_000)] {
        build(n, size);
        let expected = disk_total(&root) - start;
        watch.until_seen_by_events(&format!("build {n}, {expected:+} bytes"), |f| {
            delta(f) == expected
        });
    }
    let rescans: Vec<_> = watch
        .seen
        .iter()
        .filter(|f| {
            !f["pending_rescan"].is_null()
                || f["notes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|n| n.as_str().unwrap().starts_with("rescanned everything"))
        })
        .collect();
    assert!(
        rescans.is_empty(),
        "a full rescan was asked for: {rescans:?}"
    );
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

/// A folder removed and made again under the same name is a new folder. On
/// inotify its watch, and every watch below it, went with the old one, so
/// unless the watch notices, nothing written into it is ever seen again.
#[test]
fn a_folder_deleted_and_made_again_is_still_watched() {
    let (_dir, root) = root();
    write(&root.join("build/old.o"), 1_000);
    write(&root.join("build/sub/old.o"), 1_000);
    let mut watch = Watch::start(&root, &[]);

    std::fs::remove_dir_all(root.join("build")).unwrap();
    std::fs::create_dir(root.join("build")).unwrap();
    // Wait until the recreated folder has been taken in before writing, so
    // the write cannot ride on the same tick as the recreate.
    watch.until("build/ emptied", |f| delta(f) == -2_000);
    write(&root.join("build/new.bin"), 5_000_000);
    write(&root.join("build/sub2/deeper.bin"), 1_000_000);

    let frame = watch.until_seen_by_events("the writes into the new build/", |f| {
        delta(f) == 6_000_000 - 2_000
    });
    assert_eq!(
        row(&frame, "build"),
        Some(("grown".into(), 6_000_000 - 2_000))
    );
    assert_eq!(frame["unwatched"], 0);
}

/// Renamed away and straight back: the same folder, the same inode, and on
/// inotify no watch any more.
#[test]
fn a_folder_renamed_away_and_back_is_still_watched() {
    let (_dir, root) = root();
    write(&root.join("d/f"), 10);
    write(&root.join("d/inner/f"), 10);
    let mut watch = Watch::start(&root, &[]);

    std::fs::rename(root.join("d"), root.join("d2")).unwrap();
    std::fs::rename(root.join("d2"), root.join("d")).unwrap();
    // A marker elsewhere, so the renames are known to have been handled.
    write(&root.join("marker/m"), 1);
    watch.until("the marker", |f| row(f, "marker").is_some());
    write(&root.join("d/big"), 4_000_000);
    write(&root.join("d/inner/big"), 1_000_000);

    let frame = watch.until_seen_by_events("5 000 000 more under d", |f| {
        row(f, "d") == Some(("grown".into(), 5_000_000))
    });
    assert_eq!(delta(&frame), 5_000_001);
}

/// A folder replaced by another of the same name — what `npm install` and
/// most deploys do — holds what the new one holds, not what the old one did.
#[test]
fn a_folder_swapped_for_another_of_the_same_name_shows_the_new_contents() {
    let (_dir, root) = root();
    let (_staging, staging) = self::root();
    write(&root.join("app/v1.bin"), 2_000_000);
    write(&staging.join("new/v2.bin"), 6_000_000);
    write(&staging.join("new/lib/v2.so"), 1_000_000);
    let mut watch = Watch::start(&root, &[]);

    std::fs::rename(root.join("app"), root.join("app.old")).unwrap();
    std::fs::rename(staging.join("new"), root.join("app")).unwrap();

    let frame = watch.until_seen_by_events("the new app/ and the old one beside it", |f| {
        delta(f) == 7_000_000
    });
    assert_eq!(row(&frame, "app"), Some(("grown".into(), 5_000_000)));
    assert_eq!(row(&frame, "app.old"), Some(("added".into(), 2_000_000)));
}

/// Names that are not UTF-8 are legal on Linux, and the bytes in them count.
#[cfg(target_os = "linux")]
#[test]
fn folders_whose_names_are_not_utf8_are_watched_like_any_other() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let (_dir, root) = root();
    let odd = root.join(OsStr::from_bytes(b"\xffx"));
    write(&odd.join("f"), 10);
    let mut watch = Watch::start(&root, &[]);

    write(&odd.join("grows"), 3_000_000);
    write(
        &root.join(OsStr::from_bytes(b"\xfenew")).join("f"),
        2_000_000,
    );

    let frame =
        watch.until_seen_by_events("both non-UTF-8 folders counted", |f| delta(f) == 5_000_000);
    assert_eq!(frame["unwatched"], 0);
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
