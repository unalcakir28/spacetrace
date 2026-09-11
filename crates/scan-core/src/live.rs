//! Watching a scan fill in, instead of watching a number go up.
//!
//! A progress bar says a scan is working. It does not say what it is finding,
//! and "what is it finding" is the only reason anyone started it. This is the
//! smallest structure that answers that *while the walk is still running*: the
//! root's own children, each with a total that climbs as its subtree is read.
//!
//! **One level, not the whole tree, and that is a decision rather than a first
//! draft.** A treemap draws nothing below a minimum area, so at the moment a
//! scan is in flight the top level is the only part anyone can read — and the
//! question being asked of a running scan is always "which of these is the big
//! one". Carrying every level would mean a structure that grows with the disk,
//! locked for writing from every worker, to render tiles too small to see.
//!
//! **Totals are partial and say so.** A branch's figure is what has been read
//! so far, never what is there; it only equals the truth when the walk ends.
//! That is why the finished scan replaces this view rather than merging into
//! it — a number that stops climbing looks identical to a number that is
//! complete, and only one of them can be quoted.
//!
//! **Bytes are added once per directory, not once per file.** The walk's
//! hottest loop is per entry and the project has already paid to keep shared
//! writes out of it (see the note on `ScanProgress::cancel`). A directory sums
//! its own entries on the thread that listed them and publishes once — roughly
//! a thirteenth of the writes, for a figure that moves thousands of times a
//! second either way.
//!
//! **What it costs, measured.** 4 ns per publish, so 0.13 ms across the 34,000
//! directories of `/Applications` — against a scan of 770 ms. An interleaved
//! whole-scan A/B could not resolve it and was not asked to: it came back at
//! -3.45% and -0.45% on two corpora, which is the noise floor and not a
//! speed-up. The honest form of that measurement is the per-operation one
//! above multiplied by the operation count, the same way the mount guard's
//! cost was settled.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

/// One of the root's children, as the scan currently understands it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LiveEntry {
    pub name: String,
    pub is_dir: bool,
    /// Logical bytes found so far.
    pub size: u64,
    /// On-disk bytes found so far.
    pub alloc: u64,
    /// Files found so far, the entry itself included when it is one.
    pub files: u64,
}

/// A branch's running totals.
///
/// Three atomics rather than a locked struct: every worker under a branch
/// writes to it, and the three are read together only by a viewer who is
/// already looking at a number that is changing. A torn read here shows a size
/// from one instant beside a count from the next, which is what any live view
/// of a moving thing shows anyway.
#[derive(Debug, Default)]
struct Branch {
    name: String,
    is_dir: bool,
    size: AtomicU64,
    alloc: AtomicU64,
    files: AtomicU64,
}

/// The root's children and what has been found under each.
///
/// Created empty and filled in once, when the root directory itself has been
/// listed. Before that there is nothing to show and every add is a no-op,
/// which is the honest state: a scan that has not finished reading its own
/// root has found nothing to attribute.
/// A `OnceLock` and not a lock, because "written once, then only read" is
/// exactly what it means — and the difference is measured, not assumed. The
/// first version used an `RwLock` and cost **203 ns** per publish under eight
/// contending threads; this one costs **4 ns**, and unlike the lock it does
/// not get worse with more threads (1 and 8 both measure 4 ns). It also
/// removes the second piece of state the lock needed: refusing a second
/// install is what `set` already does.
#[derive(Debug, Default)]
pub struct LiveTree {
    branches: OnceLock<Vec<Branch>>,
}

impl LiveTree {
    /// Register the root's children. Ignored if it has already been done.
    ///
    /// Returns the index each caller should hand down to the child at the same
    /// position, or an empty list when the branches were already installed —
    /// which can only happen if a scan is restarted on the same progress
    /// object, and in that case the second set would be describing a different
    /// walk.
    pub fn install(&self, children: impl IntoIterator<Item = (String, bool)>) -> Vec<u32> {
        let branches: Vec<Branch> = children
            .into_iter()
            .map(|(name, is_dir)| Branch {
                name,
                is_dir,
                ..Branch::default()
            })
            .collect();
        let count = branches.len() as u32;
        match self.branches.set(branches) {
            Ok(()) => (0..count).collect(),
            // Already installed. The second caller is describing a different
            // walk, so it gets no ids and attributes nothing.
            Err(_) => Vec::new(),
        }
    }

    /// Add what one directory holds to the branch it belongs to.
    pub fn add(&self, branch: u32, size: u64, alloc: u64, files: u64) {
        let Some(found) = self
            .branches
            .get()
            .and_then(|branches| branches.get(branch as usize))
        else {
            return;
        };
        found.size.fetch_add(size, Ordering::Relaxed);
        found.alloc.fetch_add(alloc, Ordering::Relaxed);
        found.files.fetch_add(files, Ordering::Relaxed);
    }

    /// What has been found so far, largest on-disk first.
    ///
    /// Sorted here rather than by the caller so that two readers of the same
    /// scan see the same order, and ordered by `alloc` because that is the
    /// measure the desktop shows by default (invariant #6: the order and the
    /// number beside it have to come from the same place).
    pub fn snapshot(&self) -> Vec<LiveEntry> {
        let Some(branches) = self.branches.get() else {
            return Vec::new();
        };
        let mut out: Vec<LiveEntry> = branches
            .iter()
            .map(|b| LiveEntry {
                name: b.name.clone(),
                is_dir: b.is_dir,
                size: b.size.load(Ordering::Relaxed),
                alloc: b.alloc.load(Ordering::Relaxed),
                files: b.files.load(Ordering::Relaxed),
            })
            .collect();
        // Ties broken by name so the order is stable while nothing has been
        // found yet — otherwise the first few frames of every scan reshuffle a
        // list of zeroes.
        out.sort_by(|a, b| b.alloc.cmp(&a.alloc).then_with(|| a.name.cmp(&b.name)));
        out
    }

    /// Whether the root's children are known yet.
    pub fn is_ready(&self) -> bool {
        self.branches.get().is_some_and(|b| !b.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree_with(names: &[(&str, bool)]) -> (LiveTree, Vec<u32>) {
        let live = LiveTree::default();
        let ids = live.install(names.iter().map(|(n, d)| (n.to_string(), *d)));
        (live, ids)
    }

    #[test]
    fn nothing_is_attributed_before_the_root_is_listed() {
        let live = LiveTree::default();
        assert!(!live.is_ready());
        // An add against a branch that does not exist must be ignored rather
        // than panic: the walk calls this from every worker and a scan must
        // not die for the sake of a preview.
        live.add(0, 100, 100, 1);
        assert!(live.snapshot().is_empty());
    }

    #[test]
    fn a_branch_grows_as_its_subtree_is_read() {
        let (live, ids) = tree_with(&[("Users", true), ("Applications", true)]);
        assert_eq!(ids, vec![0, 1]);

        live.add(ids[0], 100, 128, 2);
        live.add(ids[0], 50, 64, 1);
        live.add(ids[1], 10, 16, 1);

        let seen = live.snapshot();
        assert_eq!(seen[0].name, "Users");
        assert_eq!(seen[0].size, 150);
        assert_eq!(seen[0].alloc, 192);
        assert_eq!(seen[0].files, 3);
        assert_eq!(seen[1].name, "Applications");
    }

    /// The list is what a person watches reorder, so the order has to be the
    /// one the numbers justify — and the same for two readers of one scan.
    #[test]
    fn the_largest_is_first_and_ties_do_not_shuffle() {
        let (live, ids) = tree_with(&[("b", true), ("a", true), ("c", true)]);
        assert_eq!(
            live.snapshot()
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b", "c"],
            "all zero: by name, so the first frames do not reshuffle"
        );

        live.add(ids[2], 1, 900, 1);
        live.add(ids[0], 1, 100, 1);
        assert_eq!(
            live.snapshot()
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            vec!["c", "b", "a"],
            "largest first, and the untouched one falls to the end"
        );
    }

    /// A second install would describe a different walk, and quietly replacing
    /// the branches mid-scan would leave figures from the first attributed to
    /// names from the second.
    #[test]
    fn the_branches_are_installed_only_once() {
        let (live, ids) = tree_with(&[("first", true)]);
        live.add(ids[0], 5, 5, 1);

        let second = live.install([("second".to_string(), true)]);
        assert!(second.is_empty(), "the second install must be refused");

        let seen = live.snapshot();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].name, "first");
        assert_eq!(seen[0].size, 5, "and the first one's figures survive");
    }

    /// Every worker under a branch writes to it. The totals have to be the sum
    /// of what was added, not of what happened to land without a race.
    #[test]
    fn concurrent_adds_are_all_counted() {
        use std::sync::Arc;

        let live = Arc::new(LiveTree::default());
        let ids = live.install([("one".to_string(), true), ("two".to_string(), true)]);

        let handles: Vec<_> = (0..8)
            .map(|worker| {
                let live = Arc::clone(&live);
                let branch = ids[worker % 2];
                std::thread::spawn(move || {
                    for _ in 0..1_000 {
                        live.add(branch, 1, 2, 1);
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }

        let seen = live.snapshot();
        let total: u64 = seen.iter().map(|e| e.files).sum();
        assert_eq!(total, 8_000);
        assert_eq!(seen.iter().map(|e| e.alloc).sum::<u64>(), 16_000);
    }
}
