//! Object keys into a folder tree.
//!
//! S3 has no folders. A key is a string, and `/` in it is only a convention
//! that every console and CLI honours by drawing folders. This module draws
//! them the same way and hands the result to `Tree::from_nested`, so the arena
//! layout and the aggregation are the scanner's own and not a second copy
//! (CLAUDE.md, "tree from outside").
//!
//! What a folder tree cannot say directly, and what is done instead:
//!
//! * **A folder marker** — a key ending in `/`, usually zero bytes, which the
//!   console writes for "Create folder" — becomes that folder, not a file.
//!   Should it hold bytes, they are the folder's own cost on the `alloc` side,
//!   the way a directory's own blocks are on a disk (invariant 1), and stay out
//!   of the logical total.
//! * **An object and a folder with one name** (`a` and `a/b`) — the folder
//!   keeps the name, and the object's bytes are charged to it the same way as
//!   a marker's: present in `alloc`, absent from `size`. Two siblings with one
//!   name would break `diff`, which pairs entries by name, and `--subpath`,
//!   which finds them by name; renaming the object would invent a key that
//!   does not exist. Counted and sampled, so the summary says it happened.
//! * **Empty segments** (`/x`, `a//b`) have no name a path can carry, so they
//!   are collapsed: `a//b` is drawn as `a/b`. Should that make two keys land on
//!   one name, the two are one entry with both sizes, again counted.
//!
//! Keys arrive in byte order from ListObjectsV2, which makes every folder's
//! keys contiguous and lets this hold only the folders on the current path
//! open. Nothing here depends on that for correctness — a folder met again
//! after it was closed is reopened in place — only for speed.

use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use spacetrace_scan_core::{EntryKind, ImportedNode};

use super::xml::Object;

/// AWS, MinIO, R2, B2 and Wasabi all cap a key at 1024 bytes. Four times that
/// is refused: a key is depth, depth reaches recursion in `ImportedNode`'s
/// drop, and the server deciding our stack depth is the one thing a trust
/// boundary is for.
pub const MAX_KEY_BYTES: usize = 4096;

/// How many example keys a summary shows for each kind of oddity.
const SAMPLES: usize = 5;

/// What the listing held beyond what the tree shows.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct KeyStats {
    /// Every object listed, markers included.
    pub objects: u64,
    /// Bytes of every object listed — what the bucket stores for the current
    /// versions, and what the tree's `alloc` total adds up to.
    pub bytes: u64,
    /// Keys ending in `/`, drawn as their folder.
    pub folder_markers: u64,
    /// Objects whose name is also a folder's, charged to that folder.
    pub shadowed: u64,
    /// Objects that landed on another object's name once empty segments were
    /// collapsed, and were added into it.
    pub merged: u64,
    pub samples: Vec<String>,
}

struct Open {
    node: ImportedNode,
    /// Name → index into `node.children`, for this folder only. Dropped when
    /// the folder closes, so memory is held for the open path and not for the
    /// whole bucket.
    names: HashMap<String, usize>,
    /// Where in the parent's children this folder goes back when it closes:
    /// `None` for a folder seen for the first time, `Some` for one reopened.
    slot: Option<usize>,
}

impl Open {
    fn new(node: ImportedNode, slot: Option<usize>) -> Self {
        let names = node
            .children
            .iter()
            .enumerate()
            .map(|(i, c)| (c.name.clone(), i))
            .collect();
        Open { node, names, slot }
    }
}

pub struct KeyTree {
    prefix: String,
    /// `stack[0]` is the root; `stack[1..]` the folders on the current path.
    stack: Vec<Open>,
    stats: KeyStats,
    folders: u64,
}

impl KeyTree {
    /// `prefix` is the folder being listed, `""` or ending in `/`; every key
    /// is drawn relative to it. `root_name` is what the root calls itself.
    pub fn new(root_name: &str, prefix: &str) -> Self {
        KeyTree {
            prefix: prefix.to_string(),
            stack: vec![Open::new(ImportedNode::dir(root_name), None)],
            stats: KeyStats::default(),
            folders: 0,
        }
    }

    /// Folders created so far, for the progress counter.
    pub fn folders(&self) -> u64 {
        self.folders
    }

    pub fn insert(&mut self, object: &Object) -> Result<()> {
        if object.key.len() > MAX_KEY_BYTES {
            bail!(
                "the server listed a key of {} bytes; S3 allows 1024, and this refuses more than {MAX_KEY_BYTES}",
                object.key.len()
            );
        }
        let Some(rel) = object.key.strip_prefix(self.prefix.as_str()) else {
            bail!(
                "the server listed {:?}, which is outside the requested prefix {:?}",
                object.key,
                self.prefix
            );
        };
        // Checked once, on the grand total: every folder's sum is a part of
        // it, so nothing the tree adds up afterwards can overflow either.
        self.stats.bytes = self.stats.bytes.checked_add(object.size).with_context(|| {
            format!(
                "the listing adds up to more than {} bytes at {:?}; that is not a real bucket",
                u64::MAX,
                object.key
            )
        })?;
        self.stats.objects += 1;

        let is_marker = rel.is_empty() || rel.ends_with('/');
        let mut segments: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
        let file_name = match is_marker {
            true => None,
            // A key not ending in `/` ends in a non-empty segment.
            false => segments.pop(),
        };

        self.descend_to(&segments);
        let folder = self.top();

        let Some(name) = file_name else {
            charge_to_folder(&mut folder.node, object);
            self.stats.folder_markers += 1;
            return Ok(());
        };

        match folder.names.get(name).copied() {
            None => {
                folder
                    .names
                    .insert(name.to_string(), folder.node.children.len());
                folder.node.children.push(ImportedNode {
                    mtime: object.last_modified,
                    ..ImportedNode::file(name, object.size, object.size)
                });
            }
            Some(i) if folder.node.children[i].kind == EntryKind::Dir => {
                charge_to_folder(&mut folder.node.children[i], object);
                self.note_shadowed(&object.key);
            }
            Some(i) => {
                let existing = &mut folder.node.children[i];
                existing.size += object.size;
                existing.alloc += object.size;
                existing.mtime = existing.mtime.max(object.last_modified);
                self.stats.merged += 1;
                self.sample(&object.key);
            }
        }
        Ok(())
    }

    /// Close everything and hand back the root.
    pub fn finish(mut self) -> (ImportedNode, KeyStats) {
        while self.stack.len() > 1 {
            self.close();
        }
        let mut root = self
            .stack
            .pop()
            .expect("the root is never popped before here");
        root.node.mtime = newest(&root.node);
        (root.node, self.stats)
    }

    fn top(&mut self) -> &mut Open {
        self.stack
            .last_mut()
            .expect("the root stays on the stack until finish")
    }

    /// Make `segments` the open path, closing and opening folders as needed.
    fn descend_to(&mut self, segments: &[&str]) {
        let open_path = &self.stack[1..];
        let common = open_path
            .iter()
            .zip(segments)
            .take_while(|(open, seg)| open.node.name == **seg)
            .count();
        while self.stack.len() > common + 1 {
            self.close();
        }
        for seg in &segments[common..] {
            self.open(seg);
        }
    }

    fn open(&mut self, name: &str) {
        let parent = self.top();
        let opened = match parent.names.get(name).copied() {
            None => {
                self.folders += 1;
                Open::new(ImportedNode::dir(name), None)
            }
            Some(i) => {
                // Taken out and put back on close, rather than removed, so no
                // other index in `names` moves.
                let existing =
                    std::mem::replace(&mut parent.node.children[i], ImportedNode::dir(""));
                match existing.kind {
                    EntryKind::Dir => Open::new(existing, Some(i)),
                    // `a` listed before `a/b` — the usual order, since a key
                    // sorts before every key it prefixes.
                    _ => {
                        self.folders += 1;
                        let mut folder = ImportedNode::dir(name);
                        folder.alloc = existing.alloc;
                        folder.mtime = existing.mtime;
                        let key = format!("{}{name}", self.current_path());
                        self.note_shadowed(&key);
                        Open::new(folder, Some(i))
                    }
                }
            }
        };
        self.stack.push(opened);
    }

    fn close(&mut self) {
        let Some(mut done) = self.stack.pop() else {
            return;
        };
        done.node.mtime = newest(&done.node);
        let parent = self.top();
        match done.slot {
            Some(i) => parent.node.children[i] = done.node,
            None => {
                parent
                    .names
                    .insert(done.node.name.clone(), parent.node.children.len());
                parent.node.children.push(done.node);
            }
        }
    }

    /// The prefix plus the open folders, for naming a key in a sample.
    fn current_path(&self) -> String {
        let mut path = self.prefix.clone();
        for open in &self.stack[1..] {
            path.push_str(&open.node.name);
            path.push('/');
        }
        path
    }

    fn note_shadowed(&mut self, key: &str) {
        self.stats.shadowed += 1;
        self.sample(key);
    }

    fn sample(&mut self, key: &str) {
        if self.stats.samples.len() < SAMPLES {
            self.stats.samples.push(key.to_string());
        }
    }
}

/// A marker's or a shadowed object's bytes, as the folder's own cost.
fn charge_to_folder(folder: &mut ImportedNode, object: &Object) {
    folder.alloc += object.size;
    folder.mtime = folder.mtime.max(object.last_modified);
}

/// A folder's time is its newest content's: S3 keeps no time for a prefix, and
/// "last changed when something in it last changed" is what a folder's time
/// means to someone reading `age` or a file manager. A marker's own time
/// counts too.
fn newest(folder: &ImportedNode) -> i64 {
    folder
        .children
        .iter()
        .map(|c| c.mtime)
        .fold(folder.mtime, i64::max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use spacetrace_scan_core::{SizeBasis, Tree};
    use spacetrace_store::Store;
    use std::path::PathBuf;

    fn obj(key: &str, size: u64, t: i64) -> Object {
        Object {
            key: key.into(),
            size,
            last_modified: t,
        }
    }

    fn build(prefix: &str, objects: &[Object]) -> (Tree, KeyStats) {
        let mut keys = KeyTree::new("s3://b", prefix);
        for o in objects {
            keys.insert(o).unwrap();
        }
        let (root, stats) = keys.finish();
        (Tree::from_nested(PathBuf::from("s3://b"), root), stats)
    }

    fn sorted(mut objects: Vec<Object>) -> Vec<Object> {
        objects.sort_by(|a, b| a.key.as_bytes().cmp(b.key.as_bytes()));
        objects
    }

    /// The same fixture every test below leans on: nesting, siblings after a
    /// folder (the shape a wrong layout breaks), a marker, an object shadowed
    /// by a folder, and the characters that get mangled on the way.
    fn fixture() -> Vec<Object> {
        sorted(vec![
            obj("a", 7, 100),
            obj("a/b", 10, 200),
            obj("dir/a b.txt", 17, 300),
            obj("dir/plus+sign.txt", 23, 301),
            obj("dir/100%.txt", 18, 302),
            obj("dir/ünï/ファイル.txt", 39, 303),
            obj("empty/", 0, 400),
            obj("heavy-marker/", 5, 401),
            obj("heavy-marker/x", 1, 50),
            obj("z.bin", 1000, 500),
        ])
    }

    fn named(tree: &Tree, path: &str) -> u32 {
        tree.find(path)
            .unwrap_or_else(|| panic!("{path} is not in the tree"))
    }

    #[test]
    fn keys_become_folders_and_files() {
        let (tree, stats) = build("", &fixture());
        assert_eq!(tree.node(named(&tree, "dir")).kind, EntryKind::Dir);
        assert_eq!(tree.node(named(&tree, "dir/ünï/ファイル.txt")).size, 39);
        assert_eq!(tree.node(named(&tree, "dir/a b.txt")).size, 17);
        assert_eq!(tree.node(named(&tree, "dir/plus+sign.txt")).size, 23);
        assert_eq!(tree.node(named(&tree, "dir/100%.txt")).size, 18);
        assert_eq!(stats.objects, 10);
        assert_eq!(stats.bytes, 7 + 10 + 17 + 23 + 18 + 39 + 5 + 1 + 1000);
    }

    /// `alloc` is the whole bucket; `size` leaves out the bytes the tree can
    /// only charge to a folder. With neither markers nor shadowed objects
    /// holding bytes the two are equal, which is the usual case.
    #[test]
    fn the_totals_split_the_way_the_documentation_says() {
        let (tree, stats) = build("", &fixture());
        assert_eq!(
            tree.total_alloc(),
            stats.bytes,
            "alloc is every listed byte"
        );
        assert_eq!(
            tree.total_size(),
            stats.bytes - 7 - 5,
            "size leaves out the shadowed object and the marker's bytes"
        );
        assert_eq!(tree.node(tree.root()).files, 7);
    }

    #[test]
    fn a_folder_marker_is_its_folder_not_a_file() {
        let (tree, stats) = build("", &fixture());
        let empty = tree.node(named(&tree, "empty"));
        assert_eq!(empty.kind, EntryKind::Dir);
        assert_eq!(empty.children_len, 0);
        assert_eq!(empty.mtime, 400, "the marker's time is the folder's");
        assert_eq!(stats.folder_markers, 2);
        assert!(
            tree.iter()
                .all(|id| tree.name(id) != "empty/" && !tree.name(id).is_empty()),
            "no entry is named after the marker key"
        );
        let heavy = tree.node(named(&tree, "heavy-marker"));
        assert_eq!((heavy.size, heavy.alloc), (1, 6));
    }

    #[test]
    fn an_object_shadowed_by_a_folder_is_charged_to_it_and_reported() {
        let (tree, stats) = build("", &fixture());
        let a = tree.node(named(&tree, "a"));
        assert_eq!(a.kind, EntryKind::Dir);
        assert_eq!((a.size, a.alloc), (10, 17));
        assert_eq!(a.mtime, 200, "newest of the object and its content");
        assert_eq!(stats.shadowed, 1);
        assert_eq!(stats.samples, ["a"]);
        let siblings: Vec<&str> = tree.children(tree.root()).map(|c| tree.name(c)).collect();
        assert_eq!(
            siblings.iter().filter(|n| **n == "a").count(),
            1,
            "{siblings:?}"
        );
    }

    #[test]
    fn folders_take_their_newest_content_s_time() {
        let (tree, _) = build("", &fixture());
        assert_eq!(tree.node(named(&tree, "dir")).mtime, 303);
        assert_eq!(tree.node(named(&tree, "dir/ünï")).mtime, 303);
        assert_eq!(tree.node(tree.root()).mtime, 500);
    }

    /// `/a/x` sorts before `/b`, which closes `a`, and `a//b` then has to find
    /// that same `a` again rather than drawing a second one beside it.
    #[test]
    fn empty_segments_are_collapsed_and_collisions_counted() {
        let (tree, stats) = build(
            "",
            &sorted(vec![
                obj("/lead", 1, 0),
                obj("lead", 2, 0),
                obj("a//b", 4, 0),
                obj("a/b", 8, 0),
                obj("a/c", 16, 0),
                obj("/a/x", 32, 0),
                obj("/b", 64, 0),
            ]),
        );
        assert_eq!(tree.node(named(&tree, "lead")).size, 3);
        assert_eq!(tree.node(named(&tree, "a/b")).size, 12);
        assert_eq!(tree.total_size(), 127, "nothing lost");
        assert_eq!(stats.merged, 2);
        assert_eq!(tree.node(named(&tree, "a")).children_len, 3, "x, b, c");
        let root_names: Vec<&str> = tree.children(tree.root()).map(|c| tree.name(c)).collect();
        assert_eq!(
            root_names.len(),
            3,
            "a, b, lead and nothing twice: {root_names:?}"
        );
    }

    /// The prefix is the root: keys are drawn below it, and its own marker is
    /// the root's.
    #[test]
    fn keys_are_relative_to_the_prefix() {
        let (tree, stats) = build(
            "photos/",
            &[
                obj("photos/", 0, 9),
                obj("photos/2024/a.jpg", 5, 10),
                obj("photos/b.jpg", 6, 11),
            ],
        );
        assert_eq!(tree.node(named(&tree, "2024/a.jpg")).size, 5);
        assert_eq!(tree.total_size(), 11);
        assert_eq!(stats.folder_markers, 1);
        assert!(tree.find("photos").is_none());
    }

    #[test]
    fn a_key_outside_the_prefix_or_too_long_is_refused() {
        let mut keys = KeyTree::new("s3://b", "photos/");
        let err = keys.insert(&obj("other/x", 1, 0)).unwrap_err();
        assert!(
            err.to_string().contains("outside the requested prefix"),
            "{err}"
        );
        let deep = "a/".repeat(MAX_KEY_BYTES);
        let err = keys
            .insert(&obj(&format!("photos/{deep}"), 1, 0))
            .unwrap_err();
        assert!(err.to_string().contains("bytes"), "{err}");
    }

    /// Sizes come off the network. Two that add up past `u64` are an error
    /// here, before aggregation could wrap or panic on them.
    #[test]
    fn sizes_that_overflow_the_total_are_refused() {
        let mut keys = KeyTree::new("s3://b", "");
        keys.insert(&obj("a", u64::MAX, 0)).unwrap();
        let err = keys.insert(&obj("b", 1, 0)).unwrap_err();
        assert!(err.to_string().contains("not a real bucket"), "{err}");
    }

    /// Order is a speed assumption, not a correctness one: the same keys in
    /// reverse give the same tree, path by path.
    #[test]
    fn key_order_does_not_change_the_answer() {
        let forward = fixture();
        let mut backward = forward.clone();
        backward.reverse();
        let (a, sa) = build("", &forward);
        let (b, sb) = build("", &backward);
        assert_eq!(paths(&a), paths(&b));
        assert_eq!((sa.objects, sa.bytes), (sb.objects, sb.bytes));
        assert_eq!(a.total_alloc(), b.total_alloc());
    }

    fn paths(tree: &Tree) -> Vec<(String, u64, u64, i64)> {
        let mut out: Vec<_> = tree
            .iter()
            .map(|id| {
                let n = tree.node(id);
                (tree.rel_path(id), n.size, n.alloc, n.mtime)
            })
            .collect();
        out.sort();
        out
    }

    /// Invariant 2, and the store's own validation: the tree is saved, loaded
    /// back through `TreeAssembler::finish`, and is the same tree. An in-memory
    /// comparison alone missed a broken root once (CLAUDE.md, 11 September).
    #[test]
    fn the_tree_survives_save_and_load() {
        let (tree, _) = build("", &fixture());
        assert!(
            !tree.node(tree.root()).has_parent(),
            "root parent is NO_PARENT"
        );
        let stats = spacetrace_scan_core::ScanStats {
            files: u64::from(tree.node(tree.root()).files),
            dirs: u64::from(tree.node(tree.root()).dirs),
            errors: 0,
            hardlinks_deduped: 0,
            clones_deduped: 0,
            error_samples: Vec::new(),
            duration_ms: 0,
            capacity: None,
        };
        let mut store = Store::open_in_memory().unwrap();
        let id = store.save(&tree, &stats, "localhost:9000", None).unwrap();
        let (loaded, meta) = store.load(id).unwrap();
        assert_eq!(meta.root, "s3://b");
        assert_eq!(meta.host, "localhost:9000");
        assert_eq!(paths(&loaded), paths(&tree));
        assert_eq!(loaded.total_size(), tree.total_size());
        assert_eq!(loaded.total_alloc(), tree.total_alloc());
        assert_eq!(
            loaded.children_by(loaded.root(), SizeBasis::Logical).len(),
            tree.children_by(tree.root(), SizeBasis::Logical).len()
        );
        assert_eq!(
            store.verify(id).unwrap(),
            spacetrace_store::Integrity::Intact
        );
    }
}
