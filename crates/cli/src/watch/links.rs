//! Which folder a hardlinked file is charged to, between full scans.
//!
//! A walk settles a hardlink by meeting every name of it in one pass: the
//! first name claims the inode and the rest are charged nothing (invariant 3).
//! The watch lists one folder at a time and never meets them in one pass, so
//! it keeps the settlement here instead: for every hardlinked file the scans
//! it took in have met, by `(dev, ino)`, the folders holding a name of it and
//! the one charged. A file is counted once however many of its names a
//! listing meets, and only when none is left does it stop counting.
//!
//! **Folders, not names.** Every name of a file in one folder charges that
//! folder the same, so the ledger keeps the folder once per name and no name
//! at all: on a backup tree of hardlinked copies the names would be most of
//! its size. Files sit in a slab, and folders refer to them by slot.
//!
//! **A charge stays where it is.** Which name carries the bytes is undefined
//! (invariant 3), but a watch that moved it on every listing would show bytes
//! leaving one folder for another with nothing changed on disk. So a file
//! stays charged to its folder for as long as that folder holds a name of it,
//! and only then is another chosen: the first in `(depth, path)` order, the
//! kind of order the scanner settles clones by — so the choice does not depend
//! on which of two folders an event happened to have listed first. The totals
//! never depend on it; the rows do.
//!
//! **A name it has not met may be counted already.** A folder listed while a
//! file in it had one name counted that file as an ordinary one; a link made
//! afterwards elsewhere brings the file here through the new name alone. The
//! link count gives that away: more links than names known, beyond the ones a
//! full scan found outside the walk, and some name is unaccounted for. Such a
//! file is charged to no folder until the rest turn up — if one of them was
//! counted as an ordinary file, that already holds its bytes, and counting it
//! again is the one mistake a frame must never show. The watch asks for a
//! full rescan if they do not turn up.
//!
//! **Changes come in batches**, one per scan taken in, settled together at the
//! end: a folder listed again first drops every name it held and then adds the
//! ones it holds now, and in between a charge must not move to some other
//! folder merely because its own was briefly missing. What each folder is
//! charged is kept as a running sum, moved at settlement by what changed, so a
//! tick costs what it listed and not the size of the ledger.

use std::collections::{HashMap, HashSet};

use super::model::DirId;

/// A file, as the filesystem identifies it: `(dev, ino)`.
pub(crate) type Inode = (u64, u64);

/// A file's place in the ledger's slab.
type Slot = u32;

/// One hardlinked name a scan met, in the folder it is being taken in for.
pub(crate) struct Seen {
    pub inode: Inode,
    /// Its link count, as the scan read it.
    pub nlink: u32,
    /// What the scan charged this name: the file's figures, or 0 for a name
    /// of it the scan had already charged elsewhere.
    pub size: u64,
    pub alloc: u64,
}

/// The folder of every name of a file, once per name. Two fit without an
/// allocation of their own, and most hardlinked files have two names; the
/// rest are boxed, so a file costs 16 bytes of names either way.
enum Names {
    Few(u8, [DirId; 2]),
    // Boxed on purpose: a bare `Vec` would make every file's names 24 bytes
    // for the few that have more than two.
    #[allow(clippy::box_collection)]
    Many(Box<Vec<DirId>>),
}

impl Default for Names {
    fn default() -> Self {
        Names::Few(0, [0; 2])
    }
}

impl Names {
    fn as_slice(&self) -> &[DirId] {
        match self {
            Names::Few(len, dirs) => &dirs[..usize::from(*len)],
            Names::Many(dirs) => dirs.as_slice(),
        }
    }

    fn len(&self) -> usize {
        self.as_slice().len()
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn push(&mut self, dir: DirId) {
        match self {
            Names::Few(len, dirs) if usize::from(*len) < dirs.len() => {
                dirs[usize::from(*len)] = dir;
                *len += 1;
            }
            Names::Few(_, dirs) => *self = Names::Many(Box::new(vec![dirs[0], dirs[1], dir])),
            Names::Many(dirs) => dirs.push(dir),
        }
    }

    /// Take one occurrence of `dir` out; order does not matter.
    fn remove_one(&mut self, dir: DirId) {
        let Some(at) = self.as_slice().iter().position(|&d| d == dir) else {
            return;
        };
        match self {
            Names::Few(len, dirs) => {
                dirs[at] = dirs[usize::from(*len) - 1];
                *len -= 1;
            }
            Names::Many(dirs) => {
                dirs.swap_remove(at);
            }
        }
    }

    /// Every folder through `map`, dropping the ones it maps to `None`.
    fn remap(&mut self, map: impl Fn(DirId) -> Option<DirId>) {
        let kept: Vec<DirId> = self.as_slice().iter().filter_map(|&d| map(d)).collect();
        *self = Names::default();
        for dir in kept {
            self.push(dir);
        }
    }
}

struct File {
    inode: Inode,
    names: Names,
    /// The folder that carries the bytes; `None` while a name of it is not
    /// accounted for (see the module comment).
    charged: Option<DirId>,
    size: u64,
    alloc: u64,
    /// The link count last read for it, less the names that went since.
    nlink: u32,
    /// Links the last full scan did not meet: outside the root, or in a folder
    /// the walk does not enter. Those are no cause for doubt.
    outside: u32,
    /// Ticks in a row it has ended charged to none.
    doubted: u8,
}

impl File {
    /// More links than names known and names known to be elsewhere.
    fn short(&self) -> bool {
        self.names.len() as u64 + u64::from(self.outside) < u64::from(self.nlink)
    }
}

/// What the files charged to none ask of the watch at the end of a tick.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Doubts {
    /// One became so this tick: its other names may be in a folder listed a
    /// moment before the link was made, which a listing now would find.
    pub fresh: bool,
    /// A folder holding a name of one that has been so for two ticks: the
    /// listings had their chance, and only a full scan finds the names.
    pub long: Option<DirId>,
}

/// What one batch did to one file.
struct Touch {
    /// The folder it was charged to before and the figures it was charged
    /// there — `None` for a file new to the ledger or charged to none.
    was: Option<(DirId, u64, u64)>,
    /// A scan in the batch met a name of it, and read its link count.
    met: bool,
    /// Names of it that left the folders the batch listed.
    removed: u32,
}

#[derive(Default)]
pub(crate) struct Ledger {
    files: Vec<File>,
    /// Slots of files that left, for the next file to take.
    free: Vec<Slot>,
    slots: HashMap<Inode, Slot>,
    /// The file of each name a folder held at its last listing, by folder.
    by_dir: Vec<Vec<Slot>>,
    /// What each folder is charged for the files it carries, by folder.
    charged: Vec<(u64, u64)>,
    /// Files touched since the last [`Ledger::settle`].
    touched: HashMap<Slot, Touch>,
    /// Files charged to none.
    doubtful: HashSet<Slot>,
}

impl Ledger {
    /// Take `seen` as every hardlinked name `dir` holds now, in place of what
    /// it held before.
    pub(crate) fn replace(&mut self, dir: DirId, seen: Vec<Seen>) {
        let mut held = self
            .by_dir
            .get_mut(dir as usize)
            .map(std::mem::take)
            .unwrap_or_default();
        for &slot in &held {
            self.touch(slot).removed += 1;
            self.files[slot as usize].names.remove_one(dir);
        }
        held.clear();
        for s in seen {
            let slot = self.slot_for(s.inode);
            // A file's figures are the largest of what its names in one batch
            // were charged: the one name the scan charged has them all.
            let first = !std::mem::replace(&mut self.touch(slot).met, true);
            let file = &mut self.files[slot as usize];
            file.names.push(dir);
            file.nlink = s.nlink;
            (file.size, file.alloc) = match first {
                true => (s.size, s.alloc),
                false => (file.size.max(s.size), file.alloc.max(s.alloc)),
            };
            held.push(slot);
        }
        if held.is_empty() {
            return;
        }
        let at = dir as usize;
        if self.by_dir.len() <= at {
            self.by_dir.resize_with(at + 1, Vec::new);
        }
        self.by_dir[at] = held;
    }

    /// `dir` went away, and every name in it.
    pub(crate) fn forget(&mut self, dir: DirId) {
        self.replace(dir, Vec::new());
    }

    /// The slot of `inode`, a new one charged to none if it has none yet.
    fn slot_for(&mut self, inode: Inode) -> Slot {
        if let Some(&slot) = self.slots.get(&inode) {
            return slot;
        }
        let file = File {
            inode,
            names: Names::default(),
            charged: None,
            size: 0,
            alloc: 0,
            nlink: 0,
            outside: 0,
            doubted: 0,
        };
        let slot = match self.free.pop() {
            Some(slot) => {
                self.files[slot as usize] = file;
                slot
            }
            None => {
                self.files.push(file);
                (self.files.len() - 1) as Slot
            }
        };
        self.slots.insert(inode, slot);
        slot
    }

    /// What this batch did to `slot` so far, noting on first touch what it
    /// was charged before.
    fn touch(&mut self, slot: Slot) -> &mut Touch {
        let file = &self.files[slot as usize];
        self.touched.entry(slot).or_insert_with(|| Touch {
            was: file.charged.map(|dir| (dir, file.size, file.alloc)),
            met: false,
            removed: 0,
        })
    }

    /// End a batch: every file touched stays charged to its folder if that
    /// folder still holds a name of it, is charged to the folder first by
    /// `order` if not, and leaves the ledger once no name is left. `order` is
    /// the model's `(depth, path)` key for a folder.
    ///
    /// `whole` says the batch was a scan of the whole root, which met every
    /// name there is to meet: the links it did not meet are outside, from now
    /// on.
    pub(crate) fn settle<K: Ord>(&mut self, whole: bool, order: impl Fn(DirId) -> K) {
        // Taken, not drained: a full scan touches every file, and a drained
        // table would keep that many slots for the rest of the session.
        for (slot, touch) in std::mem::take(&mut self.touched) {
            if let Some((dir, size, alloc)) = touch.was {
                let sum = &mut self.charged[dir as usize];
                sum.0 -= size;
                sum.1 -= alloc;
            }
            let file = &mut self.files[slot as usize];
            if file.names.is_empty() {
                self.slots.remove(&file.inode);
                self.doubtful.remove(&slot);
                file.charged = None;
                self.free.push(slot);
                continue;
            }
            // A name that left without any scan of this batch meeting the file
            // again took its link with it, as far as anyone knows: an unlink
            // does, and a rename turns up as a name elsewhere, whose listing
            // reads the count again.
            if !touch.met {
                file.nlink = file.nlink.saturating_sub(touch.removed);
            }
            let names = file.names.as_slice();
            let choice = match touch.was.map(|w| w.0).filter(|was| names.contains(was)) {
                Some(kept) => kept,
                None => *names
                    .iter()
                    .min_by_key(|&&dir| order(dir))
                    .unwrap_or(&names[0]),
            };
            let elsewhere = file.nlink.saturating_sub(names.len() as u32);
            file.outside = match whole {
                true => elsewhere,
                // A link outside that went away lowers the count; one that
                // came is a name not accounted for, and stays a doubt.
                false => file.outside.min(elsewhere),
            };
            if file.short() {
                file.charged = None;
                if self.doubtful.insert(slot) {
                    file.doubted = 0;
                }
                continue;
            }
            file.charged = Some(choice);
            file.doubted = 0;
            self.doubtful.remove(&slot);
            let at = choice as usize;
            if self.charged.len() <= at {
                self.charged.resize(at + 1, (0, 0));
            }
            self.charged[at].0 += file.size;
            self.charged[at].1 += file.alloc;
        }
        // The first scan fills the slab by doubling, up to a third of it
        // spare; a full scan is when the ledger is as big as it gets.
        if whole {
            self.files.shrink_to_fit();
        }
    }

    /// Whether a batch is open, for the model to check it never reads one.
    pub(crate) fn unsettled(&self) -> bool {
        !self.touched.is_empty()
    }

    /// End a tick: what the files charged to none ask of the watch. The
    /// folder a lasting doubt names is the first by `order` of all such
    /// files' folders, so the message names the same folder every run.
    pub(crate) fn doubts<K: Ord>(&mut self, order: impl Fn(DirId) -> K) -> Doubts {
        let mut fresh = false;
        let mut long: Option<(K, DirId)> = None;
        for &slot in &self.doubtful {
            let file = &mut self.files[slot as usize];
            file.doubted = file.doubted.saturating_add(1);
            if file.doubted < 2 {
                fresh = true;
                continue;
            }
            for &dir in file.names.as_slice() {
                let key = order(dir);
                if long.as_ref().is_none_or(|(best, _)| key < *best) {
                    long = Some((key, dir));
                }
            }
        }
        Doubts {
            fresh,
            long: long.map(|(_, dir)| dir),
        }
    }

    /// What each folder is charged for the hardlinked files it carries, by
    /// folder; a folder past the end carries none.
    pub(crate) fn charged(&self) -> &[(u64, u64)] {
        &self.charged
    }

    /// Follow the model's records to their new ids after a compaction.
    ///
    /// `DirId::MAX` marks a record that was dropped. A dropped record held no
    /// names — it had gone away, and [`Ledger::forget`] ran then — so nothing
    /// should map there; a name that did is dropped with it.
    pub(crate) fn remap(&mut self, remap: &[DirId]) {
        debug_assert!(!self.unsettled(), "compaction inside a batch");
        let new = |id: DirId| remap.get(id as usize).copied().filter(|&n| n != DirId::MAX);
        let mut by_dir = Vec::new();
        for (old, held) in std::mem::take(&mut self.by_dir).into_iter().enumerate() {
            let Some(n) = new(old as DirId).filter(|_| !held.is_empty()) else {
                continue;
            };
            if by_dir.len() <= n as usize {
                by_dir.resize_with(n as usize + 1, Vec::new);
            }
            by_dir[n as usize] = held;
        }
        self.by_dir = by_dir;
        self.charged.clear();
        for (slot, file) in self.files.iter_mut().enumerate() {
            if file.names.is_empty() {
                continue;
            }
            file.names.remap(new);
            if file.names.is_empty() {
                self.slots.remove(&file.inode);
                self.doubtful.remove(&(slot as Slot));
                file.charged = None;
                self.free.push(slot as Slot);
                continue;
            }
            let Some(old) = file.charged else {
                continue;
            };
            let dir = new(old)
                .filter(|d| file.names.as_slice().contains(d))
                .unwrap_or(file.names.as_slice()[0]);
            file.charged = Some(dir);
            if self.charged.len() <= dir as usize {
                self.charged.resize(dir as usize + 1, (0, 0));
            }
            self.charged[dir as usize].0 += file.size;
            self.charged[dir as usize].1 += file.alloc;
        }
    }

    /// How many names it holds.
    #[cfg(test)]
    pub(crate) fn names(&self) -> usize {
        self.files.iter().map(|f| f.names.len()).sum()
    }

    /// Whether any file is charged to none.
    #[cfg(test)]
    pub(crate) fn doubtful(&self) -> bool {
        !self.doubtful.is_empty()
    }

    /// Every file counted, with the folder charged for it.
    #[cfg(test)]
    pub(crate) fn charges(&self) -> impl Iterator<Item = (Inode, DirId)> + '_ {
        self.files
            .iter()
            .filter_map(|f| Some((f.inode, f.charged?)))
    }
}
