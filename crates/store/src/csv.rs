//! Export a tree as CSV, for the spreadsheet a report ends up in.
//!
//! The audience is someone who has to hand a number to a person who does not
//! have this tool installed. That shapes three decisions.
//!
//! **Both measures are columns, never a choice.** Elsewhere in this project
//! which measure a view uses is a parameter (invariant #6) because a figure
//! and the order it is sorted in have to agree. A file has no sort order and
//! no label beside the number, so the honest thing is to carry both and let
//! the reader pick — and to name them in a way that cannot be misread.
//!
//! **A directory's numbers are its subtree's**, because that is the question
//! a spreadsheet is opened to answer ("which folder is the 400 GB?"). The
//! `own_*` columns are there too, so the total can be reconstructed without
//! double counting: summing `size` over every row counts a file once for
//! every directory above it.
//!
//! **RFC 4180 quoting, no shortcuts.** Paths contain commas, quotes and —
//! on Unix — newlines. A writer that assumed otherwise would produce a file
//! that opens in Excel with the rows silently wrong, which is worse than
//! failing, because nobody checks.
//!
//! **And a filename is not allowed to become a formula.** Excel, LibreOffice
//! and Sheets evaluate a cell that begins with `=`, `+`, `-` or `@`, and a
//! filename is untrusted input: it comes off a disk that may be shared,
//! downloaded into, or scanned on someone else's server. Quoting does not
//! help — a spreadsheet evaluates `"=1+1"` exactly as it evaluates `=1+1`.
//!
//! The textbook payload (`=cmd|'/c calc'!A1`) cannot actually be a filename,
//! because `/` is illegal in one on every platform this runs on — worth
//! saying, because overstating a risk is its own kind of wrong. What remains
//! is still worth stopping: a cell that reads other cells, a report whose
//! rows quietly show computed values instead of names, and whatever the next
//! spreadsheet feature turns out to evaluate. The fix costs one apostrophe in
//! front of a rare and strange name — Excel treats a leading apostrophe as
//! "this is text" and does not display it — and the person opening the file
//! did not ask to run anything.

use std::io::{self, Write};

use spacetrace_scan_core::{EntryKind, NodeId, Tree};

/// Column order. Written once as the header and once as the doc a reader of
/// this file needs.
const HEADER: &str = "path,type,size,alloc,own_size,own_alloc,files,dirs,mtime\n";

/// Write `tree` to `out` as CSV.
///
/// `max_depth` limits how far down the tree the rows go; `None` writes
/// everything. A full scan can be millions of rows, which no spreadsheet will
/// open, and a report is usually about the top few levels.
pub fn export_csv(tree: &Tree, out: &mut impl Write, max_depth: Option<usize>) -> io::Result<()> {
    out.write_all(HEADER.as_bytes())?;
    write_row(tree, tree.root(), out)?;
    let mut stack = vec![(tree.root(), 1usize)];
    while let Some((id, depth)) = stack.pop() {
        if max_depth.is_some_and(|max| depth > max) {
            continue;
        }
        // Pushed in reverse so the file reads in the same order the tree does;
        // a diff between two exports is worth more when the rows line up.
        let children: Vec<NodeId> = tree.children(id).collect();
        for child in children.iter().rev() {
            stack.push((*child, depth + 1));
        }
        for child in &children {
            write_row(tree, *child, out)?;
        }
    }
    Ok(())
}

/// One row.
///
/// `rel_path` walks to the root for every entry, which the tree's own notes
/// warn against in a hot loop (debt D6). Measured here rather than assumed:
/// 412,380 rows of `/Applications` take 0.28 s against 0.16 s for the ncdu
/// export, which does not build paths at all. The extra is the path building
/// and it is linear in depth, not in the tree — acceptable for a file
/// somebody is about to open in a spreadsheet, and not worth a second
/// traversal to precompute. If this ever reads a tree deep enough to matter,
/// the fix is to carry the parent's path down rather than to cache.
fn write_row(tree: &Tree, id: NodeId, out: &mut impl Write) -> io::Result<()> {
    let node = tree.node(id);
    let path = if id == tree.root() {
        tree.root_path().to_string_lossy().into_owned()
    } else {
        tree.rel_path(id)
    };
    let kind = match node.kind {
        EntryKind::Dir => "dir",
        EntryKind::File => "file",
        EntryKind::Symlink => "symlink",
        EntryKind::Other => "other",
    };
    writeln!(
        out,
        "{},{},{},{},{},{},{},{},{}",
        quote(&path),
        kind,
        node.size,
        node.alloc,
        node.own_size,
        node.own_alloc,
        node.files,
        node.dirs,
        node.mtime
    )
}

/// Characters that make a spreadsheet treat the rest of the cell as a
/// formula. Tab and carriage return are here because some readers strip
/// leading whitespace first and look at what follows.
const FORMULA_STARTERS: [char; 6] = ['=', '+', '-', '@', '\t', '\r'];

/// A field as CSV, safe to open in a spreadsheet.
///
/// Two separate jobs. RFC 4180 quoting keeps the *rows* right: wrap in quotes
/// when the field contains a comma, a quote, a newline or a carriage return,
/// and double any quote inside. Defusing keeps the reader's machine safe: a
/// leading apostrophe on anything a spreadsheet would evaluate.
///
/// Only the path column goes through this. The numeric columns are ours and
/// must stay numeric — `mtime` is legitimately negative for a file dated
/// before 1970, and an apostrophe there would turn a number into text.
fn quote(field: &str) -> String {
    let dangerous = field.starts_with(FORMULA_STARTERS);
    let special = field.chars().any(|c| matches!(c, ',' | '"' | '\n' | '\r'));
    if !dangerous && !special {
        return field.to_string();
    }
    let mut out = String::with_capacity(field.len() + 3);
    out.push('"');
    if dangerous {
        out.push('\'');
    }
    for c in field.chars() {
        if c == '"' {
            out.push('"');
        }
        out.push(c);
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use spacetrace_scan_core::ImportedNode;
    use std::path::PathBuf;

    /// A minimal CSV reader, so the tests assert that a *reader* recovers the
    /// fields rather than that the bytes match a string someone typed. A
    /// golden string would pass just as happily for an escaping bug that
    /// produced consistently wrong output.
    fn parse(text: &str) -> Vec<Vec<String>> {
        let mut rows = Vec::new();
        let mut row: Vec<String> = Vec::new();
        let mut field = String::new();
        let mut in_quotes = false;
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match (in_quotes, c) {
                (true, '"') if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                (true, '"') => in_quotes = false,
                (true, c) => field.push(c),
                (false, '"') => in_quotes = true,
                (false, ',') => row.push(std::mem::take(&mut field)),
                (false, '\n') => {
                    row.push(std::mem::take(&mut field));
                    rows.push(std::mem::take(&mut row));
                }
                (false, '\r') => {}
                (false, c) => field.push(c),
            }
        }
        if !field.is_empty() || !row.is_empty() {
            row.push(field);
            rows.push(row);
        }
        rows
    }

    fn tree_with(children: Vec<ImportedNode>) -> Tree {
        let mut root = ImportedNode::dir("root");
        root.children = children;
        Tree::from_nested(PathBuf::from("/root"), root)
    }

    fn csv_of(tree: &Tree, depth: Option<usize>) -> String {
        let mut buf = Vec::new();
        export_csv(tree, &mut buf, depth).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn the_header_names_every_column_that_follows() {
        let tree = tree_with(vec![ImportedNode::file("a.bin", 10, 4096)]);
        let rows = parse(&csv_of(&tree, None));
        assert_eq!(rows[0].len(), 9);
        assert_eq!(rows[0][0], "path");
        for row in &rows[1..] {
            assert_eq!(row.len(), rows[0].len(), "every row matches the header");
        }
    }

    #[test]
    fn a_directory_reports_its_subtree_and_its_own_cost_separately() {
        let mut sub = ImportedNode::dir("sub");
        sub.alloc = 4096;
        sub.children = vec![ImportedNode::file("b.bin", 2000, 8192)];
        let tree = tree_with(vec![sub]);

        let rows = parse(&csv_of(&tree, None));
        let sub_row = rows.iter().find(|r| r[0] == "sub").expect("sub");
        assert_eq!(sub_row[2], "2000", "subtree logical");
        assert_eq!(
            sub_row[3], "12288",
            "subtree on disk: its own 4096 plus 8192"
        );
        assert_eq!(
            sub_row[4], "0",
            "a directory's own logical size is not counted"
        );
        assert_eq!(sub_row[5], "4096", "its own blocks are");
    }

    /// The reason this file does its own quoting instead of `format!`: every
    /// one of these characters is legal in a Unix filename, and a naive
    /// writer turns each into a silently wrong spreadsheet.
    #[test]
    fn a_name_with_commas_quotes_and_newlines_survives_a_reader() {
        let awkward = "a,b \"quoted\" c\nsecond line";
        let tree = tree_with(vec![ImportedNode::file(awkward, 1, 1)]);

        let rows = parse(&csv_of(&tree, None));
        assert_eq!(rows.len(), 3, "header, root, one entry — not four");
        assert_eq!(rows[2][0], awkward, "the name comes back exactly");
        assert_eq!(rows[2][2], "1");
    }

    #[test]
    fn a_plain_name_is_not_quoted() {
        let tree = tree_with(vec![ImportedNode::file("plain.txt", 1, 1)]);
        assert!(
            csv_of(&tree, None).contains("\nplain.txt,file,"),
            "nothing to escape means nothing added"
        );
    }

    #[test]
    fn depth_limits_how_far_down_the_rows_go() {
        let mut deep = ImportedNode::dir("one");
        let mut two = ImportedNode::dir("two");
        two.children = vec![ImportedNode::file("deep.bin", 5, 5)];
        deep.children = vec![two];
        let tree = tree_with(vec![deep]);

        let all = parse(&csv_of(&tree, None));
        assert_eq!(all.len(), 1 + 4, "header, root, one, two, deep.bin");

        let shallow = parse(&csv_of(&tree, Some(1)));
        let names: Vec<&str> = shallow[1..].iter().map(|r| r[0].as_str()).collect();
        assert_eq!(names, vec!["/root", "one"]);
    }

    /// Depth 0 is the root alone — a summary line, and a legitimate thing to
    /// ask for. An off-by-one here would either drop the root or include a
    /// level the caller excluded.
    #[test]
    fn depth_zero_is_the_root_by_itself() {
        let tree = tree_with(vec![ImportedNode::file("a", 1, 1)]);
        let rows = parse(&csv_of(&tree, Some(0)));
        assert_eq!(rows.len(), 2, "header and the root");
        assert_eq!(rows[1][0], "/root");
    }

    /// Rows come out in the tree's own order, because two exports of the same
    /// target are meant to be diffable.
    #[test]
    fn rows_follow_the_trees_order() {
        let tree = tree_with(vec![
            ImportedNode::file("first", 1, 1),
            ImportedNode::file("second", 1, 1),
            ImportedNode::file("third", 1, 1),
        ]);
        let rows = parse(&csv_of(&tree, None));
        let names: Vec<&str> = rows[1..].iter().map(|r| r[0].as_str()).collect();
        assert_eq!(names, vec!["/root", "first", "second", "third"]);
    }

    #[test]
    fn the_quoting_helper_follows_rfc_4180() {
        assert_eq!(quote("plain"), "plain");
        assert_eq!(quote("with,comma"), "\"with,comma\"");
        assert_eq!(quote("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(quote("line\nbreak"), "\"line\nbreak\"");
        assert_eq!(quote("carriage\rreturn"), "\"carriage\rreturn\"");
    }

    /// A filename is untrusted input: it comes off a disk that may be shared,
    /// downloaded into, or scanned on someone else's server. A spreadsheet
    /// evaluates a cell starting with `=`, `+`, `-` or `@`, so without this
    /// the report becomes code running on the machine of whoever opens it —
    /// and that person never asked to run anything.
    #[test]
    fn a_name_that_would_be_a_formula_is_defused() {
        // No slashes: they are illegal in a filename, so the payloads that
        // can really appear are the ones below.
        for name in ["=1+1", "=A1", "+1+1", "-1+1", "@SUM(A1)", "\tleading-tab"] {
            let tree = tree_with(vec![ImportedNode::file(name, 1, 1)]);
            let text = csv_of(&tree, None);
            let rows = parse(&text);
            let cell = &rows[2][0];
            assert!(
                cell.starts_with('\''),
                "{name:?} reached the sheet as {cell:?}, which a spreadsheet would evaluate"
            );
            assert_eq!(
                &cell[1..],
                name,
                "the name itself must survive under the apostrophe"
            );
        }
    }

    /// Quoting alone is not the fix, so the test must not accept it as one:
    /// a spreadsheet evaluates `"=1+1"` exactly as it evaluates `=1+1`.
    #[test]
    fn quoting_alone_does_not_count_as_defusing() {
        assert_eq!(quote("=1+1"), "\"'=1+1\"");
        assert_eq!(quote("=a,b"), "\"'=a,b\"", "both jobs at once");
    }

    /// And an ordinary name must not collect an apostrophe, or every report
    /// grows a column of noise.
    #[test]
    fn an_ordinary_name_is_not_defused() {
        for name in ["plain.txt", "a-b.txt", "x=y.txt", "2024-report.pdf"] {
            assert_eq!(quote(name), name, "{name} needs nothing");
        }
    }

    /// `mtime` is the one column that can legitimately start with `-`, for a
    /// file dated before 1970. It must stay a number, so defusing has to be
    /// confined to the path.
    #[test]
    fn a_negative_timestamp_stays_a_number() {
        let mut old = ImportedNode::file("ancient.txt", 1, 1);
        old.mtime = -86_400;
        let tree = tree_with(vec![old]);
        let rows = parse(&csv_of(&tree, None));
        assert_eq!(rows[2][8], "-86400");
    }
}
