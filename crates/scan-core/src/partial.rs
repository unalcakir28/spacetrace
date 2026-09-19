//! The tree as it stands, while the walk is still filling it.
//!
//! A scan of a real disk takes tens of seconds and produces nothing a reader
//! can use until the last directory lands. This hands out a **complete tree of
//! what has been read so far** — every level, every file, real per-folder
//! totals — so a caller can draw its ordinary view of a scan that is still
//! running rather than a special one built for waiting.
//!
//! **Why this can exist at all.** The arena is append-only and a child is
//! always pushed after its parent (see [`TreeBuilder::push_block`]), so a copy
//! taken mid-walk is not a fragment: it is a smaller tree, with the same
//! shape rules as a finished one. A directory whose listing has not finished
//! contributes nothing rather than half of itself, because its children are
//! pushed as one block when the listing is done.
//!
//! **This is not a way around invariant 5.** A cancelled scan still returns no
//! tree, and nothing here may be stored next to a real snapshot: the reason is
//! the same one that closed that decision — a partial tree looks complete and
//! reports a total that is simply wrong. What changed is only who may look at
//! one, and for how long: a window drawing a scan that is visibly still
//! running, for as long as it is running.
//!
//! **Totals are partial and the caller has to say so.** A folder's figure is
//! what has been read under it, never what is there, and a figure that has
//! stopped climbing looks exactly like a finished one. Nothing here can tell
//! the two apart; only the end of the scan can.
//!
//! **Node ids do not move.** An id is an arena index and the arena only grows,
//! so an id taken from one snapshot means the same entry in the next one — and
//! in the finished tree, which is built from this same arena. That is what lets
//! a caller keep a selection, an expanded folder or a zoom level across a
//! refresh instead of rebuilding its view each time.
//!
//! **What it costs the walk.** One memcpy of the arena per snapshot, taken
//! under the lock the walk already uses to push directories into it — so the
//! walk waits for the copy and nothing else. The aggregation pass, which is
//! the larger half, runs on the copy with the lock released. That split is the
//! whole reason [`TreeBuilder::copy`] and [`TreeBuilder::into_partial`] are two
//! calls rather than one; a snapshot that aggregated under the lock would stall
//! every worker for the length of a pass over the whole tree.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use crate::tree::{Tree, TreeBuilder};

/// The arena while it is being written, readable by whoever holds the progress
/// object.
#[derive(Debug, Default)]
pub struct PartialTree {
    /// The one the walk locks per directory. Nothing that can block goes
    /// inside it — the same rule the walk has always had for this lock.
    builder: Mutex<TreeBuilder>,
    /// Where the walk is rooted, needed to build a `Tree` out of a copy.
    ///
    /// A lock of its own rather than a field inside the one above, because a
    /// snapshot reads it and the walk never does: putting it in the hot lock
    /// would widen a critical section for a value that is written once.
    root_path: Mutex<Option<PathBuf>>,
}

impl PartialTree {
    /// Hand the walk's arena over, so snapshots can be taken from it.
    ///
    /// Installing a second time is how a progress object gets reused for
    /// another scan; the new arena and root replace the old ones outright,
    /// because a copy made from one and named by the other would describe a
    /// walk that never happened.
    pub(crate) fn install(&self, builder: TreeBuilder, root_path: PathBuf) {
        *self.lock() = builder;
        *self.root_path() = Some(root_path);
    }

    /// The arena, for the walk's own writes.
    pub(crate) fn lock(&self) -> MutexGuard<'_, TreeBuilder> {
        // A panicking walker must not take the scan's arena down with it; the
        // same reasoning as `ScanProgress::reading`.
        self.builder
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn root_path(&self) -> MutexGuard<'_, Option<PathBuf>> {
        self.root_path
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Take the arena back out, leaving nothing behind.
    ///
    /// Moved rather than copied: this is the end of the walk, and the finished
    /// tree is built from these exact nodes. After it the snapshots stop —
    /// [`PartialTree::snapshot`] answers `None` for an empty arena, which is
    /// the honest reading of "there is no walk here any more".
    pub(crate) fn take(&self) -> TreeBuilder {
        std::mem::take(&mut *self.lock())
    }

    /// The tree as it stands, or `None` before the walk has a root.
    ///
    /// The totals are aggregated the same way a finished tree's are, so what
    /// comes back is an ordinary [`Tree`] and every reader of one works on it
    /// unchanged. What it is *not* is a finished scan: see the note at the top
    /// of this file.
    pub fn snapshot(&self) -> Option<Tree> {
        let root_path = self.root_path().clone()?;
        // Scoped so the walk gets its lock back before the aggregation pass,
        // which is the longer of the two.
        let copy = {
            let arena = self.lock();
            if arena.is_empty() {
                return None;
            }
            arena.copy()
        };
        copy.into_partial(root_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::EntryKind;
    use crate::tree::NewNode;

    fn node(name: &str, kind: EntryKind, size: u64) -> NewNode<'_> {
        NewNode {
            name,
            kind,
            size: match kind {
                EntryKind::Dir => 0,
                _ => size,
            },
            alloc: size,
            mtime: 0,
            nlink: 1,
        }
    }

    /// The arena is installed before the walk starts, so the window in which
    /// there is nothing to copy is real and has to answer rather than panic.
    #[test]
    fn nothing_before_the_walk_has_a_root() {
        let partial = PartialTree::default();
        assert!(partial.snapshot().is_none(), "no root installed yet");

        partial.install(TreeBuilder::with_capacity(8), PathBuf::from("/root"));
        assert!(
            partial.snapshot().is_none(),
            "installed, but the root node has not been pushed"
        );
    }

    /// The point of the whole file: a copy taken mid-walk is a tree, with the
    /// totals rolled up the way a finished one's are.
    #[test]
    fn a_snapshot_is_a_tree_with_real_totals() {
        let partial = PartialTree::default();
        let mut builder = TreeBuilder::with_capacity(8);
        let root = builder.push_root(node("root", EntryKind::Dir, 0));
        partial.install(builder, PathBuf::from("/root"));

        let first = partial.lock().push_block(
            root,
            [
                node("big", EntryKind::Dir, 0),
                node("small.txt", EntryKind::File, 100),
            ]
            .into_iter(),
        );

        let seen = partial.snapshot().expect("a root has been pushed");
        assert_eq!(seen.len(), 3);
        assert_eq!(
            seen.node(seen.root()).alloc,
            100,
            "the root carries what has been found under it so far"
        );

        // `big` is listed a moment later, the way a walk fills one in.
        partial
            .lock()
            .push_block(first, [node("a.bin", EntryKind::File, 900)].into_iter());

        let seen = partial.snapshot().expect("still running");
        assert_eq!(seen.len(), 4);
        assert_eq!(
            seen.node(seen.root()).alloc,
            1000,
            "and the total climbs as the walk reads more"
        );
        assert_eq!(
            seen.node(first).alloc,
            900,
            "each folder carries its own subtree, not the whole scan"
        );
        assert_eq!(seen.node(seen.root()).files, 2);
    }

    /// The reason a caller can keep a selection across a refresh. If ids moved,
    /// every view would have to be rebuilt from nothing on each tick.
    #[test]
    fn ids_mean_the_same_entry_in_a_later_snapshot() {
        let partial = PartialTree::default();
        let mut builder = TreeBuilder::with_capacity(8);
        let root = builder.push_root(node("root", EntryKind::Dir, 0));
        partial.install(builder, PathBuf::from("/root"));
        let first = partial
            .lock()
            .push_block(root, [node("keep", EntryKind::Dir, 0)].into_iter());

        let before = partial.snapshot().expect("running");
        let named = before.name(first).to_string();

        partial
            .lock()
            .push_block(first, [node("later.bin", EntryKind::File, 7)].into_iter());

        let after = partial.snapshot().expect("still running");
        assert_eq!(after.name(first), named, "the id still names that entry");
    }

    /// Aggregation must run on the copy, never on the arena the walk is still
    /// writing to: it adds every child into its parent, so a second pass over
    /// the same nodes would count the whole tree twice.
    #[test]
    fn snapshotting_twice_does_not_inflate_the_totals() {
        let partial = PartialTree::default();
        let mut builder = TreeBuilder::with_capacity(8);
        let root = builder.push_root(node("root", EntryKind::Dir, 0));
        partial.install(builder, PathBuf::from("/root"));
        partial
            .lock()
            .push_block(root, [node("one.bin", EntryKind::File, 512)].into_iter());

        let first = partial.snapshot().expect("running").node(0).alloc;
        let second = partial.snapshot().expect("running").node(0).alloc;
        assert_eq!(first, 512);
        assert_eq!(second, 512, "the walk's own arena was left untouched");
    }

    /// After the walk hands its arena over there is no tree here any more, and
    /// the caller may still have a refresh in flight.
    #[test]
    fn nothing_is_left_once_the_walk_takes_its_arena_back() {
        let partial = PartialTree::default();
        let mut builder = TreeBuilder::with_capacity(8);
        builder.push_root(node("root", EntryKind::Dir, 0));
        partial.install(builder, PathBuf::from("/root"));
        assert!(partial.snapshot().is_some());

        let taken = partial.take();
        assert!(
            !taken.is_empty(),
            "the nodes moved out rather than being lost"
        );
        assert!(partial.snapshot().is_none());
    }
}
