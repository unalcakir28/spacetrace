//! The agent's own timer.
//!
//! A NAS owner should not have to learn systemd timers or edit a crontab to get
//! a nightly scan, so the agent schedules itself. Times are matched against UTC
//! plus a fixed offset from the config; see `cron` for why there is no timezone
//! database.

use std::sync::Arc;
use std::time::Duration;

use crate::config::RootConfig;
use crate::runner::{AlreadyRunning, Runner};

/// Never sleep longer than this in one go. Waking up hourly costs nothing and
/// bounds how far a schedule can drift if the system clock is stepped (an NTP
/// correction after a NAS reboot, say) while we are asleep.
const MAX_SLEEP: Duration = Duration::from_secs(3600);

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Run scheduled scans until the process ends.
///
/// Returns immediately when no root has a schedule, so an agent configured only
/// to serve snapshots does not keep a pointless task alive.
pub async fn run(runner: Arc<Runner>, utc_offset_minutes: i32) {
    let offset_secs = utc_offset_minutes as i64 * 60;
    let scheduled: Vec<RootConfig> = runner
        .roots()
        .iter()
        .filter(|r| r.schedule.is_some())
        .cloned()
        .collect();

    if scheduled.is_empty() {
        eprintln!("no scheduled roots; the agent will only scan when asked");
        return;
    }
    for root in &scheduled {
        eprintln!(
            "scheduled {} at \"{}\"",
            root.path.display(),
            root.schedule.as_ref().expect("filtered above")
        );
    }

    loop {
        let now = now_unix() + offset_secs;
        let Some((fire_at, due)) = next_due(&scheduled, now) else {
            // Every remaining expression is unsatisfiable (e.g. 30 February).
            eprintln!("no schedule will ever fire again; scheduler stopping");
            return;
        };

        let wait = Duration::from_secs((fire_at - now).max(0) as u64).min(MAX_SLEEP);
        tokio::time::sleep(wait).await;

        // The sleep may have been cut short by MAX_SLEEP; only fire once the
        // clock has actually reached the scheduled minute.
        if now_unix() + offset_secs < fire_at {
            continue;
        }

        for index in due {
            let root = scheduled[index].clone();
            let runner = Arc::clone(&runner);
            tokio::task::spawn_blocking(move || run_one(&runner, &root));
        }
    }
}

fn run_one(runner: &Runner, root: &RootConfig) {
    match runner.scan_root(root) {
        Ok(outcome) => eprintln!(
            "scanned {} -> snapshot #{} ({} files, {} errors, {} ms, {} pruned)",
            outcome.root,
            outcome.scan_id,
            outcome.files,
            outcome.errors,
            outcome.duration_ms,
            outcome.pruned
        ),
        // A scan running longer than its own interval is a configuration
        // problem, not a crash: say so and let the next tick try again.
        Err(err) if err.downcast_ref::<AlreadyRunning>().is_some() => {
            eprintln!("skipped {}: {err}", root.path.display())
        }
        Err(err) => eprintln!("scan of {} failed: {err:#}", root.path.display()),
    }
}

/// The earliest moment any root is due, and the indices of every root due then.
///
/// Roots sharing a minute are returned together so they all fire on the same
/// tick rather than the loop handling one and recomputing.
fn next_due(roots: &[RootConfig], now: i64) -> Option<(i64, Vec<usize>)> {
    let mut soonest: Option<i64> = None;
    let mut due: Vec<usize> = Vec::new();

    for (index, root) in roots.iter().enumerate() {
        let Some(schedule) = &root.schedule else {
            continue;
        };
        let Some(at) = schedule.next_after(now) else {
            continue;
        };
        match soonest {
            Some(best) if at > best => {}
            Some(best) if at == best => due.push(index),
            _ => {
                soonest = Some(at);
                due = vec![index];
            }
        }
    }
    soonest.map(|at| (at, due))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots_from(toml_src: &str) -> Vec<RootConfig> {
        let cfg: crate::config::Config =
            toml::from_str(&format!("db = \"/tmp/x\"\n{toml_src}")).unwrap();
        cfg.roots
    }

    /// Unix seconds for a UTC civil datetime.
    fn at(y: i64, m: u32, d: u32, hh: i64, mm: i64) -> i64 {
        let y2 = if m <= 2 { y - 1 } else { y };
        let era = y2.div_euclid(400);
        let yoe = y2 - era * 400;
        let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
        let doy = (153 * mp + 2) / 5 + d as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        (era * 146_097 + doe - 719_468) * 86_400 + hh * 3600 + mm * 60
    }

    #[test]
    fn the_soonest_root_is_picked() {
        let roots = roots_from(
            r#"
[[roots]]
path = "/a"
schedule = "0 5 * * *"
[[roots]]
path = "/b"
schedule = "0 3 * * *"
"#,
        );
        let (fire_at, due) = next_due(&roots, at(2026, 9, 7, 0, 0)).unwrap();
        assert_eq!(fire_at, at(2026, 9, 7, 3, 0));
        assert_eq!(due, vec![1], "only /b is due at 03:00");
    }

    #[test]
    fn roots_sharing_a_minute_fire_together() {
        let roots = roots_from(
            r#"
[[roots]]
path = "/a"
schedule = "0 3 * * *"
[[roots]]
path = "/b"
schedule = "0 3 * * *"
[[roots]]
path = "/c"
schedule = "0 4 * * *"
"#,
        );
        let (fire_at, due) = next_due(&roots, at(2026, 9, 7, 0, 0)).unwrap();
        assert_eq!(fire_at, at(2026, 9, 7, 3, 0));
        assert_eq!(due, vec![0, 1]);
    }

    #[test]
    fn roots_without_a_schedule_are_ignored() {
        let roots = roots_from(
            r#"
[[roots]]
path = "/a"
[[roots]]
path = "/b"
schedule = "0 3 * * *"
"#,
        );
        let (_, due) = next_due(&roots, at(2026, 9, 7, 0, 0)).unwrap();
        assert_eq!(due, vec![1]);
    }

    #[test]
    fn an_unsatisfiable_schedule_yields_nothing_rather_than_spinning() {
        let roots = roots_from(
            r#"
[[roots]]
path = "/a"
schedule = "0 0 30 2 *"
"#,
        );
        assert!(next_due(&roots, at(2026, 9, 7, 0, 0)).is_none());
    }

    #[test]
    fn a_satisfiable_root_still_wins_when_another_is_impossible() {
        let roots = roots_from(
            r#"
[[roots]]
path = "/impossible"
schedule = "0 0 30 2 *"
[[roots]]
path = "/nightly"
schedule = "0 3 * * *"
"#,
        );
        let (fire_at, due) = next_due(&roots, at(2026, 9, 7, 0, 0)).unwrap();
        assert_eq!(fire_at, at(2026, 9, 7, 3, 0));
        assert_eq!(due, vec![1]);
    }

    #[test]
    fn the_next_due_time_is_always_in_the_future() {
        let roots = roots_from(
            r#"
[[roots]]
path = "/a"
schedule = "0 3 * * *"
"#,
        );
        // Standing exactly on a firing minute must roll to the next day, so the
        // scheduler loop cannot fire the same minute twice.
        let exactly = at(2026, 9, 7, 3, 0);
        let (fire_at, _) = next_due(&roots, exactly).unwrap();
        assert_eq!(fire_at, at(2026, 9, 8, 3, 0));
    }
}
