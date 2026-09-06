//! `spacetrace-agent` — unattended disk-usage scanning for machines nobody
//! logs into.
//!
//! Three jobs: scan on a schedule, keep the history bounded, and hand snapshots
//! to whoever asks with the right token. It never deletes anything outside its
//! own snapshot database — a deliberate limit, not a missing feature, because
//! that is the easiest way for software running on someone's server to be
//! trusted.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};

use spacetrace_agent::config::{self, Config};
use spacetrace_agent::runner::Runner;
use spacetrace_agent::{push, scheduler, serve};

#[derive(Parser, Debug)]
#[command(
    name = "spacetrace-agent",
    version,
    about = "Scan disk usage on a schedule and serve the snapshots",
    long_about = "spacetrace-agent scans configured roots on its own schedule, stores each \
result as a snapshot, and serves them over HTTP to the spacetrace CLI or a hub. It only ever \
reads the filesystem."
)]
struct Cli {
    /// Configuration file
    #[arg(
        long,
        short,
        global = true,
        default_value = "/etc/spacetrace/agent.toml",
        value_name = "FILE"
    )]
    config: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Print a starter configuration file
    Init,
    /// Check that the configuration is valid and show what it would do
    Check,
    /// Scan the configured roots once and exit
    Scan(ScanArgs),
    /// Serve snapshots over HTTP and run scheduled scans
    Serve,
    /// Send a snapshot to another agent or a hub
    Push(PushArgs),
}

#[derive(Args, Debug)]
struct ScanArgs {
    /// Scan only this root (must be listed in the config)
    #[arg(long, value_name = "PATH")]
    root: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct PushArgs {
    /// Base URL of the receiving agent, e.g. https://hub.example.com
    #[arg(value_name = "URL")]
    url: String,

    /// Snapshot id to send (defaults to the newest)
    #[arg(long, value_name = "ID", conflicts_with = "root")]
    scan: Option<i64>,

    /// Send the newest snapshot of this root
    #[arg(long, value_name = "PATH")]
    root: Option<PathBuf>,

    /// Bearer token for the receiver. Defaults to this agent's own token.
    #[arg(long, value_name = "TOKEN")]
    token: Option<String>,

    /// Send the snapshot uncompressed
    #[arg(long)]
    no_compress: bool,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();

    // `init` is the one command that must work before a config exists.
    if matches!(cli.command, Command::Init) {
        print!("{}", config::EXAMPLE_CONFIG);
        return Ok(());
    }

    let config = Config::load(&cli.config)?;
    let runner = Arc::new(Runner::new(&config));

    match &cli.command {
        Command::Init => unreachable!("handled above"),
        Command::Check => cmd_check(&config, &runner),
        Command::Scan(a) => cmd_scan(&config, &runner, a),
        Command::Serve => cmd_serve(config, runner),
        Command::Push(a) => cmd_push(&config, &runner, a),
    }
}

fn cmd_check(config: &Config, runner: &Runner) -> Result<()> {
    println!("config       ok");
    println!("database     {}", runner.db_path().display());
    println!("host         {}", runner.host());
    println!("listen       {}", config.server.listen);
    println!(
        "token        {}",
        match config.resolve_token()? {
            Some(_) => "configured",
            None => "MISSING (serve will refuse to start)",
        }
    );
    println!(
        "ad-hoc scans {}",
        if config.server.allow_adhoc_scans {
            "allowed"
        } else {
            "refused"
        }
    );

    if runner.roots().is_empty() {
        println!("\nno roots configured");
        return Ok(());
    }

    let offset = config.utc_offset_minutes as i64 * 60;
    let now = scheduler::now_unix() + offset;
    println!("\n{:<28}  {:<14}  NEXT RUN", "ROOT", "SCHEDULE");
    for root in runner.roots() {
        let (expr, next) = match &root.schedule {
            Some(s) => (
                s.as_str().to_string(),
                match s.next_after(now) {
                    Some(at) => format_stamp(at),
                    // Reported rather than swallowed: a schedule that can never
                    // fire is almost always a typo.
                    None => "never".to_string(),
                },
            ),
            None => ("-".to_string(), "on request only".to_string()),
        };
        println!(
            "{:<28}  {:<14}  {}",
            truncate(&root.path.to_string_lossy(), 28),
            expr,
            next
        );
    }
    if config.utc_offset_minutes != 0 {
        println!(
            "\nTimes are UTC{:+}:{:02}.",
            config.utc_offset_minutes / 60,
            (config.utc_offset_minutes % 60).abs()
        );
    } else {
        println!("\nTimes are UTC.");
    }
    Ok(())
}

fn cmd_scan(config: &Config, runner: &Runner, args: &ScanArgs) -> Result<()> {
    let targets: Vec<_> = match &args.root {
        Some(path) => {
            let root = runner
                .configured_root(path)
                .with_context(|| format!("{} is not a configured root", path.display()))?;
            vec![root.clone()]
        }
        None => config.roots.clone(),
    };
    anyhow::ensure!(!targets.is_empty(), "no roots configured to scan");

    let mut failures = 0;
    for root in &targets {
        match runner.scan_root(root) {
            Ok(o) => println!(
                "{}  snapshot #{}  {} files  {} dirs  {} errors  {} ms{}",
                o.root,
                o.scan_id,
                o.files,
                o.dirs,
                o.errors,
                o.duration_ms,
                if o.pruned > 0 {
                    format!("  ({} pruned)", o.pruned)
                } else {
                    String::new()
                }
            ),
            Err(err) => {
                // One unreadable root must not stop the others.
                eprintln!("error scanning {}: {err:#}", root.path.display());
                failures += 1;
            }
        }
    }
    anyhow::ensure!(
        failures == 0,
        "{failures} of {} roots failed to scan",
        targets.len()
    );
    Ok(())
}

fn cmd_serve(config: Config, runner: Arc<Runner>) -> Result<()> {
    let token = config.resolve_token()?.context(
        "no bearer token configured. Set server.token_file, server.token, or the \
         SPACETRACE_TOKEN environment variable — serving an unauthenticated \
         filesystem inventory is never the right default",
    )?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("starting the async runtime")?;

    runtime.block_on(async {
        let scheduled = Arc::clone(&runner);
        let offset = config.utc_offset_minutes;
        tokio::spawn(async move { scheduler::run(scheduled, offset).await });
        serve::serve(runner, &config, token).await
    })
}

fn cmd_push(config: &Config, runner: &Runner, args: &PushArgs) -> Result<()> {
    let token = match &args.token {
        Some(t) => t.clone(),
        None => config.resolve_token()?.context(
            "no token to authenticate with. Pass --token, or configure one for this agent",
        )?,
    };

    let selector = match (args.scan, &args.root) {
        (Some(id), _) => push::Selector::Id(id),
        (None, Some(root)) => push::Selector::LatestForRoot(root.clone()),
        (None, None) => push::Selector::Latest,
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("starting the async runtime")?;

    let outcome = runtime.block_on(push::push(
        runner,
        &args.url,
        &token,
        selector,
        !args.no_compress,
    ))?;

    println!(
        "pushed snapshot #{} to {} ({} bytes{})",
        outcome.scan_id,
        args.url,
        outcome.sent_bytes,
        if outcome.compressed { ", zstd" } else { "" }
    );
    if outcome.imported.is_empty() {
        println!(
            "the receiver already had it ({} snapshot(s) skipped); nothing was added",
            outcome.skipped
        );
    } else {
        println!(
            "the receiver stored it as #{}",
            outcome
                .imported
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(", #")
        );
    }
    Ok(())
}

/// `YYYY-MM-DD HH:MM`, matching the CLI's format. See `cron` for why there is
/// no date library behind this.
fn format_stamp(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60
    )
}

fn truncate(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(1);
    format!(
        "…{}",
        chars[chars.len() - keep..].iter().collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cli_definition_is_valid() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn timestamps_match_the_cli_format() {
        assert_eq!(format_stamp(0), "1970-01-01 00:00");
        assert_eq!(format_stamp(1_788_714_000), "2026-09-06 17:00");
    }

    #[test]
    fn long_paths_are_truncated_from_the_left() {
        // Keeping the tail is what matters for paths: the leaf identifies it.
        assert_eq!(truncate("/var/log", 20), "/var/log");
        let out = truncate("/very/long/path/to/somewhere/deep", 12);
        assert_eq!(out.chars().count(), 12);
        assert!(out.ends_with("deep"), "{out}");
    }
}
