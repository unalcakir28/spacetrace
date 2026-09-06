use anyhow::Result;
use rusqlite::Connection;

/// Bumped whenever the on-disk layout changes in a way older binaries cannot
/// read. Snapshots are cheap to recreate, so migrations may simply refuse.
const SCHEMA_VERSION: i64 = 1;

pub fn migrate(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;

    let found: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if found > SCHEMA_VERSION {
        anyhow::bail!(
            "this database was written by a newer spacetrace (schema v{found}, \
             this build understands v{SCHEMA_VERSION})"
        );
    }

    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS scans (
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

        CREATE INDEX IF NOT EXISTS scans_target
            ON scans (host, root, started_at DESC);

        -- The arena is stored as-is: `id` is the node's index and children of a
        -- node occupy children_start .. children_start + children_len, so
        -- loading is one ordered scan with no tree reconstruction.
        CREATE TABLE IF NOT EXISTS entries (
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
        "#,
    )?;

    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}
