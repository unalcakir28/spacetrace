//! Running scans on behalf of the schedule or an HTTP request.
//!
//! Everything that touches the database goes through here, for one reason: a
//! scan of `/` on a busy NAS can take minutes, and two of them running at once
//! would double the I/O for no benefit while producing two snapshots nobody
//! asked for. A root is therefore scanned by at most one task at a time.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use spacetrace_scan_core::{scan, ScanProgress};
use spacetrace_store::{ScanId, ScanMeta, Store};

use crate::config::{Config, RootConfig};

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

pub struct Runner {
    db: PathBuf,
    host: String,
    roots: Vec<RootConfig>,
    /// Roots with a scan in flight. Held across an entire scan, so it must
    /// never be locked while blocking on anything else.
    in_flight: Mutex<HashSet<PathBuf>>,
}

impl Runner {
    pub fn new(config: &Config) -> Self {
        Runner {
            db: config.db.clone(),
            host: Store::local_host(),
            roots: config.roots.clone(),
            in_flight: Mutex::new(HashSet::new()),
        }
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
        let _guard = self.try_claim(&root.path).ok_or_else(|| AlreadyRunning {
            root: root.path.clone(),
        })?;

        let progress = Arc::new(ScanProgress::default());
        let (tree, stats) = scan(&root.path, root.scan_options(), progress)
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
        let mut roots: Vec<PathBuf> = guard.iter().cloned().collect();
        roots.sort();
        roots
    }

    fn try_claim(&self, root: &Path) -> Option<RootGuard<'_>> {
        let mut guard = self.in_flight.lock().unwrap_or_else(|e| e.into_inner());
        if !guard.insert(root.to_path_buf()) {
            return None;
        }
        Some(RootGuard {
            runner: self,
            root: root.to_path_buf(),
        })
    }
}

/// Releases the in-flight claim even if the scan panics.
struct RootGuard<'a> {
    runner: &'a Runner,
    root: PathBuf,
}

impl Drop for RootGuard<'_> {
    fn drop(&mut self) {
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
        let claim = runner.try_claim(&cfg.roots[0].path);
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

        let _claim = runner.try_claim(&cfg.roots[0].path);
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
}
