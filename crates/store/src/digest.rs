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

/// The digest of one scan, read from the database that holds it.
///
/// Both the writer and every reader go through this one function rather than
/// each encoding what it happens to have in hand. A second implementation —
/// hashing the in-memory tree at save time, say — would be faster by one pass
/// over the rows and would eventually disagree with this one over some field
/// nobody thought about, which is a bug that only shows up as "your snapshot
/// is corrupt" on a snapshot that is fine.
///
/// `schema` is interpolated into the SQL and must never come from user input;
/// the callers pass string literals.
pub fn of(conn: &Connection, schema: &str, scan_id: ScanId) -> Result<String> {
    let mut encoder = Encoder::new();

    // `id` is deliberately absent: it is reassigned on import, so including it
    // would make every digest fail the moment it arrived anywhere.
    conn.query_row(
        &format!(
            "SELECT host, root, started_at, duration_ms, total_size, total_alloc, files,
                    dirs, errors, hardlinks_deduped, scanner_version, label,
                    fs_total, fs_available
             FROM {schema}.scans WHERE id = ?1"
        ),
        [scan_id],
        |row| {
            encoder
                .text(&row.get::<_, String>(0)?)
                .text(&row.get::<_, String>(1)?)
                .int(row.get(2)?)
                .int(row.get(3)?)
                .int(row.get(4)?)
                .int(row.get(5)?)
                .int(row.get(6)?)
                .int(row.get(7)?)
                .int(row.get(8)?)
                .int(row.get(9)?)
                .text(&row.get::<_, String>(10)?)
                .opt_text(row.get::<_, Option<String>>(11)?.as_deref())
                .opt_int(row.get(12)?)
                .opt_int(row.get(13)?);
            Ok(())
        },
    )
    .with_context(|| format!("reading scan {scan_id} to digest it"))?;

    let mut stmt = conn.prepare(&format!(
        "SELECT id, parent_id, name, kind, size, alloc, mtime, nlink, files, dirs,
                children_start, children_len
         FROM {schema}.entries WHERE scan_id = ?1 ORDER BY id"
    ))?;
    let mut rows = stmt.query([scan_id])?;
    // Counted as well as hashed: a table truncated at a row boundary would
    // otherwise be a prefix of the real stream, and a prefix of a hash input
    // is not detectable from the hash.
    let mut count: i64 = 0;
    while let Some(row) = rows.next()? {
        count += 1;
        encoder
            .int(row.get(0)?)
            .opt_int(row.get(1)?)
            .text(&row.get::<_, String>(2)?)
            .int(row.get::<_, i64>(3)?)
            .int(row.get(4)?)
            .int(row.get(5)?)
            .int(row.get(6)?)
            .int(row.get(7)?)
            .int(row.get(8)?)
            .int(row.get(9)?)
            .int(row.get(10)?)
            .int(row.get(11)?);
    }
    encoder.int(count);

    Ok(encoder.finish())
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
