use anyhow::Result;
use rusqlite::Connection;

/// Bumped whenever the on-disk layout changes in a way older binaries cannot
/// read. Snapshots are cheap to recreate, so migrations may simply refuse.
///
/// v2 added `fs_total` / `fs_available`: the capacity of the filesystem the
/// root sits on, without which "when does this fill up" cannot be answered.
/// Both are nullable, because a v1 snapshot genuinely does not know.
pub const SCHEMA_VERSION: i64 = 2;

/// Prepare a connection, migrating the file only if it actually needs it.
///
/// **Opening an already-current database must not take a write lock.** The
/// agent and the hub open a connection per request, so a read routinely lands
/// while a scan is saving. `PRAGMA journal_mode` and `CREATE TABLE IF NOT
/// EXISTS` both want a write lock even when there is nothing to change, so
/// doing them unconditionally meant an open could sit out the whole busy
/// timeout and then knock over the writer with SQLITE_BUSY. Everything below is
/// therefore guarded by a read first.
pub fn migrate(conn: &Connection) -> Result<()> {
    // Connection-local, no lock, no persistence: always safe to set.
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // WAL allows one writer at a time; without a busy timeout the loser gets
    // SQLITE_BUSY immediately instead of waiting its turn.
    conn.busy_timeout(std::time::Duration::from_secs(30))?;

    // Persistent and lock-taking, so only set it when it is not already right.
    let journal: String = conn.pragma_query_value(None, "journal_mode", |r| r.get(0))?;
    if !journal.eq_ignore_ascii_case("wal") {
        conn.pragma_update(None, "journal_mode", "WAL")?;
    }

    let found: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if found > SCHEMA_VERSION {
        anyhow::bail!(
            "this database was written by a newer spacetrace (schema v{found}, \
             this build understands v{SCHEMA_VERSION})"
        );
    }
    if found == SCHEMA_VERSION {
        // Already current: nothing to create, nothing to stamp, no lock taken.
        return Ok(());
    }

    create_tables(conn, "main")?;
    migrate_from(conn, found)?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

/// Bring an existing database up to the current schema.
///
/// `create_tables` only ever creates what is missing, so a database written by
/// an older build keeps its old columns and needs them added explicitly.
fn migrate_from(conn: &Connection, found: i64) -> Result<()> {
    if found >= SCHEMA_VERSION {
        return Ok(());
    }
    // v1 -> v2: filesystem capacity. Existing rows keep NULL, which reads back
    // as "unknown" rather than as zero — a scan taken before this existed did
    // not measure a full disk.
    for column in ["fs_total", "fs_available"] {
        if !has_column(conn, "scans", column)? {
            conn.execute_batch(&format!(
                "ALTER TABLE main.scans ADD COLUMN {column} INTEGER"
            ))?;
        }
    }
    Ok(())
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Create the tables in `schema`, which is `main` for the open database or the
/// alias of an ATTACHed one. Taking the schema as a parameter is what lets
/// `Store::export_snapshot` build a standalone snapshot file without a second
/// connection and without recomputing anything.
///
/// `schema` is interpolated into the SQL, so it must never come from user
/// input; the only two callers pass string literals.
pub fn create_tables(conn: &Connection, schema: &str) -> Result<()> {
    conn.execute_batch(&format!(
        r#"
        CREATE TABLE IF NOT EXISTS {schema}.scans (
            id                INTEGER PRIMARY KEY AUTOINCREMENT,
            host              TEXT    NOT NULL,
            root              TEXT    NOT NULL,
            started_at        INTEGER NOT NULL,
            duration_ms       INTEGER NOT NULL,
            total_size        INTEGER NOT NULL,
            total_alloc       INTEGER NOT NULL,
            files             INTEGER NOT NULL,
            dirs              INTEGER NOT NULL,
            errors            INTEGER NOT NULL,
            hardlinks_deduped INTEGER NOT NULL,
            scanner_version   TEXT    NOT NULL,
            label             TEXT,
            -- Capacity of the filesystem the root sits on. NULL when the
            -- platform could not say, or when the snapshot predates v2.
            fs_total          INTEGER,
            fs_available      INTEGER
        );

        CREATE INDEX IF NOT EXISTS {schema}.scans_target
            ON scans (host, root, started_at DESC);

        -- The arena is stored as-is: `id` is the node's index and children of a
        -- node occupy children_start .. children_start + children_len, so
        -- loading is one ordered scan with no tree reconstruction.
        CREATE TABLE IF NOT EXISTS {schema}.entries (
            scan_id        INTEGER NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
            id             INTEGER NOT NULL,
            parent_id      INTEGER,
            name           TEXT    NOT NULL,
            kind           INTEGER NOT NULL,
            size           INTEGER NOT NULL,
            alloc          INTEGER NOT NULL,
            mtime          INTEGER NOT NULL,
            nlink          INTEGER NOT NULL,
            files          INTEGER NOT NULL,
            dirs           INTEGER NOT NULL,
            children_start INTEGER NOT NULL,
            children_len   INTEGER NOT NULL,
            PRIMARY KEY (scan_id, id)
        ) WITHOUT ROWID;
        "#
    ))?;
    Ok(())
}
