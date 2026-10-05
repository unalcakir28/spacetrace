//! Snapshot storage.
//!
//! A snapshot is one scan of one root on one host, stored in SQLite so that a
//! later scan can be compared against it. The arena layout from `scan-core` is
//! written verbatim, which makes loading a snapshot a single ordered query with
//! no tree rebuilding.

mod csv;
mod digest;
#[cfg(feature = "dupes")]
mod hash_cache;
mod ncdu;
mod ncdu_import;
mod schema;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use spacetrace_scan_core::{
    Base, EntryKind, Fallback, Phase, RescanKind, ScanProgress, ScanStats, StoredNode, Tree,
    TreeAssembler,
};

pub use csv::export_csv;
#[cfg(feature = "dupes")]
pub use hash_cache::SqliteHashCache;
pub use ncdu::export_ncdu;
pub use ncdu_import::import_ncdu;

pub type ScanId = i64;

pub const SCANNER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// What `scanner_version` starts with when the tree was not scanned here but
/// imported from another tool's export (`spacetrace import`).
///
/// An export says nothing about which machine it describes, and `import`
/// files it under this one unless told otherwise — so the host column cannot
/// tell an import from a scan, and anything that reads this machine's disk
/// against a snapshot (`spacetrace pkgs --scan`) needs to. The column is the
/// one that already answers "what produced these numbers", and for an import
/// that is not this scanner: before this marker it claimed it was.
pub const IMPORTED_PREFIX: &str = "import/";

/// Everything about a stored scan except its entries.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScanMeta {
    pub id: ScanId,
    pub host: String,
    pub root: String,
    /// Unix seconds when the scan started.
    pub started_at: i64,
    pub duration_ms: u64,
    pub total_size: u64,
    pub total_alloc: u64,
    pub files: u64,
    pub dirs: u64,
    pub errors: u64,
    pub hardlinks_deduped: u64,
    pub scanner_version: String,
    pub label: Option<String>,
    /// Size of the filesystem the root sits on, when it could be measured.
    /// `None` for snapshots taken before schema v2, or on a platform that
    /// cannot answer — which is different from zero.
    #[serde(default)]
    pub fs_total: Option<u64>,
    /// Bytes available to the scanning user on that filesystem.
    #[serde(default)]
    pub fs_available: Option<u64>,
}

impl ScanMeta {
    /// Fraction of the filesystem still available at scan time, `0.0..=1.0`.
    ///
    /// Free rather than used on purpose: on a shared-space filesystem (APFS,
    /// btrfs, thin LVM) "used" would include sibling volumes and disagree with
    /// what `df` prints, while "available" is the same number on both.
    pub fn fs_free_fraction(&self) -> Option<f64> {
        let (total, available) = (self.fs_total?, self.fs_available?);
        if total == 0 {
            return None;
        }
        Some(available.min(total) as f64 / total as f64)
    }
}

impl ScanMeta {
    /// Whether this tree came from another tool's export rather than a scan.
    pub fn is_import(&self) -> bool {
        self.scanner_version.starts_with(IMPORTED_PREFIX)
    }

    /// `host:root`, the identity used to decide which scans are comparable.
    pub fn target(&self) -> String {
        format!("{}:{}", self.host, self.root)
    }
}

/// What checking a snapshot's digest concluded.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Integrity {
    /// The content matches the digest stored with it.
    Intact,
    /// No digest was stored. Snapshots written before schema v3 have none,
    /// and that is different from failing a check — there is nothing to check.
    Unknown,
    /// The content does not match. Reported rather than repaired: which of the
    /// two is wrong cannot be known from here.
    Mismatch { stored: String, computed: String },
}

pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (or create) a snapshot database.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let conn = Connection::open(path)
            .with_context(|| format!("opening snapshot database {}", path.display()))?;
        schema::migrate(&conn)?;
        Ok(Store { conn })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        schema::migrate(&conn)?;
        Ok(Store { conn })
    }

    /// Begin a write transaction, taking the write lock at `BEGIN`.
    ///
    /// Not `transaction()`, which is DEFERRED: that starts as a reader and
    /// upgrades on the first write, and SQLite answers a failed upgrade with
    /// SQLITE_BUSY *immediately*, without consulting the busy handler — so
    /// `busy_timeout` cannot save it. Taking the lock up front is the only way
    /// a concurrent writer waits its turn instead of failing.
    fn write_transaction(&mut self) -> Result<rusqlite::Transaction<'_>> {
        Ok(self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?)
    }

    /// Name this machine reports itself as. Snapshots from different hosts stay
    /// distinguishable in one database.
    pub fn local_host() -> String {
        gethostname::gethostname().to_string_lossy().into_owned()
    }

    /// Write a scan and its whole tree in one transaction.
    pub fn save(
        &mut self,
        tree: &Tree,
        stats: &ScanStats,
        host: &str,
        label: Option<&str>,
    ) -> Result<ScanId> {
        self.save_reporting(tree, stats, host, label, &ScanProgress::default())
    }

    /// `save`, reporting how far it has got.
    ///
    /// Writing a tree is not a quick tail on the end of a scan: measured at
    /// **571 ms** for the 412,983 entries of `/Applications` against 753 ms
    /// for the walk itself, which extrapolates to about fourteen seconds at
    /// ten million. Invariant 8 says a phase that long needs a counter, and
    /// without one the CLI cleared its progress line the moment the walk ended
    /// and then sat silent — the exact shape of "it looks stuck".
    ///
    /// One pass and one phase: the digest is hashed from the values as they
    /// are bound. It used to be a second pass reading every row back (208 ms
    /// after 273 ms of writing on that tree), reported as `Checksumming`.
    pub fn save_reporting(
        &mut self,
        tree: &Tree,
        stats: &ScanStats,
        host: &str,
        label: Option<&str>,
        progress: &ScanProgress,
    ) -> Result<ScanId> {
        self.save_as(tree, stats, host, label, progress, Origin::Scanned)
    }

    /// `save`, for a tree read from another tool's export: marked with
    /// [`IMPORTED_PREFIX`], so it can be told from a scan later.
    pub fn save_import(
        &mut self,
        tree: &Tree,
        stats: &ScanStats,
        host: &str,
        label: Option<&str>,
    ) -> Result<ScanId> {
        self.save_as(
            tree,
            stats,
            host,
            label,
            &ScanProgress::default(),
            Origin::Imported,
        )
    }

    fn save_as(
        &mut self,
        tree: &Tree,
        stats: &ScanStats,
        host: &str,
        label: Option<&str>,
        progress: &ScanProgress,
        origin: Origin,
    ) -> Result<ScanId> {
        let started_at = now_unix() - (stats.duration_ms / 1000) as i64;
        let scanner_version = match origin {
            Origin::Scanned => SCANNER_VERSION.to_string(),
            Origin::Imported => format!("{IMPORTED_PREFIX}{SCANNER_VERSION}"),
        };
        let root = tree.root_path().to_string_lossy();
        // Every value is written once, here, and both bound and hashed from
        // this one place, so what is stored and what is digested cannot part.
        let meta = digest::ScanRow {
            host,
            root: &root,
            started_at,
            duration_ms: stats.duration_ms as i64,
            total_size: tree.total_size() as i64,
            total_alloc: tree.total_alloc() as i64,
            files: stats.files as i64,
            dirs: stats.dirs as i64,
            errors: stats.errors as i64,
            hardlinks_deduped: stats.hardlinks_deduped as i64,
            scanner_version: &scanner_version,
            label,
            fs_total: stats.capacity.map(|c| c.total as i64),
            fs_available: stats.capacity.map(|c| c.available as i64),
        };
        let tx = self.write_transaction()?;
        tx.execute(
            "INSERT INTO scans (host, root, started_at, duration_ms, total_size, total_alloc,
                                files, dirs, errors, hardlinks_deduped, scanner_version, label,
                                fs_total, fs_available)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                meta.host,
                meta.root,
                meta.started_at,
                meta.duration_ms,
                meta.total_size,
                meta.total_alloc,
                meta.files,
                meta.dirs,
                meta.errors,
                meta.hardlinks_deduped,
                meta.scanner_version,
                meta.label,
                meta.fs_total,
                meta.fs_available,
            ],
        )?;
        let scan_id = tx.last_insert_rowid();

        // Hashed as it is written rather than read back afterwards: the
        // digest is `digest::Running` either way, and the second pass over
        // every row cost 208 ms against 273 ms of writing on 412,983 entries.
        let mut digest = digest::Running::scan(&meta);
        {
            progress.begin_rows(Phase::Saving, tree.len() as u64);
            let mut rows = RowCounter::new(Some(progress));
            let mut stmt = tx.prepare(
                "INSERT INTO entries (scan_id, id, parent_id, name, kind, size, alloc, mtime,
                                      nlink, files, dirs, children_start, children_len)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            )?;
            for (idx, node) in tree.nodes().iter().enumerate() {
                let row = digest::EntryRow {
                    id: idx as i64,
                    parent: node.has_parent().then_some(node.parent as i64),
                    name: tree.name(idx as u32),
                    kind: i64::from(node.kind as u8),
                    size: node.size as i64,
                    alloc: node.alloc as i64,
                    mtime: node.mtime,
                    nlink: i64::from(node.nlink),
                    files: i64::from(node.files),
                    dirs: i64::from(node.dirs),
                    children_start: i64::from(node.children_start),
                    children_len: i64::from(node.children_len),
                };
                stmt.execute(params![
                    scan_id,
                    row.id,
                    row.parent,
                    row.name,
                    row.kind,
                    row.size,
                    row.alloc,
                    row.mtime,
                    row.nlink,
                    row.files,
                    row.dirs,
                    row.children_start,
                    row.children_len,
                ])?;
                digest.entry(&row);
                rows.tick();
            }
        }
        let content_hash = digest.finish();
        tx.execute(
            "UPDATE scans SET content_hash = ?1 WHERE id = ?2",
            params![content_hash, scan_id],
        )?;

        // An imported tree was not walked here: it has no journal position,
        // no flags worth the name, and no record of how it was read. No row
        // is what a rescan reads as "nothing to start from".
        if let Origin::Scanned = origin {
            schema::create_rescan_state(&tx)?;
            let record = stats.rescan.record();
            let flags = encode_flags(tree);
            let check =
                digest::rescan_state(&content_hash, stats.journal.as_deref(), &record, &flags);
            tx.execute(
                "INSERT INTO rescan_state (scan_id, journal, rescan, flags, digest)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![scan_id, stats.journal, record, flags, check],
            )?;
        }

        tx.commit()?;
        // Back to zero, so a caller still polling after this returns does not
        // show a finished phase as if it were running.
        progress.begin_rows(Phase::Saving, 0);
        Ok(scan_id)
    }

    /// Load one snapshot's tree back into memory.
    pub fn load(&self, scan_id: ScanId) -> Result<(Tree, ScanMeta)> {
        let meta = self
            .scan(scan_id)?
            .with_context(|| format!("no scan with id {scan_id}"))?;
        let hint = meta.files.saturating_add(meta.dirs);
        let tree = assemble(
            &self.conn,
            "main",
            scan_id,
            hint,
            &meta.root,
            Flags::Unknown,
            None,
            None,
        )?;
        Ok((tree, meta))
    }

    /// The newest scan of `root` by `host`, as the base an incremental
    /// rescan would start from (`spacetrace_scan_core::rescan`); `None` when
    /// there is no scan of it at all.
    ///
    /// Two refusals are decided here, from what only the store knows: a scan
    /// imported from another tool's export, and one dated in the future —
    /// the clock moved backwards, and what "since then" means is no longer
    /// clear. A scan that arrived by `import_snapshot` needs no rule of its
    /// own: the import does not carry the cursor across, so it has none.
    ///
    /// Nothing is read beyond one row until the rescan asks: the tree is
    /// loaded by the returned `load`, which a scan that falls back never
    /// calls.
    pub fn rescan_base(&self, root: &str, host: &str) -> Result<Option<Base<'_>>> {
        let Some(meta) = self.latest_for(root, Some(host))? else {
            return Ok(None);
        };
        // One read for everything the load needs besides the rows.
        let sql = if schema::has_rescan_state(&self.conn)? {
            "SELECT s.content_hash, r.journal, r.flags, r.rescan, r.digest
             FROM scans s LEFT JOIN rescan_state r ON r.scan_id = s.id WHERE s.id = ?1"
        } else {
            "SELECT content_hash, NULL, NULL, NULL, NULL FROM scans WHERE id = ?1"
        };
        let state: StoredState = self.conn.query_row(sql, [meta.id], |row| {
            Ok(StoredState {
                content_hash: row.get(0)?,
                journal: row.get(1)?,
                flags: row.get(2)?,
                rescan: row.get(3)?,
                digest: row.get(4)?,
            })
        })?;
        let journal = if meta.is_import() {
            Err(Fallback::ImportedBase)
        } else if meta.started_at > now_unix() {
            Err(Fallback::FutureBase)
        } else if !state.is_intact() {
            // Checked before the journal is asked: the cursor is one of the
            // values it vouches for.
            Err(Fallback::BaseDamaged)
        } else {
            state.journal.ok_or(Fallback::NoCursor)
        };
        let (content_hash, flags) = (state.content_hash, state.flags);
        Ok(Some(Base {
            journal,
            load: Box::new(move |progress: &ScanProgress| {
                self.load_base(&meta, content_hash, flags, progress)
            }),
        }))
    }

    /// One scan's `rescan_state.rescan`; `None` where there is none — the
    /// table not created yet, a scan this build did not walk.
    fn rescan_record(&self, scan_id: ScanId) -> Result<Option<String>> {
        if !schema::has_rescan_state(&self.conn)? {
            return Ok(None);
        }
        let record = self
            .conn
            .query_row(
                "SELECT rescan FROM rescan_state WHERE scan_id = ?1",
                [scan_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(record)
    }

    /// Load `meta`'s scan as a rescan's base, checked against its digest in
    /// the same pass.
    ///
    /// **The check is the point.** An incremental scan takes over every value
    /// it does not read again, and its own digest is computed over them as
    /// they are — so a base damaged on disk would pass its damage on to a
    /// snapshot that then verifies as intact, and to every rescan after it.
    /// Hashed while the rows are assembled, and compared before the tree is
    /// handed over: one read of the scan, not one to check and one to load.
    ///
    /// The rows are counted under `Walking` (invariant 8): there is no phase
    /// for loading a base, and the walk is what it prepares.
    fn load_base(
        &self,
        meta: &ScanMeta,
        content_hash: Option<String>,
        flags: Option<Vec<u8>>,
        progress: &ScanProgress,
    ) -> Result<Tree, Fallback> {
        // No digest is no evidence: every scan this build saved has one. No
        // flags is no knowledge of what may be copied, and a row this build
        // wrote always has them.
        let stored = content_hash.ok_or(Fallback::BaseDamaged)?;
        let flags =
            decode_flags(&flags.ok_or(Fallback::BaseDamaged)?).ok_or(Fallback::BaseDamaged)?;
        let mut digest = digest::Running::read_scan(&self.conn, "main", meta.id)
            .map_err(|_| Fallback::BaseDamaged)?;
        let hint = meta.files.saturating_add(meta.dirs);
        progress.begin_rows(Phase::Walking, hint);
        let tree = assemble(
            &self.conn,
            "main",
            meta.id,
            hint,
            &meta.root,
            Flags::Sparse(&flags),
            Some(progress),
            Some(&mut digest),
        );
        progress.begin_rows(Phase::Walking, 0);
        let tree = tree.map_err(|_| Fallback::BaseDamaged)?;
        if digest.finish() != stored {
            return Err(Fallback::BaseDamaged);
        }
        Ok(tree)
    }

    /// How a stored scan's tree was produced.
    ///
    /// `None` for a scan this build did not walk — saved before this was
    /// recorded, imported from another tool, or pushed from another machine,
    /// whose record is its sender's business and is not carried across.
    ///
    /// Also `None` for a record this build cannot read — a reason a newer
    /// build added — rather than an error: it says nothing this build could
    /// act on.
    pub fn rescan_of(&self, scan_id: ScanId) -> Result<Option<RescanKind>> {
        anyhow::ensure!(self.scan(scan_id)?.is_some(), "no scan with id {scan_id}");
        Ok(self
            .rescan_record(scan_id)?
            .and_then(|record| RescanKind::parse(&record)))
    }

    /// Recompute a stored scan's digest and compare it with the one written
    /// beside it.
    ///
    /// This is the on-demand check for a snapshot that has been sitting on a
    /// disk. The transfer path does not need it: `export_snapshot` and
    /// `import_snapshot` check by themselves, because a body arriving over the
    /// network is the case nobody would think to check by hand.
    pub fn verify(&self, scan_id: ScanId) -> Result<Integrity> {
        anyhow::ensure!(
            self.scan(scan_id)?.is_some(),
            "no scan with id {scan_id} to verify"
        );
        integrity_of(&self.conn, "main", scan_id)
    }

    pub fn scan(&self, scan_id: ScanId) -> Result<Option<ScanMeta>> {
        let meta = self
            .conn
            .query_row(
                &format!("{} WHERE id = ?1", SELECT_SCAN),
                [scan_id],
                row_to_meta,
            )
            .optional()?;
        Ok(meta)
    }

    /// All scans, newest first.
    pub fn list(&self) -> Result<Vec<ScanMeta>> {
        let mut stmt = self
            .conn
            .prepare(&format!("{SELECT_SCAN} ORDER BY started_at DESC, id DESC"))?;
        let rows = stmt.query_map([], row_to_meta)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The most recent scan of a given root (optionally on a given host).
    pub fn latest_for(&self, root: &str, host: Option<&str>) -> Result<Option<ScanMeta>> {
        let meta = match host {
            // Two statements rather than `(?2 IS NULL OR host = ?2)`: SQLite
            // plans a statement once for every binding, so that form cannot
            // seek the `(host, root, started_at)` index and reads all of it
            // and sorts instead. Measured through `/metrics`, which asks this
            // per root on every scrape: 102 ms at 200,005 snapshots and
            // linear in the store, where the seek does not grow with it.
            Some(host) => self
                .conn
                .query_row(
                    &format!(
                        "{SELECT_SCAN} WHERE host = ?1 AND root = ?2
                         ORDER BY started_at DESC, id DESC LIMIT 1"
                    ),
                    params![host, root],
                    row_to_meta,
                )
                .optional()?,
            None => self
                .conn
                .query_row(
                    &format!(
                        "{SELECT_SCAN} WHERE root = ?1
                         ORDER BY started_at DESC, id DESC LIMIT 1"
                    ),
                    params![root],
                    row_to_meta,
                )
                .optional()?,
        };
        Ok(meta)
    }

    /// How many scans of a given root this host has stored.
    ///
    /// A count rather than `list().len()`: the agent's `/metrics` asks this
    /// on every scrape, and an agent that has kept every snapshot for years
    /// should not read them all to answer with one number. The host is
    /// required so the count is a seek on the `(host, root, …)` index; see
    /// `latest_for` for what the optional form costs.
    pub fn count_for(&self, root: &str, host: &str) -> Result<u64> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM scans WHERE host = ?1 AND root = ?2",
            params![host, root],
            |row| row.get(0),
        )?;
        Ok(count as u64)
    }

    /// The two most recent scans of the same target, newest first. This is what
    /// `spacetrace diff` uses when no ids are given.
    pub fn last_two_for(&self, root: &str, host: Option<&str>) -> Result<Vec<ScanMeta>> {
        let sql = format!(
            "{SELECT_SCAN} WHERE root = ?1 AND (?2 IS NULL OR host = ?2)
             ORDER BY started_at DESC, id DESC LIMIT 2"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![root, host], row_to_meta)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn delete(&self, scan_id: ScanId) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM scans WHERE id = ?1", [scan_id])?;
        Ok(n > 0)
    }

    /// Drop all but the newest `keep` scans of one target.
    ///
    /// The agent needs this because retention is configured per root, so the
    /// database-wide [`Store::prune`] would apply one machine's policy to every
    /// other root in the same file.
    pub fn prune_target(&mut self, root: &str, host: &str, keep: usize) -> Result<usize> {
        let tx = self.write_transaction()?;
        let removed = tx.execute(
            "DELETE FROM scans WHERE id IN (
                 SELECT id FROM (
                     SELECT id, ROW_NUMBER() OVER (
                         ORDER BY started_at DESC, id DESC
                     ) AS rn
                     FROM scans WHERE root = ?1 AND host = ?2
                 ) WHERE rn > ?3
             )",
            params![root, host, keep as i64],
        )?;
        tx.commit()?;
        Ok(removed)
    }

    /// Write one snapshot to its own SQLite file, metadata preserved exactly.
    ///
    /// This is what goes over the wire (see docs/DECISIONS.md K4). It copies
    /// rows into an ATTACHed database rather than `VACUUM INTO`, for two
    /// reasons: the result holds only the requested scan instead of the whole
    /// history, and the new file gets a default rollback journal instead of
    /// inheriting WAL, so it is a single self-contained file the moment the
    /// call returns.
    pub fn export_snapshot(&self, scan_id: ScanId, out: &Path) -> Result<()> {
        anyhow::ensure!(
            self.scan(scan_id)?.is_some(),
            "no scan with id {scan_id} to export"
        );
        // Checked before anything is written: shipping data already known to
        // be wrong is worse than refusing, because the receiver's own check
        // will blame the network for damage that was here all along.
        if let Integrity::Mismatch { stored, computed } = self.verify(scan_id)? {
            anyhow::bail!(
                "scan {scan_id} does not match its own digest and was not exported \
                 (stored {stored}, computed {computed}); the local database is damaged"
            );
        }
        // SQLite will happily open an existing file and merge into it, which
        // would silently produce a snapshot containing someone else's scan.
        if out.exists() {
            std::fs::remove_file(out)
                .with_context(|| format!("replacing existing file {}", out.display()))?;
        }

        self.conn
            .execute("ATTACH DATABASE ?1 AS snap", [out.to_string_lossy()])
            .with_context(|| format!("attaching {}", out.display()))?;

        let result = self.copy_scan_into_attached(scan_id);
        // Detach whether or not the copy worked, so the connection stays usable.
        let detached = self.conn.execute_batch("DETACH DATABASE snap");
        if result.is_err() {
            let _ = std::fs::remove_file(out);
        }
        result?;
        detached.context("detaching the snapshot database")?;
        Ok(())
    }

    /// Take every scan out of a snapshot file and add it to this database.
    ///
    /// Ids are reassigned, because the sender's numbering means nothing here.
    /// Everything else — host, root, timestamps, scanner version — is carried
    /// across verbatim, since that is what makes a pushed snapshot comparable
    /// with the rest of the target's history.
    ///
    /// A scan already present with the same host, root and start time is
    /// skipped, so re-pushing is harmless.
    pub fn import_snapshot(&mut self, incoming: &Path) -> Result<Vec<ScanId>> {
        anyhow::ensure!(
            incoming.exists(),
            "no such snapshot file: {}",
            incoming.display()
        );
        self.conn
            .execute(
                "ATTACH DATABASE ?1 AS incoming",
                [incoming.to_string_lossy()],
            )
            .with_context(|| format!("attaching {}", incoming.display()))?;

        let result = self.copy_scans_from_attached();
        let detached = self.conn.execute_batch("DETACH DATABASE incoming");
        let imported = result?;
        detached.context("detaching the incoming database")?;
        Ok(imported)
    }

    fn copy_scans_from_attached(&mut self) -> Result<Vec<ScanId>> {
        // Every check that reads the whole snapshot runs here, before the
        // write lock: nothing else writes the attached file, so it cannot
        // change between the check and the copy. Under the lock they held
        // other writers off for the whole import, 1.16 s for a million entries
        // and so seconds at the agent's 512 MiB upload limit, against the 30 s
        // busy timeout a scheduled save or a second push waits before failing.
        // Now the lock covers the copy alone: 0.40 s for the same million
        // (`examples/importprobe.rs`, macOS, October 2026).
        check_incoming_shape(&self.conn)?;

        let incoming: Vec<(ScanId, String, String, i64)> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id, host, root, started_at FROM incoming.scans ORDER BY id")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        let mut checked = Vec::with_capacity(incoming.len());
        for (source_id, host, root, started_at) in incoming {
            if self.already_holds(&host, &root, started_at)? {
                continue;
            }

            let ahead = started_at - now_unix();
            anyhow::ensure!(
                ahead <= MAX_CLOCK_AHEAD_SECS,
                "the snapshot of {root} from {host} starts {} hours in the future; its \
                 clock is wrong, and a snapshot dated ahead would stay the newest of its \
                 target for good. Nothing was imported",
                ahead / 3600
            );

            // The one place a body that crossed a network is opened. A flipped
            // bit leaves a perfectly valid tree — `TreeAssembler::finish` is
            // about structure, not values — so without this the scan imports
            // and reports a wrong number with full confidence.
            //
            // The whole import fails rather than this one scan being skipped:
            // the transaction is all-or-nothing, and a push carries exactly
            // one scan, so "skip the bad one" would only ever mean "import
            // nothing" while sounding like partial success.
            if let Integrity::Mismatch { stored, computed } =
                integrity_of(&self.conn, "incoming", source_id)?
            {
                anyhow::bail!(
                    "the snapshot of {root} from {host} does not match its digest \
                     (stored {stored}, computed {computed}); nothing was imported"
                );
            }

            // And the structure. A digest says the body is what the sender
            // meant, not that the sender meant a tree: whoever wrote the body
            // can recompute it over a broken arena. Checking only on `load`
            // would commit that snapshot first, and every later read of the
            // database would trip over it.
            let hint: u64 = self.conn.query_row(
                "SELECT files, dirs FROM incoming.scans WHERE id = ?1",
                [source_id],
                |row| {
                    let (f, d): (i64, i64) = (row.get(0)?, row.get(1)?);
                    Ok((f.max(0) as u64).saturating_add(d.max(0) as u64))
                },
            )?;
            let entries = match assemble(
                &self.conn,
                "incoming",
                source_id,
                hint,
                &root,
                Flags::Unknown,
                None,
                None,
            ) {
                Ok(tree) => tree.len() as u64,
                Err(e) => anyhow::bail!(
                    "the snapshot of {root} from {host} is refused: {e}; nothing was imported"
                ),
            };
            checked.push((source_id, host, root, started_at, entries));
        }

        // The copy, which is all the lock is held for. It reproduces exactly
        // what was checked above: every value already has the type `main`
        // stores it as, so nothing is converted, ids keep their order, and
        // the counts below prove no row was lost or doubled on the way.
        let tx = self.write_transaction()?;
        let mut imported = Vec::new();
        for (source_id, host, root, started_at, entries) in checked {
            // Again, under the lock: the same snapshot may have been pushed
            // twice at once, and the first push to commit wins.
            let already: Option<ScanId> = tx
                .query_row(
                    "SELECT id FROM main.scans WHERE host = ?1 AND root = ?2 AND started_at = ?3",
                    params![host, root, started_at],
                    |row| row.get(0),
                )
                .optional()?;
            if already.is_some() {
                continue;
            }

            // `content_hash` travels with the row it describes. Leaving it
            // out would drop the digest at exactly the moment it stops being
            // recomputable: the receiver would hold a snapshot it can never
            // check again, and would say "unknown" for the rest of its life.
            let scans = tx.execute(
                "INSERT INTO main.scans (host, root, started_at, duration_ms, total_size,
                                         total_alloc, files, dirs, errors, hardlinks_deduped,
                                         scanner_version, label, fs_total, fs_available,
                                         content_hash)
                 SELECT host, root, started_at, duration_ms, total_size, total_alloc, files,
                        dirs, errors, hardlinks_deduped, scanner_version, label,
                        fs_total, fs_available, content_hash
                 FROM incoming.scans WHERE id = ?1",
                [source_id],
            )?;
            anyhow::ensure!(
                scans == 1,
                "the snapshot of {root} from {host} copied as {scans} scans, not one; \
                 nothing was imported"
            );
            let new_id = tx.last_insert_rowid();

            // Only scan_id is rewritten: `id` is the node's index inside its own
            // arena and must keep matching children_start/children_len.
            let copied = tx.execute(
                "INSERT INTO main.entries (scan_id, id, parent_id, name, kind, size, alloc,
                                           mtime, nlink, files, dirs, children_start, children_len)
                 SELECT ?1, id, parent_id, name, kind, size, alloc, mtime, nlink, files, dirs,
                        children_start, children_len
                 FROM incoming.entries WHERE scan_id = ?2",
                params![new_id, source_id],
            )?;
            anyhow::ensure!(
                copied as u64 == entries,
                "the snapshot of {root} from {host} copied {copied} entries where its tree \
                 has {entries}; nothing was imported"
            );
            imported.push(new_id);
        }
        tx.commit()?;
        Ok(imported)
    }

    /// Whether a scan of this target with this start time is already here.
    /// Read without the write lock; the copy asks again under it.
    fn already_holds(&self, host: &str, root: &str, started_at: i64) -> Result<bool> {
        let id: Option<ScanId> = self
            .conn
            .query_row(
                "SELECT id FROM main.scans WHERE host = ?1 AND root = ?2 AND started_at = ?3",
                params![host, root, started_at],
                |row| row.get(0),
            )
            .optional()?;
        Ok(id.is_some())
    }

    fn copy_scan_into_attached(&self, scan_id: ScanId) -> Result<()> {
        schema::create_tables(&self.conn, "snap")?;
        self.conn.execute(
            "INSERT INTO snap.scans SELECT * FROM main.scans WHERE id = ?1",
            [scan_id],
        )?;
        self.conn.execute(
            "INSERT INTO snap.entries SELECT * FROM main.entries WHERE scan_id = ?1",
            [scan_id],
        )?;
        self.conn.pragma_update(
            Some(rusqlite::DatabaseName::Attached("snap")),
            "user_version",
            schema::SCHEMA_VERSION,
        )?;
        Ok(())
    }

    /// Drop all but the newest `keep` scans of each target.
    pub fn prune(&mut self, keep: usize) -> Result<usize> {
        let tx = self.write_transaction()?;
        let removed = tx.execute(
            "DELETE FROM scans WHERE id IN (
                 SELECT id FROM (
                     SELECT id, ROW_NUMBER() OVER (
                         PARTITION BY host, root ORDER BY started_at DESC, id DESC
                     ) AS rn
                     FROM scans
                 ) WHERE rn > ?1
             )",
            [keep as i64],
        )?;
        tx.commit()?;
        Ok(removed)
    }
}

/// A memory hint is all the stored entry count is, and it comes from the file
/// being read — which may have crossed a network. Unclamped, a scan claiming
/// 2^40 files asks `with_capacity` for 72 TiB and aborts the process before a
/// single row is read.
const MAX_CAPACITY_HINT: u64 = spacetrace_scan_core::MAX_CAPACITY_HINT as u64;

/// How far into the future an imported scan's start time may lie.
///
/// `latest_for`, `prune` and every "newest snapshot" question order by
/// `started_at`, so a scan stamped years ahead would be the newest of its
/// target forever: diffs would compare against it, retention would keep it
/// and delete real ones, and the agent would size every scan of that root
/// from its entry count. A day covers what a legitimate sender gets wrong —
/// a box whose clock was set to local time and read as UTC is off by at most
/// fourteen hours — and a clock further off than that is worth an error
/// rather than a snapshot that quietly outranks everything after it.
const MAX_CLOCK_AHEAD_SECS: i64 = 24 * 60 * 60;

/// The value types every column of an incoming snapshot must hold.
///
/// The sender writes the file, its DDL included, so its columns may have no
/// affinity at all. `main`'s do, and `INSERT … SELECT` converts on the way in:
/// an entry id stored as the text `'-1'` sorts after every integer in the
/// sender's file and before them in ours. What was checked there is then not
/// what `load` reads here. Requiring the types `main` would store anyway makes
/// the copy exact, so the two cannot differ.
const ENTRY_TYPES: &[(&str, &str)] = &[
    ("scan_id", "'integer'"),
    ("id", "'integer'"),
    ("parent_id", "'integer', 'null'"),
    ("name", "'text'"),
    ("kind", "'integer'"),
    ("size", "'integer'"),
    ("alloc", "'integer'"),
    ("mtime", "'integer'"),
    ("nlink", "'integer'"),
    ("files", "'integer'"),
    ("dirs", "'integer'"),
    ("children_start", "'integer'"),
    ("children_len", "'integer'"),
];

const SCAN_TYPES: &[(&str, &str)] = &[
    ("id", "'integer'"),
    ("host", "'text'"),
    ("root", "'text'"),
    ("started_at", "'integer'"),
    ("duration_ms", "'integer'"),
    ("total_size", "'integer'"),
    ("total_alloc", "'integer'"),
    ("files", "'integer'"),
    ("dirs", "'integer'"),
    ("errors", "'integer'"),
    ("hardlinks_deduped", "'integer'"),
    ("scanner_version", "'text'"),
    ("label", "'text', 'null'"),
    ("fs_total", "'integer', 'null'"),
    ("fs_available", "'integer', 'null'"),
    ("content_hash", "'text', 'null'"),
];

/// Refuse an attached snapshot whose rows `main` would store differently from
/// how they read there, or whose scans cannot be told apart by id.
///
/// One pass per table for the common case, and a pass per column only to name
/// the culprit once something is known to be wrong.
fn check_incoming_shape(conn: &Connection) -> Result<()> {
    for (table, columns) in [("scans", SCAN_TYPES), ("entries", ENTRY_TYPES)] {
        let wrong = |(column, types): &(&str, &str)| format!("typeof({column}) NOT IN ({types})");
        let any = columns.iter().map(wrong).collect::<Vec<_>>().join(" OR ");
        let bad: i64 = conn.query_row(
            &format!("SELECT count(*) FROM incoming.{table} WHERE {any}"),
            [],
            |row| row.get(0),
        )?;
        if bad == 0 {
            continue;
        }
        for column in columns {
            let n: i64 = conn.query_row(
                &format!(
                    "SELECT count(*) FROM incoming.{table} WHERE {}",
                    wrong(column)
                ),
                [],
                |row| row.get(0),
            )?;
            anyhow::ensure!(
                n == 0,
                "the snapshot is refused: {table}.{} holds a value of the wrong type in {n} \
                 rows; nothing was imported",
                column.0
            );
        }
    }

    // The sender's scans table need not have a key, and two rows sharing an
    // id would each match the `WHERE id = ?` that copies one of them.
    let duplicated: i64 = conn.query_row(
        "SELECT count(*) - count(DISTINCT id) FROM incoming.scans",
        [],
        |row| row.get(0),
    )?;
    anyhow::ensure!(
        duplicated == 0,
        "the snapshot is refused: {duplicated} scans share an id with another; nothing was imported"
    );
    Ok(())
}

/// Who wrote a scan being saved.
#[derive(Clone, Copy)]
enum Origin {
    /// This build walked it.
    Scanned,
    /// Another tool exported it (`spacetrace import`).
    Imported,
}

/// One scan's row of `rescan_state`, with its scan's `content_hash`; every
/// field `None` where the scan has no row.
struct StoredState {
    content_hash: Option<String>,
    journal: Option<String>,
    flags: Option<Vec<u8>>,
    rescan: Option<String>,
    digest: Option<String>,
}

impl StoredState {
    /// Whether the row matches its own digest. No row is intact — it reads
    /// as "no cursor" — and so is a row whose scan has no digest to bind it
    /// to, which `load_base` refuses on its own.
    fn is_intact(&self) -> bool {
        let (Some(rescan), Some(stored)) = (&self.rescan, &self.digest) else {
            return self.rescan.is_none();
        };
        let Some(content_hash) = &self.content_hash else {
            return true;
        };
        let flags = self.flags.as_deref().unwrap_or_default();
        digest::rescan_state(content_hash, self.journal.as_deref(), rescan, flags) == *stored
    }
}

/// What `assemble` knows of each entry's `Node::flags`.
#[derive(Clone, Copy)]
enum Flags<'a> {
    /// Nothing: every flag set, the reading that lets a rescan take nothing
    /// over. Every load but a rescan's base.
    Unknown,
    /// The directories with flags, in id order; every other entry has none.
    Sparse(&'a [(u32, u8)]),
}

/// One record of the flags blob: a `u32` id and a `u8`, little-endian.
const FLAG_RECORD: usize = 5;

/// The directories of `tree` whose flags are not zero, as stored in
/// `rescan_state.flags`.
///
/// Directories only, because a file's flags matter only to the directory
/// above it, whose own flags already carry them rolled up: a rescan copies a
/// directory only when its flags are zero, and then everything below it is
/// zero too. On a developer's 1.4 million entries that is 20,794 records
/// (104 KB) where one byte per entry would be 1.4 MB; on 932,000 entries of
/// `~/Library`, 1,140.
fn encode_flags(tree: &Tree) -> Vec<u8> {
    let mut out = Vec::new();
    for (id, node) in tree.nodes().iter().enumerate() {
        if node.is_dir() && node.flags() != 0 {
            out.extend_from_slice(&(id as u32).to_le_bytes());
            out.push(node.flags());
        }
    }
    out
}

/// `encode_flags`, read back; `None` for a blob it did not write — a length
/// that is not whole records, or ids out of order.
fn decode_flags(blob: &[u8]) -> Option<Vec<(u32, u8)>> {
    if blob.len() % FLAG_RECORD != 0 {
        return None;
    }
    let records: Vec<(u32, u8)> = blob
        .chunks_exact(FLAG_RECORD)
        .map(|r| (u32::from_le_bytes([r[0], r[1], r[2], r[3]]), r[4]))
        .collect();
    records
        .windows(2)
        .all(|pair| pair[0].0 < pair[1].0)
        .then_some(records)
}

/// Read one scan's rows from `schema` (`main`, or the alias of an ATTACHed
/// file) and assemble them into a checked tree.
///
/// One function for `load` and for `import_snapshot`, so the check a snapshot
/// passes on the way in is the same one it passes on every later read.
#[allow(clippy::too_many_arguments)]
fn assemble(
    conn: &Connection,
    schema: &str,
    scan_id: ScanId,
    entries_hint: u64,
    root: &str,
    flags: Flags<'_>,
    progress: Option<&ScanProgress>,
    mut digest: Option<&mut digest::Running>,
) -> Result<Tree> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {} FROM {schema}.entries WHERE scan_id = ?1 ORDER BY id",
        digest::ENTRY_COLUMNS
    ))?;
    let mut flagged = match flags {
        Flags::Unknown => None,
        Flags::Sparse(records) => Some(records.iter().peekable()),
    };
    let mut counter = RowCounter::new(progress);
    // Names are interned into the tree's shared arena as the rows arrive,
    // so the assembler holds the only offsets and no caller can invent one.
    let mut rows = stmt.query([scan_id])?;
    let mut assembler = TreeAssembler::with_capacity(entries_hint.min(MAX_CAPACITY_HINT) as usize);
    let mut index: u32 = 0;
    while let Some(row) = rows.next()? {
        counter.tick();
        let row = digest::EntryRow::from_row(row)?;
        if let Some(digest) = digest.as_deref_mut() {
            digest.entry(&row);
        }
        let node = StoredNode {
            parent: row.parent.map_or(Tree::NO_PARENT, |p| p as u32),
            name: row.name,
            kind: EntryKind::from_u8(row.kind as u8),
            size: row.size as u64,
            alloc: row.alloc as u64,
            // Not stored: a snapshot keeps subtree totals, and the entry's
            // own share of them is only used while a live tree is being
            // edited. Loading one back therefore reports zero here, which
            // is pre-existing behaviour and not introduced by the arena.
            own_size: 0,
            own_alloc: 0,
            mtime: row.mtime,
            nlink: row.nlink as u32,
            files: row.files as u32,
            dirs: row.dirs as u32,
            children_start: row.children_start as u32,
            children_len: row.children_len as u32,
        };
        // Rows arrive in id order and the records are in id order, so one
        // cursor walks both. Bits this build does not know are kept: a newer
        // writer's extra reasons not to copy stay reasons not to copy.
        match flagged.as_mut() {
            None => assembler.push(node),
            Some(records) => {
                let own = records
                    .next_if(|&&(id, _)| id == index)
                    .map_or(0, |&(_, f)| f);
                assembler.push_with_flags(node, own);
            }
        }
        index = index.wrapping_add(1);
    }
    // A record for an id the scan does not have is a blob for another tree.
    if flagged.is_some_and(|mut records| records.next().is_some()) {
        anyhow::bail!("scan {scan_id}'s flags name entries it does not have");
    }

    // Checked rather than trusted: this same code path loads snapshots
    // downloaded from an agent, and a malformed arena would panic on an
    // out-of-range index or loop forever on a backwards child pointer.
    assembler
        .finish(PathBuf::from(root))
        .map_err(|e| anyhow::anyhow!("scan {scan_id} is not a usable tree: {e}"))
}

/// Counts rows into `ScanProgress::rows_done` a batch at a time.
///
/// Per row was one shared atomic increment per row on the hottest loops in
/// the store; a watcher only needs the number to move (invariant 8), and it
/// moves thousands of times a second either way. Whatever is left is added
/// when the counter goes, early return included.
struct RowCounter<'a> {
    progress: Option<&'a ScanProgress>,
    pending: u64,
}

impl<'a> RowCounter<'a> {
    const BATCH: u64 = 4096;

    fn new(progress: Option<&'a ScanProgress>) -> Self {
        RowCounter {
            progress,
            pending: 0,
        }
    }

    fn tick(&mut self) {
        self.pending += 1;
        if self.pending == Self::BATCH {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if let Some(progress) = self.progress {
            progress
                .rows_done
                .fetch_add(self.pending, std::sync::atomic::Ordering::Relaxed);
        }
        self.pending = 0;
    }
}

impl Drop for RowCounter<'_> {
    fn drop(&mut self) {
        self.flush();
    }
}

/// Compare the stored digest of one scan against the content beside it.
///
/// `schema` is `main`, or the alias of an ATTACHed snapshot — the same check
/// serves a local database and one that just came off the wire.
fn integrity_of(conn: &Connection, schema: &str, scan_id: ScanId) -> Result<Integrity> {
    let stored: Option<String> = conn.query_row(
        &format!("SELECT content_hash FROM {schema}.scans WHERE id = ?1"),
        [scan_id],
        |row| row.get(0),
    )?;
    let Some(stored) = stored else {
        return Ok(Integrity::Unknown);
    };
    let computed = digest::of(conn, schema, scan_id)?;
    if computed == stored {
        return Ok(Integrity::Intact);
    }
    Ok(Integrity::Mismatch { stored, computed })
}

const SELECT_SCAN: &str = "SELECT id, host, root, started_at, duration_ms, total_size,
        total_alloc, files, dirs, errors, hardlinks_deduped, scanner_version, label,
        fs_total, fs_available FROM scans";

fn row_to_meta(row: &rusqlite::Row<'_>) -> rusqlite::Result<ScanMeta> {
    Ok(ScanMeta {
        id: row.get(0)?,
        host: row.get(1)?,
        root: row.get(2)?,
        started_at: row.get(3)?,
        duration_ms: row.get::<_, i64>(4)? as u64,
        total_size: row.get::<_, i64>(5)? as u64,
        total_alloc: row.get::<_, i64>(6)? as u64,
        files: row.get::<_, i64>(7)? as u64,
        dirs: row.get::<_, i64>(8)? as u64,
        errors: row.get::<_, i64>(9)? as u64,
        hardlinks_deduped: row.get::<_, i64>(10)? as u64,
        scanner_version: row.get(11)?,
        label: row.get(12)?,
        fs_total: row.get::<_, Option<i64>>(13)?.map(|v| v as u64),
        fs_available: row.get::<_, Option<i64>>(14)?.map(|v| v as u64),
    })
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
