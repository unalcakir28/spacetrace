//! Running scans on behalf of the schedule or an HTTP request.
//!
//! Everything that touches the database goes through here, for one reason: a
//! scan of `/` on a busy NAS can take minutes, and two of them running at once
//! would double the I/O for no benefit while producing two snapshots nobody
//! asked for. A root is therefore scanned by at most one task at a time.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use spacetrace_scan_core::{scan, Phase, ScanProgress, StallWatch, STALL_GRACE};
use spacetrace_store::{ScanId, ScanMeta, Store};

use crate::config::{Config, RootConfig};

/// How often the watcher thread looks at a running scan's counters.
///
/// One second rather than something finer: the only question it answers is
/// "have these moved in the last ten", and a tighter loop would wake a sleeping
/// NAS more often for no better answer.
const WATCH_EVERY: Duration = Duration::from_secs(1);

/// What a completed scan produced.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ScanOutcome {
    pub scan_id: ScanId,
    pub root: String,
    pub total_size: u64,
    pub total_alloc: u64,
    pub files: u64,
    pub dirs: u64,
    pub errors: u64,
    pub duration_ms: u64,
    /// Snapshots dropped by this root's retention policy after saving.
    pub pruned: usize,
}

/// Returned instead of an outcome when the same root is already being scanned.
#[derive(Debug)]
pub struct AlreadyRunning {
    pub root: PathBuf,
}

impl std::fmt::Display for AlreadyRunning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a scan of {} is already running", self.root.display())
    }
}

impl std::error::Error for AlreadyRunning {}

/// What a scan still running looks like from outside it.
///
/// Assembled for `/status`. Everything here is a snapshot of atomics read one
/// after another, so the counters can disagree with each other by a few
/// entries; that is the same looseness the CLI's progress line has always had
/// and it is invisible at the scale these numbers are read at.
#[derive(Debug, Clone, serde::Serialize)]
pub struct InFlight {
    pub root: String,
    pub elapsed_ms: u64,
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub errors: u64,
    pub clones_probed: u64,
    /// `walking` or `finishing`. After the walk only `clones_probed` moves, so
    /// a reader who does not know the phase reads a healthy scan as a stuck one.
    pub phase: &'static str,
    /// How long every counter has stood still, once that is worth mentioning.
    /// Absent while the scan is moving.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stalled_ms: Option<u64>,
    /// Directories being listed right now. Sent only while stalled, because
    /// that is the only time the answer is worth anything: during a healthy
    /// scan it is a different directory by the time it is read.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub waiting_on: Vec<String>,
}

/// A scan the runner is currently running.
struct Tracked {
    progress: Arc<ScanProgress>,
    started: Instant,
    /// Milliseconds the counters have been still, or 0 while moving. Written
    /// only by this scan's watcher thread.
    ///
    /// A watcher rather than a calculation at request time: a stall is
    /// measured from when the counters *stopped*, which nobody can know from a
    /// single observation. Computing it inside `/status` would report the time
    /// since the previous request instead, and get longer the less often the
    /// endpoint is polled.
    stalled_ms: Arc<AtomicU64>,
}

pub struct Runner {
    db: PathBuf,
    host: String,
    roots: Vec<RootConfig>,
    /// Roots with a scan in flight, and how each one is doing. Held across an
    /// entire scan, so it must never be locked while blocking on anything else.
    in_flight: Mutex<HashMap<PathBuf, Tracked>>,
    /// Poll interval and grace period handed to each scan's stall watcher.
    ///
    /// A field rather than the constants read directly, so a test can check
    /// that `try_claim` actually *starts* a watcher without waiting out the
    /// real ten seconds. Production always gets the constants; only the
    /// numbers vary, never the path through the code.
    stall_timings: (Duration, Duration),
}

impl Runner {
    pub fn new(config: &Config) -> Self {
        Runner {
            db: config.db.clone(),
            host: Store::local_host(),
            roots: config.roots.clone(),
            in_flight: Mutex::new(HashMap::new()),
            stall_timings: (WATCH_EVERY, STALL_GRACE),
        }
    }

    /// Shorten the stall watcher's clock. Tests only.
    #[cfg(test)]
    fn watch_fast(&mut self) {
        self.stall_timings = (Duration::from_millis(2), Duration::from_millis(20));
    }

    pub fn db_path(&self) -> &Path {
        &self.db
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn roots(&self) -> &[RootConfig] {
        &self.roots
    }

    /// The configured entry for `path`, if the agent was told about it.
    pub fn configured_root(&self, path: &Path) -> Option<&RootConfig> {
        self.roots.iter().find(|r| r.path == path)
    }

    /// Open the snapshot database, creating its parent directory if needed.
    pub fn open_store(&self) -> Result<Store> {
        if let Some(parent) = self.db.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("cannot create directory {}", parent.display()))?;
            }
        }
        Store::open(&self.db)
    }

    /// Scan one root, store it, and apply that root's retention policy.
    ///
    /// Blocking: this walks the filesystem and writes SQLite. Callers on an
    /// async runtime must put it on a blocking thread.
    pub fn scan_root(&self, root: &RootConfig) -> Result<ScanOutcome> {
        let progress = Arc::new(ScanProgress::default());
        // Registered together with the claim rather than after it: a `/status`
        // arriving in between would otherwise see a root listed as scanning
        // with no counters at all.
        let _guard = self
            .try_claim(&root.path, Arc::clone(&progress))
            .ok_or_else(|| AlreadyRunning {
                root: root.path.clone(),
            })?;

        let (tree, stats) = scan(&root.path, root.scan_options(), Arc::clone(&progress))
            .with_context(|| format!("scanning {}", root.path.display()))?;

        let mut store = self.open_store()?;
        let scan_id = store
            .save(&tree, &stats, &self.host, root.label.as_deref())
            .with_context(|| format!("saving snapshot of {}", root.path.display()))?;

        // Retention runs against the canonical root the scanner recorded, not
        // the configured string, so a symlinked or relative-ish path still
        // matches the rows that were just written.
        let stored_root = tree.root_path().to_string_lossy().into_owned();
        let pruned = match root.keep {
            Some(keep) => store
                .prune_target(&stored_root, &self.host, keep)
                .with_context(|| format!("pruning snapshots of {stored_root}"))?,
            None => 0,
        };

        Ok(ScanOutcome {
            scan_id,
            root: stored_root,
            total_size: tree.total_size(),
            total_alloc: tree.total_alloc(),
            files: stats.files,
            dirs: stats.dirs,
            errors: stats.errors,
            duration_ms: stats.duration_ms,
            pruned,
        })
    }

    /// Snapshots this agent holds, newest first.
    pub fn list_scans(&self) -> Result<Vec<ScanMeta>> {
        self.open_store()?.list()
    }

    pub fn scan_meta(&self, id: ScanId) -> Result<Option<ScanMeta>> {
        self.open_store()?.scan(id)
    }

    /// Write one snapshot to a standalone file for transfer.
    pub fn export(&self, id: ScanId, out: &Path) -> Result<()> {
        self.open_store()?.export_snapshot(id, out)
    }

    pub fn in_flight_roots(&self) -> Vec<PathBuf> {
        let guard = self.in_flight.lock().unwrap_or_else(|e| e.into_inner());
        let mut roots: Vec<PathBuf> = guard.keys().cloned().collect();
        roots.sort();
        roots
    }

    /// Every running scan and how far it has got, sorted by root.
    pub fn in_flight(&self) -> Vec<InFlight> {
        let now = Instant::now();
        let guard = self.in_flight.lock().unwrap_or_else(|e| e.into_inner());
        let mut running: Vec<InFlight> = guard
            .iter()
            .map(|(root, tracked)| {
                // Read unconditionally and let `render` decide whether it is
                // worth reporting: one mutex lock per running scan per request
                // costs nothing, and it keeps the "only while stalled" rule in
                // one testable place rather than split across two.
                let reading = tracked.progress.reading_now();
                render(root, tracked, now, reading)
            })
            .collect();
        running.sort_by(|a, b| a.root.cmp(&b.root));
        running
    }

    fn try_claim(&self, root: &Path, progress: Arc<ScanProgress>) -> Option<RootGuard<'_>> {
        let done = Arc::new(AtomicBool::new(false));
        let stalled_ms = Arc::new(AtomicU64::new(0));
        {
            let mut guard = self.in_flight.lock().unwrap_or_else(|e| e.into_inner());
            if guard.contains_key(root) {
                return None;
            }
            guard.insert(
                root.to_path_buf(),
                Tracked {
                    progress: Arc::clone(&progress),
                    started: Instant::now(),
                    stalled_ms: Arc::clone(&stalled_ms),
                },
            );
        }
        let (every, grace) = self.stall_timings;
        watch_for_stall(progress, stalled_ms, done.clone(), every, grace);
        Some(RootGuard {
            runner: self,
            root: root.to_path_buf(),
            done,
        })
    }
}

/// The phase as `/status` names it.
///
/// Its own function so the words can be pinned by a test: `Phase` cannot be
/// set from outside the scanner, so a `ScanProgress` in a test is always
/// walking and the other arm would never be reached.
fn phase_word(phase: Phase) -> &'static str {
    match phase {
        Phase::Walking => "walking",
        Phase::Finishing => "finishing",
    }
}

/// One running scan, as `/status` reports it.
///
/// Separate from `in_flight` so the reporting rules — which counters, which
/// phase word, when a path list is worth sending — can be tested without a
/// filesystem or a clock.
fn render(root: &Path, tracked: &Tracked, now: Instant, reading: Vec<PathBuf>) -> InFlight {
    let p = &tracked.progress;
    let stalled = match tracked.stalled_ms.load(Ordering::Relaxed) {
        0 => None,
        ms => Some(ms),
    };
    InFlight {
        root: root.to_string_lossy().into_owned(),
        elapsed_ms: now
            .checked_duration_since(tracked.started)
            .unwrap_or_default()
            .as_millis() as u64,
        files: p.files.load(Ordering::Relaxed),
        dirs: p.dirs.load(Ordering::Relaxed),
        bytes: p.bytes.load(Ordering::Relaxed),
        errors: p.errors.load(Ordering::Relaxed),
        clones_probed: p.clones_probed.load(Ordering::Relaxed),
        phase: phase_word(p.phase()),
        stalled_ms: stalled,
        // Only while stalled. During a healthy scan the answer is already
        // stale by the time it is read, and a list of directories that were
        // being listed a moment ago reads as a problem when there is none.
        waiting_on: match stalled {
            Some(_) => reading
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            None => Vec::new(),
        },
    }
}

/// Keep one running scan's stall time up to date until it finishes.
///
/// A plain thread, not a tokio task: `scan_root` is called from the scheduler
/// as well as from HTTP, and the scheduler has no runtime of its own. One
/// sleeping thread per running scan, and a root is scanned by at most one task
/// at a time, so this is bounded by the number of configured roots.
/// `every` and `grace` are arguments rather than the constants directly so a
/// test can watch a stall happen in milliseconds. Without that the only way to
/// cover this function is to wait out the real grace period, which means it
/// would not be covered — and a watcher that never reports anything looks
/// exactly like a scan that never stalls.
fn watch_for_stall(
    progress: Arc<ScanProgress>,
    stalled_ms: Arc<AtomicU64>,
    done: Arc<AtomicBool>,
    every: Duration,
    grace: Duration,
) {
    std::thread::Builder::new()
        .name("spacetrace-stall-watch".to_string())
        .spawn(move || {
            let mut watch = StallWatch::new(Instant::now(), grace);
            while !done.load(Ordering::Relaxed) {
                std::thread::sleep(every);
                let stalled = watch
                    .observe(&progress, Instant::now())
                    .map_or(0, |d| d.as_millis() as u64);
                stalled_ms.store(stalled, Ordering::Relaxed);
            }
        })
        // A scan must not fail because the machine would not give us a thread
        // for watching it; the counters are still there to be read, only the
        // stall verdict is missing.
        .ok();
}

/// Releases the in-flight claim even if the scan panics, and stops the
/// watcher thread with it.
struct RootGuard<'a> {
    runner: &'a Runner,
    root: PathBuf,
    done: Arc<AtomicBool>,
}

impl Drop for RootGuard<'_> {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Relaxed);
        let mut guard = self
            .runner
            .in_flight
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        guard.remove(&self.root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn config_for(dir: &Path, db: &Path) -> Config {
        let toml = format!(
            "db = {:?}\n[[roots]]\npath = {:?}\nkeep = 2\n",
            db.to_string_lossy(),
            dir.to_string_lossy()
        );
        toml::from_str(&toml).unwrap()
    }

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), vec![b'a'; 2048]).unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub/b.bin"), vec![0u8; 4096]).unwrap();
        dir
    }

    #[test]
    fn scanning_a_root_stores_a_snapshot() {
        let dir = fixture();
        let home = tempfile::tempdir().unwrap();
        let db = home.path().join("nested/snapshots.sqlite");
        let cfg = config_for(dir.path(), &db);
        let runner = Runner::new(&cfg);

        let outcome = runner.scan_root(&cfg.roots[0]).unwrap();

        assert_eq!(outcome.files, 2);
        assert!(outcome.total_size >= 2048 + 4096);
        assert_eq!(outcome.pruned, 0);
        assert!(db.exists(), "the parent directory should be created");

        let scans = runner.list_scans().unwrap();
        assert_eq!(scans.len(), 1);
        assert_eq!(scans[0].id, outcome.scan_id);
    }

    #[test]
    fn retention_applies_per_root_after_each_scan() {
        let dir = fixture();
        let home = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path(), &home.path().join("db.sqlite"));
        let runner = Runner::new(&cfg);

        for _ in 0..4 {
            runner.scan_root(&cfg.roots[0]).unwrap();
        }

        // keep = 2, so the fourth scan should have dropped one more.
        let scans = runner.list_scans().unwrap();
        assert_eq!(scans.len(), 2, "keep = 2 should bound the history");
    }

    #[test]
    fn a_root_without_a_keep_setting_retains_everything() {
        let dir = fixture();
        let home = tempfile::tempdir().unwrap();
        let toml = format!(
            "db = {:?}\n[[roots]]\npath = {:?}\n",
            home.path().join("db.sqlite").to_string_lossy(),
            dir.path().to_string_lossy()
        );
        let cfg: Config = toml::from_str(&toml).unwrap();
        let runner = Runner::new(&cfg);

        for _ in 0..3 {
            runner.scan_root(&cfg.roots[0]).unwrap();
        }
        assert_eq!(runner.list_scans().unwrap().len(), 3);
    }

    #[test]
    fn the_same_root_cannot_be_scanned_twice_at_once() {
        let dir = fixture();
        let home = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path(), &home.path().join("db.sqlite"));
        let runner = Arc::new(Runner::new(&cfg));

        // Claim the root directly, mimicking a scan already in progress.
        let claim = runner.try_claim(&cfg.roots[0].path, Arc::new(ScanProgress::default()));
        assert!(claim.is_some());
        assert_eq!(runner.in_flight_roots().len(), 1);

        let err = runner.scan_root(&cfg.roots[0]).unwrap_err();
        assert!(
            err.downcast_ref::<AlreadyRunning>().is_some(),
            "expected AlreadyRunning, got: {err:#}"
        );

        drop(claim);
        assert!(runner.in_flight_roots().is_empty());
        // Once the claim is released the root is scannable again.
        runner.scan_root(&cfg.roots[0]).unwrap();
    }

    #[test]
    fn a_released_claim_does_not_block_a_different_root() {
        let dir_a = fixture();
        let dir_b = fixture();
        let home = tempfile::tempdir().unwrap();
        let toml = format!(
            "db = {:?}\n[[roots]]\npath = {:?}\n[[roots]]\npath = {:?}\n",
            home.path().join("db.sqlite").to_string_lossy(),
            dir_a.path().to_string_lossy(),
            dir_b.path().to_string_lossy()
        );
        let cfg: Config = toml::from_str(&toml).unwrap();
        let runner = Runner::new(&cfg);

        let _claim = runner.try_claim(&cfg.roots[0].path, Arc::new(ScanProgress::default()));
        runner
            .scan_root(&cfg.roots[1])
            .expect("a different root must not be blocked");
    }

    #[test]
    fn exporting_a_scan_produces_a_loadable_file() {
        let dir = fixture();
        let home = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path(), &home.path().join("db.sqlite"));
        let runner = Runner::new(&cfg);
        let outcome = runner.scan_root(&cfg.roots[0]).unwrap();

        let out = home.path().join("snap.sqlite");
        runner.export(outcome.scan_id, &out).unwrap();

        let exported = Store::open(&out).unwrap();
        assert_eq!(exported.list().unwrap().len(), 1);
    }

    #[test]
    fn configured_root_lookup_matches_on_the_configured_path() {
        let dir = fixture();
        let home = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path(), &home.path().join("db.sqlite"));
        let runner = Runner::new(&cfg);

        assert!(runner.configured_root(dir.path()).is_some());
        assert!(runner.configured_root(Path::new("/nope")).is_none());
    }

    // ------------------------------------------------- what /status reports

    fn tracked(stalled_ms: u64) -> Tracked {
        Tracked {
            progress: Arc::new(ScanProgress::default()),
            started: Instant::now(),
            stalled_ms: Arc::new(AtomicU64::new(stalled_ms)),
        }
    }

    /// The reason this endpoint changed shape: a path alone cannot say whether
    /// a scan is working.
    #[test]
    fn a_running_scan_reports_its_counters() {
        let t = tracked(0);
        t.progress.files.store(42, Ordering::Relaxed);
        t.progress.dirs.store(7, Ordering::Relaxed);
        t.progress.bytes.store(1024, Ordering::Relaxed);
        t.progress.errors.store(1, Ordering::Relaxed);
        t.progress.clones_probed.store(3, Ordering::Relaxed);

        let out = render(Path::new("/data"), &t, Instant::now(), Vec::new());
        assert_eq!(out.root, "/data");
        assert_eq!((out.files, out.dirs, out.bytes), (42, 7, 1024));
        assert_eq!((out.errors, out.clones_probed), (1, 3));
    }

    /// Invariant 8 from the other side: after the walk only `clones_probed`
    /// moves, so a reader who cannot see the phase reads a healthy scan as a
    /// stuck one.
    #[test]
    fn the_phase_is_named_not_guessed() {
        assert_eq!(phase_word(Phase::Walking), "walking");
        assert_eq!(phase_word(Phase::Finishing), "finishing");
        // And a fresh scan really does start in the first of them, so the
        // word above is the one a reader sees.
        let t = tracked(0);
        assert_eq!(
            render(Path::new("/d"), &t, Instant::now(), Vec::new()).phase,
            "walking"
        );
    }

    #[test]
    fn a_moving_scan_reports_no_stall() {
        let out = render(Path::new("/d"), &tracked(0), Instant::now(), Vec::new());
        assert_eq!(out.stalled_ms, None);
    }

    /// The paths are the answer to "stuck on what", so they travel only with a
    /// stall. During a healthy scan the list is already out of date by the
    /// time it is read, and publishing it makes a working scan look wedged.
    #[test]
    fn directories_being_listed_travel_only_with_a_stall() {
        let reading = vec![PathBuf::from("/mnt/dead/x"), PathBuf::from("/mnt/dead/y")];

        let moving = render(
            Path::new("/d"),
            &tracked(0),
            Instant::now(),
            reading.clone(),
        );
        assert!(
            moving.waiting_on.is_empty(),
            "a moving scan must not publish the directories it is reading"
        );

        let stuck = render(Path::new("/d"), &tracked(12_000), Instant::now(), reading);
        assert_eq!(stuck.stalled_ms, Some(12_000));
        assert_eq!(stuck.waiting_on, vec!["/mnt/dead/x", "/mnt/dead/y"]);
    }

    /// The wire contract. `/status` is read by `curl` and by whatever an
    /// operator has wired to it; renaming a field here is a breaking change
    /// and should have to be done deliberately.
    #[test]
    fn the_status_payload_has_the_documented_shape() {
        let t = tracked(0);
        t.progress.files.store(5, Ordering::Relaxed);
        let json =
            serde_json::to_value(render(Path::new("/d"), &t, Instant::now(), Vec::new())).unwrap();

        // Names, not order: `serde_json::Value` sorts its keys and the order
        // on the wire was never part of the contract.
        let keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        assert_eq!(
            keys,
            vec![
                "bytes",
                "clones_probed",
                "dirs",
                "elapsed_ms",
                "errors",
                "files",
                "phase",
                "root"
            ],
            "a moving scan sends neither stalled_ms nor waiting_on"
        );

        let stuck = serde_json::to_value(render(
            Path::new("/d"),
            &tracked(11_000),
            Instant::now(),
            vec![PathBuf::from("/mnt/dead")],
        ))
        .unwrap();
        assert_eq!(stuck["stalled_ms"], 11_000);
        assert_eq!(stuck["waiting_on"][0], "/mnt/dead");
    }

    /// A scan that has only just started must still be listed. It is the one
    /// most likely to be wedged on its own root, where every counter is zero
    /// and the path is all there is to report.
    #[test]
    fn a_scan_with_no_progress_yet_is_still_listed() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("s.sqlite");
        let cfg = config_for(dir.path(), &db);
        let runner = Runner::new(&cfg);

        let _claim = runner.try_claim(&cfg.roots[0].path, Arc::new(ScanProgress::default()));
        let running = runner.in_flight();
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].files, 0);
        assert_eq!(running[0].phase, "walking");
    }

    /// Wait for `check` to hold, or give up. Generous on purpose: the claim is
    /// that the watcher reports a stall *at all*, not that it does so quickly,
    /// and a loaded CI machine must not be able to turn that into a failure.
    fn within(deadline: Duration, mut check: impl FnMut() -> bool) -> bool {
        let until = Instant::now() + deadline;
        while Instant::now() < until {
            if check() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        false
    }

    /// The feature itself: a scan whose counters stop moving becomes visible.
    /// Without this test the watcher could store nothing at all and every
    /// other test here would still pass — a hung scan would go on being
    /// silent, which is the bug this work exists to fix.
    #[test]
    fn the_watcher_reports_a_scan_that_stops_moving_and_clears_when_it_resumes() {
        let progress = Arc::new(ScanProgress::default());
        let stalled = Arc::new(AtomicU64::new(0));
        let done = Arc::new(AtomicBool::new(false));
        watch_for_stall(
            Arc::clone(&progress),
            Arc::clone(&stalled),
            Arc::clone(&done),
            Duration::from_millis(2),
            Duration::from_millis(20),
        );

        assert!(
            within(Duration::from_secs(5), || stalled.load(Ordering::Relaxed)
                > 0),
            "counters never moved, so the watcher should have reported a stall"
        );

        progress.files.fetch_add(1, Ordering::Relaxed);
        assert!(
            within(Duration::from_secs(5), || stalled.load(Ordering::Relaxed)
                == 0),
            "the scan started moving again, so the stall should have cleared"
        );

        done.store(true, Ordering::Relaxed);
    }

    /// The wiring, not the mechanism: claiming a root must actually start a
    /// watcher. A mutation that deletes that call leaves every other test
    /// here green while making `/status` stay silent about a wedged scan.
    #[test]
    fn claiming_a_root_starts_its_stall_watcher() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("s.sqlite");
        let cfg = config_for(dir.path(), &db);
        let mut runner = Runner::new(&cfg);
        runner.watch_fast();

        let _claim = runner.try_claim(&cfg.roots[0].path, Arc::new(ScanProgress::default()));
        assert!(
            within(Duration::from_secs(5), || runner.in_flight()[0]
                .stalled_ms
                .is_some()),
            "nothing moved this scan's counters, so /status should say it is stalled"
        );
    }
}
