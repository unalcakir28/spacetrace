//! Snapshot storage.
//!
//! A snapshot is one scan of one root on one host, stored in SQLite so that a
//! later scan can be compared against it. The arena layout from `scan-core` is
//! written verbatim, which makes loading a snapshot a single ordered query with
//! no tree rebuilding.

mod ncdu;
mod schema;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use spacetrace_scan_core::{EntryKind, Node, ScanStats, Tree};

pub use ncdu::export_ncdu;

pub type ScanId = i64;

pub const SCANNER_VERSION: &str = env!("CARGO_PKG_VERSION");

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
}

impl ScanMeta {
    /// `host:root`, the identity used to decide which scans are comparable.
    pub fn target(&self) -> String {
        format!("{}:{}", self.host, self.root)
    }
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
        let started_at = now_unix() - (stats.duration_ms / 1000) as i64;
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO scans (host, root, started_at, duration_ms, total_size, total_alloc,
                                files, dirs, errors, hardlinks_deduped, scanner_version, label)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                host,
                tree.root_path().to_string_lossy(),
                started_at,
                stats.duration_ms as i64,
                tree.total_size() as i64,
                tree.total_alloc() as i64,
                stats.files as i64,
                stats.dirs as i64,
                stats.errors as i64,
                stats.hardlinks_deduped as i64,
                SCANNER_VERSION,
                label,
            ],
        )?;
        let scan_id = tx.last_insert_rowid();

        {
            let mut stmt = tx.prepare(
                "INSERT INTO entries (scan_id, id, parent_id, name, kind, size, alloc, mtime,
                                      nlink, files, dirs, children_start, children_len)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            )?;
            for (idx, node) in tree.nodes().iter().enumerate() {
                let parent: Option<i64> = if node.has_parent() {
                    Some(node.parent as i64)
                } else {
                    None
                };
                stmt.execute(params![
                    scan_id,
                    idx as i64,
                    parent,
                    node.name,
                    node.kind as u8,
                    node.size as i64,
                    node.alloc as i64,
                    node.mtime,
                    node.nlink as i64,
                    node.files as i64,
                    node.dirs as i64,
                    node.children_start as i64,
                    node.children_len as i64,
                ])?;
            }
        }

        tx.commit()?;
        Ok(scan_id)
    }

    /// Load one snapshot's tree back into memory.
    pub fn load(&self, scan_id: ScanId) -> Result<(Tree, ScanMeta)> {
        let meta = self
            .scan(scan_id)?
            .with_context(|| format!("no scan with id {scan_id}"))?;

        let mut stmt = self.conn.prepare(
            "SELECT parent_id, name, kind, size, alloc, mtime, nlink, files, dirs,
                    children_start, children_len
             FROM entries WHERE scan_id = ?1 ORDER BY id",
        )?;
        let nodes = stmt
            .query_map([scan_id], |row| {
                let parent: Option<i64> = row.get(0)?;
                Ok(Node {
                    parent: parent.map_or(Tree::NO_PARENT, |p| p as u32),
                    name: row.get(1)?,
                    kind: EntryKind::from_u8(row.get::<_, u8>(2)?),
                    size: row.get::<_, i64>(3)? as u64,
                    alloc: row.get::<_, i64>(4)? as u64,
                    own_size: 0,
                    own_alloc: 0,
                    mtime: row.get(5)?,
                    nlink: row.get::<_, i64>(6)? as u64,
                    files: row.get::<_, i64>(7)? as u64,
                    dirs: row.get::<_, i64>(8)? as u64,
                    children_start: row.get::<_, i64>(9)? as u32,
                    children_len: row.get::<_, i64>(10)? as u32,
                })
            })?
            .collect::<rusqlite::Result<Vec<Node>>>()?;

        // Checked rather than trusted: this same code path loads snapshots
        // downloaded from an agent, and a malformed arena would panic on an
        // out-of-range index or loop forever on a backwards child pointer.
        let tree = Tree::from_parts_checked(nodes, PathBuf::from(&meta.root))
            .map_err(|e| anyhow::anyhow!("scan {scan_id} is not a usable tree: {e}"))?;
        Ok((tree, meta))
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
        let sql = format!(
            "{SELECT_SCAN} WHERE root = ?1 AND (?2 IS NULL OR host = ?2)
             ORDER BY started_at DESC, id DESC LIMIT 1"
        );
        let meta = self
            .conn
            .query_row(&sql, params![root, host], row_to_meta)
            .optional()?;
        Ok(meta)
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
        let tx = self.conn.transaction()?;
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
        let incoming: Vec<(ScanId, String, String, i64)> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id, host, root, started_at FROM incoming.scans ORDER BY id")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        let tx = self.conn.transaction()?;
        let mut imported = Vec::new();
        for (source_id, host, root, started_at) in incoming {
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

            tx.execute(
                "INSERT INTO main.scans (host, root, started_at, duration_ms, total_size,
                                         total_alloc, files, dirs, errors, hardlinks_deduped,
                                         scanner_version, label)
                 SELECT host, root, started_at, duration_ms, total_size, total_alloc, files,
                        dirs, errors, hardlinks_deduped, scanner_version, label
                 FROM incoming.scans WHERE id = ?1",
                [source_id],
            )?;
            let new_id = tx.last_insert_rowid();

            // Only scan_id is rewritten: `id` is the node's index inside its own
            // arena and must keep matching children_start/children_len.
            tx.execute(
                "INSERT INTO main.entries (scan_id, id, parent_id, name, kind, size, alloc,
                                           mtime, nlink, files, dirs, children_start, children_len)
                 SELECT ?1, id, parent_id, name, kind, size, alloc, mtime, nlink, files, dirs,
                        children_start, children_len
                 FROM incoming.entries WHERE scan_id = ?2",
                params![new_id, source_id],
            )?;
            imported.push(new_id);
        }
        tx.commit()?;
        Ok(imported)
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
        let tx = self.conn.transaction()?;
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

const SELECT_SCAN: &str = "SELECT id, host, root, started_at, duration_ms, total_size,
        total_alloc, files, dirs, errors, hardlinks_deduped, scanner_version, label FROM scans";

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
    })
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
