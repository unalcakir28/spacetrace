//! What a tree of N entries actually costs in memory, and what a walk costs
//! while it is building one.
//!
//! Kept in the repository rather than in a scratch directory because every
//! memory claim in TODO.md and ARCHITECTURE.md came out of it, and a number
//! nobody can reproduce is a number nobody can correct. It measured the 96
//! bytes/entry figure (D4) and the platform split behind it; B1-K is measured
//! against the same probe so the before and after are the same question.
//!
//! Three modes, because they answer three different questions:
//!
//! ```text
//!   memprobe <dirs> <files>        synthetic tree, built through TreeAssembler
//!   memprobe scan <root> [threads] one real walk: peak, holding, and the gap
//!   memprobe repeat <root> <n>     n walks in one process: does RSS settle?
//! ```
//!
//! The synthetic mode goes through `TreeAssembler`, which is the path
//! `store::load` uses — so it measures the structure itself and not a model of
//! it. It builds **one** size per run: a process that built three trees would
//! report the peak of the largest for all three.

use std::sync::Arc;

use spacetrace_scan_core::{EntryKind, ScanOptions, ScanProgress, StoredNode, Tree, TreeAssembler};

/// Peak resident set for this process so far, in bytes.
///
/// `ru_maxrss` is bytes on macOS and kilobytes on Linux. That difference is a
/// well-known trap and worth one branch rather than a factor-of-1024 error in
/// a number somebody is going to quote.
#[cfg(unix)]
fn peak_rss() -> u64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    let raw = usage.ru_maxrss as u64;
    if cfg!(target_os = "macos") {
        raw
    } else {
        raw * 1024
    }
}

/// Resident size *right now*, which peak cannot give once it has been high.
///
/// `mach_task_self` is deprecated in `libc` in favour of the `mach2` crate.
/// Taking a dependency for one call in a development example would be the
/// wrong trade in a project that hand-wrote a cron parser to avoid one, and
/// the deprecation is about where the binding lives rather than about the
/// call: the port it returns is the same `mach_task_self_` the kernel exports.
#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn current_rss() -> u64 {
    unsafe {
        let mut info: libc::mach_task_basic_info = std::mem::zeroed();
        let mut count = (std::mem::size_of::<libc::mach_task_basic_info>()
            / std::mem::size_of::<libc::integer_t>())
            as libc::mach_msg_type_number_t;
        let ok = libc::task_info(
            libc::mach_task_self(),
            libc::MACH_TASK_BASIC_INFO,
            &mut info as *mut _ as libc::task_info_t,
            &mut count,
        );
        if ok == 0 {
            info.resident_size
        } else {
            0
        }
    }
}

/// `/proc/self/statm`, second field: resident pages.
///
/// Read rather than stubbed. An earlier version of this probe returned zero
/// here and the run read as "flat on Linux", which is the most convincing kind
/// of wrong answer — the conclusion it supported happened to be true, and it
/// was luck.
#[cfg(all(unix, not(target_os = "macos")))]
fn current_rss() -> u64 {
    let text = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let pages: u64 = text
        .split_whitespace()
        .nth(1)
        .and_then(|field| field.parse().ok())
        .unwrap_or(0);
    pages * 4096
}

/// Windows has no equivalent this example needs, and `libc` is not a
/// dependency there. The example still has to *compile* for the msvc target,
/// because `cargo check --all-targets` builds examples and that check is the
/// only Windows type checking this project can do locally.
#[cfg(windows)]
fn peak_rss() -> u64 {
    0
}

#[cfg(windows)]
fn current_rss() -> u64 {
    0
}

fn mib(bytes: u64) -> String {
    format!("{:.1} MiB", bytes as f64 / 1_048_576.0)
}

/// A tree of `dirs` directories each holding `files` files, laid out so the
/// arena invariants hold: children contiguous, every child after its parent.
fn synthetic(dirs: usize, files: usize) -> Tree {
    let total = 1 + dirs + dirs * files;
    let mut asm = TreeAssembler::with_capacity(total);

    asm.push(StoredNode {
        parent: Tree::NO_PARENT,
        name: "root",
        kind: EntryKind::Dir,
        size: 0,
        alloc: 0,
        own_size: 0,
        own_alloc: 0,
        mtime: 1_700_000_000,
        nlink: 1,
        files: (dirs * files) as u32,
        dirs: dirs as u32,
        children_start: 1,
        children_len: dirs as u32,
    });

    for d in 0..dirs {
        // Names sized to what a real disk holds: 8.6 MB of names across 400k
        // entries when this was measured, so a little over 20 bytes each.
        let name = format!("directory-{d:0>12}");
        asm.push(StoredNode {
            parent: 0,
            name: &name,
            kind: EntryKind::Dir,
            size: 0,
            alloc: 0,
            own_size: 0,
            own_alloc: 4096,
            mtime: 1_700_000_000,
            nlink: 1,
            files: files as u32,
            dirs: 0,
            children_start: (1 + dirs + d * files) as u32,
            children_len: files as u32,
        });
    }

    for d in 0..dirs {
        for f in 0..files {
            let name = format!("file-{d:0>8}-{f:0>6}.bin");
            asm.push(StoredNode {
                parent: (1 + d) as u32,
                name: &name,
                kind: EntryKind::File,
                size: 4096,
                alloc: 4096,
                own_size: 4096,
                own_alloc: 4096,
                mtime: 1_700_000_000,
                nlink: 1,
                files: 1,
                dirs: 0,
                children_start: 0,
                children_len: 0,
            });
        }
    }

    asm.finish(std::path::PathBuf::from("/synthetic"))
        .expect("a valid arena")
}

fn walk(root: &str, threads: Option<usize>) -> (Tree, spacetrace_scan_core::ScanStats) {
    let options = ScanOptions {
        threads,
        ..Default::default()
    };
    spacetrace_scan_core::scan(
        std::path::Path::new(root),
        options,
        Arc::new(ScanProgress::default()),
    )
    .expect("the scan to finish")
}

/// One real walk: what it peaked at, what it settled at, and how much of that
/// is the tree rather than whatever the walk held on the way there.
fn scan_mode(root: &str, threads: Option<usize>) {
    let before = current_rss();
    let (tree, stats) = walk(root, threads);
    let holding = current_rss();
    let peak = peak_rss();

    println!(
        "{root}: {} entries ({} files, {} dirs)",
        tree.len(),
        stats.files,
        stats.dirs
    );
    println!("  duration     {} ms", stats.duration_ms);
    println!("  rss before   {}", mib(before));
    println!("  rss holding  {}", mib(holding));
    println!("  peak         {}", mib(peak));
    println!(
        "  peak/holding {:.2}x   peak per entry {:.0} B",
        peak as f64 / holding.max(1) as f64,
        peak as f64 / tree.len() as f64
    );

    // What the tree itself is, so the gap between it and RSS is visible rather
    // than assumed to be the tree.
    let nodes = tree.len() as u64 * std::mem::size_of::<spacetrace_scan_core::Node>() as u64;
    let names = tree.names().len() as u64;
    println!(
        "  tree itself  {} nodes + {} names = {} ({:.0} B/entry, Node = {} B)",
        mib(nodes),
        mib(names),
        mib(nodes + names),
        (nodes + names) as f64 / tree.len() as f64,
        std::mem::size_of::<spacetrace_scan_core::Node>(),
    );
    println!(
        "  not the tree {} ({:.0}% of peak)",
        mib(peak.saturating_sub(nodes + names)),
        peak.saturating_sub(nodes + names) as f64 / peak.max(1) as f64 * 100.0,
    );

    // One line a script can read without parsing prose. `bench-walk.sh` takes
    // the median of these across interleaved runs; the human-readable lines
    // above are for reading one run.
    println!("RESULT {} {} {}", tree.len(), stats.duration_ms, peak);

    // Does the allocator give the pages back once the tree is gone? If it
    // does, the gap is transient and a long-lived process recovers; if it does
    // not, the gap is what a scan of this size costs a machine for good.
    drop(tree);
    println!("  after dropping the tree {}", mib(current_rss()));
}

/// The question an agent asks: it scans on a schedule and lives for weeks. If
/// the pages the allocator kept get reused by the next scan, RSS settles; if
/// every scan adds to them, a NAS runs out of memory in a fortnight and the
/// tool looks like it leaks. Same question for the desktop's "Rescan".
fn repeat_mode(root: &str, rounds: usize) {
    for round in 1..=rounds {
        let (tree, _) = walk(root, None);
        let entries = tree.len();
        let holding = current_rss();
        drop(tree);
        println!(
            "  scan {round}: {entries} entries, rss holding {}, after dropping {}",
            mib(holding),
            mib(current_rss())
        );
    }
    println!("  peak over all rounds {}", mib(peak_rss()));
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("");

    match mode {
        "scan" => {
            let root = args.get(1).expect("usage: memprobe scan <root> [threads]");
            let threads = args.get(2).and_then(|t| t.parse().ok());
            scan_mode(root, threads);
        }
        "repeat" => {
            let root = args.get(1).expect("usage: memprobe repeat <root> [rounds]");
            let rounds = args.get(2).and_then(|r| r.parse().ok()).unwrap_or(5);
            repeat_mode(root, rounds);
        }
        _ => {
            let dirs: usize = mode
                .parse()
                .expect("usage: memprobe <dirs> <files> | scan <root> | repeat <root>");
            let files: usize = args
                .get(1)
                .and_then(|f| f.parse().ok())
                .expect("usage: memprobe <dirs> <files>");

            let baseline = current_rss();
            let tree = synthetic(dirs, files);
            let holding = current_rss();
            println!(
                "{} entries: holding {} ({:.0} B/entry), peak {}, baseline {}",
                tree.len(),
                mib(holding.saturating_sub(baseline)),
                holding.saturating_sub(baseline) as f64 / tree.len() as f64,
                mib(peak_rss()),
                mib(baseline),
            );
            std::hint::black_box(&tree);
        }
    }
}
