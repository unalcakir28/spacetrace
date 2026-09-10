use anyhow::Result;
use rusqlite::Connection;

/// Bumped whenever the on-disk layout changes in a way older binaries cannot
/// read. Snapshots are cheap to recreate, so migrations may simply refuse.
///
/// v2 added `fs_total` / `fs_available`: the capacity of the filesystem the
/// root sits on, without which "when does this fill up" cannot be answered.
/// Both are nullable, because a v1 snapshot genuinely does not know.
///
/// v3 added `content_hash`: a digest of the scan's logical content, so a bit
/// flipped on the way here is refused instead of reported as a number.
/// Nullable for the same reason — an older snapshot has no digest, which is
/// "unknown", not "corrupt".
pub const SCHEMA_VERSION: i64 = 3;

/// Prepare a connection, migrating the file only if it actually needs it.
///
/// **Opening an already-current database must not take a write lock.** The
/// agent and the hub open a connection per request, so a read routinely lands
/// while a scan is saving. `PRAGMA journal_mode` and `CREATE TABLE IF NOT
/// EXISTS` both want a write lock even when there is nothing to change, so
/// doing them unconditionally meant an open could sit out the whole busy
/// timeout and then knock over the writer with SQLITE_BUSY. Everything below is
/// therefore guarded by a read first.
/// How long to keep retrying an initialisation that lost a lock race.
const INIT_RETRY_BUDGET: std::time::Duration = std::time::Duration::from_secs(15);
const INIT_RETRY_PAUSE: std::time::Duration = std::time::Duration::from_millis(20);

pub fn migrate(conn: &Connection) -> Result<()> {
    // Connection-local, no lock, no persistence: always safe to set.
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // WAL allows one writer at a time; without a busy timeout the loser gets
    // SQLITE_BUSY immediately instead of waiting its turn.
    conn.busy_timeout(std::time::Duration::from_secs(30))?;

    // Retry, because `busy_timeout` is not enough here. Switching journal mode
    // needs an exclusive lock, and SQLite does not always route that through
    // the busy handler — so two connections opening a brand-new file at the
    // same moment can leave one with SQLITE_BUSY immediately. That is a normal
    // situation for the agent, whose very first request may arrive while
    // another is already creating the database, so it is retried rather than
    // reported.
    let deadline = std::time::Instant::now() + INIT_RETRY_BUDGET;
    loop {
        match initialise(conn) {
            Ok(()) => return Ok(()),
            Err(Busy) if std::time::Instant::now() < deadline => {
                std::thread::sleep(INIT_RETRY_PAUSE);
            }
            Err(Busy) => {
                anyhow::bail!(
                    "the snapshot database stayed locked for {INIT_RETRY_BUDGET:?} while \
                     being initialised; another process may be holding it open"
                )
            }
            Err(Fatal(err)) => return Err(err),
        }
    }
}

/// Why one initialisation attempt did not finish.
enum InitError {
    /// Lost a lock race; worth trying again.
    Busy,
    /// Anything else, including a database from a newer build.
    Fatal(anyhow::Error),
}
use InitError::{Busy, Fatal};

/// One attempt at bringing the file up to the current schema.
///
/// Everything that takes a lock is guarded by a read first, so opening an
/// already-current database does no writing at all — which is what keeps a
/// reader from disturbing a scan that is saving.
fn initialise(conn: &Connection) -> Result<(), InitError> {
    let journal: String = query(conn, "journal_mode")?;
    if !journal.eq_ignore_ascii_case("wal") {
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(classify)?;
    }

    let found: i64 = query(conn, "user_version")?;
    if found > SCHEMA_VERSION {
        return Err(Fatal(anyhow::anyhow!(
            "this database was written by a newer spacetrace (schema v{found}, \
             this build understands v{SCHEMA_VERSION})"
        )));
    }
    if found == SCHEMA_VERSION {
        // Already current: nothing to create, nothing to stamp, no lock taken.
        return Ok(());
    }

    create_tables(conn, "main").map_err(to_init_error)?;
    migrate_from(conn, found).map_err(to_init_error)?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(classify)?;
    Ok(())
}

fn query<T: rusqlite::types::FromSql>(conn: &Connection, pragma: &str) -> Result<T, InitError> {
    conn.pragma_query_value(None, pragma, |row| row.get(0))
        .map_err(classify)
}

fn classify(err: rusqlite::Error) -> InitError {
    if is_busy(&err) {
        Busy
    } else {
        Fatal(err.into())
    }
}

/// `anyhow` has already erased the type by the time `create_tables` returns, so
/// look for the sqlite error underneath it.
fn to_init_error(err: anyhow::Error) -> InitError {
    match err.downcast_ref::<rusqlite::Error>() {
        Some(sqlite) if is_busy(sqlite) => Busy,
        _ => Fatal(err),
    }
}

fn is_busy(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked,
                ..
            },
            _
        )
    )
}

/// Bring an existing database up to the current schema.
///
/// `create_tables` only ever creates what is missing, so a database written by
/// an older build keeps its old columns and needs them added explicitly.
fn migrate_from(conn: &Connection, found: i64) -> Result<()> {
    if found >= SCHEMA_VERSION {
        return Ok(());
    }
    // Order matters, and not only for tidiness. `Store::export_snapshot`
    // copies with `INSERT INTO snap.scans SELECT * FROM main.scans`, where
    // `snap` was just built by `create_tables` and `main` may have reached the
    // same shape through these ALTERs. `ALTER TABLE ADD COLUMN` can only
    // append, so the declaration below must list the added columns in the same
    // order they appear at the end of `create_tables` — otherwise the copy
    // writes each value into the wrong column and says nothing.
    //
    // v1 -> v2: filesystem capacity. Existing rows keep NULL, which reads back
    // as "unknown" rather than as zero — a scan taken before this existed did
    // not measure a full disk.
    // v2 -> v3: the content digest. NULL means the snapshot predates it, so
    // there is nothing to check rather than something that failed a check.
    for (column, kind) in [
        ("fs_total", "INTEGER"),
        ("fs_available", "INTEGER"),
        ("content_hash", "TEXT"),
    ] {
        if !has_column(conn, "scans", column)? {
            conn.execute_batch(&format!(
                "ALTER TABLE main.scans ADD COLUMN {column} {kind}"
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
            fs_available      INTEGER,
            -- SHA-256 over this scan's logical content (see digest.rs), so a
            -- snapshot that changed on the way here is refused rather than
            -- believed. NULL when the snapshot predates v3.
            --
            -- Any column added after this one must also be appended in
            -- `migrate_from`, in the same order: the two shapes meet in
            -- `export_snapshot`'s `SELECT *`.
            content_hash      TEXT
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
