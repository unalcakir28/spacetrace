mod args;
mod fmt;
mod remote;

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use spacetrace_diff::{diff, ChangeKind, DiffOptions, DiffReport};
use spacetrace_scan_core::{
    scan, EntryKind, ScanOptions, ScanProgress, ScanStats, SizeBasis, Tree,
};
use spacetrace_store::{export_ncdu, ScanMeta, Store};

use crate::args::{
    parse_size, Cli, Command, DiffArgs, ExportArgs, LsArgs, PruneArgs, PullArgs, RmArgs, ScanArgs,
};
use crate::remote::Remote;

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let db_path = cli.db.clone().map(Ok).unwrap_or_else(default_db_path)?;

    let remote = match &cli.remote {
        Some(target) => Some(Remote::resolve(target, cli.token.as_deref())?),
        None => None,
    };

    // Commands that write to the local disk or the local database have no
    // remote meaning; saying so beats silently ignoring the flag.
    if let Some(r) = &remote {
        let local_only = match &cli.command {
            Command::Scan(_) => Some("scan"),
            Command::Prune(_) => Some("prune"),
            Command::Rm(_) => Some("rm"),
            _ => None,
        };
        if let Some(name) = local_only {
            anyhow::bail!(
                "`{name}` works on the local database; it cannot run against {}. \
                 The agent never deletes anything and is scanned by its own schedule",
                r.base()
            );
        }
    }

    match &cli.command {
        Command::Scan(a) => cmd_scan(a, &db_path, cli.json),
        Command::Ls(a) => cmd_ls(a, &db_path, remote.as_ref(), cli.json),
        Command::Scans => cmd_scans(&db_path, remote.as_ref(), cli.json),
        Command::Diff(a) => cmd_diff(a, &db_path, remote.as_ref(), cli.json),
        Command::Export(a) => cmd_export(a, &db_path, remote.as_ref()),
        Command::Prune(a) => cmd_prune(a, &db_path, cli.json),
        Command::Rm(a) => cmd_rm(a, &db_path),
        Command::Pull(a) => cmd_pull(a, &db_path, remote.as_ref(), cli.json),
    }
}

/// A place to put snapshots downloaded for the lifetime of one command.
fn staging() -> Result<tempfile::TempDir> {
    tempfile::tempdir().context("creating a temporary directory for the download")
}

// ---------------------------------------------------------------- scan

fn cmd_scan(a: &ScanArgs, db_path: &Path, json: bool) -> Result<()> {
    let (tree, stats) = scan_with_progress(&a.path, a.walk.to_options(), !json)?;

    let mut saved_id = None;
    if a.save {
        let mut store = open_store(db_path)?;
        let host = Store::local_host();
        saved_id = Some(store.save(&tree, &stats, &host, a.label.as_deref())?);
    }

    if let Some(out) = &a.ncdu {
        write_ncdu(&tree, out)?;
    }

    if json {
        let payload = serde_json::json!({
            "root": tree.root_path().to_string_lossy(),
            "total_size": tree.total_size(),
            "total_alloc": tree.total_alloc(),
            "files": stats.files,
            "dirs": stats.dirs,
            "errors": stats.errors,
            "hardlinks_deduped": stats.hardlinks_deduped,
            "duration_ms": stats.duration_ms,
            "scan_id": saved_id,
            "fs_total": stats.capacity.map(|c| c.total),
            "fs_available": stats.capacity.map(|c| c.available),
            "largest": largest_json(&tree, a.top),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    print_scan_summary(&tree, &stats);
    println!();
    print_children_table(&tree, tree.root(), a.top);
    print_errors(&stats);
    if let Some(id) = saved_id {
        println!("\nSnapshot #{id} saved → {}", db_path.display());
    } else {
        println!("\nAdd --save to store this snapshot (comparing requires it).");
    }
    Ok(())
}

// ---------------------------------------------------------------- ls

fn cmd_ls(a: &LsArgs, db_path: &Path, remote: Option<&Remote>, json: bool) -> Result<()> {
    let staged;
    let (tree, source) = match (remote, a.scan) {
        (Some(r), id) => {
            // A path argument means a root on the remote, not a local
            // directory to walk. `.` is clap's default, so treat only an
            // explicit path as a selector.
            let wanted_root = (a.path != Path::new(".")).then(|| a.path.to_string_lossy());
            let meta = match (id, &wanted_root) {
                (Some(id), _) => r.scan_or_fail(id)?,
                (None, Some(root)) => r.latest_for(Some(root))?,
                (None, None) => r.latest_for(None)?,
            };
            staged = staging()?;
            let store = r.fetch(meta.id, &staged.path().join("snapshot.sqlite"))?;
            let (tree, meta) = store.load(meta.id)?;
            (
                tree,
                format!("{} snapshot #{} ({})", r.base(), meta.id, meta.root),
            )
        }
        (None, Some(id)) => {
            let store = open_store(db_path)?;
            let (tree, meta) = store.load(id)?;
            (tree, format!("snapshot #{} ({})", meta.id, meta.root))
        }
        (None, None) => {
            let (tree, _) = scan_with_progress(&a.path, a.walk.to_options(), !json)?;
            (tree, tree_source(&a.path))
        }
    };

    let node = match &a.subpath {
        Some(sub) => tree
            .find(sub)
            .with_context(|| format!("not in this snapshot: {sub}"))?,
        None => tree.root(),
    };

    if json {
        let children: Vec<_> = tree
            .children_by(node, SizeBasis::Logical)
            .into_iter()
            .take(a.top)
            .map(|c| entry_json(&tree, c))
            .collect();
        // Name the source: with --remote the caller cannot otherwise tell which
        // machine's snapshot these rows came from.
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "source": source,
                "root": tree.root_path().to_string_lossy(),
                "entries": children,
            }))?
        );
        return Ok(());
    }

    println!("{source}");
    let where_ = match tree.rel_path(node) {
        rel if rel.is_empty() => tree.root_path().display().to_string(),
        rel => rel,
    };
    println!(
        "{}  ·  {} files  ·  {}",
        where_,
        fmt::count(tree.node(node).files as u64),
        fmt::size(tree.node(node).size)
    );
    println!();
    print_children_table(&tree, node, a.top);
    Ok(())
}

// ---------------------------------------------------------------- scans

fn cmd_scans(db_path: &Path, remote: Option<&Remote>, json: bool) -> Result<()> {
    let (scans, source) = match remote {
        Some(r) => (r.list()?, r.base().to_string()),
        None => (open_store(db_path)?.list()?, db_path.display().to_string()),
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&scans)?);
        return Ok(());
    }

    if scans.is_empty() {
        if remote.is_some() {
            println!("{source} has no snapshots yet.");
        } else {
            println!("No snapshots yet. Start with `spacetrace scan <path> --save`.");
        }
        return Ok(());
    }

    println!(
        "{:>5}  {:<16}  {:<10}  {:>10}  {:>9}  ROOT",
        "ID", "DATE", "HOST", "SIZE", "FILES"
    );
    for s in &scans {
        println!(
            "{:>5}  {:<16}  {:<10}  {:>10}  {:>9}  {}{}",
            s.id,
            fmt::timestamp(s.started_at),
            fmt::ellipsize(&s.host, 10),
            fmt::size(s.total_size),
            fmt::count(s.files),
            fmt::ellipsize(&s.root, 44),
            s.label
                .as_deref()
                .map(|l| format!("  [{l}]"))
                .unwrap_or_default(),
        );
    }
    println!("\n{} snapshots · {source}", scans.len());
    Ok(())
}

// ---------------------------------------------------------------- diff

fn cmd_diff(a: &DiffArgs, db_path: &Path, remote: Option<&Remote>, json: bool) -> Result<()> {
    let opts = DiffOptions {
        min_delta: parse_size(&a.min)?,
        include_files: a.files,
        max_depth: a.depth,
        ..Default::default()
    };

    let (old_tree, old_label, new_tree, new_label) = match remote {
        Some(r) => resolve_remote_diff_inputs(a, r)?,
        None => resolve_diff_inputs(a, &open_store(db_path)?, json)?,
    };
    let report = diff(&old_tree, &new_tree, &opts);

    if json {
        let payload = serde_json::json!({
            "from": old_label,
            "to": new_label,
            "old_total": report.old_total,
            "new_total": report.new_total,
            "delta": report.delta(),
            "changes": report.changes.iter().take(a.top).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    print_diff(&report, &old_label, &new_label, a.top);
    Ok(())
}

/// Work out which two trees the user meant. In order of precedence:
/// explicit ids, "last snapshot vs disk now", "last two snapshots of a path".
fn resolve_diff_inputs(
    a: &DiffArgs,
    store: &Store,
    json: bool,
) -> Result<(Tree, String, Tree, String)> {
    let progress = !json;

    if let Some(path) = &a.since_last {
        let root = canonical_string(path)?;
        let meta = store
            .latest_for(&root, None)?
            .with_context(|| format!("no stored snapshot for {root}"))?;
        let (old, _) = store.load(meta.id)?;
        let (new, _) = scan_with_progress(path, ScanOptions::default(), progress)?;
        return Ok((old, label_of(&meta), new, "now (disk)".to_string()));
    }

    if let (Some(from), Some(to)) = (a.from, a.to) {
        let (old, om) = store.load(from)?;
        let (new, nm) = store.load(to)?;
        ensure_comparable(&om, &nm);
        return Ok((old, label_of(&om), new, label_of(&nm)));
    }

    if let Some(from) = a.from {
        let (old, om) = store.load(from)?;
        let (new, _) =
            scan_with_progress(&PathBuf::from(&om.root), ScanOptions::default(), progress)?;
        return Ok((old, label_of(&om), new, "now (disk)".to_string()));
    }

    let path = a
        .path
        .clone()
        .context("what should be compared? pass --path, --since-last or --from/--to")?;
    let root = canonical_string(&path)?;
    let pair = store.last_two_for(&root, None)?;
    anyhow::ensure!(
        pair.len() == 2,
        "need two snapshots of {root} to compare (found {})",
        pair.len()
    );
    let (new, nm) = store.load(pair[0].id)?;
    let (old, om) = store.load(pair[1].id)?;
    Ok((old, label_of(&om), new, label_of(&nm)))
}

/// Pick the two remote snapshots to compare and download both.
///
/// `--since-last` is deliberately unsupported here: it means "the stored
/// snapshot versus this disk right now", and the disk in question belongs to
/// the other machine. Comparing a remote snapshot against the local filesystem
/// would silently answer a question nobody asked.
fn resolve_remote_diff_inputs(
    a: &DiffArgs,
    remote: &Remote,
) -> Result<(Tree, String, Tree, String)> {
    anyhow::ensure!(
        a.since_last.is_none(),
        "--since-last cannot be used with --remote: it would compare {}'s snapshot \
         against this machine's disk. Use --path to compare its last two snapshots",
        remote.base()
    );

    let (old_meta, new_meta) = match (a.from, a.to, &a.path) {
        (Some(from), Some(to), _) => (remote.scan_or_fail(from)?, remote.scan_or_fail(to)?),
        (Some(from), None, _) => {
            let old = remote.scan_or_fail(from)?;
            let new = remote.latest_for(Some(&old.root))?;
            anyhow::ensure!(
                new.id != old.id,
                "snapshot #{} is already the newest of {} on {}",
                old.id,
                old.root,
                remote.base()
            );
            (old, new)
        }
        (None, _, Some(path)) => {
            // The remote recorded its own canonical root, so compare the string
            // as given rather than canonicalising against the local filesystem.
            let pair = remote.last_two_for(&path.to_string_lossy())?;
            (pair[1].clone(), pair[0].clone())
        }
        (None, _, None) => anyhow::bail!(
            "what should be compared? pass --path, or --from/--to (snapshot ids on {})",
            remote.base()
        ),
    };

    ensure_comparable(&old_meta, &new_meta);

    let staged = staging()?;
    let old_store = remote.fetch(old_meta.id, &staged.path().join("old.sqlite"))?;
    let new_store = remote.fetch(new_meta.id, &staged.path().join("new.sqlite"))?;
    let (old_tree, _) = old_store.load(old_meta.id)?;
    let (new_tree, _) = new_store.load(new_meta.id)?;

    Ok((old_tree, label_of(&old_meta), new_tree, label_of(&new_meta)))
}

fn ensure_comparable(a: &ScanMeta, b: &ScanMeta) {
    if a.root != b.root || a.host != b.host {
        eprintln!(
            "warning: comparing different targets ({} ↔ {})",
            a.target(),
            b.target()
        );
    }
}

fn label_of(m: &ScanMeta) -> String {
    let base = format!("#{} {}", m.id, fmt::timestamp(m.started_at));
    match &m.label {
        Some(l) => format!("{base} [{l}]"),
        None => base,
    }
}

fn print_diff(report: &DiffReport, old_label: &str, new_label: &str, top: usize) {
    println!("{old_label}  →  {new_label}");
    println!(
        "total {} → {}   ({})",
        fmt::size(report.old_total),
        fmt::size(report.new_total),
        fmt::delta(report.delta())
    );

    if report.changes.is_empty() {
        println!("\nNo changes above the threshold.");
        return;
    }

    println!();
    println!("{:>12}  {:<7}  {:>10}  PATH", "CHANGE", "STATUS", "NEW");
    for c in report.changes.iter().take(top) {
        let status = match c.kind {
            ChangeKind::Grown => "grew",
            ChangeKind::Shrunk => "shrank",
            ChangeKind::Added => "added",
            ChangeKind::Removed => "removed",
        };
        println!(
            "{:>12}  {:<7}  {:>10}  {}{}",
            fmt::delta(c.delta()),
            status,
            fmt::size(c.new_size),
            fmt::ellipsize(&c.path, 60),
            if c.entry == EntryKind::Dir { "/" } else { "" },
        );
    }
    if report.changes.len() > top {
        println!("… and {} more rows", report.changes.len() - top);
    }
}

// ---------------------------------------------------------------- misc

fn cmd_export(a: &ExportArgs, db_path: &Path, remote: Option<&Remote>) -> Result<()> {
    let staged;
    let store = match remote {
        Some(r) => {
            staged = staging()?;
            r.fetch(a.scan, &staged.path().join("snapshot.sqlite"))?
        }
        None => open_store(db_path)?,
    };
    let (tree, _) = store.load(a.scan)?;
    write_ncdu(&tree, &a.out)
}

/// Copy a remote snapshot into the local database so it can be compared later
/// without the agent being reachable.
fn cmd_pull(a: &PullArgs, db_path: &Path, remote: Option<&Remote>, json: bool) -> Result<()> {
    let r = remote.context("pull needs --remote <url|name>")?;
    let meta = match (a.scan, &a.root) {
        (Some(id), _) => r.scan_or_fail(id)?,
        (None, Some(root)) => r.latest_for(Some(root))?,
        (None, None) => r.latest_for(None)?,
    };

    let staged = staging()?;
    let file = staged.path().join("snapshot.sqlite");
    r.fetch(meta.id, &file)?;

    let mut store = open_store(db_path)?;
    let imported = store.import_snapshot(&file)?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "remote": r.base(),
                "remote_scan_id": meta.id,
                "imported": imported,
            })
        );
        return Ok(());
    }

    match imported.first() {
        Some(id) => println!(
            "Pulled {} snapshot #{} ({}) → local #{id}",
            r.base(),
            meta.id,
            meta.root
        ),
        // import_snapshot deduplicates on host+root+timestamp, so this is the
        // normal answer when the same snapshot is pulled twice.
        None => println!(
            "Already had {} snapshot #{} ({}); nothing to do",
            r.base(),
            meta.id,
            meta.root
        ),
    }
    Ok(())
}

fn cmd_prune(a: &PruneArgs, db_path: &Path, json: bool) -> Result<()> {
    let mut store = open_store(db_path)?;
    let removed = store.prune(a.keep)?;
    if json {
        println!("{}", serde_json::json!({ "removed": removed }));
    } else {
        println!(
            "Deleted {removed} snapshots, kept the newest {} per target.",
            a.keep
        );
    }
    Ok(())
}

fn cmd_rm(a: &RmArgs, db_path: &Path) -> Result<()> {
    let store = open_store(db_path)?;
    anyhow::ensure!(store.delete(a.id)?, "snapshot #{} not found", a.id);
    println!("Snapshot #{} deleted.", a.id);
    Ok(())
}

// ---------------------------------------------------------------- helpers

fn open_store(path: &Path) -> Result<Store> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("cannot create directory: {}", parent.display()))?;
        }
    }
    Store::open(path)
}

/// `$XDG_DATA_HOME/spacetrace` on Linux, Application Support on macOS,
/// `%APPDATA%` on Windows, always falling back to the working directory.
fn default_db_path() -> Result<PathBuf> {
    let dir = if let Ok(x) = std::env::var("SPACETRACE_HOME") {
        PathBuf::from(x)
    } else if cfg!(target_os = "macos") {
        home()?.join("Library/Application Support/spacetrace")
    } else if cfg!(target_os = "windows") {
        std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or(home()?)
            .join("spacetrace")
    } else {
        std::env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home().unwrap_or_default().join(".local/share"))
            .join("spacetrace")
    };
    Ok(dir.join("snapshots.sqlite"))
}

fn home() -> Result<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .context("cannot find home directory; pass a path with --db")
}

fn canonical_string(path: &Path) -> Result<String> {
    let p = path
        .canonicalize()
        .with_context(|| format!("path not found: {}", path.display()))?;
    Ok(p.to_string_lossy().into_owned())
}

fn tree_source(path: &Path) -> String {
    format!("fresh scan: {}", path.display())
}

/// Run a scan, showing a live counter on stderr when it is a terminal.
fn scan_with_progress(
    path: &Path,
    opts: ScanOptions,
    show_progress: bool,
) -> Result<(Tree, ScanStats)> {
    let progress = Arc::new(ScanProgress::default());
    let done = Arc::new(AtomicBool::new(false));

    let ticker = if show_progress && std::io::stderr().is_terminal() {
        let progress = Arc::clone(&progress);
        let done = Arc::clone(&done);
        Some(std::thread::spawn(move || {
            let mut stderr = std::io::stderr();
            while !done.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(120));
                if done.load(Ordering::Relaxed) {
                    break;
                }
                let _ = write!(
                    stderr,
                    "\r  scanning… {} files, {} dirs, {}   ",
                    fmt::count(progress.files.load(Ordering::Relaxed)),
                    fmt::count(progress.dirs.load(Ordering::Relaxed)),
                    fmt::size(progress.bytes.load(Ordering::Relaxed)),
                );
                let _ = stderr.flush();
            }
            let _ = write!(stderr, "\r{:60}\r", "");
            let _ = stderr.flush();
        }))
    } else {
        None
    };

    let result = scan(path, opts, Arc::clone(&progress))
        .with_context(|| format!("cannot scan: {}", path.display()));

    done.store(true, Ordering::Relaxed);
    if let Some(t) = ticker {
        let _ = t.join();
    }
    result
}

fn print_scan_summary(tree: &Tree, stats: &ScanStats) {
    println!("{}", tree.root_path().display());
    println!(
        "  {} logical · {} on disk · {} files · {} dirs · {}",
        fmt::size(tree.total_size()),
        fmt::size(tree.total_alloc()),
        fmt::count(stats.files),
        fmt::count(stats.dirs),
        fmt::duration(stats.duration_ms),
    );
    if stats.hardlinks_deduped > 0 {
        println!(
            "  {} hardlinks counted once",
            fmt::count(stats.hardlinks_deduped)
        );
    }
    // The filesystem's own accounting, which covers more than the scanned root
    // and is the only thing that can answer "how much room is left".
    if let Some(capacity) = stats.capacity {
        if capacity.total > 0 {
            // Free rather than "% full": on APFS and btrfs the space is
            // shared between volumes, so "used" would count the siblings and
            // disagree with df. Available is the same number everywhere.
            println!(
                "  filesystem: {} free of {} ({:.0}% available)",
                fmt::size(capacity.available),
                fmt::size(capacity.total),
                capacity.free_fraction() * 100.0,
            );
        }
    }
}

fn print_children_table(tree: &Tree, node: spacetrace_scan_core::NodeId, top: usize) {
    let children = tree.children_by(node, SizeBasis::Logical);
    if children.is_empty() {
        println!("(empty)");
        return;
    }
    let total = tree.node(node).size.max(1);

    println!("{:>10}  {:>5}  {:<12} NAME", "SIZE", "SHARE", "");
    for &c in children.iter().take(top) {
        let n = tree.node(c);
        let share = n.size as f64 / total as f64;
        println!(
            "{:>10}  {:>4.1}%  {:<12} {}{}",
            fmt::size(n.size),
            share * 100.0,
            bar(share, 12),
            fmt::ellipsize(tree.name(c), 48),
            if n.is_dir() { "/" } else { "" },
        );
    }
    if children.len() > top {
        println!("… and {} more entries", children.len() - top);
    }
}

fn bar(share: f64, width: usize) -> String {
    let filled = (share * width as f64).round() as usize;
    let filled = filled.min(width);
    format!("{}{}", "█".repeat(filled), "·".repeat(width - filled))
}

fn print_errors(stats: &ScanStats) {
    if stats.errors == 0 {
        return;
    }
    println!(
        "\n{} paths could not be read (permission or I/O error):",
        fmt::count(stats.errors)
    );
    for (path, err) in stats.error_samples.iter().take(5) {
        println!("  {} — {err}", path.display());
    }
    if stats.error_samples.len() > 5 {
        println!("  …");
    }
}

fn write_ncdu(tree: &Tree, out: &Path) -> Result<()> {
    if out == Path::new("-") {
        let stdout = std::io::stdout();
        let mut lock = stdout.lock();
        export_ncdu(tree, &mut lock)?;
    } else {
        let file = std::fs::File::create(out)
            .with_context(|| format!("cannot write: {}", out.display()))?;
        let mut w = std::io::BufWriter::new(file);
        export_ncdu(tree, &mut w)?;
        w.flush()?;
        eprintln!("ncdu JSON written: {}", out.display());
    }
    Ok(())
}

fn entry_json(tree: &Tree, id: spacetrace_scan_core::NodeId) -> serde_json::Value {
    let n = tree.node(id);
    serde_json::json!({
        "name": tree.name(id),
        "path": tree.rel_path(id),
        "kind": n.kind,
        "size": n.size,
        "alloc": n.alloc,
        "files": n.files,
        "dirs": n.dirs,
        "mtime": n.mtime,
    })
}

fn largest_json(tree: &Tree, top: usize) -> Vec<serde_json::Value> {
    tree.largest(top, Some(EntryKind::File))
        .into_iter()
        .map(|id| entry_json(tree, id))
        .collect()
}
