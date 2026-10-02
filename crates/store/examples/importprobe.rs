//! How long `import_snapshot` keeps other writers out, and what it costs.
//!
//! ```text
//! cargo run --release -p spacetrace-store --example importprobe -- prepare wire.sqlite 1000000
//! /usr/bin/time -l cargo run ... -- import wire.sqlite receiver.sqlite   # macOS; -v on Linux
//! ```
//!
//! `prepare` writes a one-scan snapshot file of N entries, the shape the agent
//! receives in a push. `import` imports it while a second connection tries to
//! take the write lock every millisecond with no busy timeout, and reports the
//! longest stretch it could not: that is how long a scheduled save or a second
//! push would have waited. Run `import` in its own process so its peak memory
//! is the import's and not the preparation's.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use spacetrace_scan_core::{EntryKind, ImportedNode, ScanStats, Tree};
use spacetrace_store::Store;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("prepare") => prepare(&args[1], args[2].parse().expect("entry count")),
        Some("import") => import(&args[1], &args[2]),
        _ => eprintln!("usage: importprobe prepare <out> <entries> | import <wire> <receiver>"),
    }
}

fn prepare(out: &str, entries: usize) {
    const PER_DIR: usize = 1000;
    let dirs = (entries / PER_DIR).max(1);
    let file = |i: usize| ImportedNode {
        name: format!("file-{i:04}.dat"),
        kind: EntryKind::File,
        size: 4096,
        alloc: 4096,
        mtime: 1_790_000_000,
        nlink: 1,
        children: Vec::new(),
    };
    let dir = |d: usize, children: Vec<ImportedNode>| ImportedNode {
        name: format!("directory-{d:05}"),
        kind: EntryKind::Dir,
        size: 0,
        alloc: 4096,
        mtime: 1_790_000_000,
        nlink: 2,
        children,
    };
    let root = dir(
        0,
        (0..dirs)
            .map(|d| dir(d, (0..PER_DIR).map(file).collect()))
            .collect(),
    );
    let tree = Tree::from_nested(PathBuf::from("/srv/data"), root);
    let stats = ScanStats {
        files: (dirs * PER_DIR) as u64,
        dirs: dirs as u64 + 1,
        duration_ms: 1000,
        ..ScanStats::default()
    };

    let work = tempfile::tempdir().unwrap();
    let mut sender = Store::open(work.path().join("sender.sqlite")).unwrap();
    let id = sender.save(&tree, &stats, "nas", None).unwrap();
    sender.export_snapshot(id, &PathBuf::from(out)).unwrap();
    println!("wrote {} entries to {out}", tree.len());
}

fn import(wire: &str, receiver: &str) {
    let mut store = Store::open(receiver).unwrap();

    let done = Arc::new(AtomicBool::new(false));
    let probe = {
        let done = Arc::clone(&done);
        let path = receiver.to_string();
        std::thread::spawn(move || {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.busy_timeout(Duration::ZERO).unwrap();
            let mut longest = Duration::ZERO;
            let mut locked_since: Option<Instant> = None;
            while !done.load(Ordering::Relaxed) {
                match conn.execute_batch("BEGIN IMMEDIATE") {
                    Ok(()) => {
                        conn.execute_batch("ROLLBACK").unwrap();
                        if let Some(since) = locked_since.take() {
                            longest = longest.max(since.elapsed());
                        }
                    }
                    Err(_) => {
                        locked_since.get_or_insert_with(Instant::now);
                    }
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            if let Some(since) = locked_since {
                longest = longest.max(since.elapsed());
            }
            longest
        })
    };

    // Let the probe take its first turn before the import starts.
    std::thread::sleep(Duration::from_millis(50));
    let started = Instant::now();
    let imported = store.import_snapshot(&PathBuf::from(wire)).unwrap();
    let total = started.elapsed();
    std::thread::sleep(Duration::from_millis(50));
    done.store(true, Ordering::Relaxed);
    let longest = probe.join().unwrap();

    println!(
        "imported {imported:?} in {} ms; other writers locked out for at most {} ms",
        total.as_millis(),
        longest.as_millis()
    );
}
