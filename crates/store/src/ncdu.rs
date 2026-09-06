//! Export a tree in ncdu's JSON format.
//!
//! ncdu is what people already have on their servers, so being readable by
//! `ncdu -f` (and by gdu, which accepts the same format) makes a spacetrace
//! scan useful before any GUI exists.
//!
//! The format is `[majorver, minorver, metadata, tree]`, where a directory is
//! an array whose first element describes the directory itself and whose
//! remaining elements are its children.

use std::io::{self, Write};

use spacetrace_scan_core::{EntryKind, NodeId, Tree};

/// Write `tree` to `out` as ncdu-compatible JSON.
pub fn export_ncdu(tree: &Tree, out: &mut impl Write) -> io::Result<()> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    write!(
        out,
        "[1,2,{{\"progname\":\"spacetrace\",\"progver\":{},\"timestamp\":{}}},",
        json_str(env!("CARGO_PKG_VERSION")),
        timestamp
    )?;
    write_node(tree, tree.root(), out)?;
    writeln!(out, "]")
}

fn write_node(tree: &Tree, id: NodeId, out: &mut impl Write) -> io::Result<()> {
    let node = tree.node(id);
    let is_dir = node.kind == EntryKind::Dir;

    if is_dir {
        out.write_all(b"[")?;
    }

    // For a directory, `size`/`alloc` are subtree totals; ncdu wants the
    // directory's own cost here and sums the children itself.
    let (asize, dsize) = if is_dir {
        (0, node.alloc.saturating_sub(children_alloc(tree, id)))
    } else {
        (node.size, node.alloc)
    };

    write!(out, "{{\"name\":{}", json_str(&node.name))?;
    if asize > 0 {
        write!(out, ",\"asize\":{asize}")?;
    }
    if dsize > 0 {
        write!(out, ",\"dsize\":{dsize}")?;
    }
    write!(out, ",\"mtime\":{}", node.mtime)?;
    match node.kind {
        EntryKind::Symlink => out.write_all(b",\"notreg\":true")?,
        EntryKind::Other => out.write_all(b",\"notreg\":true")?,
        _ => {}
    }
    if node.nlink > 1 && node.kind == EntryKind::File {
        write!(out, ",\"nlink\":{},\"hlnkc\":true", node.nlink)?;
    }
    out.write_all(b"}")?;

    if is_dir {
        for child in tree.children(id) {
            out.write_all(b",")?;
            write_node(tree, child, out)?;
        }
        out.write_all(b"]")?;
    }
    Ok(())
}

fn children_alloc(tree: &Tree, id: NodeId) -> u64 {
    tree.children(id).map(|c| tree.node(c).alloc).sum()
}

fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}
