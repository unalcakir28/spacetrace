use anyhow::Result;
use rusqlite::Connection;

/// Bumped whenever the on-disk layout changes in a way older binaries cannot
/// read. Snapshots are cheap to recreate, so migrations may simply refuse.
pub const SCHEMA_VERSION: i64 = 1;

pub fn migrate(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // The agent opens a connection per request and can be saving a scan while
    // another request reads. WAL allows one writer at a time; without a busy
    // timeout the loser gets SQLITE_BUSY immediately instead of waiting.
    conn.busy_timeout(std::time::Duration::from_secs(30))?;

    let found: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if found > SCHEMA_VERSION {
        anyhow::bail!(
            "this database was written by a newer spacetrace (schema v{found}, \
             this build understands v{SCHEMA_VERSION})"
        );
    }

    create_tables(conn, "main")?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
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
            label             TEXT
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
