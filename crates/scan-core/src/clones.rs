//! Which name of an APFS clone family carries its blocks.
//!
//! A clone family is files that share blocks because one was copied from
//! another with `clonefile` (`cp -c`, Finder's duplicate, Cargo's artifact
//! copies). The listing names the family (`(device, clone id)`, see
//! `bulk.rs`), and the disk holds its blocks once, so one member is charged
//! and the rest carry nothing.
//!
//! **Which member is decided after the walk, in `(depth, path)` order**
//! ([`PathOrder`](crate::tree::PathOrder)), for the reason `extents.rs` gives
//! for btrfs: decided by whichever thread met a member first, the bytes moved
//! between the members from one scan to the next with nothing on disk
//! changing, and a diff read that as one folder growing and another shrinking.
//! Measured on a fixture of 200 families split across `live/`, `snap/1/` and
//! `backup/deep/copy/`: 40 scans gave five different answers.
//!
//! **The walk still charges the first member it meets**, exactly as it did:
//! the running total stays right while the walk is going, and a family that
//! turns out to have one member inside the root — most of them, on a real
//! disk — never needs anything after it. Afterwards the pass below moves the
//! charge, size and blocks both, from that member to the shallowest one, and
//! touches only families with two or more members.

use crate::meta::RawMeta;
use crate::tree::{NodeId, TreeBuilder};

/// Every clone-family member the walk charged or skipped, until the walk ends.
#[derive(Debug, Default)]
pub(crate) struct Families {
    members: Vec<Member>,
}

/// One name of a family. Its own size and blocks are kept because the walk
/// put zero in the arena for every member but the first.
#[derive(Debug, Clone, Copy)]
struct Member {
    dev: u64,
    clone_id: u64,
    node: NodeId,
    ino: u64,
    size: u64,
    alloc: u64,
    /// The member the walk charged.
    charged: bool,
}

/// What moving the charges changed, for the scan's statistics.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Moved {
    /// Change in the bytes `du` counts that `alloc` does not: the old holder's
    /// blocks are now a repeat, and the new holder's are not. Signed, because
    /// two members of one family need not hold the same number of blocks — a
    /// clone written to since has blocks of its own.
    pub shared_bytes: i64,
}

impl Families {
    /// Remember entry `node`, described by `meta`, as a member of family
    /// `clone_id` on its device; `charged` says whether the walk charged it.
    pub fn push(&mut self, meta: &RawMeta, clone_id: u64, node: NodeId, charged: bool) {
        self.members.push(Member {
            dev: meta.dev,
            clone_id,
            node,
            ino: meta.ino,
            size: meta.size,
            alloc: meta.alloc,
            charged,
        });
    }

    /// Take the members one directory collected, numbered by their index in
    /// its block, now that the block starts at node `start`.
    pub fn append_block(&mut self, block: Families, start: NodeId) {
        self.members
            .extend(block.members.into_iter().map(|m| Member {
                node: start + m.node,
                ..m
            }));
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// Charge every family with more than one member to its shallowest one,
    /// and nothing to the others.
    ///
    /// `step` is called once per member, and returning `false` from it stops
    /// the pass — that is how a cancelled scan gets out (invariant 5) and how
    /// the pass moves a counter (invariant 8).
    pub fn settle(
        mut self,
        builder: &mut TreeBuilder,
        mut step: impl FnMut() -> bool,
    ) -> Option<Moved> {
        self.members.sort_unstable_by_key(|m| (m.dev, m.clone_id));
        // Ranked only when some family has two members: a scan with no
        // clones of each other inside it pays for the sort above and no more.
        let mut moves: Vec<(Member, Member)> = Vec::new();
        {
            let mut order = None;
            for family in self
                .members
                .chunk_by(|a, b| (a.dev, a.clone_id) == (b.dev, b.clone_id))
            {
                for _ in family {
                    if !step() {
                        return None;
                    }
                }
                if family.len() < 2 {
                    continue;
                }
                let order = order.get_or_insert_with(|| builder.path_order());
                let Some(winner) = family
                    .iter()
                    .min_by(|a, b| order.cmp((a.node, a.ino), (b.node, b.ino)))
                else {
                    continue;
                };
                // Exactly one member was charged, unless the walk lost one to
                // a bug; then nobody is uncharged and the winner simply is.
                let holder = family.iter().find(|m| m.charged).copied();
                if holder.is_some_and(|h| h.node == winner.node) {
                    continue;
                }
                let holder = holder.unwrap_or(Member {
                    size: 0,
                    alloc: 0,
                    ..*winner
                });
                moves.push((holder, *winner));
            }
        }

        let mut moved = Moved::default();
        for (from, to) in moves {
            if from.node != to.node {
                set(builder, from.node, 0, 0);
            }
            set(builder, to.node, to.size, to.alloc);
            moved.shared_bytes += from.alloc as i64 - to.alloc as i64;
        }
        Some(moved)
    }
}

/// Give a file's node its own size and blocks, before aggregation.
fn set(builder: &mut TreeBuilder, node: NodeId, size: u64, alloc: u64) {
    let n = &mut builder.nodes[node as usize];
    n.size = size;
    n.own_size = size;
    n.alloc = alloc;
    n.own_alloc = alloc;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::EntryKind;
    use crate::tree::NewNode;

    /// A file of `size` bytes holding `alloc`, on device `dev`.
    fn file(dev: u64, ino: u64, size: u64, alloc: u64) -> RawMeta {
        RawMeta {
            kind: EntryKind::File,
            size,
            alloc,
            mtime: 0,
            nlink: 1,
            ino,
            dev,
        }
    }

    fn node(name: &str, kind: EntryKind) -> NewNode<'_> {
        NewNode {
            name,
            kind,
            size: 0,
            alloc: 0,
            mtime: 0,
            nlink: 1,
        }
    }

    /// `aa/deep/copy`, `zz/copy` and `.snap/copy`, with the top-level
    /// directories pushed in `order` — the way a different thread count fills
    /// the arena in a different order.
    fn cloned_tree(order: &[&str]) -> (TreeBuilder, Vec<(String, NodeId)>) {
        let mut b = TreeBuilder::default();
        let root = b.push_root(node("root", EntryKind::Dir));
        let start = b.push_block(
            root,
            order
                .iter()
                .map(|n| node(n, EntryKind::Dir))
                .collect::<Vec<_>>()
                .into_iter(),
        );
        let mut files = Vec::new();
        for (i, name) in order.iter().enumerate() {
            let dir = start + i as NodeId;
            if *name == "aa" {
                let deep = b.push_block(dir, [node("deep", EntryKind::Dir)].into_iter());
                let f = b.push_block(deep, [node("copy", EntryKind::File)].into_iter());
                files.push(("aa/deep/copy".to_string(), f));
            } else {
                let f = b.push_block(dir, [node("copy", EntryKind::File)].into_iter());
                files.push((format!("{name}/copy"), f));
            }
        }
        (b, files)
    }

    /// Settle one family whose members were met in arena order, the first
    /// one charged, as the walk does; report `(path, size, alloc)`.
    fn settle_and_report(order: &[&str]) -> (Vec<(String, u64, u64)>, Moved) {
        let (mut b, files) = cloned_tree(order);
        let mut families = Families::default();
        for (i, (_, id)) in files.iter().enumerate() {
            let charged = i == 0;
            if charged {
                set(&mut b, *id, 1000, 4096);
            }
            families.push(&file(1, 10 + i as u64, 1000, 4096), 7, *id, charged);
        }
        let moved = families.settle(&mut b, || true).unwrap();
        let mut out: Vec<(String, u64, u64)> = files
            .into_iter()
            .map(|(path, id)| {
                let n = &b.nodes[id as usize];
                (path, n.size, n.alloc)
            })
            .collect();
        out.sort();
        (out, moved)
    }

    /// The arena order changes with the thread count; who carries the family
    /// must not. Depth decides first: `aa/deep/copy` is one level deeper than
    /// the other two and loses to both, although `aa` sorts before `zz`.
    /// Between equal depths path order decides, so `.snap/copy` beats
    /// `zz/copy`.
    #[test]
    fn the_same_name_carries_a_clone_family_whatever_order_the_arena_was_filled_in() {
        let (one, _) = settle_and_report(&["zz", "aa", ".snap"]);
        let (two, _) = settle_and_report(&[".snap", "zz", "aa"]);
        let (three, _) = settle_and_report(&["aa", ".snap", "zz"]);
        assert_eq!(one, two);
        assert_eq!(one, three);
        assert_eq!(
            one,
            vec![
                (".snap/copy".to_string(), 1000, 4096),
                ("aa/deep/copy".to_string(), 0, 0),
                ("zz/copy".to_string(), 0, 0),
            ]
        );
    }

    /// Two members holding different amounts — one was written to after it
    /// was cloned — move the shared total by the difference, either way.
    #[test]
    fn moving_the_charge_between_unequal_members_moves_the_shared_total_by_the_difference() {
        let (mut b, files) = cloned_tree(&["zz", ".snap"]);
        let (zz, snap) = (files[0].1, files[1].1);
        set(&mut b, zz, 1000, 8192);
        let mut families = Families::default();
        families.push(&file(1, 1, 1000, 8192), 7, zz, true);
        families.push(&file(1, 2, 1000, 4096), 7, snap, false);
        let moved = families.settle(&mut b, || true).unwrap();
        // Before: 4096 of `.snap` were the repeat. After: 8192 of `zz` are.
        assert_eq!(moved.shared_bytes, 8192 - 4096);
        assert_eq!(b.nodes[snap as usize].alloc, 4096);
        assert_eq!(b.nodes[zz as usize].alloc, 0);
    }

    /// Families are told apart by device as well as by id: the same clone id
    /// on two volumes is two families, each charged once.
    #[test]
    fn the_same_clone_id_on_two_devices_is_two_families() {
        let (mut b, files) = cloned_tree(&["zz", ".snap"]);
        let mut families = Families::default();
        for (i, (_, id)) in files.iter().enumerate() {
            set(&mut b, *id, 1000, 4096);
            families.push(&file(i as u64, 1, 1000, 4096), 7, *id, true);
        }
        let moved = families.settle(&mut b, || true).unwrap();
        assert_eq!(moved, Moved::default());
        for (_, id) in files {
            assert_eq!(b.nodes[id as usize].alloc, 4096);
        }
    }

    /// The pass stops when told to, which is how a cancelled scan leaves it.
    #[test]
    fn settling_clones_stops_when_the_step_says_so() {
        let (mut b, files) = cloned_tree(&["zz", "aa", ".snap"]);
        let mut families = Families::default();
        for (i, (_, id)) in files.iter().enumerate() {
            families.push(&file(1, 1, 1000, 4096), 7, *id, i == 0);
        }
        let mut calls = 0;
        let settled = families.settle(&mut b, || {
            calls += 1;
            false
        });
        assert_eq!(settled, None);
        assert_eq!(calls, 1);
    }
}
