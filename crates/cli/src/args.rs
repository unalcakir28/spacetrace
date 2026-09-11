use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "spacetrace",
    version,
    // `-V` stays the bare number, for scripts that parse it; `--version`
    // carries the commit and the channel, which is what a bug report actually
    // needs, since every continuous build shares one version number.
    long_version = spacetrace_buildinfo::long_version(env!("CARGO_PKG_VERSION")),
    about = "Scan disk usage, snapshot it, and see what grew",
    long_about = "spacetrace scans a disk or folder, writes the result to a SQLite \
snapshot, and compares two snapshots to show what is eating the space. The same binary \
also runs on a server, a NAS and inside a container."
)]
pub struct Cli {
    /// Snapshot database (default: user data directory)
    #[arg(long, global = true, value_name = "FILE")]
    pub db: Option<PathBuf>,

    /// Emit output as JSON
    #[arg(long, global = true)]
    pub json: bool,

    /// Read snapshots from an agent instead of the local database.
    /// Takes a URL, or the name of a remote in remotes.toml.
    #[arg(long, global = true, value_name = "URL|NAME")]
    pub remote: Option<String>,

    /// Bearer token for --remote (defaults to remotes.toml, then SPACETRACE_TOKEN)
    #[arg(long, global = true, value_name = "TOKEN")]
    pub token: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Scan a path and print a summary
    Scan(ScanArgs),
    /// List folders by size, from a snapshot or a fresh scan
    Ls(LsArgs),
    /// List stored snapshots
    Scans,
    /// Compare two snapshots: what grew, what shrank
    Diff(DiffArgs),
    /// Export a snapshot as ncdu-compatible JSON
    Export(ExportArgs),
    /// Delete all but the newest N snapshots per target
    Prune(PruneArgs),
    /// Delete a snapshot
    Rm(RmArgs),
    /// Check stored snapshots against the digest saved with them
    Verify(VerifyArgs),
    /// Copy a snapshot from a remote agent into the local database
    Pull(PullArgs),
    /// Install the newest release over this one
    Update(UpdateArgs),
}

#[derive(Args, Debug)]
pub struct VerifyArgs {
    /// Snapshot id (defaults to every snapshot in the database)
    #[arg(value_name = "ID")]
    pub id: Option<i64>,
}

#[derive(Args, Debug)]
pub struct UpdateArgs {
    /// Report what is available without installing anything
    #[arg(long)]
    pub check: bool,
}

#[derive(Args, Debug)]
pub struct PullArgs {
    /// Snapshot id on the remote (defaults to its newest)
    #[arg(long, value_name = "ID")]
    pub scan: Option<i64>,

    /// Pull the newest snapshot of this root instead
    #[arg(long, value_name = "PATH", conflicts_with = "scan")]
    pub root: Option<String>,
}

#[derive(Args, Debug)]
pub struct ScanArgs {
    /// Path to scan
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Store the result in the database
    #[arg(long)]
    pub save: bool,

    /// Label for the stored snapshot (e.g. "weekly")
    #[arg(long, value_name = "TEXT")]
    pub label: Option<String>,

    #[command(flatten)]
    pub walk: WalkArgs,

    /// How many large entries to show
    #[arg(long, default_value_t = 15, value_name = "N")]
    pub top: usize,

    /// Also write the result as ncdu-compatible JSON to this file ("-" = stdout)
    #[arg(long, value_name = "FILE")]
    pub ncdu: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct WalkArgs {
    /// Never descend into folders with this name (repeatable)
    #[arg(long = "exclude", value_name = "NAME")]
    pub exclude: Vec<String>,

    /// Do not cross filesystem boundaries (like du -x)
    #[arg(short = 'x', long = "one-file-system")]
    pub one_file_system: bool,

    /// Do not descend below this depth
    #[arg(long, value_name = "N")]
    pub depth: Option<usize>,

    /// Do not deduplicate hardlinks; count every copy
    #[arg(long)]
    pub no_dedupe: bool,

    /// Do not deduplicate copy-on-write clones; count every copy (macOS)
    #[arg(long)]
    pub no_clone_dedupe: bool,

    /// Walk with this many threads (default: measured, not one per core)
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u16).range(1..))]
    pub threads: Option<u16>,

    /// Seconds to wait for a mounted filesystem before skipping it; 0 waits
    /// forever
    #[arg(long, value_name = "SECONDS")]
    pub mount_timeout: Option<u64>,
}

#[derive(Args, Debug)]
pub struct LsArgs {
    /// Path to scan (when --scan is not given)
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Use a stored snapshot instead of a fresh scan
    #[arg(long, value_name = "ID")]
    pub scan: Option<i64>,

    /// Subpath within the snapshot to show
    #[arg(long, value_name = "PATH")]
    pub subpath: Option<String>,

    /// How many rows to show
    #[arg(long, default_value_t = 20, value_name = "N")]
    pub top: usize,

    #[command(flatten)]
    pub walk: WalkArgs,
}

#[derive(Args, Debug)]
pub struct DiffArgs {
    /// Id of the older snapshot
    #[arg(long, value_name = "ID")]
    pub from: Option<i64>,

    /// Id of the newer snapshot (scans now when omitted)
    #[arg(long, value_name = "ID")]
    pub to: Option<i64>,

    /// Compare the last two snapshots of this path
    #[arg(long, value_name = "PATH")]
    pub path: Option<PathBuf>,

    /// Compare the latest snapshot of this path against the disk right now
    #[arg(long, conflicts_with_all = ["from", "to"], value_name = "PATH")]
    pub since_last: Option<PathBuf>,

    /// Ignore changes smaller than this (e.g. 10M, 500K)
    #[arg(long, default_value = "1M", value_name = "SIZE")]
    pub min: String,

    /// Report files too, not just folders
    #[arg(long)]
    pub files: bool,

    /// Do not descend below this depth when reporting (how far to chase the culprit)
    #[arg(long, value_name = "N")]
    pub depth: Option<usize>,

    /// How many rows to show
    #[arg(long, default_value_t = 25, value_name = "N")]
    pub top: usize,
}

#[derive(Args, Debug)]
pub struct ExportArgs {
    /// Snapshot to export
    #[arg(long, value_name = "ID")]
    pub scan: i64,

    /// Destination file ("-" = stdout)
    #[arg(long, default_value = "-", value_name = "FILE")]
    pub out: PathBuf,
}

#[derive(Args, Debug)]
pub struct PruneArgs {
    /// How many snapshots to keep per target
    #[arg(long, default_value_t = 10, value_name = "N")]
    pub keep: usize,
}

#[derive(Args, Debug)]
pub struct RmArgs {
    /// Id of the snapshot to delete
    pub id: i64,
}

/// `10M`, `500K`, `2G` or a plain byte count.
pub fn parse_size(s: &str) -> anyhow::Result<u64> {
    let s = s.trim();
    let (num, mult) = match s.chars().last() {
        Some('K') | Some('k') => (&s[..s.len() - 1], 1024),
        Some('M') | Some('m') => (&s[..s.len() - 1], 1024 * 1024),
        Some('G') | Some('g') => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        Some('T') | Some('t') => (&s[..s.len() - 1], 1024_u64.pow(4)),
        Some('B') | Some('b') => (&s[..s.len() - 1], 1),
        _ => (s, 1),
    };
    let value: f64 = num
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("cannot parse size: {s:?} (e.g. 10M, 500K, 2G)"))?;
    anyhow::ensure!(value >= 0.0, "size cannot be negative: {s:?}");
    Ok((value * mult as f64) as u64)
}

impl WalkArgs {
    pub fn to_options(&self) -> spacetrace_scan_core::ScanOptions {
        spacetrace_scan_core::ScanOptions {
            exclude_names: self.exclude.clone(),
            one_filesystem: self.one_file_system,
            max_depth: self.depth,
            dedupe_hardlinks: !self.no_dedupe,
            dedupe_clones: !self.no_clone_dedupe,
            threads: self.threads.map(usize::from),
            // `0` means "wait forever", which is what every version before
            // this did: the flag exists to restore the old behaviour, so the
            // value that switches the protection off should be the obvious
            // one rather than a word.
            mount_timeout: match self.mount_timeout {
                Some(0) => None,
                Some(secs) => Some(std::time::Duration::from_secs(secs)),
                None => Some(spacetrace_scan_core::MOUNT_TIMEOUT),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_accept_units() {
        assert_eq!(parse_size("1024").unwrap(), 1024);
        assert_eq!(parse_size("1K").unwrap(), 1024);
        assert_eq!(parse_size("10M").unwrap(), 10 * 1024 * 1024);
        assert_eq!(parse_size("1.5G").unwrap(), 1610612736);
        assert_eq!(parse_size(" 2g ").unwrap(), 2 * 1024 * 1024 * 1024);
    }

    #[test]
    fn bad_sizes_are_rejected() {
        assert!(parse_size("abc").is_err());
        assert!(parse_size("-5M").is_err());
    }

    #[test]
    fn the_cli_definition_is_valid() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    /// `0` is the escape hatch back to the old behaviour, so it has to mean
    /// "wait forever" and not "give up instantly" — the two are opposites and
    /// the wrong one silently drops whole volumes from a total.
    #[test]
    fn a_mount_timeout_of_zero_switches_the_protection_off() {
        // Through clap rather than by building the struct, so the flag name
        // and its parsing are covered too.
        #[derive(Parser)]
        struct Wrap {
            #[command(flatten)]
            walk: WalkArgs,
        }
        let walk = |args: &[&str]| {
            let mut all = vec!["spacetrace"];
            all.extend_from_slice(args);
            Wrap::parse_from(all).walk.to_options().mount_timeout
        };

        assert_eq!(
            walk(&[]),
            Some(spacetrace_scan_core::MOUNT_TIMEOUT),
            "no flag means the default patience, not none"
        );
        assert_eq!(walk(&["--mount-timeout", "0"]), None);
        assert_eq!(
            walk(&["--mount-timeout", "5"]),
            Some(std::time::Duration::from_secs(5))
        );
    }
}
