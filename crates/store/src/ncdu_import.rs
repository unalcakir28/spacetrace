//! Read ncdu's JSON format back in.
//!
//! We could already write it; being able to read it is what makes an existing
//! ncdu user's history worth something here. gdu writes the same format, so
//! this takes their exports too.
//!
//! The format is `[majorver, minorver, metadata, tree]`. A directory is an
//! array whose first element describes the directory itself and whose
//! remaining elements are its children; anything else is an object.
//!
//! **Sizes in this format are each entry's own**, and totals are the reader's
//! job — which is why the nested result goes through
//! `Tree::from_nested` rather than being laid out here. Aggregation and the
//! arena invariants have one implementation in this project and this is not a
//! second one.
//!
//! **This is a trust boundary.** The file came from somewhere else. Nothing
//! here may panic on input: no indexing without a check, and no unwrap on a
//! field a hand-written file can omit. A file made of ten thousand opening
//! brackets is stopped by `serde_json`, which refuses beyond 128 levels of
//! nesting before this code sees anything — measured, not assumed. A depth
//! limit of our own was written first and then removed: it sat above
//! `serde_json`'s and could never fire, and an untested guard is worse than
//! the one that actually does the work.

use anyhow::{bail, Context, Result};
use serde_json::Value;
use spacetrace_scan_core::{EntryKind, ImportedNode, Tree};
use std::path::PathBuf;

/// Parse an ncdu export into a tree.
///
/// `root_path` is what the tree will call itself; `None` takes the name the
/// file gives its own root, which is the absolute path ncdu scanned and
/// almost always what the reader wants. It is overridable because an imported
/// scan of `/var` from another machine is not this machine's `/var`, and
/// because a caller may want it filed under something clearer.
pub fn import_ncdu(json: &str, root_path: Option<PathBuf>) -> Result<Tree> {
    // Also the stack guard: `serde_json` refuses more than 128 levels of
    // nesting, which no real directory tree comes near.
    let value: Value = serde_json::from_str(json).context("this is not JSON")?;

    let Value::Array(top) = value else {
        bail!("an ncdu export is a JSON array, and this is not one");
    };
    // `[majorver, minorver, metadata, tree]`. Older ncdu wrote the same shape,
    // so the length is the only structural check worth making.
    if top.len() < 4 {
        bail!(
            "an ncdu export has four elements (version, version, metadata, tree); this has {}",
            top.len()
        );
    }
    let major = top[0].as_u64().unwrap_or(0);
    if major != 1 {
        bail!("this is ncdu format version {major}; only version 1 is understood");
    }

    let root = node_from(&top[3]).context("reading the tree")?;
    let root_path = root_path.unwrap_or_else(|| PathBuf::from(&root.name));
    Ok(Tree::from_nested(root_path, root))
}

/// One entry, and its children when it is a directory.
fn node_from(value: &Value) -> Result<ImportedNode> {
    // A directory is `[self, child, child, …]`; everything else is the object
    // on its own.
    let (info, children) = match value {
        Value::Array(items) => {
            let Some((first, rest)) = items.split_first() else {
                bail!(
                    "a directory must describe itself before its children, and this array is empty"
                );
            };
            (first, rest)
        }
        other => (other, &[][..]),
    };

    let Value::Object(fields) = info else {
        bail!("an entry is a JSON object; found {}", kind_of(info));
    };

    let is_dir = matches!(value, Value::Array(_));

    let name = fields
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .context("an entry has no name")?;

    // `notreg` covers symlinks, sockets, devices — everything ncdu will not
    // call a regular file. It does not say which, and guessing would put a
    // wrong type in the tree; `Other` is the honest answer, and the one place
    // it matters (symlinks are never followed) is already true of an imported
    // tree because nothing is walked.
    let not_regular = fields
        .get("notreg")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let kind = match (is_dir, not_regular) {
        (true, _) => EntryKind::Dir,
        (false, true) => EntryKind::Other,
        (false, false) => EntryKind::File,
    };

    // Absent means zero: ncdu omits a field rather than writing 0, and both
    // our own exporter and ncdu's do that for an empty file.
    //
    // **A directory's own `asize` is dropped** (invariant #1): the logical
    // total is file bytes, and adding each directory's inode size is exactly
    // the mistake that makes GNU `du --apparent-size` disagree with us. Our
    // own exporter writes 0 there, so a round trip through it could never
    // show this; real ncdu writes the inode size, and importing it inflated
    // the logical total by the sum of every directory in the tree.
    // `dsize` is kept for directories, because the blocks a directory
    // occupies are genuinely on the disk.
    let size = match is_dir {
        true => 0,
        false => fields.get("asize").and_then(Value::as_u64).unwrap_or(0),
    };
    let alloc = fields.get("dsize").and_then(Value::as_u64).unwrap_or(0);
    let mtime = fields.get("mtime").and_then(Value::as_i64).unwrap_or(0);
    let nlink = fields
        .get("nlink")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .min(u32::MAX as u64) as u32;

    let mut node = ImportedNode {
        name,
        kind,
        size,
        alloc,
        mtime,
        nlink,
        children: Vec::with_capacity(children.len()),
    };
    for child in children {
        node.children.push(node_from(child)?);
    }
    Ok(node)
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spacetrace_scan_core::SizeBasis;
    use std::path::Path;

    fn tree_of(json: &str) -> Tree {
        import_ncdu(json, Some(PathBuf::from("/imported"))).expect("should parse")
    }

    /// Shape matters here, not just content. `z.bin` comes **after** the
    /// subdirectory on purpose: without a sibling following a directory, a
    /// depth-first layout still happens to leave every child contiguous and
    /// `the_arena_layout_holds` passes while proving nothing. Mutation
    /// testing found exactly that — the fixture, not the assertion, was the
    /// weak part.
    const SMALL: &str = r#"[1,2,{"progname":"ncdu"},
        [{"name":"root","dsize":4096,"mtime":100},
          {"name":"a.bin","asize":1000,"dsize":4096,"mtime":101},
          [{"name":"sub","dsize":4096,"mtime":102},
            {"name":"b.bin","asize":2000,"dsize":8192,"mtime":103}],
          {"name":"z.bin","asize":500,"dsize":4096,"mtime":104}]]"#;

    #[test]
    fn an_export_becomes_a_tree_with_its_totals_summed() {
        let tree = tree_of(SMALL);
        let root = tree.root();
        assert_eq!(tree.name(root), "root");
        // Own sizes are per entry in this format; the totals are ours to add.
        assert_eq!(tree.total_size(), 3500);
        assert_eq!(tree.total_alloc(), 4096 * 4 + 8192);
        assert_eq!(tree.node(root).files, 3);
        assert_eq!(tree.node(root).dirs, 1);
    }

    /// Invariant 2: children are contiguous and every child's index is greater
    /// than its parent's. An importer that laid the tree out depth-first would
    /// pass every total above and corrupt every consumer.
    #[test]
    fn the_arena_layout_holds() {
        let tree = tree_of(SMALL);
        // The fixture must actually be able to break: a directory with a
        // sibling after it is the only shape a depth-first layout gets wrong.
        let root = tree.root();
        assert!(
            tree.node(root).children_len >= 3,
            "the fixture stopped exercising this"
        );
        for id in tree.iter() {
            let node = tree.node(id);
            for child in tree.children(id) {
                assert!(child > id, "child {child} must come after parent {id}");
                assert_eq!(tree.node(child).parent, id);
            }
            // Contiguity, stated as what it means: the slice starting at
            // `children_start` is exactly this node's children, and the
            // iterator agrees with it.
            let start = node.children_start;
            let by_range: Vec<u32> = (0..node.children_len).map(|o| start + o).collect();
            let by_iter: Vec<u32> = tree.children(id).collect();
            assert_eq!(by_range, by_iter, "children of {id} are not contiguous");
            for child in &by_range {
                assert_eq!(tree.node(*child).parent, id);
            }
        }
    }

    #[test]
    fn a_directory_keeps_its_own_cost() {
        let tree = tree_of(SMALL);
        let sub = tree
            .iter()
            .find(|&id| tree.name(id) == "sub")
            .expect("sub should be there");
        // 4096 of its own plus the 8192 of the file inside it.
        assert_eq!(tree.node(sub).measure(SizeBasis::OnDisk), 12288);
        assert_eq!(tree.node(sub).measure(SizeBasis::Logical), 2000);
    }

    #[test]
    fn a_file_that_is_not_regular_is_not_counted_as_one() {
        let tree = tree_of(r#"[1,2,{},[{"name":"root"},{"name":"link","asize":7,"notreg":true}]]"#);
        let link = tree.iter().find(|&id| tree.name(id) == "link").unwrap();
        assert_eq!(tree.node(link).kind, EntryKind::Other);
    }

    #[test]
    fn missing_sizes_are_zero_not_an_error() {
        let tree = tree_of(r#"[1,2,{},[{"name":"root"},{"name":"empty.txt"}]]"#);
        assert_eq!(tree.total_size(), 0);
        assert_eq!(tree.len(), 2);
    }

    // ------------------------------------------------- refusing bad input

    fn refusal(json: &str) -> String {
        format!(
            "{:#}",
            import_ncdu(json, None).expect_err("should have been refused")
        )
    }

    #[test]
    fn something_that_is_not_json_is_refused_by_name() {
        assert!(refusal("not json at all").contains("not JSON"));
    }

    #[test]
    fn a_json_document_that_is_not_an_export_is_refused() {
        assert!(refusal(r#"{"hello":"world"}"#).contains("array"));
        assert!(refusal("[1,2]").contains("four elements"));
    }

    /// ncdu 2 writes a binary format under a different major version. Reading
    /// its numbers as if they were version 1's would produce a tree full of
    /// plausible wrong sizes, which is worse than refusing.
    #[test]
    fn a_future_format_version_is_refused_rather_than_guessed_at() {
        assert!(refusal(r#"[2,0,{},[{"name":"root"}]]"#).contains("version 2"));
    }

    #[test]
    fn an_entry_without_a_name_is_refused() {
        assert!(refusal(r#"[1,2,{},[{"asize":5}]]"#).contains("no name"));
    }

    #[test]
    fn an_empty_directory_array_is_refused_rather_than_silently_dropped() {
        assert!(refusal(r#"[1,2,{},[]]"#).contains("describe itself"));
    }

    /// The trust-boundary claim, made concrete: a file built to blow the
    /// stack comes back as an error, and the process is still here to report
    /// it. The message is `serde_json`'s, because its recursion limit is what
    /// stops this and a limit of our own above it could never fire.
    #[test]
    fn a_file_nested_beyond_all_reason_is_refused_not_crashed_on() {
        let depth = 5_000;
        let mut json = String::from("[1,2,{},");
        for _ in 0..depth {
            json.push_str(r#"[{"name":"d"},"#);
        }
        json.push_str(r#"{"name":"f","asize":1}"#);
        for _ in 0..depth {
            json.push(']');
        }
        json.push(']');
        assert!(refusal(&json).contains("recursion limit"));
    }

    /// And a tree deeper than anything real, but within what the parser
    /// allows, still imports — the guard must not be so tight that it refuses
    /// a legitimate export. Our own fixtures go 40 levels down.
    #[test]
    fn a_deep_but_legal_tree_still_imports() {
        let depth = 100;
        let mut json = String::from("[1,2,{},");
        for i in 0..depth {
            json.push_str(&format!(r#"[{{"name":"d{i}"}},"#));
        }
        json.push_str(r#"{"name":"f","asize":7}"#);
        for _ in 0..depth {
            json.push(']');
        }
        json.push(']');
        let tree = tree_of(&json);
        assert_eq!(tree.total_size(), 7);
        assert_eq!(tree.len(), depth + 1);
    }

    /// A name long enough to be unaddressable is shortened, not fatal — the
    /// same choice the arena makes for a scanned name.
    #[test]
    fn an_absurdly_long_name_does_not_stop_the_import() {
        let long = "x".repeat(100_000);
        let json = format!(r#"[1,2,{{}},[{{"name":"root"}},{{"name":"{long}","asize":1}}]]"#);
        let tree = tree_of(&json);
        assert_eq!(tree.len(), 2);
    }

    /// Without an override the tree is filed under the path the export names,
    /// because that is the one piece of provenance the file actually carries.
    #[test]
    fn the_root_path_comes_from_the_file_unless_overridden() {
        let json = r#"[1,2,{},[{"name":"/srv/data"},{"name":"a","asize":1}]]"#;
        let taken = import_ncdu(json, None).unwrap();
        assert_eq!(taken.root_path(), Path::new("/srv/data"));

        let overridden = import_ncdu(json, Some(PathBuf::from("/elsewhere"))).unwrap();
        assert_eq!(overridden.root_path(), Path::new("/elsewhere"));
    }

    /// A file as ncdu itself writes it, not as we write it.
    ///
    /// Our own exporter emits a subset; ncdu adds `ino`, `hlnkc`,
    /// `read_error`, `excluded` and more, and a parser that only ever saw its
    /// own output would meet those for the first time on a user's machine.
    /// ncdu is not installed here, so this fixture is built from the format's
    /// documentation rather than captured from a run — which is worth saying
    /// out loud, because it is the one claim in this file not backed by
    /// something executable.
    #[test]
    fn a_file_written_by_ncdu_itself_is_read() {
        let json = r#"[1,0,{"progname":"ncdu","progver":"1.19","timestamp":1700000000},
        [{"name":"/var","asize":4096,"dsize":4096,"dev":2049,"ino":12,"mtime":1699000000},
          {"name":"big.log","asize":123456,"dsize":126976,"ino":13,"mtime":1699000001},
          {"name":"link.a","asize":500,"dsize":4096,"ino":14,"nlink":2,"hlnkc":true},
          {"name":"link.b","asize":500,"dsize":4096,"ino":14,"nlink":2,"hlnkc":true},
          {"name":"dead.sock","asize":0,"dsize":0,"notreg":true},
          [{"name":"locked","read_error":true,"dsize":4096}],
          [{"name":"skipped","excluded":"pattern","dsize":4096}],
          {"name":"tail.txt","asize":10,"dsize":4096}]]"#;

        let tree = import_ncdu(json, None).unwrap();
        assert_eq!(tree.root_path(), Path::new("/var"));
        assert_eq!(tree.len(), 8, "root plus seven entries");

        // Fields we do not model must be ignored, not fatal, and must not
        // shift anything else: the totals are the same ones a reader would
        // add up by hand.
        // One term per entry, in file order, so a wrong total says which
        // entry moved. The directories contribute nothing logical (#1) and
        // their blocks only on disk.
        let sizes: u64 = [123456, 500, 500, 0, 10].iter().sum();
        assert_eq!(tree.total_size(), sizes);
        let allocs: u64 = [4096, 126976, 4096, 4096, 0, 4096, 4096, 4096].iter().sum();
        assert_eq!(tree.total_alloc(), allocs);

        let named = |n: &str| tree.iter().find(|&id| tree.name(id) == n).expect(n);
        assert_eq!(tree.node(named("dead.sock")).kind, EntryKind::Other);
        assert_eq!(tree.node(named("locked")).kind, EntryKind::Dir);
        assert_eq!(tree.node(named("link.a")).nlink, 2);
        // The last entry must survive the two directories before it — a
        // layout bug shows up here and nowhere else.
        assert_eq!(tree.node(named("tail.txt")).own_size, 10);
    }

    /// ncdu deduplicates hardlinks in its own display but writes every name,
    /// and so do we. Both copies are present and both carry the link count;
    /// what a reader does with that is the reader's business, and inventing a
    /// deduplication the file does not describe would change the total.
    #[test]
    fn both_names_of_a_hardlink_are_kept() {
        let json = r#"[1,0,{},[{"name":"/d"},
          {"name":"a","asize":500,"dsize":4096,"nlink":2,"hlnkc":true},
          {"name":"b","asize":500,"dsize":4096,"nlink":2,"hlnkc":true}]]"#;
        let tree = import_ncdu(json, None).unwrap();
        assert_eq!(tree.total_size(), 1000);
        assert_eq!(tree.node(tree.root()).files, 2);
    }
}
