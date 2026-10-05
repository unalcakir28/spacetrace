//! A content digest for one snapshot, so corruption in transit is loud.
//!
//! **Not the file's bytes.** `Store::export_snapshot` builds a brand-new
//! SQLite file, so the sender's file hash would never match what it sends;
//! and page layout, `VACUUM` or a different SQLite build can all change the
//! file without changing a single value in it. A digest that moves on its own
//! is worse than no digest, because the first false alarm teaches everyone to
//! ignore it.
//!
//! What is hashed instead is the scan's *logical content*: its metadata row
//! and every entry row in `id` order. That survives compression, HTTP and
//! re-packing, and it covers exactly the values a reader loads back.
//!
//! **This is not authentication.** Anyone who can alter the body can
//! recompute the digest. The threat here is a flipped bit, not an adversary;
//! tampering needs a signature and a key distribution story, and neither has
//! been decided.

use anyhow::{Context, Result};
use rusqlite::Connection;
use sha2::{Digest, Sha256};

use crate::ScanId;

/// Prefixed to every digest. If the encoding below ever changes, this changes
/// with it, so an old digest and a new one cannot silently compare unequal and
/// be read as corruption.
const FORMAT: u8 = 1;

/// Field tags. They are part of the hashed stream, which is what stops
/// `("a", "bc")` and `("ab", "c")` — or an integer and a string that happen to
/// share bytes — from hashing alike.
const TAG_INT: u8 = b'i';
const TAG_TEXT: u8 = b's';
const TAG_NULL: u8 = b'0';
const TAG_BLOB: u8 = b'b';

/// Accumulates the encoded stream.
///
/// Every value is tagged and every string is length-prefixed, so the encoding
/// is unambiguous: no separator can appear inside a value and no two different
/// row sets can produce one stream.
struct Encoder(Sha256);

impl Encoder {
    fn new() -> Self {
        let mut hasher = Sha256::new();
        hasher.update([FORMAT]);
        Encoder(hasher)
    }

    fn int(&mut self, value: i64) -> &mut Self {
        self.0.update([TAG_INT]);
        self.0.update(value.to_le_bytes());
        self
    }

    fn opt_int(&mut self, value: Option<i64>) -> &mut Self {
        match value {
            Some(v) => self.int(v),
            None => self.null(),
        }
    }

    fn text(&mut self, value: &str) -> &mut Self {
        self.0.update([TAG_TEXT]);
        // The length as well as the bytes: without it "ab" + "c" and "a" + "bc"
        // are the same stream.
        self.0.update((value.len() as u64).to_le_bytes());
        self.0.update(value.as_bytes());
        self
    }

    fn opt_text(&mut self, value: Option<&str>) -> &mut Self {
        match value {
            Some(v) => self.text(v),
            None => self.null(),
        }
    }

    fn blob(&mut self, value: &[u8]) -> &mut Self {
        self.0.update([TAG_BLOB]);
        self.0.update((value.len() as u64).to_le_bytes());
        self.0.update(value);
        self
    }

    /// A present-but-empty string and a NULL are different values and must
    /// hash differently, so NULL gets a tag of its own rather than being
    /// encoded as "".
    fn null(&mut self) -> &mut Self {
        self.0.update([TAG_NULL]);
        self
    }

    fn finish(self) -> String {
        // Lowercase hex, like the SHA256SUMS this project already publishes.
        self.0
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

/// One scan's metadata row, in the order the digest hashes it.
///
/// `id` is deliberately absent: it is reassigned on import, so including it
/// would make every digest fail the moment it arrived anywhere.
pub(crate) struct ScanRow<'a> {
    pub host: &'a str,
    pub root: &'a str,
    pub started_at: i64,
    pub duration_ms: i64,
    pub total_size: i64,
    pub total_alloc: i64,
    pub files: i64,
    pub dirs: i64,
    pub errors: i64,
    pub hardlinks_deduped: i64,
    pub scanner_version: &'a str,
    pub label: Option<&'a str>,
    pub fs_total: Option<i64>,
    pub fs_available: Option<i64>,
}

/// One entry row, in the order the digest hashes it.
pub(crate) struct EntryRow<'a> {
    pub id: i64,
    pub parent: Option<i64>,
    pub name: &'a str,
    pub kind: i64,
    pub size: i64,
    pub alloc: i64,
    pub mtime: i64,
    pub nlink: i64,
    pub files: i64,
    pub dirs: i64,
    pub children_start: i64,
    pub children_len: i64,
}

impl<'r> EntryRow<'r> {
    /// The columns of `ENTRY_COLUMNS`, from `row`.
    pub fn from_row(row: &'r rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(EntryRow {
            id: row.get(0)?,
            parent: row.get(1)?,
            // Borrowed from the row: a `String` per entry was an allocation
            // per entry on the one pass every load and every check makes.
            name: text(row, 2)?,
            kind: row.get(3)?,
            size: row.get(4)?,
            alloc: row.get(5)?,
            mtime: row.get(6)?,
            nlink: row.get(7)?,
            files: row.get(8)?,
            dirs: row.get(9)?,
            children_start: row.get(10)?,
            children_len: row.get(11)?,
        })
    }
}

/// The entry columns, in [`EntryRow`] order, for every pass that reads them.
pub(crate) const ENTRY_COLUMNS: &str = "id, parent_id, name, kind, size, alloc, mtime, nlink, \
                                        files, dirs, children_start, children_len";

/// A text column, borrowed from the row it is in.
fn text<'r>(row: &'r rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<&'r str> {
    row.get_ref(idx)?.as_str().map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Text, Box::new(e))
    })
}

/// A digest being computed one row at a time, by whichever pass is already
/// going over the rows.
///
/// **One encoding, several feeders.** The save hashes the values it binds,
/// a rescan's base is checked in the pass that loads it, and `of` reads the
/// rows back for verify, export and import. They agree because every one of
/// them goes through [`Running::scan`] and [`Running::entry`] here, and a
/// test holds the save's answer to `of`'s (`integrity.rs`). Reading every
/// row back after writing it, so that one function produced both, cost a
/// second pass over the whole scan: 208 ms against the 273 ms the writing
/// took, on 412,983 entries.
pub(crate) struct Running {
    encoder: Encoder,
    /// Counted as well as hashed: a table truncated at a row boundary would
    /// otherwise be a prefix of the real stream, and a prefix of a hash input
    /// is not detectable from the hash.
    count: i64,
}

impl Running {
    /// Start with the scan's metadata row.
    pub fn scan(row: &ScanRow<'_>) -> Running {
        let mut encoder = Encoder::new();
        encoder
            .text(row.host)
            .text(row.root)
            .int(row.started_at)
            .int(row.duration_ms)
            .int(row.total_size)
            .int(row.total_alloc)
            .int(row.files)
            .int(row.dirs)
            .int(row.errors)
            .int(row.hardlinks_deduped)
            .text(row.scanner_version)
            .opt_text(row.label)
            .opt_int(row.fs_total)
            .opt_int(row.fs_available);
        Running { encoder, count: 0 }
    }

    /// Start with scan `scan_id`'s metadata row as `schema` holds it.
    pub fn read_scan(conn: &Connection, schema: &str, scan_id: ScanId) -> Result<Running> {
        conn.query_row(
            &format!(
                "SELECT host, root, started_at, duration_ms, total_size, total_alloc, files,
                        dirs, errors, hardlinks_deduped, scanner_version, label,
                        fs_total, fs_available
                 FROM {schema}.scans WHERE id = ?1"
            ),
            [scan_id],
            |row| {
                Ok(Running::scan(&ScanRow {
                    host: text(row, 0)?,
                    root: text(row, 1)?,
                    started_at: row.get(2)?,
                    duration_ms: row.get(3)?,
                    total_size: row.get(4)?,
                    total_alloc: row.get(5)?,
                    files: row.get(6)?,
                    dirs: row.get(7)?,
                    errors: row.get(8)?,
                    hardlinks_deduped: row.get(9)?,
                    scanner_version: text(row, 10)?,
                    label: row.get_ref(11)?.as_str_or_null().map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            11,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?,
                    fs_total: row.get(12)?,
                    fs_available: row.get(13)?,
                }))
            },
        )
        .with_context(|| format!("reading scan {scan_id} to digest it"))
    }

    /// The next entry, in `id` order.
    pub fn entry(&mut self, row: &EntryRow<'_>) {
        self.count += 1;
        self.encoder
            .int(row.id)
            .opt_int(row.parent)
            .text(row.name)
            .int(row.kind)
            .int(row.size)
            .int(row.alloc)
            .int(row.mtime)
            .int(row.nlink)
            .int(row.files)
            .int(row.dirs)
            .int(row.children_start)
            .int(row.children_len);
    }

    pub fn finish(mut self) -> String {
        self.encoder.int(self.count);
        self.encoder.finish()
    }
}

/// The digest of one scan's `rescan_state` row, bound to that scan's
/// `content_hash`.
///
/// The row decides what a rescan copies unread (`flags`) and where its
/// replay starts (`journal`), and the snapshot digest covers neither — it
/// cannot without changing what every export carries. So the row carries a
/// digest of its own, and including the scan's digest ties it to that one
/// snapshot: a row moved to another scan, or a scan rewritten under its row,
/// no longer matches. Corruption, not tampering, as for `content_hash`.
pub(crate) fn rescan_state(
    content_hash: &str,
    journal: Option<&str>,
    rescan: &str,
    flags: &[u8],
) -> String {
    let mut encoder = Encoder::new();
    encoder
        .text("rescan_state")
        .text(content_hash)
        .opt_text(journal)
        .text(rescan)
        .blob(flags);
    encoder.finish()
}

/// The digest of one scan, read from the database that holds it: the check
/// for verify, export and import, which have nothing in hand but the rows.
///
/// `schema` is interpolated into the SQL and must never come from user input;
/// the callers pass string literals.
pub fn of(conn: &Connection, schema: &str, scan_id: ScanId) -> Result<String> {
    let mut running = Running::read_scan(conn, schema, scan_id)?;
    let mut stmt = conn.prepare(&format!(
        "SELECT {ENTRY_COLUMNS} FROM {schema}.entries WHERE scan_id = ?1 ORDER BY id"
    ))?;
    let mut rows = stmt.query([scan_id])?;
    while let Some(row) = rows.next()? {
        running.entry(&EntryRow::from_row(row)?);
    }
    Ok(running.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two adjacent strings must not be able to trade a character across the
    /// boundary. The tag byte alone does not stop this, because a string may
    /// *contain* the tag byte: with `TAG_TEXT` being `s`, the pair
    /// `("a", "sb")` and the pair `("as", "b")` write the same five bytes.
    /// The length prefix is what separates them — and `scans.host` followed by
    /// `scans.root` is exactly such a pair of adjacent strings.
    #[test]
    fn adjacent_strings_cannot_trade_a_character() {
        assert_eq!(TAG_TEXT, b's', "the collision below is built on this");

        let mut left = Encoder::new();
        left.text("a").text("sb");
        let mut right = Encoder::new();
        right.text("as").text("b");
        assert_ne!(
            left.finish(),
            right.finish(),
            "the length prefix is what makes the stream unambiguous"
        );
    }

    #[test]
    fn a_null_is_not_an_empty_string() {
        let mut null = Encoder::new();
        null.opt_text(None);
        let mut empty = Encoder::new();
        empty.text("");
        assert_ne!(null.finish(), empty.finish());
    }

    #[test]
    fn a_null_is_not_a_zero() {
        let mut null = Encoder::new();
        null.opt_int(None);
        let mut zero = Encoder::new();
        zero.int(0);
        assert_ne!(null.finish(), zero.finish());
    }

    #[test]
    fn an_integer_is_not_the_text_of_that_integer() {
        let mut number = Encoder::new();
        number.int(1000);
        let mut text = Encoder::new();
        text.text("1000");
        assert_ne!(number.finish(), text.finish());
    }
}
