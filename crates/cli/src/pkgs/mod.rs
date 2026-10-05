//! Which installed package each byte belongs to — and which belong to none.
//!
//! The databases are read straight off the disk wherever their format is a
//! plain file (dpkg, pacman, apk, Homebrew's directory layout) or a binary one
//! simple enough to read here (macOS installer receipts, `receipt.rs`). rpm's
//! is not, so `rpm` itself is asked, and only when it exists; when it does
//! not, the report says so instead of quietly calling every rpm file unowned.
//!
//! **Every path a database names has its directory resolved before it is
//! stored.** A package list says `/bin/ls` and the scan finds `/usr/bin/ls`,
//! because on a merged-/usr system `/bin` is a symlink to `usr/bin` and the
//! scanner does not follow symlinks. Matching the strings as written left 396
//! of bookworm's paths — `ls`, `bash`, the whole of `/lib` — looking unowned.
//! Only the directory part is resolved: the last component may itself be a
//! symlink the package ships, and that link is what the scan found. How that
//! is done without touching the rest of the system is `load.rs`.
//!
//! **macOS shows its data volume twice**: at `/System/Volumes/Data`, and through
//! firmlinks at the places the system names — `/Applications`, `/Library`,
//! `/usr/local`. A firmlink is no symlink, so resolving finds nothing to follow;
//! the receipts name one spelling and a scan of the other must still match.
//! See [`Firmlinks`].

mod load;
pub mod parse;
pub mod receipt;

use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use spacetrace_scan_core::{probe_mount, EntryKind, Mounts, NodeId, SizeBasis, Tree};

pub type PackageId = u32;

/// Which database a package came from. The order is the order they are read
/// in and the order a contested file is decided by — see [`Ownership`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Manager {
    Dpkg,
    Rpm,
    Pacman,
    Apk,
    /// macOS installer receipts, the database `pkgutil` reads.
    Pkgutil,
    Homebrew,
    HomebrewCask,
}

impl Manager {
    pub fn label(self) -> &'static str {
        match self {
            Manager::Dpkg => "dpkg",
            Manager::Rpm => "rpm",
            Manager::Pacman => "pacman",
            Manager::Apk => "apk",
            Manager::Pkgutil => "pkgutil",
            Manager::Homebrew => "homebrew",
            Manager::HomebrewCask => "homebrew-cask",
        }
    }
}

/// Field order is the tie-break order: manager first, then name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub struct Package {
    pub manager: Manager,
    pub name: String,
}

/// A database that was found, and whether it could be read.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Database {
    pub manager: Manager,
    pub location: PathBuf,
    #[serde(flatten)]
    pub state: State,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum State {
    Read {
        packages: usize,
    },
    /// Present, and its files are therefore *not* known: everything it owns is
    /// in the unowned figure, and the output has to say so.
    Unreadable {
        reason: String,
    },
}

/// How a mounted filesystem is asked whether it is alive: the scanner's own
/// [`probe_mount`], or in a test one that never answers.
pub type Probe = fn(&Path, Duration) -> Option<std::io::Result<std::fs::Metadata>>;

/// How long `rpm -qa` may take. It needs about a second; what it must not do
/// is wait forever on a stale lock, which it does on RHEL 8.
pub const RPM_TIMEOUT: Duration = Duration::from_secs(60);

/// Where to look. `sysroot` is the filesystem the databases describe — `/` on
/// a real system, a temporary directory in a test, just as `dpkg --root`.
#[derive(Debug, Clone)]
pub struct Sources {
    pub sysroot: PathBuf,
    /// The `rpm` executable. A name to search `PATH` for, normally.
    pub rpm: OsString,
    pub rpm_timeout: Duration,
    /// The home folder whose own installer receipts are read too, relative to
    /// the sysroot and without slashes at either end: a package installed
    /// "for this user only" records itself in `~/Library/Receipts`, its paths
    /// relative to that home. Only macOS reads it.
    pub receipt_home: Option<String>,
    /// Where other filesystems are mounted, read once without blocking.
    pub mounts: Mounts,
    /// How long a mounted filesystem gets to answer; `None` waits forever,
    /// which is what `--mount-timeout 0` asks of the scan too.
    pub mount_timeout: Option<Duration>,
    pub probe: Probe,
    /// Counters for whoever is showing progress.
    pub progress: Arc<LoadProgress>,
}

impl Sources {
    /// This machine, with the scan's mount timeout.
    pub fn system(mount_timeout: Option<Duration>) -> Sources {
        Sources {
            sysroot: PathBuf::from("/"),
            rpm: OsString::from("rpm"),
            rpm_timeout: RPM_TIMEOUT,
            receipt_home: crate::home()
                .ok()
                .map(|home| home.to_string_lossy().trim_matches('/').to_string())
                .filter(|home| !home.is_empty()),
            // The same rule as the scan: switching the protection off does
            // not pay for a table it will not consult.
            mounts: match mount_timeout {
                Some(_) => Mounts::read(),
                None => Mounts::none(),
            },
            mount_timeout,
            probe: probe_mount,
            progress: Arc::default(),
        }
    }
}

/// What reading the databases is doing, for a progress line (invariant 8).
#[derive(Debug, Default)]
pub struct LoadProgress {
    /// Paths read out of the databases so far.
    pub listed: AtomicU64,
    /// What is being waited on that no counter can show moving: `rpm`, or a
    /// mount point being asked whether it is alive.
    pub waiting_on: Mutex<Option<(String, std::time::Instant)>>,
}

/// For the error that says none was found: what this build looks for.
pub fn where_looked() -> String {
    // Receipts are only read on macOS, so only named there.
    let receipts = match cfg!(target_os = "macos") {
        true => {
            "macOS installer receipts (/var/db/receipts, \
                 /Library/Apple/System/Library/Receipts, ~/Library/Receipts), "
        }
        false => "",
    };
    format!(
        "dpkg (/var/lib/dpkg/info), rpm (/usr/lib/sysimage/rpm, /var/lib/rpm), pacman \
         (/var/lib/pacman/local), apk (/lib/apk/db/installed), {receipts}and Homebrew \
         (/opt/homebrew, /usr/local, /home/linuxbrew/.linuxbrew)"
    )
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Claim {
    owner: PackageId,
    /// More than one package lists this path. The bytes still go to `owner`
    /// alone, so the per-package figures add up to the owned total.
    shared: bool,
}

/// Homebrew owns by position, not by list: everything under `Cellar/<formula>`
/// is that formula's, everything under `Caskroom/<cask>` that cask's.
///
/// On a scope that ignores case, `root` and the names are folded, and so is
/// every path asked about: a link an updater wrote as `../cellar/Wget/…` is
/// the formula's on that filesystem, as it is to the kernel.
#[derive(Debug)]
pub(super) struct BrewPrefix {
    root: String,
    formulae: HashMap<String, PackageId>,
    casks: HashMap<String, PackageId>,
}

impl BrewPrefix {
    /// `rest` is a path relative to the prefix, folded as the names are.
    fn owner_of(&self, rest: &str) -> Option<PackageId> {
        let (area, tail) = rest.split_once('/')?;
        let name = tail.split('/').next()?;
        let names = match area {
            "Cellar" | "cellar" => &self.formulae,
            "Caskroom" | "caskroom" => &self.casks,
            _ => return None,
        };
        names.get(name).copied()
    }
}

/// The firmlinks of a macOS system, when the scope lies on the data volume
/// as `/System/Volumes/Data` spells it.
///
/// Every package database names the firmlinked spelling — a receipt installs
/// to `/Applications`, Homebrew lives in `/opt/homebrew` — and a scan of
/// `/System/Volumes/Data/Applications` finds the same files under the other
/// one. Neither `realpath` nor anything else on disk turns one into the
/// other, so the system's own list of firmlinks does (`/usr/share/firmlinks`).
///
/// **Only for a scope on the data volume.** A scan of `/` meets both
/// spellings, since the walk enters `/System/Volumes/Data` as well as
/// `/Applications`; translating there would credit each file twice. Without
/// the translation it is credited once, under the name the databases use,
/// and the second view counts where the scan put it — with the unowned.
#[derive(Debug)]
pub(super) struct Firmlinks {
    /// `<sysroot>/System/Volumes/Data`.
    data: String,
    root: String,
    /// Where the system shows a folder, relative to the root, and where it
    /// is on the data volume, relative to that. Usually the same.
    links: Vec<(String, String)>,
    /// How many paths were searched for in `links`, so a test can tell one
    /// search per scope from one per file.
    #[cfg(test)]
    pub(super) translated: std::cell::Cell<usize>,
}

impl Firmlinks {
    pub(super) fn new(root: &str, links: Vec<(String, String)>) -> Firmlinks {
        Firmlinks {
            data: join(root, "System/Volumes/Data"),
            root: root.to_string(),
            links,
            #[cfg(test)]
            translated: std::cell::Cell::new(0),
        }
    }

    /// `path` relative to the data volume as `/System/Volumes/Data` spells
    /// it — the empty string for the volume itself — or `None` off it.
    pub(super) fn data_rest<'p>(&self, path: &'p str) -> Option<&'p str> {
        match path == self.data {
            true => Some(""),
            false => below(path, &self.data),
        }
    }

    /// `path` as the databases spell it, when it lies in a firmlinked folder.
    pub(super) fn listed(&self, path: &str) -> Option<String> {
        let mut out = String::new();
        self.listed_into(path, &mut out).then_some(out)
    }

    /// [`listed`](Firmlinks::listed) into `out`, for a caller asking once per
    /// file: whether `path` was translated, and if so `out` holds it.
    pub(super) fn listed_into(&self, path: &str, out: &mut String) -> bool {
        #[cfg(test)]
        self.translated.set(self.translated.get() + 1);
        let Some(rest) = self.data_rest(path).filter(|rest| !rest.is_empty()) else {
            return false;
        };
        let found = self
            .links
            .iter()
            .filter_map(|(shown, at)| {
                let tail = match rest == at {
                    true => "",
                    false => rest.strip_prefix(at.as_str())?.strip_prefix('/')?,
                };
                Some((shown, tail, at.len()))
            })
            // `System/Library/Caches` is firmlinked on its own; the longest
            // match is the folder the path is really in.
            .max_by_key(|&(_, _, len)| len)
            .map(|(shown, tail, _)| (shown, tail));
        let Some((shown, tail)) = found else {
            return false;
        };
        out.clear();
        out.push_str(&self.root);
        for part in [shown.as_str(), tail] {
            if part.is_empty() {
                continue;
            }
            if !out.ends_with('/') {
                out.push('/');
            }
            out.push_str(part);
        }
        true
    }

    /// Whether `path`, on the data volume, holds firmlinked folders without
    /// being inside one: the volume itself, or `…/Data/System`.
    pub(super) fn holds_links(&self, path: &str) -> bool {
        let Some(rest) = self.data_rest(path) else {
            return false;
        };
        rest.is_empty() || self.links.iter().any(|(_, at)| below(at, rest).is_some())
    }
}

/// Every owned path under one root, keyed by its canonical absolute path.
///
/// **A path claimed by more than one package is credited to exactly one**:
/// the first by manager in [`Manager`] order, then by package name. That makes
/// the answer independent of the order the databases happened to be read in,
/// and keeps the per-package figures summing to the owned total — splitting a
/// file's bytes between its claimants would make every figure a fraction of a
/// file. How much was contested is reported beside it, so the choice is never
/// silent; and a single-file lookup names every claimant.
#[derive(Debug)]
pub struct Ownership {
    packages: Vec<Package>,
    paths: HashMap<Box<str>, Claim>,
    /// The claimants after the first, for the few paths that have them.
    also: HashMap<Box<str>, Vec<PackageId>>,
    brew: Vec<BrewPrefix>,
    databases: Vec<Database>,
    /// Paths the databases named, before the scope filter.
    listed: u64,
    /// Directories looked up on disk to resolve what the databases list.
    looked_up: u64,
    /// Entries that could not be used — an unreadable list, a pacman
    /// directory without `files` — and the first few of them by name.
    damaged: u64,
    damaged_samples: Vec<(PathBuf, String)>,
    /// Mount points that did not answer, and below which nothing was read.
    unanswered: Vec<PathBuf>,
    /// Set when the scope is on macOS's data volume, spelled through
    /// `/System/Volumes/Data`: the paths above are keyed the other way.
    firmlinks: Option<Firmlinks>,
    /// Whether the scope's filesystem ignores case, so the keys above are
    /// folded (see [`fold_case`]).
    fold: bool,
    /// Every path the resolver looked up, so a test can say what it did not.
    #[cfg(test)]
    touched: Vec<PathBuf>,
}

impl Ownership {
    /// Read every database found under `sources`, keeping only the paths at or
    /// below `scope` — a canonical absolute path. A scan of `/opt` has no use
    /// for the other 400,000 entries of a Debian desktop, and does not look at
    /// the folders they name either.
    ///
    /// Never an error: a database that cannot be read is in [`databases`]
    /// with the reason, a damaged entry in [`damaged`], a mount that did not
    /// answer in [`unanswered`].
    ///
    /// [`databases`]: Ownership::databases
    /// [`damaged`]: Ownership::damaged
    /// [`unanswered`]: Ownership::unanswered
    pub fn load(sources: &Sources, scope: &Path) -> Ownership {
        load::load(sources, scope)
    }

    fn empty() -> Ownership {
        Ownership {
            packages: Vec::new(),
            paths: HashMap::new(),
            also: HashMap::new(),
            brew: Vec::new(),
            databases: Vec::new(),
            listed: 0,
            looked_up: 0,
            damaged: 0,
            damaged_samples: Vec::new(),
            unanswered: Vec::new(),
            firmlinks: None,
            fold: false,
            #[cfg(test)]
            touched: Vec::new(),
        }
    }

    /// `abs`, a path as the scan spells it, as the databases spell it — which
    /// is what [`owners`](Ownership::owners) is asked with. The same string
    /// unless the scope is macOS's data volume (see [`Firmlinks`]).
    pub fn listed_spelling<'a>(&self, abs: &'a str) -> Cow<'a, str> {
        match self.firmlinks.as_ref().and_then(|f| f.listed(abs)) {
            Some(listed) => Cow::Owned(listed),
            None => Cow::Borrowed(abs),
        }
    }

    /// How many entries could not be used, and the first few with the reason.
    pub fn damaged(&self) -> (u64, &[(PathBuf, String)]) {
        (self.damaged, &self.damaged_samples)
    }

    pub fn unanswered(&self) -> &[PathBuf] {
        &self.unanswered
    }

    /// Directories looked up on disk while resolving the lists.
    pub fn looked_up(&self) -> u64 {
        self.looked_up
    }

    pub fn databases(&self) -> &[Database] {
        &self.databases
    }

    pub fn package(&self, id: PackageId) -> &Package {
        &self.packages[id as usize]
    }

    /// How many paths the databases listed, and how many of them fell inside
    /// the scope and were kept.
    pub fn sizes(&self) -> (u64, usize) {
        (self.listed, self.paths.len())
    }

    /// Who the bytes at `abs` are credited to. The lists are asked before
    /// Homebrew's layout, which is [`Manager`] order anyway: a path a list
    /// names belongs to that list's package even inside a Cellar.
    ///
    /// `scratch` is where a folded key is built, reused from node to node.
    fn claim(&self, abs: &str, kind: EntryKind, scratch: &mut String) -> Option<Claim> {
        let key = fold_into(abs, self.fold, scratch);
        if let Some(&claim) = self.paths.get(key) {
            return Some(claim);
        }
        self.brew_owner(abs, key, kind).map(|owner| Claim {
            owner,
            shared: false,
        })
    }

    /// Every package that claims `abs`, the credited one first.
    pub fn owners(&self, abs: &str, kind: EntryKind) -> Vec<PackageId> {
        let mut scratch = String::new();
        let Some(claim) = self.claim(abs, kind, &mut scratch) else {
            return Vec::new();
        };
        let key = fold_into(abs, self.fold, &mut scratch);
        let mut rest = self.also.get(key).cloned().unwrap_or_default();
        // In the order the claim rule ranks them, not the order they were read.
        rest.sort_by(|&a, &b| self.package(a).cmp(self.package(b)));
        let mut all = vec![claim.owner];
        all.extend(rest);
        all
    }

    /// A Homebrew symlink outside the Cellar — `bin/wget`, `opt/wget` — belongs
    /// to the formula it points into. Read off the live link: the caller has
    /// already made sure the tree describes this machine.
    ///
    /// `key` is `abs` folded as the prefixes are; the link is read through
    /// `abs`, the spelling on disk.
    fn brew_owner(&self, abs: &str, key: &str, kind: EntryKind) -> Option<PackageId> {
        let prefix = self.brew.iter().find(|p| below(key, &p.root).is_some())?;
        let rest = below(key, &prefix.root)?;
        if let Some(owner) = prefix.owner_of(rest) {
            return Some(owner);
        }
        if kind != EntryKind::Symlink {
            return None;
        }
        let target = std::fs::read_link(abs).ok()?;
        let dir = abs.rsplit_once('/').map_or("/", |(dir, _)| dir);
        let resolved = parse::resolve_link(dir, &target.to_string_lossy());
        let resolved = fold_case(&resolved, self.fold);
        prefix.owner_of(below(&resolved, &prefix.root)?)
    }
}

/// `path` as the index keys it: lowercased when the scope's filesystem ignores
/// case, unchanged otherwise.
///
/// **Measured, not hypothetical.** Microsoft AutoUpdate rewrote 442 of Word's
/// files with new capitals — the receipt says `dropdownarrow_16x16x32.png`,
/// the disk `DropDownArrow_16x16x32.png`. On APFS's default, case-insensitive
/// volume both are the one file, `pkgutil --files` and `stat` find it, and an
/// exact match called its 2 MB unowned. Only matching is folded: every path
/// handed to the filesystem or compared with the mount table keeps its
/// spelling. `to_lowercase` is Unicode's lowercase mapping, close to but not
/// exactly APFS's own folding, and names that differ only in Unicode
/// normalisation are still told apart.
pub(super) fn fold_case(path: &str, fold: bool) -> Cow<'_, str> {
    if !needs_folding(path, fold) {
        return Cow::Borrowed(path);
    }
    let mut out = String::with_capacity(path.len());
    push_folded(&mut out, path);
    Cow::Owned(out)
}

/// [`fold_case`] into `scratch`, for a lookup that keeps nothing: one buffer
/// for a whole report instead of a string per node.
pub(super) fn fold_into<'a>(path: &'a str, fold: bool, scratch: &'a mut String) -> &'a str {
    if !needs_folding(path, fold) {
        return path;
    }
    scratch.clear();
    push_folded(scratch, path);
    scratch
}

fn needs_folding(path: &str, fold: bool) -> bool {
    fold && !path
        .bytes()
        .all(|b| b.is_ascii() && !b.is_ascii_uppercase())
}

/// Character by character, so the keys stored and the keys looked up are
/// folded the one way — `str::to_lowercase` treats a final sigma apart.
fn push_folded(out: &mut String, path: &str) {
    if path.is_ascii() {
        let start = out.len();
        out.push_str(path);
        out[start..].make_ascii_lowercase();
        return;
    }
    for c in path.chars() {
        out.extend(c.to_lowercase());
    }
}

/// `dir/name`, without doubling the slash after `/`; either side may be
/// empty, and then the other is the answer.
pub(super) fn join(dir: &str, name: &str) -> String {
    match (dir.is_empty(), name.is_empty(), dir.ends_with('/')) {
        (true, _, _) => name.to_string(),
        (_, true, _) => dir.to_string(),
        (_, _, true) => format!("{dir}{name}"),
        _ => format!("{dir}/{name}"),
    }
}

/// `path` relative to `dir`, when it lies strictly below it.
pub(super) fn below<'a>(path: &'a str, dir: &str) -> Option<&'a str> {
    let rest = path.strip_prefix(dir)?;
    match dir.ends_with('/') {
        true => Some(rest).filter(|r| !r.is_empty()),
        false => rest.strip_prefix('/').filter(|r| !r.is_empty()),
    }
}

// ---------------------------------------------------------------- report

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct Usage {
    pub size: u64,
    pub files: u64,
}

impl Usage {
    fn add(&mut self, size: u64) {
        self.size += size;
        self.files += 1;
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PackageUsage {
    pub name: String,
    pub manager: Manager,
    pub size: u64,
    pub files: u64,
}

/// A piece of the unowned bytes: a folder nothing in which belongs to a
/// package, or a single unowned file in a folder that also holds owned ones.
/// The pieces partition the unowned total — each unowned byte is in exactly
/// one — so a list of them is a list of where those bytes are, without a
/// folder and its own subfolder both claiming the same gigabyte.
#[derive(Debug, Clone, serde::Serialize)]
pub struct UnownedPart {
    /// Relative to the root; the empty string is the root itself.
    pub path: String,
    pub dir: bool,
    pub size: u64,
    pub files: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Report {
    pub basis: SizeBasis,
    pub total: Usage,
    pub owned: Usage,
    pub unowned: Usage,
    /// Owned files more than one package lists. Inside `owned`, not beside it.
    pub shared: Usage,
    /// Every package with at least one file under the root, largest first.
    pub packages: Vec<PackageUsage>,
    /// The largest pieces of `unowned`, largest first.
    pub unowned_parts: Vec<UnownedPart>,
}

/// Credit every file in `tree` to its package or to nobody.
///
/// Only files carry bytes: directories are skipped whoever lists them, and a
/// hardlink's second name carries none (invariant 3), so it adds a file and
/// nothing else to whichever side it falls on. The measure is the caller's
/// (invariant 6).
pub fn report(tree: &Tree, own: &Ownership, basis: SizeBasis, parts: usize) -> Report {
    let mut per_package = vec![Usage::default(); own.packages.len()];
    let mut total = Usage::default();
    let mut owned = Usage::default();
    let mut shared = Usage::default();
    // Per node: unowned bytes and files below it, and whether any owned file
    // is below it. Filled for files in the walk, summed upwards after it.
    let mut unowned = vec![Usage::default(); tree.len()];
    let mut has_owned = vec![false; tree.len()];

    // Spelled as the databases spell it (see `Firmlinks`). A scope inside one
    // firmlinked folder is translated here, once, and every path below
    // follows from it; only a scope holding several — the data volume itself
    // — is translated file by file, into a reused buffer.
    let scanned = tree.root_path().to_string_lossy();
    let per_file = own.firmlinks.as_ref().filter(|f| f.holds_links(&scanned));
    let root = match per_file {
        Some(_) => scanned.into_owned(),
        None => own.listed_spelling(&scanned).into_owned(),
    };
    let mut abs = root.clone();
    let separator = !root.ends_with('/');
    let mut scratch = String::new();
    let mut listed = String::new();
    tree.for_each_path(None, |id, rel| {
        let node = tree.node(id);
        if node.is_dir() {
            return;
        }
        abs.truncate(root.len());
        if !rel.is_empty() {
            if separator {
                abs.push('/');
            }
            abs.push_str(rel);
        }
        let size = node.measure(basis);
        total.add(size);
        let spelled = match per_file {
            Some(links) if links.listed_into(&abs, &mut listed) => listed.as_str(),
            _ => abs.as_str(),
        };
        let Some(claim) = own.claim(spelled, node.kind, &mut scratch) else {
            unowned[id as usize].add(size);
            return;
        };
        per_package[claim.owner as usize].add(size);
        owned.add(size);
        has_owned[id as usize] = true;
        if claim.shared {
            shared.add(size);
        }
    });

    // One reverse pass, as `TreeBuilder::aggregate` does: a child's index is
    // always greater than its parent's (invariant 2), so every node is
    // complete before it is added to the one above.
    for id in (0..tree.len()).rev() {
        let node = tree.node(id as NodeId);
        if !node.has_parent() {
            continue;
        }
        let parent = node.parent as usize;
        let below = unowned[id];
        unowned[parent].size += below.size;
        unowned[parent].files += below.files;
        has_owned[parent] |= has_owned[id];
    }

    // A piece is a node with unowned files and no owned one, whose parent has
    // an owned one — the highest point at which "none of this is packaged" is
    // still true. Found in a second walk so the paths come from
    // `for_each_path` and not from climbing to the root once per piece.
    let mut pieces = Vec::new();
    tree.for_each_path(None, |id, rel| {
        let node = tree.node(id);
        let here = unowned[id as usize];
        if here.files == 0 || has_owned[id as usize] {
            return;
        }
        if node.has_parent() && !has_owned[node.parent as usize] {
            return;
        }
        pieces.push(UnownedPart {
            path: rel.to_string(),
            dir: node.is_dir(),
            size: here.size,
            files: here.files,
        });
    });
    // By path within a size, never by id: two scans of one disk lay the arena
    // out differently (invariant 2), and the list must not change with it.
    pieces.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
    pieces.truncate(parts);

    let mut packages: Vec<PackageUsage> = per_package
        .iter()
        .enumerate()
        .filter(|(_, usage)| usage.files > 0)
        .map(|(id, usage)| {
            let package = own.package(id as PackageId);
            PackageUsage {
                name: package.name.clone(),
                manager: package.manager,
                size: usage.size,
                files: usage.files,
            }
        })
        .collect();
    packages.sort_by(|a, b| {
        b.size
            .cmp(&a.size)
            .then_with(|| a.manager.cmp(&b.manager))
            .then_with(|| a.name.cmp(&b.name))
    });

    Report {
        basis,
        total,
        unowned: Usage {
            size: total.size - owned.size,
            files: total.files - owned.files,
        },
        owned,
        shared,
        packages,
        unowned_parts: pieces,
    }
}

/// The canonical form of a path as named: its directory resolved, its last
/// component left alone, so a symlink is looked up as the link the package
/// ships rather than as whatever it points at.
pub fn canonical_name(path: &Path) -> Result<PathBuf> {
    let absolute =
        std::path::absolute(path).with_context(|| format!("cannot resolve {}", path.display()))?;
    let (Some(dir), Some(name)) = (absolute.parent(), absolute.file_name()) else {
        return absolute
            .canonicalize()
            .with_context(|| format!("path not found: {}", path.display()));
    };
    let dir = dir
        .canonicalize()
        .with_context(|| format!("path not found: {}", path.display()))?;
    Ok(dir.join(name))
}

// Unix only: every fixture is a merged-/usr layout built from symlinks, and no
// package database below exists anywhere else.
#[cfg(all(test, unix))]
mod tests;
