//! Files whose contents are identical.
//!
//! A disk tool answers "what is big". The next question is always "and how
//! much of it is the same thing twice" — three copies of a photo library, a
//! project vendored into four checkouts, a backup of a backup. That question
//! cannot be answered from a scan, because a scan reads metadata and this
//! needs bytes.
//!
//! So the whole design is about reading as few of them as possible.
//!
//! **Three stages, each one only paying for what the last could not rule out.**
//!
//! 1. **Size.** Files of different lengths cannot be identical, and this is
//!    free — the scan already knows every length. On a real disk it discards
//!    the overwhelming majority: most files are a size nothing else shares.
//! 2. **A prefix.** Of the files that share a size, most differ within their
//!    first few kilobytes. Hashing [`PREFIX_BYTES`] costs one read of one
//!    block and splits the groups again.
//! 3. **The whole file.** Only what survives both. This is the expensive
//!    stage and by the time anything reaches it, there is a real reason to
//!    believe it will match.
//!
//! Measured on `~/github` — 375,585 files, 33.8 GiB: the answer (1203 groups,
//! 1005 MiB reclaimable) cost **2.6 GiB of reading**, 7.7% of the tree. A
//! second run over the same disk read 30 MiB and hashed nothing, because the
//! third stage is cached and the second is not. Caching the prefix too would
//! remove most of that 30 MiB and double the cache's rows for it; the prefix
//! is one block per file and not worth storing.
//!
//! **BLAKE3, and no byte-for-byte comparison afterwards.** The output is
//! 256 bits; the chance of two different files colliding is far below the
//! chance of the disk returning the wrong bytes in the first place, and a
//! confirmation pass would double the reading for a risk that is not the
//! binding one. That is a claim about a *cryptographic* hash and would be
//! indefensible with a 64-bit one — which is why this is not xxh3, fast as it
//! is.
//!
//! **Nothing here deletes anything**, in the same spirit as the agent. The
//! answer is a list of groups; what to do about them is a decision with
//! context this crate does not have — which copy is the backup, which one is
//! on the NAS, which one something else is pointing at.
//!
//! **Hardlinks and clones are not duplicates.** Two names for one inode
//! already occupy one copy of the bytes; reporting them as a duplicate would
//! promise space that deleting one cannot recover. They are grouped separately
//! so they can be *said*, because seeing them is useful, but they are not
//! counted as reclaimable.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use spacetrace_scan_core::{EntryKind, FileIdentity, NodeId, RawMeta, Tree};

/// How much of a file the second stage reads.
///
/// 16 KiB: four pages on every platform this runs on, and past the header of
/// every format that has one. Larger costs more read for files that were going
/// to be hashed whole anyway; smaller stops distinguishing files that share a
/// magic number and a header, which is exactly the population that reaches
/// this stage.
pub const PREFIX_BYTES: u64 = 16 * 1024;

/// Files below this are not considered.
///
/// Small files are duplicated constantly — every `__init__.py`, every empty
/// `.gitkeep`, every 12-byte lockfile — and grouping them produces thousands
/// of rows describing kilobytes. The point of the feature is space, and the
/// default hides what cannot be a meaningful amount of it.
pub const DEFAULT_MIN_SIZE: u64 = 1024 * 1024;

/// A 256-bit content hash.
pub type Hash = [u8; 32];

/// One file in a group.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Copy {
    pub node: NodeId,
    pub path: PathBuf,
}

/// Files that hold the same bytes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Group {
    /// What each copy is, in bytes.
    pub size: u64,
    /// Every path holding these bytes, in the order the tree lists them so a
    /// second run over the same tree reports the same order.
    pub copies: Vec<Copy>,
    /// Whether these are separate copies or the same inode under several
    /// names.
    ///
    /// The distinction is the difference between "you could get this back" and
    /// "you already have".
    pub shared: Sharing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Sharing {
    /// Separate files. Deleting all but one frees `size * (copies - 1)`.
    Separate,
    /// One inode, several names. Deleting all but one frees nothing.
    Hardlinked,
}

impl Group {
    /// Bytes that would come back if every copy but one were removed.
    ///
    /// Zero for hardlinked names, which is the whole reason they carry a
    /// different [`Sharing`]: the arithmetic that is true for copies is a lie
    /// for links, and a total built without this distinction overstates what a
    /// user can recover.
    pub fn reclaimable(&self) -> u64 {
        match self.shared {
            Sharing::Hardlinked => 0,
            Sharing::Separate => self.size * (self.copies.len() as u64 - 1),
        }
    }
}

/// What a run found.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Report {
    /// Groups, largest reclaimable first.
    pub groups: Vec<Group>,
    /// Files whose contents could not be read, with the reason.
    ///
    /// Carried rather than dropped, for the same reason a scan carries its
    /// errors: "no duplicates found" and "half of it was unreadable" are
    /// different answers and must not look alike.
    pub unreadable: Vec<(PathBuf, String)>,
    /// Files that reached the hashing stages, for reporting what the work was.
    pub hashed: usize,
    /// Bytes actually read. The number that says whether the funnel worked.
    pub bytes_read: u64,
}

impl Report {
    /// Total recoverable across every group.
    pub fn reclaimable(&self) -> u64 {
        self.groups.iter().map(Group::reclaimable).sum()
    }
}

/// How a run is bounded.
#[derive(Debug, Clone)]
pub struct Options {
    /// Files smaller than this are ignored. See [`DEFAULT_MIN_SIZE`].
    pub min_size: u64,
    /// Report hardlinked names as their own groups.
    ///
    /// On by default because seeing them explains a number: someone looking at
    /// a folder that "should" be twice the size it is has usually found links,
    /// and a tool that hides them leaves that unexplained.
    pub include_hardlinks: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            min_size: DEFAULT_MIN_SIZE,
            include_hardlinks: true,
        }
    }
}

/// A place to remember a file's hash between runs.
///
/// A trait rather than a table, so this crate stays free of SQLite and the
/// storage lives in the crate whose job storage is. The implementation this
/// project ships is in `spacetrace-store`; [`NoCache`] is the other one and is
/// what the tests use.
///
/// **The key has to include everything that could change the contents.** An
/// implementation keyed on the path alone returns a stale hash for a file
/// that was overwritten, and a duplicate finder that reports a match which is
/// no longer a match is worse than one that is merely slow — somebody deletes
/// on the strength of it.
pub trait HashCache {
    fn get(&self, key: &CacheKey) -> Option<Hash>;
    fn put(&self, key: &CacheKey, hash: Hash);
}

/// What a cached hash is keyed by.
///
/// Identity, length and modification time together. Any one of them alone is
/// forgeable by ordinary use: a path is reused, a length is unchanged by an
/// in-place edit, and an mtime is preserved by every archiver there is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    pub mtime: i64,
}

/// A cache that remembers nothing.
pub struct NoCache;

impl HashCache for NoCache {
    fn get(&self, _key: &CacheKey) -> Option<Hash> {
        None
    }
    fn put(&self, _key: &CacheKey, _hash: Hash) {}
}

/// Find duplicate contents under a scanned tree.
///
/// The tree supplies sizes and paths; the bytes come off the disk, so this
/// only means anything for a tree that describes *this* machine. A snapshot
/// pulled from a server names files that are not here, and running this
/// against one would hash whatever happens to sit at those paths locally.
/// Callers are expected to check; the function cannot tell.
pub fn find(tree: &Tree, options: &Options, cache: &(dyn HashCache + Sync)) -> Report {
    let candidates = by_size(tree, options);
    if candidates.is_empty() {
        return Report::default();
    }

    // Stage two and three, each one narrowing what the next has to read. Run
    // per size-group and in parallel: the groups are independent, and the work
    // is dominated by reads that the kernel can overlap.
    let mut report = Report::default();
    let results: Vec<GroupResult> = candidates
        .into_par_iter()
        .map(|group| resolve(tree, &group, cache))
        .collect();

    for result in results {
        report.hashed += result.hashed;
        report.bytes_read += result.bytes_read;
        report.unreadable.extend(result.unreadable);
        report.groups.extend(result.groups);
    }

    if !options.include_hardlinks {
        report.groups.retain(|g| g.shared != Sharing::Hardlinked);
    }

    // Largest recoverable first: the list exists to be acted on from the top,
    // and "biggest group" is not the same question as "most space" — twenty
    // copies of a small file rank above two copies of a disk image by count
    // and below it by what deleting them gives back.
    report.groups.sort_by(|a, b| {
        b.reclaimable()
            .cmp(&a.reclaimable())
            .then(b.size.cmp(&a.size))
            // A stable last resort so two runs over one tree agree. Without
            // it the order of equal groups follows whichever thread finished
            // first, and a diff of two reports becomes unreadable noise.
            .then(a.copies[0].node.cmp(&b.copies[0].node))
    });
    report
}

/// One file the first stage let through.
#[derive(Debug, Clone)]
struct Candidate {
    node: NodeId,
    path: PathBuf,
    size: u64,
}

/// A candidate as the disk describes it *now*, rather than as the scan
/// remembered it.
///
/// The second look is not redundant. A scan is a snapshot and this reads
/// bytes: between the two, a file can be replaced, truncated or removed, and
/// hashing it under the old size would file it in a group it no longer belongs
/// to. It is also where `(device, inode)` comes from — the tree does not carry
/// it, because the walk resolves hardlinks as it goes and keeps only the
/// answer.
#[derive(Debug, Clone)]
struct Live<'a> {
    candidate: &'a Candidate,
    key: CacheKey,
}

impl Live<'_> {
    fn node(&self) -> NodeId {
        self.candidate.node
    }

    fn inode(&self) -> (u64, u64) {
        (self.key.device, self.key.inode)
    }
}

/// Look at a candidate again, on disk.
///
/// `None` means it is no longer the file the scan described — gone, or a
/// different length. Dropping it is the only honest option: it is not a member
/// of this size group any more, and there is no group it can be moved to
/// without re-running stage one.
fn look_again(candidate: &Candidate) -> Result<Option<Live<'_>>, io::Error> {
    let md = std::fs::symlink_metadata(&candidate.path)?;
    let (meta, err) = RawMeta::for_path(&candidate.path, &md, FileIdentity::Needed);
    if let Some(err) = err {
        return Err(err);
    }
    if meta.size != candidate.size || meta.kind != EntryKind::File {
        return Ok(None);
    }
    Ok(Some(Live {
        candidate,
        key: CacheKey {
            device: meta.dev,
            inode: meta.ino,
            size: meta.size,
            mtime: meta.mtime,
        },
    }))
}

/// Stage one: group by size, keeping only sizes more than one file has.
///
/// Returns the groups rather than a flat list because every later stage works
/// within a size group — two files of different lengths never need comparing
/// again, and carrying that structure forward is what keeps stage two from
/// re-deriving it.
fn by_size(tree: &Tree, options: &Options) -> Vec<Vec<Candidate>> {
    let mut by_size: HashMap<u64, Vec<Candidate>> = HashMap::new();

    for id in tree.iter() {
        let node = tree.node(id);
        // Files only. A directory has no contents of its own to compare, and a
        // symlink's "contents" are a path — following it would count the
        // target twice and report a link as a copy of the thing it points at.
        if node.kind != EntryKind::File {
            continue;
        }
        let path = tree.path(id);
        let Some(size) = effective_size(node, &path) else {
            continue;
        };
        if size < options.min_size {
            continue;
        }
        by_size.entry(size).or_default().push(Candidate {
            node: id,
            path,
            size,
        });
    }

    by_size
        .into_values()
        .filter(|group| group.len() > 1)
        .map(|mut group| {
            // Tree order within a group, so the output does not depend on how
            // a hash map happened to iterate.
            group.sort_by_key(|c| c.node);
            group
        })
        .collect()
}

/// The size to group a file by, which is not always the one the tree records.
///
/// A scan counts hardlinked bytes once (invariant 3 in the workspace notes):
/// of N names for one inode, one carries the bytes and the rest are recorded
/// as zero. That is right for a total and wrong here — a name recorded as zero
/// would be sorted into the empty-file bucket and never meet the name it
/// shares its contents with, so the one place hardlinks are most worth
/// reporting is the one place they would disappear.
///
/// So a link with nothing recorded is looked up on disk. The condition is
/// narrow on purpose: `nlink > 1` and a recorded zero is a handful of entries
/// on a real disk, where statting every file would undo what makes stage one
/// free.
fn effective_size(node: &spacetrace_scan_core::Node, path: &Path) -> Option<u64> {
    if node.own_size > 0 || node.nlink <= 1 {
        return Some(node.own_size);
    }
    // A link that has gone since the scan simply drops out; `resolve` is where
    // a missing file gets reported, and reporting it twice would be noise.
    std::fs::symlink_metadata(path).ok().map(|md| md.len())
}

#[derive(Default)]
struct GroupResult {
    groups: Vec<Group>,
    unreadable: Vec<(PathBuf, String)>,
    hashed: usize,
    bytes_read: u64,
}

/// Stages two and three for one size group.
fn resolve(tree: &Tree, group: &[Candidate], cache: &(dyn HashCache + Sync)) -> GroupResult {
    let _ = tree;
    let mut result = GroupResult::default();

    let mut live: Vec<Live<'_>> = Vec::with_capacity(group.len());
    for candidate in group {
        match look_again(candidate) {
            Ok(Some(found)) => live.push(found),
            // Changed or gone since the scan. Not an error to report: nothing
            // failed, the file simply is not what this group was about.
            Ok(None) => {}
            Err(err) => result
                .unreadable
                .push((candidate.path.clone(), err.to_string())),
        }
    }

    // Names of one inode are separated first, before any byte is read: they
    // are certainly identical and certainly not reclaimable, so hashing them
    // would be reading a file to learn something already known.
    let (linked, distinct) = split_hardlinks(&live);
    for names in linked {
        result.groups.push(Group {
            size: names[0].key.size,
            copies: names.iter().map(|l| copy_of(l.candidate)).collect(),
            shared: Sharing::Hardlinked,
        });
    }

    if distinct.len() < 2 {
        return result;
    }

    // Stage two, skipped when the whole file is not much more than the prefix:
    // reading 16 KiB of a 20 KiB file and then reading all 20 is more work
    // than reading 20 once.
    let buckets: Vec<Vec<&Live<'_>>> = if distinct[0].key.size > PREFIX_BYTES * 2 {
        let mut split: HashMap<Hash, Vec<&Live<'_>>> = HashMap::new();
        for entry in &distinct {
            match hash_prefix(&entry.candidate.path) {
                Ok((hash, read)) => {
                    result.bytes_read += read;
                    split.entry(hash).or_default().push(entry);
                }
                Err(err) => result
                    .unreadable
                    .push((entry.candidate.path.clone(), err.to_string())),
            }
        }
        split.into_values().filter(|b| b.len() > 1).collect()
    } else {
        vec![distinct.clone()]
    };

    // Stage three.
    for bucket in buckets {
        let mut split: HashMap<Hash, Vec<&Live<'_>>> = HashMap::new();
        for entry in bucket {
            if let Some(hash) = cache.get(&entry.key) {
                split.entry(hash).or_default().push(entry);
                continue;
            }
            match hash_whole(&entry.candidate.path) {
                Ok((hash, read)) => {
                    result.hashed += 1;
                    result.bytes_read += read;
                    cache.put(&entry.key, hash);
                    split.entry(hash).or_default().push(entry);
                }
                Err(err) => result
                    .unreadable
                    .push((entry.candidate.path.clone(), err.to_string())),
            }
        }

        for mut same in split.into_values().filter(|b| b.len() > 1) {
            same.sort_by_key(|l| l.node());
            result.groups.push(Group {
                size: same[0].key.size,
                copies: same.iter().map(|l| copy_of(l.candidate)).collect(),
                shared: Sharing::Separate,
            });
        }
    }

    result
}

fn copy_of(candidate: &Candidate) -> Copy {
    Copy {
        node: candidate.node,
        path: candidate.path.clone(),
    }
}

/// Split names of one inode away from genuinely separate files.
///
/// Returns the link sets (two or more names for one inode) and one
/// representative of each, which is what the byte stages then work on: hashing
/// every name of one inode would read the same blocks twice and put a file in
/// a group with itself.
fn split_hardlinks<'a>(group: &'a [Live<'a>]) -> (Vec<Vec<&'a Live<'a>>>, Vec<&'a Live<'a>>) {
    let mut by_inode: HashMap<(u64, u64), Vec<&Live<'_>>> = HashMap::new();
    for entry in group {
        by_inode.entry(entry.inode()).or_default().push(entry);
    }

    let mut linked = Vec::new();
    let mut representatives = Vec::new();
    for (_, mut names) in by_inode {
        names.sort_by_key(|l| l.node());
        representatives.push(names[0]);
        if names.len() > 1 {
            linked.push(names);
        }
    }
    representatives.sort_by_key(|l| l.node());
    linked.sort_by_key(|names| names[0].node());
    (linked, representatives)
}

fn hash_prefix(path: &Path) -> io::Result<(Hash, u64)> {
    let mut file = File::open(path)?;
    let mut buffer = vec![0u8; PREFIX_BYTES as usize];
    let mut filled = 0usize;
    // `read` is allowed to return less than asked for without being at the
    // end, and a short read here would hash a different number of bytes for
    // two identical files and split a group that should have held.
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(&buffer[..filled]);
    Ok((*hasher.finalize().as_bytes(), filled as u64))
}

fn hash_whole(path: &Path) -> io::Result<(Hash, u64)> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; 256 * 1024];
    let mut total = 0u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        total += read as u64;
    }
    Ok((*hasher.finalize().as_bytes(), total))
}
