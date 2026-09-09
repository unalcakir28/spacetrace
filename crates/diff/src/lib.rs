//! Comparing two snapshots of the same tree.
//!
//! The useful question is not "which paths differ" — after a week of work that
//! is thousands of them — but "where did the space actually go". So the walk
//! descends while a single child explains almost all of a directory's change,
//! and reports the first directory where the change genuinely spreads out.
//! A directory that appeared or vanished is reported once, as a whole.

use std::cmp::Ordering;

use spacetrace_scan_core::{EntryKind, NodeId, Tree};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    /// Present in both snapshots, bigger now.
    Grown,
    /// Present in both snapshots, smaller now.
    Shrunk,
    /// Only in the new snapshot.
    Added,
    /// Only in the old snapshot.
    Removed,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Change {
    /// Path relative to the scan root, `/`-separated.
    pub path: String,
    pub entry: EntryKind,
    pub kind: ChangeKind,
    pub old_size: u64,
    pub new_size: u64,
    pub old_alloc: u64,
    pub new_alloc: u64,
    /// How many levels below the root this entry sits.
    pub depth: usize,
}

impl Change {
    /// Signed change in logical bytes.
    pub fn delta(&self) -> i64 {
        self.new_size as i64 - self.old_size as i64
    }

    /// Signed change in allocated bytes.
    pub fn delta_alloc(&self) -> i64 {
        self.new_alloc as i64 - self.old_alloc as i64
    }
}

#[derive(Debug, Clone)]
pub struct DiffOptions {
    /// Ignore anything whose logical change is smaller than this.
    pub min_delta: u64,
    /// Keep descending while one child explains at least this fraction of a
    /// directory's change. 0.9 means "one folder is 90% of the story".
    pub concentration: f64,
    /// Report changed files too, not only directories.
    pub include_files: bool,
    /// Never descend deeper than this many levels below the root.
    pub max_depth: Option<usize>,
}

impl Default for DiffOptions {
    fn default() -> Self {
        DiffOptions {
            min_delta: 1024 * 1024,
            concentration: 0.9,
            include_files: false,
            max_depth: None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DiffReport {
    pub old_total: u64,
    pub new_total: u64,
    pub old_alloc: u64,
    pub new_alloc: u64,
    /// Biggest absolute change first.
    pub changes: Vec<Change>,
}

impl DiffReport {
    pub fn delta(&self) -> i64 {
        self.new_total as i64 - self.old_total as i64
    }

    pub fn delta_alloc(&self) -> i64 {
        self.new_alloc as i64 - self.old_alloc as i64
    }

    pub fn is_unchanged(&self) -> bool {
        self.changes.is_empty() && self.delta() == 0
    }

    pub fn grown(&self) -> impl Iterator<Item = &Change> {
        self.changes
            .iter()
            .filter(|c| matches!(c.kind, ChangeKind::Grown | ChangeKind::Added))
    }

    pub fn shrunk(&self) -> impl Iterator<Item = &Change> {
        self.changes
            .iter()
            .filter(|c| matches!(c.kind, ChangeKind::Shrunk | ChangeKind::Removed))
    }
}

/// Compare two snapshots of the same root.
pub fn diff(old: &Tree, new: &Tree, opts: &DiffOptions) -> DiffReport {
    let mut changes = Vec::new();
    walk(old, old.root(), new, new.root(), "", 0, opts, &mut changes);
    changes.sort_unstable_by(|a, b| {
        b.delta()
            .abs()
            .cmp(&a.delta().abs())
            .then_with(|| a.path.cmp(&b.path))
    });

    DiffReport {
        old_total: old.total_size(),
        new_total: new.total_size(),
        old_alloc: old.total_alloc(),
        new_alloc: new.total_alloc(),
        changes,
    }
}

#[allow(clippy::too_many_arguments)]
fn walk(
    old: &Tree,
    old_id: NodeId,
    new: &Tree,
    new_id: NodeId,
    path: &str,
    depth: usize,
    opts: &DiffOptions,
    out: &mut Vec<Change>,
) {
    // Children handled here sit at `depth + 1`, so this is the level at which
    // we must stop descending to keep every reported path within max_depth.
    let deep_enough = opts.max_depth.is_some_and(|max| depth + 1 >= max);

    let pairs = match_children(old, old_id, new, new_id);

    // Children that exist on only one side are whole-subtree events; report the
    // topmost one and never descend into it.
    let mut matched: Vec<(NodeId, NodeId, &str)> = Vec::new();
    for pair in &pairs {
        match *pair {
            Pair::Both(o, n, name) => matched.push((o, n, name)),
            Pair::OnlyOld(o, name) => {
                let node = old.node(o);
                if node.size >= opts.min_delta && (opts.include_files || node.is_dir()) {
                    out.push(Change {
                        path: join(path, name),
                        entry: node.kind,
                        kind: ChangeKind::Removed,
                        old_size: node.size,
                        new_size: 0,
                        old_alloc: node.alloc,
                        new_alloc: 0,
                        depth: depth + 1,
                    });
                }
            }
            Pair::OnlyNew(n, name) => {
                let node = new.node(n);
                if node.size >= opts.min_delta && (opts.include_files || node.is_dir()) {
                    out.push(Change {
                        path: join(path, name),
                        entry: node.kind,
                        kind: ChangeKind::Added,
                        old_size: 0,
                        new_size: node.size,
                        old_alloc: 0,
                        new_alloc: node.alloc,
                        depth: depth + 1,
                    });
                }
            }
        }
    }

    for (o, n, name) in matched {
        let (on, nn) = (old.node(o), new.node(n));
        let delta = nn.size as i64 - on.size as i64;
        if delta.unsigned_abs() < opts.min_delta {
            continue;
        }
        let child_path = join(path, name);

        let is_dir = on.is_dir() && nn.is_dir();
        if !is_dir {
            if opts.include_files {
                out.push(make_change(child_path, on.kind, on, nn, depth + 1));
            }
            continue;
        }

        // Does one subdirectory explain nearly all of it? If so, the useful
        // answer is deeper down, so keep going rather than blaming this level.
        if !deep_enough && dominated_by_one_child(old, o, new, n, delta, opts.concentration) {
            walk(old, o, new, n, &child_path, depth + 1, opts, out);
        } else {
            out.push(make_change(
                child_path.clone(),
                EntryKind::Dir,
                on,
                nn,
                depth + 1,
            ));
            if !deep_enough && opts.include_files {
                walk(old, o, new, n, &child_path, depth + 1, opts, out);
            }
        }
    }
}

fn make_change(
    path: String,
    entry: EntryKind,
    on: &spacetrace_scan_core::Node,
    nn: &spacetrace_scan_core::Node,
    depth: usize,
) -> Change {
    Change {
        path,
        entry,
        kind: if nn.size >= on.size {
            ChangeKind::Grown
        } else {
            ChangeKind::Shrunk
        },
        old_size: on.size,
        new_size: nn.size,
        old_alloc: on.alloc,
        new_alloc: nn.alloc,
        depth,
    }
}

/// True when a single matched child accounts for `>= concentration` of the
/// directory's own change, so the real story is one level deeper.
fn dominated_by_one_child(
    old: &Tree,
    old_id: NodeId,
    new: &Tree,
    new_id: NodeId,
    parent_delta: i64,
    concentration: f64,
) -> bool {
    if parent_delta == 0 {
        return false;
    }
    let threshold = (parent_delta.abs() as f64 * concentration) as i64;
    for pair in match_children(old, old_id, new, new_id) {
        if let Pair::Both(o, n, _) = pair {
            let (on, nn) = (old.node(o), new.node(n));
            if !on.is_dir() || !nn.is_dir() {
                continue;
            }
            let child_delta = nn.size as i64 - on.size as i64;
            // Same direction, and big enough to be the whole explanation.
            if child_delta.signum() == parent_delta.signum() && child_delta.abs() >= threshold {
                return true;
            }
        }
    }
    false
}

enum Pair<'a> {
    Both(NodeId, NodeId, &'a str),
    OnlyOld(NodeId, &'a str),
    OnlyNew(NodeId, &'a str),
}

/// Merge-join the children of two directories by name.
fn match_children<'a>(
    old: &'a Tree,
    old_id: NodeId,
    new: &'a Tree,
    new_id: NodeId,
) -> Vec<Pair<'a>> {
    let mut a: Vec<NodeId> = old.children(old_id).collect();
    let mut b: Vec<NodeId> = new.children(new_id).collect();
    a.sort_unstable_by(|&x, &y| old.name(x).cmp(old.name(y)));
    b.sort_unstable_by(|&x, &y| new.name(x).cmp(new.name(y)));

    let mut out = Vec::with_capacity(a.len().max(b.len()));
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        let an = old.name(a[i]);
        let bn = new.name(b[j]);
        match an.cmp(bn) {
            Ordering::Equal => {
                out.push(Pair::Both(a[i], b[j], an));
                i += 1;
                j += 1;
            }
            Ordering::Less => {
                out.push(Pair::OnlyOld(a[i], an));
                i += 1;
            }
            Ordering::Greater => {
                out.push(Pair::OnlyNew(b[j], bn));
                j += 1;
            }
        }
    }
    for &id in &a[i..] {
        out.push(Pair::OnlyOld(id, old.name(id)));
    }
    for &id in &b[j..] {
        out.push(Pair::OnlyNew(id, new.name(id)));
    }
    out
}

fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}/{name}")
    }
}
