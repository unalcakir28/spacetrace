//! Agent configuration, read from a TOML file.
//!
//! The agent is meant to be dropped onto a NAS or a server and left alone, so
//! everything it needs lives in one file that a human can read and a config
//! manager can template. Anything security-sensitive (the bearer token) can be
//! kept out of it and supplied by file or environment instead.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::cron::Schedule;

/// Environment variable checked when the config gives no inline token.
pub const TOKEN_ENV: &str = "SPACETRACE_TOKEN";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Snapshot database. Every root this agent scans lands in the same file.
    pub db: PathBuf,

    /// Minutes to add to UTC when matching `schedule` expressions.
    ///
    /// The agent carries no timezone database on purpose (see the same choice
    /// in the CLI's date formatting), so "3 AM" has to be pinned down by an
    /// explicit offset rather than a zone name. Zero means schedules are UTC.
    #[serde(default)]
    pub utc_offset_minutes: i32,

    /// Ask GitHub once a day whether a newer release exists, and report the
    /// answer on `/status`.
    ///
    /// Reporting only — the agent never installs anything (see `update.rs`).
    /// On by default because an operator who does not know a fix shipped is
    /// the reason the field exists, and off is one line away for anyone whose
    /// agent should not reach the internet at all.
    #[serde(default = "yes")]
    pub update_check: bool,

    #[serde(default)]
    pub server: ServerConfig,

    /// What to scan. An agent with no roots is still useful: it can serve
    /// snapshots pushed to it, and `POST /scans` can name any path.
    #[serde(default)]
    pub roots: Vec<RootConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    /// Bind address. Defaults to loopback: exposing a filesystem inventory to
    /// the whole network should be a deliberate edit, not a default.
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,

    /// Bearer token, inline. Prefer `token_file` or the environment.
    #[serde(default)]
    pub token: Option<String>,

    /// File holding the bearer token. Leading and trailing whitespace is
    /// trimmed so `echo secret > token` behaves as expected.
    #[serde(default)]
    pub token_file: Option<PathBuf>,

    /// Allow `POST /scans` to name a path that is not in `[[roots]]`.
    ///
    /// Off by default. The agent only ever reads, but a filesystem inventory is
    /// still information: with this on, anyone holding the token can enumerate
    /// any directory the agent's user can read. Turning it on should be a
    /// deliberate act.
    #[serde(default)]
    pub allow_adhoc_scans: bool,

    /// Largest snapshot body `POST /snapshots` will accept, in bytes.
    ///
    /// Measured cost is roughly 50 bytes per filesystem entry uncompressed, so
    /// the default leaves room for a root of about ten million files while
    /// still bounding what an authenticated client can make the agent buffer.
    #[serde(default = "default_max_upload")]
    pub max_upload_bytes: usize,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RootConfig {
    pub path: PathBuf,

    /// Cron expression (five fields). Omit to scan only when asked over HTTP.
    #[serde(default)]
    pub schedule: Option<Schedule>,

    #[serde(default)]
    pub label: Option<String>,

    #[serde(default)]
    pub exclude: Vec<String>,

    #[serde(default)]
    pub one_file_system: bool,

    #[serde(default)]
    pub depth: Option<usize>,

    #[serde(default = "default_dedupe")]
    pub dedupe_hardlinks: bool,

    /// Count copy-on-write clones once (macOS/APFS). Defaults on, like
    /// hardlink deduplication: both answer "how much would freeing this give
    /// back", and both cost a syscall per candidate to answer honestly.
    #[serde(default = "default_dedupe")]
    pub dedupe_clones: bool,

    /// Snapshots of this root to keep. `None` keeps everything.
    #[serde(default)]
    pub keep: Option<usize>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            listen: default_listen(),
            token: None,
            token_file: None,
            allow_adhoc_scans: false,
            max_upload_bytes: default_max_upload(),
        }
    }
}

fn default_listen() -> SocketAddr {
    // Loopback, not 0.0.0.0: see ServerConfig::listen.
    SocketAddr::from(([127, 0, 0, 1], 7878))
}

fn default_max_upload() -> usize {
    512 * 1024 * 1024
}

/// Serde needs a function for a `true` default.
fn yes() -> bool {
    true
}

fn default_dedupe() -> bool {
    true
}

impl RootConfig {
    /// A root with nothing but a path, carrying the same defaults the config
    /// file applies to a `[[roots]]` entry that mentions nothing else.
    ///
    /// Built directly rather than by formatting a TOML string and parsing it
    /// back: a path containing a control character has no valid TOML escape, so
    /// the round trip turned an odd directory name into a panic.
    /// `defaults_match_the_config_file` keeps this in step with serde.
    pub fn new(path: PathBuf) -> Self {
        RootConfig {
            path,
            schedule: None,
            label: None,
            exclude: Vec::new(),
            one_file_system: false,
            depth: None,
            dedupe_hardlinks: default_dedupe(),
            dedupe_clones: default_dedupe(),
            keep: None,
        }
    }

    pub fn scan_options(&self) -> spacetrace_scan_core::ScanOptions {
        spacetrace_scan_core::ScanOptions {
            exclude_names: self.exclude.clone(),
            one_filesystem: self.one_file_system,
            max_depth: self.depth,
            dedupe_hardlinks: self.dedupe_hardlinks,
            dedupe_clones: self.dedupe_clones,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let config: Config =
            toml::from_str(&text).with_context(|| format!("parsing config {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        for root in &self.roots {
            anyhow::ensure!(
                root.path.is_absolute(),
                "root path must be absolute: {}",
                root.path.display()
            );
            if let Some(keep) = root.keep {
                anyhow::ensure!(
                    keep > 0,
                    "keep must be at least 1 for {} (use no keep at all to retain every snapshot)",
                    root.path.display()
                );
            }
        }
        anyhow::ensure!(
            self.utc_offset_minutes.abs() <= 14 * 60,
            "utc_offset_minutes out of range: {}",
            self.utc_offset_minutes
        );
        Ok(())
    }

    /// Resolve the bearer token from, in order: the config, a token file, the
    /// environment. `None` means the server refuses to start — an unauthenticated
    /// agent would hand its whole filesystem inventory to anyone who can reach
    /// the port.
    pub fn resolve_token(&self) -> Result<Option<String>> {
        if let Some(t) = &self.server.token {
            return Ok(Some(t.clone()));
        }
        if let Some(path) = &self.server.token_file {
            let raw = std::fs::read_to_string(path)
                .with_context(|| format!("reading token file {}", path.display()))?;
            let token = raw.trim().to_string();
            anyhow::ensure!(!token.is_empty(), "token file is empty: {}", path.display());
            return Ok(Some(token));
        }
        match std::env::var(TOKEN_ENV) {
            Ok(t) if !t.trim().is_empty() => Ok(Some(t.trim().to_string())),
            _ => Ok(None),
        }
    }
}

/// Written by `spacetrace-agent init`, and the worked example in the docs.
///
/// Platform-specific because `validate` requires absolute root paths and
/// "absolute" differs: `/var` is not an absolute path on Windows, so a single
/// Unix-flavoured example would print a config that cannot start there.
#[cfg(not(windows))]
pub const EXAMPLE_CONFIG: &str = r#"# spacetrace agent configuration.

# Every root below is stored in this one database.
db = "/var/lib/spacetrace/snapshots.sqlite"

# Schedules are matched against UTC plus this offset. The agent ships no
# timezone database, so DST is not handled: pick the offset you want scans to
# happen at, or leave it 0 and think in UTC.
utc_offset_minutes = 0

# Ask GitHub once a day whether a newer release exists and report it on
# /status. Reporting only — this agent never installs anything. Set to false
# if it should not reach the internet at all.
update_check = true

[server]
# Loopback by default. Put a reverse proxy in front, or change this to
# "0.0.0.0:7878" once you have set a token and, ideally, TLS.
listen = "127.0.0.1:7878"
# Supply the bearer token by file (recommended) or via SPACETRACE_TOKEN.
# token_file = "/etc/spacetrace/token"

[[roots]]
path = "/var"
schedule = "0 3 * * *"      # 03:00 every day
label = "nightly"
exclude = ["node_modules", ".git"]
one_file_system = true
keep = 14

[[roots]]
path = "/srv"
schedule = "30 3 * * 0"     # 03:30 on Sundays
keep = 8
"#;

/// Windows flavour of [`EXAMPLE_CONFIG`]. Paths use TOML literal strings
/// (single quotes) so backslashes need no escaping.
#[cfg(windows)]
pub const EXAMPLE_CONFIG: &str = r#"# spacetrace agent configuration.

# Every root below is stored in this one database.
db = 'C:\ProgramData\spacetrace\snapshots.sqlite'

# Schedules are matched against UTC plus this offset. The agent ships no
# timezone database, so DST is not handled: pick the offset you want scans to
# happen at, or leave it 0 and think in UTC.
utc_offset_minutes = 0

# Ask GitHub once a day whether a newer release exists and report it on
# /status. Reporting only — this agent never installs anything. Set to false
# if it should not reach the internet at all.
update_check = true

[server]
# Loopback by default. Put a reverse proxy in front, or change this to
# "0.0.0.0:7878" once you have set a token and, ideally, TLS.
listen = "127.0.0.1:7878"
# Supply the bearer token by file (recommended) or via SPACETRACE_TOKEN.
# token_file = 'C:\ProgramData\spacetrace\token'

[[roots]]
path = 'C:\Users'
schedule = "0 3 * * *"      # 03:00 every day
label = "nightly"
exclude = ["node_modules", ".git", "AppData"]
one_file_system = true
keep = 14

[[roots]]
path = 'C:\ProgramData'
schedule = "30 3 * * 0"     # 03:30 on Sundays
keep = 8
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_config_file() {
        let parsed: Config =
            toml::from_str("db = \"/tmp/x\"\n[[roots]]\npath = \"/var/log\"\n").unwrap();
        let built = RootConfig::new(PathBuf::from("/var/log"));
        let from_file = &parsed.roots[0];

        assert_eq!(built.path, from_file.path);
        assert_eq!(built.label, from_file.label);
        assert_eq!(built.exclude, from_file.exclude);
        assert_eq!(built.one_file_system, from_file.one_file_system);
        assert_eq!(built.depth, from_file.depth);
        assert_eq!(built.dedupe_hardlinks, from_file.dedupe_hardlinks);
        assert_eq!(built.keep, from_file.keep);
        assert!(built.schedule.is_none() && from_file.schedule.is_none());
    }

    #[test]
    fn a_path_with_a_control_character_does_not_panic() {
        // Not reachable from a config file, but `POST /scans` accepts a path
        // from the request body.
        let root = RootConfig::new(PathBuf::from("/tmp/we\u{7}ird"));
        assert!(root.path.to_string_lossy().contains('\u{7}'));
    }

    /// `agent init` prints this, so it has to be startable on the platform it
    /// was printed on — including the absolute-path rule, which differs.
    #[test]
    fn the_example_config_parses() {
        let cfg: Config = toml::from_str(EXAMPLE_CONFIG).expect("example config must parse");
        cfg.validate().expect("example config must validate");
        assert_eq!(cfg.roots.len(), 2);
        assert_eq!(cfg.roots[0].keep, Some(14));
        assert!(cfg.roots[0].one_file_system);
        assert!(
            cfg.roots.iter().all(|r| r.path.is_absolute()),
            "every example root must be absolute here: {:?}",
            cfg.roots.iter().map(|r| &r.path).collect::<Vec<_>>()
        );
        assert!(
            cfg.db.is_absolute(),
            "the example database path must be absolute"
        );
        // Defaults apply to roots that do not mention them.
        assert!(cfg.roots[1].dedupe_hardlinks);
        assert!(!cfg.roots[1].one_file_system);
    }

    #[test]
    fn a_minimal_config_is_enough() {
        let cfg: Config = toml::from_str(r#"db = "/tmp/x.sqlite""#).unwrap();
        assert!(cfg.roots.is_empty());
        assert_eq!(cfg.server.listen.port(), 7878);
        assert!(cfg.server.listen.ip().is_loopback());
    }

    #[test]
    fn unknown_keys_are_rejected_rather_than_ignored() {
        // A typo in a config file that silently does nothing is worse than a
        // startup failure on a machine nobody is watching.
        let err = toml::from_str::<Config>("db = \"/tmp/x\"\nexclud = 3\n").unwrap_err();
        assert!(err.to_string().contains("exclud"), "{err}");
    }

    #[test]
    fn relative_root_paths_are_rejected() {
        let cfg: Config =
            toml::from_str("db = \"/tmp/x\"\n[[roots]]\npath = \"var/log\"\n").unwrap();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn keep_zero_is_rejected() {
        let cfg: Config =
            toml::from_str("db = \"/tmp/x\"\n[[roots]]\npath = \"/var\"\nkeep = 0\n").unwrap();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn an_absurd_utc_offset_is_rejected() {
        let cfg: Config = toml::from_str("db = \"/tmp/x\"\nutc_offset_minutes = 2000\n").unwrap();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn a_token_file_wins_over_the_environment_and_is_trimmed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        std::fs::write(&path, "  s3cret\n").unwrap();
        let cfg = Config {
            update_check: false,
            db: PathBuf::from("/tmp/x"),
            utc_offset_minutes: 0,
            server: ServerConfig {
                listen: default_listen(),
                token: None,
                token_file: Some(path),
                allow_adhoc_scans: false,
                max_upload_bytes: default_max_upload(),
            },
            roots: vec![],
        };
        assert_eq!(cfg.resolve_token().unwrap().as_deref(), Some("s3cret"));
    }

    #[test]
    fn an_empty_token_file_is_an_error_not_an_empty_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        std::fs::write(&path, "   \n").unwrap();
        let cfg = Config {
            update_check: false,
            db: PathBuf::from("/tmp/x"),
            utc_offset_minutes: 0,
            server: ServerConfig {
                listen: default_listen(),
                token: None,
                token_file: Some(path),
                allow_adhoc_scans: false,
                max_upload_bytes: default_max_upload(),
            },
            roots: vec![],
        };
        assert!(cfg.resolve_token().is_err());
    }

    /// Absent means on. An operator upgrading from a build that had no such
    /// key must not silently lose the notice — and one who wrote `false` must
    /// not silently regain it.
    #[test]
    fn the_update_check_defaults_to_on_and_can_be_switched_off() {
        let bare: Config = toml::from_str(r#"db = "/tmp/x.sqlite""#).expect("parses");
        assert!(bare.update_check);

        let off: Config =
            toml::from_str("db = \"/tmp/x.sqlite\"\nupdate_check = false\n").expect("parses");
        assert!(!off.update_check);
    }
}
