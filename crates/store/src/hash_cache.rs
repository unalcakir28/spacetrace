//! Remembering file hashes between duplicate scans.
//!
//! **Its own file, not the snapshot database.** Two reasons, and both of them
//! are about what a snapshot database is for. It travels: `export_snapshot`
//! copies a scan to another machine, and a table of local inode numbers means
//! nothing there — worse than nothing, since the numbers are valid-looking and
//! belong to a different disk. And it is versioned: adding a table would bump
//! `SCHEMA_VERSION`, which forces the release ordering in RELEASING.md
//! (desktop and hub first, then the CLI) for a cache that can be deleted at
//! any moment with no loss but time.
//!
//! So this is a throwaway file beside the database, and the worst thing that
//! can happen to it is that it has to be rebuilt.
//!
//! **Every failure is a miss.** A cache that cannot answer costs a re-read; a
//! cache that propagates its errors turns a locked file or a full disk into a
//! failed duplicate scan. Nothing here returns a `Result` for that reason.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, Connection};
use spacetrace_dupes::{CacheKey, Hash, HashCache};

/// Hashes keyed by identity, length and modification time.
pub struct SqliteHashCache {
    /// One connection behind a mutex rather than a pool. The work between two
    /// lookups is reading and hashing a file, so this is never the contended
    /// thing — and a pool would be machinery for a contention that does not
    /// exist.
    conn: Option<Mutex<Connection>>,
}

impl SqliteHashCache {
    /// Where the cache lives for a given snapshot database.
    ///
    /// Beside it and named after it, so a second database gets a second cache
    /// rather than sharing one — and so deleting the pair is one obvious
    /// gesture.
    pub fn path_for(database: &Path) -> PathBuf {
        let mut name = database.file_name().unwrap_or_default().to_os_string();
        name.push(".hashes");
        database.with_file_name(name)
    }

    /// Open or create the cache. Never fails: an unusable cache is one that
    /// answers nothing.
    pub fn open(path: &Path) -> Self {
        SqliteHashCache {
            conn: Connection::open(path).ok().and_then(|conn| {
                conn.busy_timeout(std::time::Duration::from_secs(5)).ok()?;
                conn.execute_batch(
                    "PRAGMA journal_mode = WAL;
                     CREATE TABLE IF NOT EXISTS hashes (
                        device INTEGER NOT NULL,
                        inode  INTEGER NOT NULL,
                        size   INTEGER NOT NULL,
                        mtime  INTEGER NOT NULL,
                        hash   BLOB    NOT NULL,
                        seen_at INTEGER NOT NULL,
                        PRIMARY KEY (device, inode, size, mtime)
                     ) WITHOUT ROWID;",
                )
                .ok()?;
                Some(Mutex::new(conn))
            }),
        }
    }

    /// A cache that remembers nothing, for when no database path is known.
    pub fn disabled() -> Self {
        SqliteHashCache { conn: None }
    }

    /// Whether this cache is actually storing anything.
    ///
    /// Worth asking, because every failure here is silent by design and a
    /// caller reporting "cached" should be able to check rather than assume.
    pub fn is_active(&self) -> bool {
        self.conn.is_some()
    }

    /// Drop entries not seen in the last `days`.
    ///
    /// Without this the file grows for the life of the machine, keyed by inode
    /// numbers that the filesystem reuses — so old rows are not merely waste,
    /// they are rows whose key may come round again. The `size` and `mtime` in
    /// the key make a stale hit vanishingly unlikely, but "unlikely" is not
    /// the standard for something somebody deletes files on the strength of.
    pub fn forget_older_than(&self, days: i64, now: i64) -> usize {
        let Some(conn) = &self.conn else { return 0 };
        let Ok(conn) = conn.lock() else { return 0 };
        conn.execute(
            "DELETE FROM hashes WHERE seen_at < ?1",
            [now - days * 86_400],
        )
        .unwrap_or(0)
    }

    pub fn len(&self) -> usize {
        let Some(conn) = &self.conn else { return 0 };
        let Ok(conn) = conn.lock() else { return 0 };
        conn.query_row("SELECT COUNT(*) FROM hashes", [], |row| {
            row.get::<_, i64>(0)
        })
        .map(|n| n as usize)
        .unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// SQLite integers are signed; inode and device numbers are not. Stored as the
/// same 64 bits reinterpreted, which round-trips exactly — the alternative,
/// storing them as text, would make the primary key three times the size for a
/// value nobody reads by eye.
fn as_i64(value: u64) -> i64 {
    value as i64
}

impl HashCache for SqliteHashCache {
    fn get(&self, key: &CacheKey) -> Option<Hash> {
        let conn = self.conn.as_ref()?;
        let conn = conn.lock().ok()?;
        let bytes: Vec<u8> = conn
            .query_row(
                "SELECT hash FROM hashes
                 WHERE device = ?1 AND inode = ?2 AND size = ?3 AND mtime = ?4",
                params![
                    as_i64(key.device),
                    as_i64(key.inode),
                    key.size as i64,
                    key.mtime
                ],
                |row| row.get(0),
            )
            .ok()?;
        bytes.try_into().ok()
    }

    fn put(&self, key: &CacheKey, hash: Hash) {
        let Some(conn) = &self.conn else { return };
        let Ok(conn) = conn.lock() else { return };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        // `seen_at` is refreshed on conflict so that a file hashed again keeps
        // its place: expiry is meant to remove what is no longer looked at,
        // not what was first seen long ago.
        let _ = conn.execute(
            "INSERT INTO hashes (device, inode, size, mtime, hash, seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(device, inode, size, mtime)
             DO UPDATE SET hash = ?5, seen_at = ?6",
            params![
                as_i64(key.device),
                as_i64(key.inode),
                key.size as i64,
                key.mtime,
                &hash[..],
                now
            ],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(inode: u64, mtime: i64) -> CacheKey {
        CacheKey {
            device: 1,
            inode,
            size: 4096,
            mtime,
        }
    }

    #[test]
    fn a_hash_survives_being_put_and_got() {
        let dir = tempfile::tempdir().unwrap();
        let cache = SqliteHashCache::open(&dir.path().join("c.sqlite"));
        assert!(cache.is_active());

        cache.put(&key(7, 100), [9u8; 32]);
        assert_eq!(cache.get(&key(7, 100)), Some([9u8; 32]));
    }

    /// The point of the key. A file edited in place keeps its inode and often
    /// its length; only the mtime moves, and if that did not invalidate the
    /// entry the finder would report a match that is no longer a match.
    #[test]
    fn a_changed_file_is_a_miss_on_every_part_of_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let cache = SqliteHashCache::open(&dir.path().join("c.sqlite"));
        cache.put(&key(7, 100), [9u8; 32]);

        assert_eq!(cache.get(&key(7, 101)), None, "rewritten");
        assert_eq!(cache.get(&key(8, 100)), None, "a different inode");
        let mut resized = key(7, 100);
        resized.size = 4097;
        assert_eq!(cache.get(&resized), None, "a different length");
    }

    /// Inode numbers routinely exceed `i64::MAX` on some filesystems, and a
    /// conversion that wrapped would collide two files into one entry.
    #[test]
    fn very_large_identifiers_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let cache = SqliteHashCache::open(&dir.path().join("c.sqlite"));
        let big = CacheKey {
            device: u64::MAX,
            inode: u64::MAX - 1,
            size: u64::MAX,
            mtime: -1,
        };
        cache.put(&big, [3u8; 32]);
        assert_eq!(cache.get(&big), Some([3u8; 32]));

        let neighbour = CacheKey {
            inode: u64::MAX,
            ..big
        };
        assert_eq!(cache.get(&neighbour), None, "and does not collide");
    }

    #[test]
    fn entries_can_be_expired() {
        let dir = tempfile::tempdir().unwrap();
        let cache = SqliteHashCache::open(&dir.path().join("c.sqlite"));
        cache.put(&key(1, 1), [1u8; 32]);
        cache.put(&key(2, 1), [2u8; 32]);
        assert_eq!(cache.len(), 2);

        // Far enough in the future that everything just written is stale.
        let far = 10_i64.pow(12);
        assert_eq!(cache.forget_older_than(30, far), 2);
        assert!(cache.is_empty());
    }

    /// Every failure has to be a miss. A cache that cannot be opened must not
    /// be the reason a duplicate scan fails.
    #[test]
    fn an_unusable_cache_answers_nothing_and_complains_about_nothing() {
        let dir = tempfile::tempdir().unwrap();
        // A directory where a file should be: open fails, and that is all.
        let path = dir.path().join("subdir");
        std::fs::create_dir(&path).unwrap();

        let cache = SqliteHashCache::open(&path);
        assert!(!cache.is_active());
        cache.put(&key(1, 1), [1u8; 32]);
        assert_eq!(cache.get(&key(1, 1)), None);
        assert_eq!(cache.forget_older_than(1, 0), 0);
    }

    #[test]
    fn the_cache_sits_beside_the_database_it_belongs_to() {
        let path = SqliteHashCache::path_for(Path::new("/var/lib/spacetrace/scans.sqlite"));
        assert_eq!(
            path,
            Path::new("/var/lib/spacetrace/scans.sqlite.hashes"),
            "one obvious pair to delete"
        );
    }
}
