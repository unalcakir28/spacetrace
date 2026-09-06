mod args;
mod fmt;

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use spacetrace_diff::{diff, ChangeKind, DiffOptions, DiffReport};
use spacetrace_scan_core::{scan, EntryKind, ScanOptions, ScanProgress, ScanStats, Tree};
use spacetrace_store::{export_ncdu, ScanMeta, Store};

use crate::args::{
    parse_size, Cli, Command, DiffArgs, ExportArgs, LsArgs, PruneArgs, RmArgs, ScanArgs,
};

fn main() {
    if let Err(err) = run() {
        eprintln!("hata: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let db_path = cli.db.clone().map(Ok).unwrap_or_else(default_db_path)?;

    match &cli.command {
        Command::Scan(a) => cmd_scan(a, &db_path, cli.json),
        Command::Ls(a) => cmd_ls(a, &db_path, cli.json),
        Command::Scans => cmd_scans(&db_path, cli.json),
        Command::Diff(a) => cmd_diff(a, &db_path, cli.json),
        Command::Export(a) => cmd_export(a, &db_path),
        Command::Prune(a) => cmd_prune(a, &db_path, cli.json),
        Command::Rm(a) => cmd_rm(a, &db_path),
    }
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
        println!("\nAnlık görüntü #{id} kaydedildi → {}", db_path.display());
    } else {
        println!("\nKaydetmek için --save ekleyin (karşılaştırma bunu gerektirir).");
    }
    Ok(())
}

// ---------------------------------------------------------------- ls

fn cmd_ls(a: &LsArgs, db_path: &Path, json: bool) -> Result<()> {
    let (tree, source) = match a.scan {
        Some(id) => {
            let store = open_store(db_path)?;
            let (tree, meta) = store.load(id)?;
            (tree, format!("anlık görüntü #{} ({})", meta.id, meta.root))
        }
        None => {
            let (tree, _) = scan_with_progress(&a.path, a.walk.to_options(), !json)?;
            (tree, tree_source(&a.path))
        }
    };

    let node = match &a.subpath {
        Some(sub) => tree
            .find(sub)
            .with_context(|| format!("bu anlık görüntüde yok: {sub}"))?,
        None => tree.root(),
    };

    if json {
        let children: Vec<_> = tree
            .children_by_size(node)
            .into_iter()
            .take(a.top)
            .map(|c| entry_json(&tree, c))
            .collect();
        println!("{}", serde_json::to_string_pretty(&children)?);
        return Ok(());
    }

    println!("{source}");
    let where_ = match tree.rel_path(node) {
        rel if rel.is_empty() => tree.root_path().display().to_string(),
        rel => rel,
    };
    println!(
        "{}  ·  {} dosya  ·  {}",
        where_,
        fmt::count(tree.node(node).files),
        fmt::size(tree.node(node).size)
    );
    println!();
    print_children_table(&tree, node, a.top);
    Ok(())
}

// ---------------------------------------------------------------- scans

fn cmd_scans(db_path: &Path, json: bool) -> Result<()> {
    let store = open_store(db_path)?;
    let scans = store.list()?;

    if json {
        println!("{}", serde_json::to_string_pretty(&scans)?);
        return Ok(());
    }

    if scans.is_empty() {
        println!("Henüz anlık görüntü yok. `spacetrace scan <yol> --save` ile başlayın.");
        return Ok(());
    }

    println!(
        "{:>5}  {:<16}  {:<10}  {:>10}  {:>9}  KÖK",
        "ID", "TARİH", "MAKİNE", "BOYUT", "DOSYA"
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
    println!("\n{} anlık görüntü · {}", scans.len(), db_path.display());
    Ok(())
}

// ---------------------------------------------------------------- diff

fn cmd_diff(a: &DiffArgs, db_path: &Path, json: bool) -> Result<()> {
    let store = open_store(db_path)?;
    let opts = DiffOptions {
        min_delta: parse_size(&a.min)?,
        include_files: a.files,
        max_depth: a.depth,
        ..Default::default()
    };

    let (old_tree, old_label, new_tree, new_label) = resolve_diff_inputs(a, &store, json)?;
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
            .with_context(|| format!("{root} için kayıtlı anlık görüntü yok"))?;
        let (old, _) = store.load(meta.id)?;
        let (new, _) = scan_with_progress(path, ScanOptions::default(), progress)?;
        return Ok((old, label_of(&meta), new, "şimdi (disk)".to_string()));
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
        return Ok((old, label_of(&om), new, "şimdi (disk)".to_string()));
    }

    let path = a
        .path
        .clone()
        .context("ne karşılaştırılacak? --path, --since-last ya da --from/--to verin")?;
    let root = canonical_string(&path)?;
    let pair = store.last_two_for(&root, None)?;
    anyhow::ensure!(
        pair.len() == 2,
        "{root} için karşılaştırılacak iki anlık görüntü yok ({} tane var)",
        pair.len()
    );
    let (new, nm) = store.load(pair[0].id)?;
    let (old, om) = store.load(pair[1].id)?;
    Ok((old, label_of(&om), new, label_of(&nm)))
}

fn ensure_comparable(a: &ScanMeta, b: &ScanMeta) {
    if a.root != b.root || a.host != b.host {
        eprintln!(
            "uyarı: farklı hedefler karşılaştırılıyor ({} ↔ {})",
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
        "toplam {} → {}   ({})",
        fmt::size(report.old_total),
        fmt::size(report.new_total),
        fmt::delta(report.delta())
    );

    if report.changes.is_empty() {
        println!("\nEşiğin üstünde değişiklik yok.");
        return;
    }

    println!();
    println!("{:>12}  {:<6}  {:>10}  YOL", "DEĞİŞİM", "DURUM", "YENİ");
    for c in report.changes.iter().take(top) {
        let status = match c.kind {
            ChangeKind::Grown => "büyüdü",
            ChangeKind::Shrunk => "küçüldü",
            ChangeKind::Added => "yeni",
            ChangeKind::Removed => "silindi",
        };
        println!(
            "{:>12}  {:<6}  {:>10}  {}{}",
            fmt::delta(c.delta()),
            status,
            fmt::size(c.new_size),
            fmt::ellipsize(&c.path, 60),
            if c.entry == EntryKind::Dir { "/" } else { "" },
        );
    }
    if report.changes.len() > top {
        println!("… ve {} satır daha", report.changes.len() - top);
    }
}

// ---------------------------------------------------------------- misc

fn cmd_export(a: &ExportArgs, db_path: &Path) -> Result<()> {
    let store = open_store(db_path)?;
    let (tree, _) = store.load(a.scan)?;
    write_ncdu(&tree, &a.out)
}

fn cmd_prune(a: &PruneArgs, db_path: &Path, json: bool) -> Result<()> {
    let mut store = open_store(db_path)?;
    let removed = store.prune(a.keep)?;
    if json {
        println!("{}", serde_json::json!({ "removed": removed }));
    } else {
        println!(
            "{removed} anlık görüntü silindi, hedef başına en yeni {} tutuldu.",
            a.keep
        );
    }
    Ok(())
}

fn cmd_rm(a: &RmArgs, db_path: &Path) -> Result<()> {
    let store = open_store(db_path)?;
    anyhow::ensure!(store.delete(a.id)?, "anlık görüntü #{} bulunamadı", a.id);
    println!("Anlık görüntü #{} silindi.", a.id);
    Ok(())
}

// ---------------------------------------------------------------- helpers

fn open_store(path: &Path) -> Result<Store> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("klasör oluşturulamadı: {}", parent.display()))?;
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
        .context("ev klasörü bulunamadı; --db ile yol verin")
}

fn canonical_string(path: &Path) -> Result<String> {
    let p = path
        .canonicalize()
        .with_context(|| format!("yol bulunamadı: {}", path.display()))?;
    Ok(p.to_string_lossy().into_owned())
}

fn tree_source(path: &Path) -> String {
    format!("taze tarama: {}", path.display())
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
                    "\r  taranıyor… {} dosya, {} klasör, {}   ",
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
        .with_context(|| format!("taranamadı: {}", path.display()));

    done.store(true, Ordering::Relaxed);
    if let Some(t) = ticker {
        let _ = t.join();
    }
    result
}

fn print_scan_summary(tree: &Tree, stats: &ScanStats) {
    println!("{}", tree.root_path().display());
    println!(
        "  {} mantıksal · {} diskte · {} dosya · {} klasör · {}",
        fmt::size(tree.total_size()),
        fmt::size(tree.total_alloc()),
        fmt::count(stats.files),
        fmt::count(stats.dirs),
        fmt::duration(stats.duration_ms),
    );
    if stats.hardlinks_deduped > 0 {
        println!(
            "  {} sabit bağlantı bir kez sayıldı",
            fmt::count(stats.hardlinks_deduped)
        );
    }
}

fn print_children_table(tree: &Tree, node: spacetrace_scan_core::NodeId, top: usize) {
    let children = tree.children_by_size(node);
    if children.is_empty() {
        println!("(boş)");
        return;
    }
    let total = tree.node(node).size.max(1);

    println!("{:>10}  {:>5}  {:<12} AD", "BOYUT", "PAY", "");
    for &c in children.iter().take(top) {
        let n = tree.node(c);
        let share = n.size as f64 / total as f64;
        println!(
            "{:>10}  {:>4.1}%  {:<12} {}{}",
            fmt::size(n.size),
            share * 100.0,
            bar(share, 12),
            fmt::ellipsize(&n.name, 48),
            if n.is_dir() { "/" } else { "" },
        );
    }
    if children.len() > top {
        println!("… ve {} girdi daha", children.len() - top);
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
        "\n{} yol okunamadı (izin veya G/Ç hatası):",
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
        let file =
            std::fs::File::create(out).with_context(|| format!("yazılamadı: {}", out.display()))?;
        let mut w = std::io::BufWriter::new(file);
        export_ncdu(tree, &mut w)?;
        w.flush()?;
        eprintln!("ncdu JSON yazıldı: {}", out.display());
    }
    Ok(())
}

fn entry_json(tree: &Tree, id: spacetrace_scan_core::NodeId) -> serde_json::Value {
    let n = tree.node(id);
    serde_json::json!({
        "name": n.name,
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
