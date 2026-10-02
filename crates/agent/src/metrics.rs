//! `GET /metrics`: what the agent knows, in the Prometheus text format.
//!
//! **Hand-written, like the cron parser and the rate limiter.** The format is
//! lines of `name{label="value"} number` with two comment kinds, and every
//! value here is a gauge read off something the agent already holds. A client
//! library would bring a registry, a global, and a dependency to an agent that
//! has to stay one static binary — to print thirteen families.
//!
//! **Every number comes from what is already stored or already in memory.** A
//! scrape reads the newest snapshot's row and a count per root from the
//! `(host, root, started_at)` index, plus the runner's in-flight table. It
//! starts no scan, loads no tree, takes no write lock (invariant 0), and does
//! not touch the scanned filesystems: capacity is the one the newest snapshot
//! recorded, not a fresh `statvfs`, because `statvfs` on a network share whose
//! server has gone never returns (invariant 7), and this endpoint is asked
//! every fifteen seconds precisely so that someone notices when things break.
//! Live free space is node_exporter's job, and it already does it.
//!
//! **A configured root that has never been scanned is still listed**, with
//! `scan_running` and `snapshots` at zero, so an alert can be written for it.
//! The families that describe a snapshot are absent for it rather than zero:
//! a size of 0 would draw as the disk being emptied, and a timestamp of 0 as a
//! scan in 1970.

use std::fmt::Write as _;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Result;
use spacetrace_store::ScanMeta;

use crate::runner::Runner;

/// The text format's media type. Version 0.0.4 is the one every Prometheus
/// since 2.0 accepts; OpenMetrics would add nothing a gauge needs.
pub const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// How long one scrape waits, in total, to learn the stored name of roots it
/// has not resolved yet. See `Runner::recorded_root`.
///
/// On a healthy disk the answer takes microseconds and this is never reached.
/// It is reached by a root on a dead share, every scrape until the share
/// comes back, so it is kept well inside Prometheus's default ten-second
/// scrape timeout: that root's snapshot families go missing, and the scrape
/// itself still succeeds with everything else.
const RESOLVE_WAIT: Duration = Duration::from_millis(500);

/// One configured root, as a scrape sees it.
#[derive(Debug, Clone)]
pub struct RootReading {
    /// The path as configured, which is what an operator writes an alert
    /// against — not the canonical path the scanner stored.
    pub root: String,
    pub scanning: bool,
    /// `None` when the path the root is stored under is not known yet, which
    /// only happens while that path is not answering. Distinct from an empty
    /// history: saying "0 snapshots" there would be a claim nobody checked.
    pub history: Option<History>,
}

#[derive(Debug, Clone)]
pub struct History {
    /// Snapshots of this root taken on this host.
    pub snapshots: u64,
    /// The newest of them; `None` for a root never scanned.
    pub latest: Option<ScanMeta>,
}

/// Read everything a scrape reports. Blocking: it opens SQLite and may wait
/// up to `RESOLVE_WAIT` on a root's path.
pub fn collect(runner: &Runner) -> Result<Vec<RootReading>> {
    let store = runner.open_store()?;
    let running = runner.in_flight_roots();
    let deadline = Instant::now() + RESOLVE_WAIT;

    let mut readings: Vec<RootReading> = Vec::new();
    for root in runner.roots() {
        // The config does not forbid listing a root twice, and the format does
        // forbid the same series twice in one scrape: Prometheus rejects the
        // duplicate. Compared as the label it becomes, because two paths that
        // are not UTF-8 can become the same label. The first one wins.
        let label = root.path.to_string_lossy();
        if readings.iter().any(|seen| seen.root == label) {
            continue;
        }
        let history = match runner.recorded_root(&root.path, deadline) {
            Some(stored) => Some(History {
                snapshots: store.count_for(&stored, runner.host())?,
                latest: store.latest_for(&stored, Some(runner.host()))?,
            }),
            None => None,
        };
        readings.push(RootReading {
            root: label.into_owned(),
            scanning: running.contains(&root.path),
            history,
        });
    }
    Ok(readings)
}

/// The exposition, from readings already taken.
///
/// Separate from `collect` so the format — family grouping, escaping, which
/// families a never-scanned root gets — is testable without a filesystem.
pub fn render(roots: &[RootReading], started: SystemTime) -> String {
    let mut out = String::new();

    family(
        &mut out,
        "spacetrace_agent_info",
        "Which build is running. Always 1; the build is in the labels.",
        [(
            vec![
                ("version", env!("CARGO_PKG_VERSION")),
                ("commit", spacetrace_buildinfo::GIT_SHA),
                ("channel", spacetrace_buildinfo::CHANNEL),
            ],
            "1".to_string(),
        )],
    );
    // A start time rather than an uptime, as `process_start_time_seconds`
    // does: it is constant for the life of the process, so a restart is a
    // step in a flat line, and `time() - x` is the uptime when wanted.
    let started = started
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    family(
        &mut out,
        "spacetrace_agent_start_time_seconds",
        "Unix time the agent started serving.",
        [(Vec::new(), started.to_string())],
    );

    per_root(
        &mut out,
        roots,
        "spacetrace_root_scan_running",
        "1 while a scan of this configured root is running, else 0.",
        |r| Some(u64::from(r.scanning).to_string()),
    );
    per_root(
        &mut out,
        roots,
        "spacetrace_root_snapshots",
        "Snapshots of this root taken on this host and still stored.",
        |r| Some(r.history.as_ref()?.snapshots.to_string()),
    );
    per_snapshot(
        &mut out,
        roots,
        "spacetrace_root_last_scan_timestamp_seconds",
        "Unix time the newest stored snapshot of this root started.",
        |m| Some(m.started_at.to_string()),
    );
    per_snapshot(
        &mut out,
        roots,
        "spacetrace_root_last_scan_duration_seconds",
        "How long the newest stored snapshot of this root took to walk.",
        |m| Some((m.duration_ms as f64 / 1000.0).to_string()),
    );
    per_snapshot(
        &mut out,
        roots,
        "spacetrace_root_size_bytes",
        "Logical size of this root in the newest snapshot: file bytes only.",
        |m| Some(m.total_size.to_string()),
    );
    per_snapshot(
        &mut out,
        roots,
        "spacetrace_root_alloc_bytes",
        "Bytes this root holds on disk in the newest snapshot, shared blocks counted once.",
        |m| Some(m.total_alloc.to_string()),
    );
    per_snapshot(
        &mut out,
        roots,
        "spacetrace_root_files",
        "Files in the newest snapshot of this root.",
        |m| Some(m.files.to_string()),
    );
    per_snapshot(
        &mut out,
        roots,
        "spacetrace_root_directories",
        "Directories in the newest snapshot of this root.",
        |m| Some(m.dirs.to_string()),
    );
    per_snapshot(
        &mut out,
        roots,
        "spacetrace_root_unreadable_paths",
        "Paths the newest scan of this root could not read; its totals are short by them.",
        |m| Some(m.errors.to_string()),
    );
    // Free and total, never a percentage (K6): on a shared-space filesystem
    // `total - available` is the container's usage and disagrees with `df`.
    // Absent when the snapshot could not measure them, which is not zero.
    per_snapshot(
        &mut out,
        roots,
        "spacetrace_root_filesystem_available_bytes",
        "Bytes available to the agent on the filesystem holding this root, when the newest snapshot was taken.",
        |m| m.fs_available.map(|v| v.to_string()),
    );
    per_snapshot(
        &mut out,
        roots,
        "spacetrace_root_filesystem_size_bytes",
        "Size of the filesystem holding this root, when the newest snapshot was taken.",
        |m| m.fs_total.map(|v| v.to_string()),
    );

    out
}

/// One family with one sample per root that has a value.
fn per_root(
    out: &mut String,
    roots: &[RootReading],
    name: &str,
    help: &str,
    value: impl Fn(&RootReading) -> Option<String>,
) {
    let samples = roots
        .iter()
        .filter_map(|r| Some((vec![("root", r.root.as_str())], value(r)?)));
    family(out, name, help, samples);
}

/// `per_root`, for the families that describe the newest snapshot and so
/// exist only for a root that has one.
fn per_snapshot(
    out: &mut String,
    roots: &[RootReading],
    name: &str,
    help: &str,
    value: impl Fn(&ScanMeta) -> Option<String>,
) {
    per_root(out, roots, name, help, |r| {
        value(r.history.as_ref()?.latest.as_ref()?)
    });
}

/// Write one family: `# HELP`, `# TYPE`, then every sample, together.
///
/// Every family here is a gauge. Nothing the agent reports only ever goes up
/// for the life of the process, which is what a counter (and a `_total`
/// suffix) would promise. A family with no samples is left out entirely.
fn family<'a>(
    out: &mut String,
    name: &str,
    help: &str,
    samples: impl IntoIterator<Item = (Vec<(&'a str, &'a str)>, String)>,
) {
    let mut samples = samples.into_iter().peekable();
    if samples.peek().is_none() {
        return;
    }
    // Writing to a String cannot fail.
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} gauge");
    for (labels, value) in samples {
        out.push_str(name);
        if !labels.is_empty() {
            out.push('{');
            for (i, (label, raw)) in labels.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                let _ = write!(out, "{label}=\"{}\"", escape_label(raw));
            }
            out.push('}');
        }
        let _ = writeln!(out, " {value}");
    }
}

/// Escape a label value as the text format requires: backslash, double quote
/// and line feed, and nothing else. A root path may contain any of the three,
/// and an unescaped one ends the value early or splits the line, which makes
/// Prometheus reject the whole scrape rather than the one series.
fn escape_label(raw: &str) -> String {
    let mut escaped = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            c => escaped.push(c),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> ScanMeta {
        ScanMeta {
            id: 7,
            host: "h".into(),
            root: "/data".into(),
            started_at: 1_790_000_000,
            duration_ms: 1_250,
            total_size: 1000,
            total_alloc: 4096,
            files: 3,
            dirs: 2,
            errors: 1,
            hardlinks_deduped: 0,
            scanner_version: "0".into(),
            label: None,
            fs_total: Some(10_000),
            fs_available: Some(2_500),
        }
    }

    fn reading(root: &str, history: Option<History>) -> RootReading {
        RootReading {
            root: root.into(),
            scanning: false,
            history,
        }
    }

    #[test]
    fn the_three_characters_the_format_reserves_are_escaped_and_nothing_else() {
        assert_eq!(escape_label(r#"a\b"c"#), r#"a\\b\"c"#);
        assert_eq!(escape_label("a\nb"), r"a\nb");
        // Tabs, carriage returns and non-ASCII pass through: the format only
        // reserves the three above, and escaping more would change the value
        // Prometheus stores.
        assert_eq!(escape_label("t\tr\rü"), "t\tr\rü");
    }

    /// A family is written once with all its roots under it. Writing it per
    /// root would repeat `# TYPE`, which the parser refuses.
    #[test]
    fn each_family_appears_once_however_many_roots_there_are() {
        let roots = vec![
            reading(
                "/a",
                Some(History {
                    snapshots: 2,
                    latest: Some(meta()),
                }),
            ),
            reading(
                "/b",
                Some(History {
                    snapshots: 1,
                    latest: Some(meta()),
                }),
            ),
        ];
        let text = render(&roots, SystemTime::UNIX_EPOCH);
        for name in ["spacetrace_root_size_bytes", "spacetrace_root_scan_running"] {
            let types = text
                .lines()
                .filter(|l| *l == format!("# TYPE {name} gauge"))
                .count();
            assert_eq!(types, 1, "{name}");
        }
        assert!(text.contains("spacetrace_root_size_bytes{root=\"/a\"} 1000\n"));
        assert!(text.contains("spacetrace_root_size_bytes{root=\"/b\"} 1000\n"));
    }

    /// The decision about a root with no history, pinned: the two families an
    /// alert on it needs are present, the ones describing a snapshot are not.
    #[test]
    fn a_never_scanned_root_reports_zero_snapshots_and_no_snapshot_values() {
        let roots = vec![reading(
            "/new",
            Some(History {
                snapshots: 0,
                latest: None,
            }),
        )];
        let text = render(&roots, SystemTime::UNIX_EPOCH);
        assert!(text.contains("spacetrace_root_scan_running{root=\"/new\"} 0\n"));
        assert!(text.contains("spacetrace_root_snapshots{root=\"/new\"} 0\n"));
        assert!(!text.contains("spacetrace_root_size_bytes"), "{text}");
        assert!(!text.contains("spacetrace_root_last_scan_timestamp_seconds"));
    }

    /// A root whose path has not answered yet is not reported as having no
    /// snapshots: nobody looked, so "0" would be invented.
    #[test]
    fn a_root_not_resolved_yet_claims_nothing_about_its_history() {
        let text = render(&[reading("/dead", None)], SystemTime::UNIX_EPOCH);
        assert!(text.contains("spacetrace_root_scan_running{root=\"/dead\"} 0\n"));
        assert!(!text.contains("spacetrace_root_snapshots"), "{text}");
    }

    /// Unmeasured capacity is absent, not zero. A zero would read as a full
    /// disk and page somebody.
    #[test]
    fn capacity_the_snapshot_did_not_measure_is_left_out() {
        let mut unmeasured = meta();
        unmeasured.fs_total = None;
        unmeasured.fs_available = None;
        let roots = vec![reading(
            "/a",
            Some(History {
                snapshots: 1,
                latest: Some(unmeasured),
            }),
        )];
        let text = render(&roots, SystemTime::UNIX_EPOCH);
        assert!(!text.contains("spacetrace_root_filesystem_"), "{text}");
        assert!(text.contains("spacetrace_root_size_bytes{root=\"/a\"} 1000\n"));
    }

    #[test]
    fn durations_are_seconds_and_a_running_scan_is_one() {
        let mut busy = reading(
            "/a",
            Some(History {
                snapshots: 1,
                latest: Some(meta()),
            }),
        );
        busy.scanning = true;
        let text = render(&[busy], SystemTime::UNIX_EPOCH);
        assert!(text.contains("spacetrace_root_last_scan_duration_seconds{root=\"/a\"} 1.25\n"));
        assert!(text.contains("spacetrace_root_scan_running{root=\"/a\"} 1\n"));
    }
}
