//! A tree built from the previous snapshot of the same root and what the
//! volume's change journal reported since — reading again only what changed.
//!
//! **Nothing is patched in place.** Arena ids are not stable between scans
//! (invariant 2), so the new tree is built the way every tree is: the walk
//! lists directories into a fresh `TreeBuilder`, and where it meets a
//! subdirectory it may skip, the previous snapshot's subtree is copied in
//! below it, block by block through `push_block`, after the walk. Both
//! arena properties hold by construction, exactly as for a full scan.
//!
//! **What gets read again.** Every directory on the path from the root to a
//! changed entry, so that every directory whose listing or own metadata may
//! differ is listed afresh (a directory's own size and mtime live in its
//! parent's listing). Below that, a subdirectory is copied only when the
//! journal named nothing under it *and* the previous snapshot's flags for it
//! are clear (`Node::flags`): nothing in it shares an identity with a name
//! outside it, it held no error, and no volume is mounted in it. Anything
//! else is listed, and its own subdirectories decided the same way.
//!
//! **Erring is one-sided.** Reading a directory that did not change costs a
//! listing; copying one that did is a wrong snapshot. Every rule below that
//! is not certain picks the first: a name the journal spelled differently
//! from the listing is matched case-insensitively, a change the journal
//! could not describe reads its whole subtree, and a changed name that
//! cannot be found in its directory's listing reads that directory's whole
//! subtree.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use crate::journal::{
    Change, ChangeKind, Cursor, Fallback, Incremental, Journal, NoAnswer, Rescan,
};
use crate::scan::{Phase, ScanProgress};
use crate::tree::{NewNode, Node, NodeId, Tree, TreeBuilder, ROOT};

/// The scan an incremental rescan may start from, as the code that stores
/// snapshots hands it over.
pub struct Base<'a> {
    /// The cursor stored with it — `ScanStats::journal` of the scan that
    /// wrote it — or why it cannot be a base whatever the journal says (an
    /// imported snapshot, one dated in the future, one with no cursor).
    pub journal: Result<String, Fallback>,
    /// Loads its tree, flags included, and checks it. Called once the stored
    /// cursor has passed every check and before the journal is asked, while
    /// the journal's barrier makes its way (see `Journal::barrier`): a scan
    /// whose replay then falls back has paid for reading a snapshot it does
    /// not use, and every incremental scan saves the barrier's wait.
    pub load: LoadBase<'a>,
}

/// How a [`Base`] reads its tree: once, reporting progress, and answering
/// with the reason when the tree cannot be used.
pub type LoadBase<'a> = Box<dyn FnOnce(&ScanProgress) -> Result<Tree, Fallback> + 'a>;

/// How a scan is going to be built, decided before the walk starts.
pub(crate) enum Plan {
    /// Read everything; the record says why.
    Full(Rescan),
    /// Read what changed and copy the rest.
    Incremental {
        splice: Box<Splice>,
        report: Incremental,
        /// The budget the next scan inherits (`Cursor::full_ms`).
        full_ms: u64,
    },
}

/// Why a scan that could have been incremental is not.
enum Refusal {
    Fallback(Fallback),
    Cancelled,
}

impl From<Fallback> for Refusal {
    fn from(reason: Fallback) -> Self {
        Refusal::Fallback(reason)
    }
}

/// Decide how to build the scan of `root`, whose journal and own cursor are
/// `now`.
///
/// `Err` only for a cancelled scan (invariant 5); every other way this can
/// go wrong is a full scan with the reason recorded.
pub(crate) fn plan(
    root: &Path,
    now: Option<(&dyn Journal, &Cursor)>,
    base: Base<'_>,
    progress: &ScanProgress,
    mounted: &crate::Mounts,
) -> Result<Plan, ()> {
    // No journal for this root now. If the previous scan had one, the root's
    // volume changed under it; if it did not either, nothing was ever
    // possible here and there is nothing to report.
    let Some((journal, now)) = now else {
        return Ok(Plan::Full(match base.journal {
            Ok(_) => Rescan::Fallback(Fallback::OtherVolume),
            Err(_) => Rescan::Full,
        }));
    };
    match try_plan(root, journal, now, base, progress, mounted) {
        Ok(plan) => Ok(plan),
        Err(Refusal::Fallback(reason)) => Ok(Plan::Full(Rescan::Fallback(reason))),
        Err(Refusal::Cancelled) => Err(()),
    }
}

fn try_plan(
    root: &Path,
    journal: &dyn Journal,
    now: &Cursor,
    base: Base<'_>,
    progress: &ScanProgress,
    mounted: &crate::Mounts,
) -> Result<Plan, Refusal> {
    let stored = base.journal?;
    let old = Cursor::decode(&stored).ok_or(Fallback::StaleCursor)?;
    if old.kind != now.kind {
        return Err(Fallback::StaleCursor.into());
    }
    if old.volume != now.volume {
        return Err(Fallback::OtherVolume.into());
    }
    if old.options != now.options {
        return Err(Fallback::OptionsChanged.into());
    }
    if old.root_ino != now.root_ino {
        return Err(Fallback::RootReplaced.into());
    }
    // Positions only grow, so one from ahead of the present came from
    // somewhere this journal has not been.
    if old.position > now.position {
        return Err(Fallback::StaleCursor.into());
    }
    // The clock moved backwards: how old the cursor is cannot be told.
    if old.taken_at > now.taken_at {
        return Err(Fallback::FutureBase.into());
    }
    if now.taken_at - old.taken_at > MAX_CURSOR_AGE.as_secs() {
        return Err(Fallback::TooOld.into());
    }
    if progress.is_cancelled() {
        return Err(Refusal::Cancelled);
    }

    // Marked before the base is loaded: the mark takes fseventsd about
    // 300 ms to pass on live, and the load is time the rescan spends anyway
    // (see `fsevents::Marker`). The price is a load paid for by a scan whose
    // replay then falls back.
    let barrier = journal.barrier(root).map_err(refusal)?;

    let loading = std::time::Instant::now();
    let mut tree = (base.load)(progress)?;
    if progress.is_cancelled() {
        return Err(Refusal::Cancelled);
    }
    // The cursor said the root was a directory; a base whose root is not
    // one is not the scan the cursor belongs to.
    if !tree.node(ROOT).is_dir() {
        return Err(Fallback::BaseDamaged.into());
    }
    tree.prepare_as_base();
    let load_ms = loading.elapsed().as_millis() as u64;

    let asked = std::time::Instant::now();
    let budget = budget_for(&old);
    let changes = journal
        .replay(root, old.position, budget, barrier, progress)
        .map_err(refusal)?;
    let replay_ms = asked.elapsed().as_millis() as u64;
    let mut dirty = Dirty::from_changes(root, &changes)?;
    dirty.mark_mounted(root, mounted);

    Ok(Plan::Incremental {
        report: Incremental {
            events: changes.len() as u64,
            distance: now.position - old.position,
            replay_ms,
            load_ms,
            dirs_listed: 0,
            entries_reused: 0,
        },
        full_ms: old.full_ms,
        splice: Box::new(Splice {
            base: tree,
            dirty,
            empty: Dirty::default(),
            pending: Mutex::new(Vec::new()),
            dirs_listed: AtomicU64::new(0),
        }),
    })
}

/// Why the journal gave no answer, as the rescan acts on it.
fn refusal(why: NoAnswer) -> Refusal {
    match why {
        NoAnswer::Deadline => Refusal::Fallback(Fallback::Deadline),
        NoAnswer::Failed => Refusal::Fallback(Fallback::ReplayFailed),
        NoAnswer::TooMany => Refusal::Fallback(Fallback::TooManyChanges),
        NoAnswer::Lost => Refusal::Fallback(Fallback::EventsLost),
        NoAnswer::NoBarrier => Refusal::Fallback(Fallback::NoBarrier),
        NoAnswer::MarkerVolume => Refusal::Fallback(Fallback::MarkerVolume),
        NoAnswer::Cancelled => Refusal::Cancelled,
    }
}

/// The least a replay may spend, however quick the last full scan was.
///
/// The budget is otherwise the last full scan's duration: past it, walking
/// is the cheaper way to find out what changed. But asking has a fixed cost
/// of its own — 7 to 162 ms measured for a cursor seconds old, on a machine
/// under a load average of 10 — and a root that a full scan reads in a few
/// milliseconds would then always fall back before the journal could answer.
/// Half a second clears that cost three times over, and is the most a
/// fallback can add to a root that small: a price nobody notices, for a
/// budget that stays the full scan's own on any root where it matters.
const BUDGET_FLOOR: Duration = Duration::from_millis(500);

/// The oldest cursor a replay is trusted from.
///
/// fseventsd discards old history when its volume needs the space, and
/// nothing in the API reports that a replay began before what is left:
/// `FSEventsGetLastEventIdForDeviceBeforeTime`, the one call that could
/// date the history, answers 0 for every time on this Mac and on a fresh
/// APFS image (measured), and the logs in `/.fseventsd` are readable only by
/// root. A replay across a discarded stretch would end with `HistoryDone`
/// and no flag, having missed it. So age is the bound. Two days keeps a
/// daily schedule incremental with one missed run to spare; past it the
/// replay is already long on any busy volume — event ids arrive at 137 to
/// 1,412 a second here, so two days is 24 to 240 million of them, where 100
/// million took 57 s — and walking is the better answer anyway.
pub(crate) const MAX_CURSOR_AGE: Duration = Duration::from_secs(2 * 24 * 60 * 60);

/// How long the replay from `old` may take.
fn budget_for(old: &Cursor) -> Duration {
    #[cfg(test)]
    if let Some(budget) = BUDGET_OVERRIDE.get() {
        return budget;
    }
    Duration::from_millis(old.full_ms).max(BUDGET_FLOOR)
}

#[cfg(test)]
thread_local! {
    /// A budget for the replays this thread makes, in place of
    /// [`budget_for`]'s: the test hook that lets a test see a deadline pass
    /// without editing a stored cursor.
    static BUDGET_OVERRIDE: std::cell::Cell<Option<Duration>> =
        const { std::cell::Cell::new(None) };
}

/// The changed paths under the root, as a tree of folded names.
#[derive(Debug, Default)]
pub(crate) struct Dirty {
    /// Keyed by [`fold`]ed name, so a name the journal spelled in another
    /// case than the directory listing still finds its entry. On a
    /// case-sensitive volume two names folding alike both match, which reads
    /// one directory too many and copies nothing wrong.
    children: HashMap<String, Dirty>,
    /// Read everything below this path again; copy nothing under it.
    recursive: bool,
    /// The journal said this path was removed or renamed away, so finding
    /// nothing by its name in a fresh listing is explained.
    gone: bool,
}

impl Dirty {
    /// Turn the journal's changes into the set of paths to read again.
    ///
    /// The rules, each the cautious reading of what a change can mean:
    ///
    /// * **Every changed path makes its parent be read again**, and with it
    ///   every directory above — a new, removed or renamed entry changes its
    ///   parent's listing, and a changed entry changes the metadata its
    ///   parent's listing reports for it.
    /// * **A directory created, removed or renamed is read in full.** A
    ///   directory renamed in from outside the root arrives with its whole
    ///   contents and no record for any of them (measured with FSEvents: one
    ///   `Renamed` event, nothing below it). Coalescing can also report a
    ///   directory created that merely changed; reading it in full is then a
    ///   cost, not an error.
    /// * **A change the journal kept no detail of** reads that path in full.
    /// * **A journal that lost track** is a full scan: nothing it says for
    ///   that stretch can be trusted.
    /// * **A change the root itself is the subject of** — the root moved, or
    ///   its whole contents unknown — is a full scan.
    /// * **A path that is not under the root** cannot be placed, and placing
    ///   it wrong would leave a real change unread: a full scan.
    pub(crate) fn from_changes(root: &Path, changes: &[Change]) -> Result<Dirty, Fallback> {
        let root_parts = folded_components(root);
        let mut top = Dirty::default();
        for change in changes {
            if change.kind == ChangeKind::Lost {
                return Err(Fallback::EventsLost);
            }
            let rel = below(&root_parts, &change.path).ok_or(Fallback::EventOutsideRoot)?;
            let moved = matches!(
                change.kind,
                ChangeKind::Created | ChangeKind::Removed | ChangeKind::Renamed
            );
            let dir_moved = change.is_dir && moved;
            let unknown = change.kind == ChangeKind::SubtreeUnknown;
            if rel.is_empty() {
                if dir_moved {
                    return Err(Fallback::RootReplaced);
                }
                if unknown {
                    return Err(Fallback::EventsLost);
                }
                // The root's own metadata changed; it is read every time.
                continue;
            }
            let node = top.descend(&rel);
            if matches!(change.kind, ChangeKind::Removed | ChangeKind::Renamed) {
                node.gone = true;
            }
            if dir_moved || unknown {
                node.recursive = true;
            }
        }
        Ok(top)
    }

    /// Read every volume mounted under `root` now in full, and the
    /// directories above it again.
    ///
    /// The journal cannot report these: mounting a volume over a directory
    /// changes nothing on the root's volume, and what is written on the
    /// mounted one is in that volume's journal or in none. Without this a
    /// volume mounted inside a subtree that nothing else touched since the
    /// base was taken over unread — the base's view of the directory it now
    /// covers (caught by `a_volume_mounted_and_unmounted_under_the_root`).
    /// A volume unmounted since needs nothing here: the base flags its old
    /// mount point (`Node::MOUNT_POINT`), and that is read again.
    pub(crate) fn mark_mounted(&mut self, root: &Path, mounted: &crate::Mounts) {
        let root_parts = folded_components(root);
        for point in mounted.points() {
            // Lossy, as the root's components are.
            let text = point.to_string_lossy();
            let Some(rel) = below(&root_parts, text.as_bytes()) else {
                continue;
            };
            if rel.is_empty() {
                continue;
            }
            self.descend(&rel).recursive = true;
        }
    }

    /// The node for `rel` below this one, made where missing. Looked up
    /// before inserting, so a path already present allocates no key.
    fn descend(&mut self, rel: &[&[u8]]) -> &mut Dirty {
        let mut node = self;
        for part in rel {
            let key = fold_bytes(part);
            if !node.children.contains_key(key.as_ref()) {
                node.children
                    .insert(key.clone().into_owned(), Dirty::default());
            }
            node = node
                .children
                .get_mut(key.as_ref())
                .expect("inserted just above");
        }
        node
    }
}

/// `root`'s components, folded, for [`below`].
fn folded_components(root: &Path) -> Vec<String> {
    // Lossy, like every name compared here: the folding is lossy anyway.
    let text = root.to_string_lossy();
    components(text.as_bytes())
        .map(|part| fold_bytes(part).into_owned())
        .collect()
}

/// `path`'s components below the root whose folded components are
/// `root_parts` — empty for the root itself — or `None` for a path not
/// under it.
fn below<'p>(root_parts: &[String], path: &'p [u8]) -> Option<Vec<&'p [u8]>> {
    let parts: Vec<&[u8]> = components(path).collect();
    let under = parts.len() >= root_parts.len()
        && parts
            .iter()
            .zip(root_parts)
            .all(|(part, root)| fold_bytes(part) == root.as_str());
    under.then(|| parts[root_parts.len()..].to_vec())
}

/// A path's components, without the empty ones a leading, doubled or
/// trailing `/` produces.
fn components(path: &[u8]) -> impl Iterator<Item = &[u8]> {
    path.split(|&b| b == b'/').filter(|part| !part.is_empty())
}

/// The key two spellings of one name share on a case-insensitive volume.
///
/// Lossy first, as the tree stores names, then lowercased — borrowed when
/// that changes nothing, which for the names on a real disk is nearly every
/// one. Unicode normalisation is not folded — there is no table for it here
/// — so a name the journal spells in another normal form than the listing
/// does fails to match, which [`DirPlan`] treats as "read this whole
/// directory".
fn fold(name: &str) -> Cow<'_, str> {
    if name.chars().any(char::is_uppercase) {
        Cow::Owned(name.to_lowercase())
    } else {
        Cow::Borrowed(name)
    }
}

fn fold_bytes(name: &[u8]) -> Cow<'_, str> {
    match String::from_utf8_lossy(name) {
        Cow::Borrowed(text) => fold(text),
        Cow::Owned(text) => Cow::Owned(fold(&text).into_owned()),
    }
}

/// What the walk carries while it rebuilds from a base.
pub(crate) struct Splice {
    base: Tree,
    dirty: Dirty,
    /// "Nothing changed below here", for a directory the base cannot be
    /// copied from but whose subdirectories still may be.
    empty: Dirty,
    /// Subdirectories to fill from the base after the walk: their node in
    /// the new arena, and the base node whose children they take.
    pending: Mutex<Vec<(NodeId, NodeId)>>,
    /// Directories listed from the disk in this scan.
    pub(crate) dirs_listed: AtomicU64,
}

/// Where in the base and in the changed paths a directory being read sits.
#[derive(Clone, Copy)]
pub(crate) struct At<'a> {
    /// The base's node for this directory; `None` for one new since.
    base: Option<NodeId>,
    dirty: &'a Dirty,
}

/// What to do with one subdirectory of a directory read afresh.
pub(crate) enum Next<'a> {
    /// List it. With an `At`, decide again for each of its subdirectories;
    /// without, read everything below it and copy nothing.
    Visit(Option<At<'a>>),
    /// Take its contents over from this base node, unread.
    Copy(NodeId),
}

/// The decisions for one directory's subdirectories, made once its listing
/// is in hand.
pub(crate) struct DirPlan<'a> {
    splice: &'a Splice,
    at: At<'a>,
    /// The base's children by name. `None` where two share a name after
    /// lossy decoding: which one a fresh entry is cannot be told, so neither
    /// is copied.
    base_children: HashMap<&'a str, Option<NodeId>>,
    /// The journal named something in this directory that the listing does
    /// not hold and that it did not say was removed: a spelling this cannot
    /// match, so nothing below this directory may be copied.
    lost_track: bool,
}

impl Splice {
    /// How many entries the base holds: the arena's size hint.
    pub(crate) fn base_len(&self) -> usize {
        self.base.len()
    }

    pub(crate) fn root(&self) -> At<'_> {
        At {
            base: Some(ROOT),
            dirty: &self.dirty,
        }
    }

    /// Plan a directory whose entries, by name, are `names`.
    pub(crate) fn plan<'a, 'n>(
        &'a self,
        at: At<'a>,
        names: impl Iterator<Item = &'n str>,
    ) -> DirPlan<'a> {
        let mut base_children = HashMap::new();
        if let Some(base) = at.base.filter(|&b| self.base.node(b).is_dir()) {
            // Subdirectories only: they are all `next` is asked about, and a
            // directory of files is the common case.
            for child in self.base.children(base) {
                if !self.base.node(child).is_dir() {
                    continue;
                }
                base_children
                    .entry(self.base.name(child))
                    .and_modify(|seen: &mut Option<NodeId>| *seen = None)
                    .or_insert(Some(child));
            }
        }
        let lost_track = !at.dirty.children.is_empty() && {
            let mut found: HashSet<&str> = HashSet::new();
            for name in names {
                if let Some((key, _)) = at.dirty.children.get_key_value(fold(name).as_ref()) {
                    found.insert(key);
                }
            }
            at.dirty
                .children
                .iter()
                .any(|(key, below)| !below.gone && !found.contains(key.as_str()))
        };
        DirPlan {
            splice: self,
            at,
            base_children,
            lost_track,
        }
    }

    /// Queue base node `from`'s contents to be copied below new node `into`
    /// once the walk is over, and count them now so the live counters and
    /// the scan's totals include them.
    pub(crate) fn defer_copy(&self, into: NodeId, from: NodeId, progress: &ScanProgress) {
        let node = self.base.node(from);
        progress
            .files
            .fetch_add(u64::from(node.files), Ordering::Relaxed);
        progress
            .dirs
            .fetch_add(u64::from(node.dirs), Ordering::Relaxed);
        progress
            .bytes
            .fetch_add(node.alloc.saturating_sub(node.own_alloc), Ordering::Relaxed);
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((into, from));
    }

    /// Copy everything queued by [`Splice::defer_copy`] into `builder`.
    /// Returns how many entries were copied, or `None` for a cancelled scan.
    ///
    /// Counted in `rows_done` (invariant 8) and cancellable (invariant 5):
    /// on a large unchanged tree it is most of the work an incremental scan
    /// does. Called while the scan is still in `Walking`, because this builds
    /// what the walk would have, and a row count under `Finishing` is what a
    /// watcher shows as charging shared blocks; `Finishing` comes after it,
    /// so the phase only runs forwards. The count is cleared afterwards, so
    /// a settlement with nothing to do does not leave it standing.
    pub(crate) fn copy_into(
        &self,
        builder: &mut TreeBuilder,
        progress: &ScanProgress,
    ) -> Option<u64> {
        let mut stack = std::mem::take(
            &mut *self
                .pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        let total: u64 = stack
            .iter()
            .map(|&(_, from)| {
                let node = self.base.node(from);
                u64::from(node.files) + u64::from(node.dirs)
            })
            .sum();
        progress.begin_rows(Phase::Walking, total);
        let base = &self.base;
        let mut copied = 0u64;
        while let Some((into, from)) = stack.pop() {
            if progress.is_cancelled() {
                return None;
            }
            // A range rather than `Tree::children`, because `push_block`
            // wants to know the length before it writes.
            let block = base.node(from);
            let start = builder.push_block(
                into,
                (block.children_start..block.children_start + block.children_len).map(|child| {
                    let node = base.node(child);
                    NewNode {
                        name: base.name(child),
                        kind: node.kind,
                        size: node.own_size,
                        alloc: node.own_alloc,
                        mtime: node.mtime,
                        nlink: node.nlink,
                        flags: node.flags(),
                    }
                }),
            );
            for (index, child) in base.children(from).enumerate() {
                if base.node(child).children_len > 0 {
                    stack.push((start + index as NodeId, child));
                }
            }
            let added = u64::from(block.children_len);
            copied += added;
            progress.rows_done.fetch_add(added, Ordering::Relaxed);
        }
        progress.begin_rows(Phase::Walking, 0);
        Some(copied)
    }
}

impl<'a> DirPlan<'a> {
    /// Decide for the subdirectory `name`, on device `dev`, of a directory on
    /// device `dir_dev`.
    pub(crate) fn next(&self, name: &str, dev: u64, dir_dev: u64) -> Next<'a> {
        if self.lost_track {
            return Next::Visit(None);
        }
        // A mount point now: its volume's changes are not in this journal,
        // and a network volume's are in none.
        if dev != dir_dev {
            return Next::Visit(None);
        }
        let base = self.base_children.get(name).copied().flatten();
        let base_node = base.map(|b| self.splice.base.node(b));
        // A mount point then, and not now: what the base holds below it is
        // the volume that used to cover this directory.
        if base_node.is_some_and(|n| n.flags() & Node::MOUNT_POINT != 0) {
            return Next::Visit(None);
        }
        let changed = if self.at.dirty.children.is_empty() {
            None
        } else {
            self.at.dirty.children.get(fold(name).as_ref())
        };
        if let Some(changed) = changed {
            if changed.recursive {
                return Next::Visit(None);
            }
            return Next::Visit(Some(At {
                base,
                dirty: changed,
            }));
        }
        let (Some(base), Some(node)) = (base, base_node) else {
            // New since the base, and the journal did not say so — which it
            // would have, as a change in this directory. Read it all.
            return Next::Visit(None);
        };
        if !node.is_dir() {
            return Next::Visit(None);
        }
        // An empty directory is listed rather than copied: one listing, and
        // it is what notices a volume mounted on it since.
        if node.flags() == 0 && node.children_len > 0 {
            return Next::Copy(base);
        }
        Next::Visit(Some(At {
            base: Some(base),
            dirty: &self.splice.empty,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(path: &str, kind: ChangeKind, is_dir: bool) -> Change {
        Change {
            path: path.as_bytes().to_vec(),
            kind,
            is_dir,
        }
    }

    fn file(path: &str, kind: ChangeKind) -> Change {
        change(path, kind, false)
    }

    fn dir(path: &str, kind: ChangeKind) -> Change {
        change(path, kind, true)
    }

    use ChangeKind::{Created, Lost, Modified, Removed, Renamed, SubtreeUnknown};

    fn dirty(changes: &[Change]) -> Result<Dirty, Fallback> {
        Dirty::from_changes(Path::new("/r/oot"), changes)
    }

    fn at<'a>(d: &'a Dirty, path: &str) -> Option<&'a Dirty> {
        let mut node = d;
        for part in path.split('/') {
            node = node.children.get(part)?;
        }
        Some(node)
    }

    #[test]
    fn a_changed_file_marks_its_directories_and_nothing_recursive() {
        let d = dirty(&[file("/r/oot/a/b/f", Created)]).unwrap();
        assert!(!at(&d, "a").unwrap().recursive);
        assert!(!at(&d, "a/b").unwrap().recursive);
        assert!(at(&d, "a/b/f").is_some());
        assert!(at(&d, "c").is_none());
    }

    #[test]
    fn a_directory_that_came_or_went_is_read_in_full() {
        for how in [Created, Removed, Renamed] {
            let d = dirty(&[dir("/r/oot/a/moved", how)]).unwrap();
            assert!(at(&d, "a/moved").unwrap().recursive, "{how:?}");
            assert!(!at(&d, "a").unwrap().recursive, "{how:?}");
        }
        let d = dirty(&[dir("/r/oot/a/chmod", Modified)]).unwrap();
        assert!(!at(&d, "a/chmod").unwrap().recursive, "metadata only");
    }

    #[test]
    fn a_change_the_journal_could_not_describe_reads_its_whole_subtree() {
        let d = dirty(&[file("/r/oot/a", SubtreeUnknown)]).unwrap();
        assert!(at(&d, "a").unwrap().recursive);
    }

    #[test]
    fn a_removal_is_remembered_so_absence_is_explained() {
        let d = dirty(&[
            file("/r/oot/a/gone", Removed),
            file("/r/oot/a/moved", Renamed),
            file("/r/oot/a/here", Modified),
        ])
        .unwrap();
        assert!(at(&d, "a/gone").unwrap().gone);
        assert!(at(&d, "a/moved").unwrap().gone);
        assert!(!at(&d, "a/here").unwrap().gone);
    }

    #[test]
    fn the_journal_losing_track_is_a_full_scan() {
        assert_eq!(
            dirty(&[file("/r/oot/a", Lost)]).unwrap_err(),
            Fallback::EventsLost
        );
    }

    #[test]
    fn the_root_itself_moving_is_a_full_scan() {
        assert_eq!(
            dirty(&[dir("/r/oot", Renamed)]).unwrap_err(),
            Fallback::RootReplaced
        );
        assert_eq!(
            dirty(&[dir("/r/oot", SubtreeUnknown)]).unwrap_err(),
            Fallback::EventsLost
        );
        let d = dirty(&[dir("/r/oot", Modified)]).unwrap();
        assert!(d.children.is_empty(), "its own metadata is read anyway");
    }

    #[test]
    fn a_path_outside_the_root_is_a_full_scan() {
        for outside in ["/r", "/r/other/a", "/elsewhere"] {
            assert_eq!(
                dirty(&[file(outside, Modified)]).unwrap_err(),
                Fallback::EventOutsideRoot,
                "{outside}"
            );
        }
    }

    /// FSEvents spells a path the way the call that changed it did; on a
    /// case-insensitive volume that need not be the way the listing does.
    #[test]
    fn names_are_matched_whatever_their_case() {
        let d = dirty(&[file("/R/OOT/Some/File", Modified)]).unwrap();
        assert!(at(&d, "some/file").is_some());
    }

    #[test]
    fn a_volume_mounted_below_the_root_is_read_in_full() {
        let mut d = Dirty::default();
        d.mark_mounted(
            Path::new("/r/oot"),
            &crate::Mounts::from_paths(["/r/oot/a/mnt", "/elsewhere/mnt", "/r", "/r/oot"]),
        );
        assert!(at(&d, "a/mnt").unwrap().recursive);
        assert!(!at(&d, "a").unwrap().recursive);
        assert_eq!(d.children.len(), 1, "only what is below the root");
    }

    // ------------------------------------------------- per-directory plans

    fn new_node(name: &str, kind: crate::EntryKind, flags: u8) -> NewNode<'_> {
        NewNode {
            name,
            kind,
            size: 10,
            alloc: 4096,
            mtime: 0,
            nlink: 1,
            flags,
        }
    }

    /// A base root holding `clean/` (copyable), `shared/` (a hardlink in
    /// it), `empty/`, `was_mount/` (a mount point then), two directories
    /// whose names decode to the same text, and a file.
    fn splice_over(dirty: Dirty) -> Splice {
        use crate::EntryKind::{Dir, File};
        let mut b = TreeBuilder::default();
        let root = b.push_root(new_node("root", Dir, 0));
        let top = b.push_block(
            root,
            [
                new_node("clean", Dir, 0),
                new_node("shared", Dir, 0),
                new_node("empty", Dir, 0),
                new_node("was_mount", Dir, Node::MOUNT | Node::MOUNT_POINT),
                new_node("twin", Dir, 0),
                new_node("twin", Dir, 0),
                new_node("file", File, 0),
            ]
            .into_iter(),
        );
        b.push_block(top, [new_node("x", File, 0)].into_iter());
        b.push_block(top + 1, [new_node("h", File, Node::SHARED)].into_iter());
        b.push_block(top + 3, [new_node("under", File, 0)].into_iter());
        b.push_block(top + 4, [new_node("t", File, 0)].into_iter());
        b.push_block(top + 5, [new_node("u", File, 0)].into_iter());
        let mut base = b.finish(std::path::PathBuf::from("/r/oot"));
        base.prepare_as_base();
        Splice {
            base,
            dirty,
            empty: Dirty::default(),
            pending: Mutex::new(Vec::new()),
            dirs_listed: AtomicU64::new(0),
        }
    }

    const LISTING: [&str; 7] = [
        "clean",
        "shared",
        "empty",
        "was_mount",
        "twin",
        "file",
        "new",
    ];

    fn decide(splice: &Splice, name: &str, dev: u64) -> &'static str {
        let plan = splice.plan(splice.root(), LISTING.into_iter());
        match plan.next(name, dev, 1) {
            Next::Copy(_) => "copy",
            Next::Visit(Some(_)) => "relist",
            Next::Visit(None) => "walk",
        }
    }

    #[test]
    fn only_a_clean_unchanged_subtree_is_copied() {
        let splice = splice_over(Dirty::default());
        assert_eq!(decide(&splice, "clean", 1), "copy");
        assert_eq!(
            decide(&splice, "shared", 1),
            "relist",
            "flagged: read, not copied"
        );
        assert_eq!(
            decide(&splice, "empty", 1),
            "relist",
            "one listing is cheaper than a mistake"
        );
        assert_eq!(decide(&splice, "new", 1), "walk", "not in the base");
        assert_eq!(
            decide(&splice, "twin", 1),
            "walk",
            "two base entries, one name"
        );
        assert_eq!(decide(&splice, "file", 1), "walk", "a file in the base");
    }

    #[test]
    fn a_mount_point_now_or_then_is_walked() {
        let splice = splice_over(Dirty::default());
        assert_eq!(decide(&splice, "clean", 2), "walk", "on another device now");
        assert_eq!(
            decide(&splice, "was_mount", 1),
            "walk",
            "on another device then"
        );
    }

    #[test]
    fn a_changed_subdirectory_is_listed_or_walked_as_the_journal_says() {
        let d = dirty(&[
            file("/r/oot/clean/x", Modified),
            dir("/r/oot/shared", Renamed),
        ])
        .unwrap();
        let splice = splice_over(d);
        assert_eq!(decide(&splice, "clean", 1), "relist");
        assert_eq!(decide(&splice, "shared", 1), "walk");
    }

    /// The journal names something the listing does not show and did not say
    /// was removed — a spelling this cannot fold, say. The change is under a
    /// name it cannot place, so nothing here may be copied.
    #[test]
    fn a_change_under_a_name_the_listing_does_not_show_copies_nothing() {
        let lost = splice_over(dirty(&[file("/r/oot/cafe\u{301}/menu", Modified)]).unwrap());
        assert_eq!(decide(&lost, "clean", 1), "walk");

        let explained = splice_over(dirty(&[file("/r/oot/gone/menu", Removed)]).unwrap());
        assert_eq!(
            decide(&explained, "clean", 1),
            "walk",
            "`gone` itself was not said to be removed, only what was in it"
        );

        let removed = splice_over(
            dirty(&[
                file("/r/oot/gone/menu", Removed),
                dir("/r/oot/gone", Removed),
            ])
            .unwrap(),
        );
        assert_eq!(decide(&removed, "clean", 1), "copy", "absence explained");
    }

    /// The copy rebuilds the base's subtree below the new node with each
    /// entry's own size, so aggregation gives back the base's totals.
    #[test]
    fn a_copied_subtree_adds_up_to_what_the_base_held() {
        let splice = splice_over(Dirty::default());
        let clean = splice.base.find("clean").unwrap();
        let mut b = TreeBuilder::default();
        let root = b.push_root(new_node("root", crate::EntryKind::Dir, 0));
        let into = b.push_block(
            root,
            [new_node("clean", crate::EntryKind::Dir, 0)].into_iter(),
        );
        let progress = ScanProgress::default();
        splice.defer_copy(into, clean, &progress);
        assert_eq!(splice.copy_into(&mut b, &progress), Some(1));
        assert_eq!(progress.rows(), None, "no count left for the next phase");
        assert_eq!(
            progress.phase(),
            crate::Phase::Walking,
            "the copy is part of the walk; `Finishing` is the scan's to enter"
        );
        let tree = b.finish(std::path::PathBuf::from("/r/oot"));
        let copied = tree.find("clean").unwrap();
        assert_eq!(tree.node(copied).size, splice.base.node(clean).size);
        assert_eq!(tree.node(copied).alloc, splice.base.node(clean).alloc);
        assert_eq!(tree.node(tree.find("clean/x").unwrap()).own_size, 10);
        assert_eq!(progress.files.load(Ordering::Relaxed), 1);
    }

    // ------------------------------------------- what a stored cursor says

    /// Scan `root` in full, edit the cursor it took, and rescan from that
    /// scan with the edited cursor: how a rescan reads a cursor, without a
    /// store and without editing stored text.
    #[cfg(target_os = "macos")]
    fn rescan_with_cursor(edit: impl FnOnce(&mut Cursor)) -> Rescan {
        rescan_stats_with_cursor(edit).rescan
    }

    /// [`rescan_with_cursor`], with the whole of the rescan's stats.
    #[cfg(target_os = "macos")]
    fn rescan_stats_with_cursor(edit: impl FnOnce(&mut Cursor)) -> crate::ScanStats {
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("a/b/f"), b"x").unwrap();
        crate::fsevents::tests::settle(&root);
        let opts = crate::ScanOptions::default();
        let (tree, stats) =
            crate::scan(&root, opts.clone(), Arc::new(ScanProgress::default())).unwrap();
        let mut cursor = Cursor::decode(stats.journal.as_deref().unwrap()).unwrap();
        edit(&mut cursor);
        let base = Base {
            journal: Ok(cursor.encode()),
            load: Box::new(move |_| Ok(tree)),
        };
        let (_, stats) =
            crate::rescan(&root, opts, Arc::new(ScanProgress::default()), Some(base)).unwrap();
        stats
    }

    /// The budget a fallback hands on is what reading everything took — not
    /// that plus the replay that failed, or each fallback would raise the
    /// next one's budget by its own wait, and the budget would only grow.
    #[test]
    #[cfg(target_os = "macos")]
    fn a_fallback_hands_on_the_walk_s_time_not_the_wait_s() {
        const BUDGET: Duration = Duration::from_millis(400);
        BUDGET_OVERRIDE.set(Some(BUDGET));
        // Far enough back that no replay finishes inside the budget.
        let stats = rescan_stats_with_cursor(|c| c.position -= 30_000_000);
        BUDGET_OVERRIDE.set(None);
        assert_eq!(stats.rescan, Rescan::Fallback(Fallback::Deadline));
        assert!(stats.duration_ms >= BUDGET.as_millis() as u64);
        let handed_on = Cursor::decode(stats.journal.as_deref().unwrap()).unwrap();
        assert!(
            handed_on.full_ms < BUDGET.as_millis() as u64 / 2,
            "the next budget counts the failed wait: {} ms of a {} ms scan",
            handed_on.full_ms,
            stats.duration_ms
        );
    }

    /// The baseline the cases below differ from by one field.
    #[test]
    #[cfg(target_os = "macos")]
    fn an_unedited_cursor_is_built_on() {
        // A budget no busy machine can outlast: what is tested is the
        // cursor, and the deadline has its own test below.
        BUDGET_OVERRIDE.set(Some(Duration::from_secs(30)));
        let got = rescan_with_cursor(|_| ());
        BUDGET_OVERRIDE.set(None);
        assert!(got.is_incremental(), "{got:?}");
    }

    /// Positions only grow, so one ahead of the present is not this
    /// journal's.
    #[test]
    #[cfg(target_os = "macos")]
    fn a_cursor_from_the_journal_s_future_is_stale() {
        let got = rescan_with_cursor(|c| c.position = u64::MAX / 2);
        assert_eq!(got, Rescan::Fallback(Fallback::StaleCursor));
    }

    /// Older than the history can be trusted to reach back, however quickly
    /// the journal would answer.
    #[test]
    #[cfg(target_os = "macos")]
    fn a_cursor_older_than_two_days_is_too_old() {
        let got = rescan_with_cursor(|c| c.taken_at -= MAX_CURSOR_AGE.as_secs() + 60);
        assert_eq!(got, Rescan::Fallback(Fallback::TooOld));
        let got = rescan_with_cursor(|c| c.taken_at += 3600);
        assert_eq!(
            got,
            Rescan::Fallback(Fallback::FutureBase),
            "the clock went back"
        );
    }

    /// A watcher reads the phase as how far along the scan is, so it only
    /// runs forwards — through a base load, the walk and the copy, and the
    /// finishing work. Copying is part of the walk (it builds what the walk
    /// would have), and `Finishing` already means charging shared blocks.
    #[test]
    #[cfg(target_os = "macos")]
    fn the_phase_only_runs_forwards_through_an_incremental_scan() {
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("kept/inner")).unwrap();
        std::fs::write(root.join("kept/inner/f"), b"x").unwrap();
        crate::fsevents::tests::settle(&root);
        let opts = crate::ScanOptions::default();
        let (tree, stats) =
            crate::scan(&root, opts.clone(), Arc::new(ScanProgress::default())).unwrap();
        let base = Base {
            journal: Ok(stats.journal.unwrap()),
            load: Box::new(move |_| Ok(tree)),
        };
        BUDGET_OVERRIDE.set(Some(Duration::from_secs(30)));
        let progress = Arc::new(ScanProgress::default());
        let (_, stats) = crate::rescan(&root, opts, Arc::clone(&progress), Some(base)).unwrap();
        BUDGET_OVERRIDE.set(None);
        let Rescan::Incremental(done) = &stats.rescan else {
            panic!("{:?}", stats.rescan);
        };
        assert!(done.entries_reused > 0, "nothing was copied: {done:?}");
        let seen = progress.phases_seen.lock().unwrap().clone();
        assert!(
            seen.windows(2).all(|w| w[0] as u8 <= w[1] as u8),
            "the phase went backwards: {seen:?}"
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn a_cursor_another_journal_wrote_is_stale() {
        let got = rescan_with_cursor(|c| c.kind = "fsevents9".into());
        assert_eq!(got, Rescan::Fallback(Fallback::StaleCursor));
    }

    /// A position in another volume's journal — or in this volume's journal
    /// before it was discarded and recreated, which gives it a new identity.
    #[test]
    #[cfg(target_os = "macos")]
    fn a_cursor_for_another_volume_or_a_recreated_journal() {
        let got = rescan_with_cursor(|c| c.volume = "00000000-0000-0000-0000-000000000000".into());
        assert_eq!(got, Rescan::Fallback(Fallback::OtherVolume));
    }

    /// A replay that cannot write its marker cannot know its answer reaches
    /// the present, and the snapshot says that was why it walked.
    #[test]
    #[cfg(target_os = "macos")]
    fn a_replay_without_its_marker_is_a_named_fallback() {
        let nowhere = tempfile::tempdir().unwrap().path().join("gone");
        crate::fsevents::MARKER_BASE.set(Some(nowhere));
        let got = rescan_with_cursor(|_| ());
        crate::fsevents::MARKER_BASE.set(None);
        assert_eq!(got, Rescan::Fallback(Fallback::NoBarrier));
        assert_eq!(
            crate::RescanKind::parse(&crate::RescanKind::Fallback(Fallback::NoBarrier).record()),
            Some(crate::RescanKind::Fallback(Fallback::NoBarrier))
        );
    }

    /// No budget left, no answer: the scan walks instead.
    #[test]
    #[cfg(target_os = "macos")]
    fn a_replay_past_its_budget_is_abandoned() {
        BUDGET_OVERRIDE.set(Some(Duration::ZERO));
        let got = rescan_with_cursor(|_| ());
        BUDGET_OVERRIDE.set(None);
        assert_eq!(got, Rescan::Fallback(Fallback::Deadline));
    }

    /// The floor holds however fast the last full scan was, and the full
    /// scan's own duration holds above it.
    #[test]
    fn the_budget_is_the_last_full_scan_but_never_below_the_floor() {
        let cursor = |full_ms| Cursor {
            kind: "k".into(),
            volume: "v".into(),
            position: 1,
            root_ino: 1,
            options: 0,
            full_ms,
            taken_at: 0,
        };
        assert_eq!(budget_for(&cursor(0)), BUDGET_FLOOR);
        assert_eq!(budget_for(&cursor(3)), BUDGET_FLOOR);
        assert_eq!(budget_for(&cursor(9000)), Duration::from_millis(9000));
    }
}
