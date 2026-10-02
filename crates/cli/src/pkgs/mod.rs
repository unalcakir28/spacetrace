//! Which installed package each byte belongs to — and which belong to none.
//!
//! The databases are read straight off the disk wherever their format is a
//! plain file (dpkg, pacman, apk, Homebrew's directory layout). rpm's is not,
//! so `rpm` itself is asked, and only when it exists; when it does not, the
//! report says so instead of quietly calling every rpm file unowned.
//!
//! **Every path a database names is canonicalised before it is stored.** A
//! package list says `/bin/ls` and the scan finds `/usr/bin/ls`, because on a
//! merged-/usr system `/bin` is a symlink to `usr/bin` and the scanner does not
//! follow symlinks. Matching the strings as written left 396 of bookworm's
//! paths — `ls`, `bash`, the whole of `/lib` — looking unowned. Only the
//! directory part is resolved: the last component may itself be a symlink the
//! package ships, and that link is what the scan found.

pub mod parse;

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use spacetrace_scan_core::{EntryKind, NodeId, SizeBasis, Tree};

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

/// Where to look. `sysroot` is the filesystem the databases describe — `/` on
/// a real system, a temporary directory in a test, just as `dpkg --root`.
#[derive(Debug, Clone)]
pub struct Sources {
    pub sysroot: PathBuf,
    /// The `rpm` executable. A name to search `PATH` for, normally.
    pub rpm: OsString,
}

impl Sources {
    pub fn system() -> Sources {
        Sources {
            sysroot: PathBuf::from("/"),
            rpm: OsString::from("rpm"),
        }
    }
}

/// Where Homebrew lives when nobody moved it: Apple silicon, Intel macOS, and
/// Linux. Read from the layout rather than by running `brew --prefix`, which
/// costs a Ruby start-up and is not there for a user who is not the owner.
const BREW_PREFIXES: [&str; 3] = ["opt/homebrew", "usr/local", "home/linuxbrew/.linuxbrew"];

/// For the error that says none was found.
pub const WHERE_LOOKED: &str = "dpkg (/var/lib/dpkg/info), rpm (/usr/lib/sysimage/rpm, \
     /var/lib/rpm), pacman (/var/lib/pacman/local), apk (/lib/apk/db/installed) and \
     Homebrew (/opt/homebrew, /usr/local, /home/linuxbrew/.linuxbrew)";

/// The two places an rpm database sits: the current one, and the old one,
/// which Fedora keeps as a symlink to it.
const RPM_DATABASES: [&str; 2] = ["usr/lib/sysimage/rpm", "var/lib/rpm"];

#[derive(Debug, Clone, Copy)]
struct Claim {
    owner: PackageId,
    /// More than one package lists this path. The bytes still go to `owner`
    /// alone, so the per-package figures add up to the owned total.
    shared: bool,
}

/// Homebrew owns by position, not by list: everything under `Cellar/<formula>`
/// is that formula's, everything under `Caskroom/<cask>` that cask's.
#[derive(Debug)]
struct BrewPrefix {
    root: String,
    formulae: HashMap<String, PackageId>,
    casks: HashMap<String, PackageId>,
}

impl BrewPrefix {
    /// `rest` is a path relative to the prefix.
    fn owner_of(&self, rest: &str) -> Option<PackageId> {
        let (area, tail) = rest.split_once('/')?;
        let name = tail.split('/').next()?;
        match area {
            "Cellar" => self.formulae.get(name).copied(),
            "Caskroom" => self.casks.get(name).copied(),
            _ => None,
        }
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
}

impl Ownership {
    /// Read every database found under `sources`, keeping only the paths at or
    /// below `scope` — a canonical absolute path. A scan of `/opt` has no use
    /// for the other 400,000 entries of a Debian desktop.
    pub fn load(sources: &Sources, scope: &Path) -> Result<Ownership> {
        let mut loader = Loader {
            sysroot: &sources.sysroot,
            scope: scope.to_string_lossy().into_owned(),
            dirs: HashMap::new(),
            last_dir: None,
            by_name: HashMap::new(),
            own: Ownership {
                packages: Vec::new(),
                paths: HashMap::new(),
                also: HashMap::new(),
                brew: Vec::new(),
                databases: Vec::new(),
                listed: 0,
            },
        };
        loader.dpkg()?;
        loader.rpm(&sources.rpm)?;
        loader.pacman()?;
        loader.apk()?;
        loader.homebrew()?;
        Ok(loader.own)
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
    fn claim(&self, abs: &str, kind: EntryKind) -> Option<Claim> {
        if let Some(&claim) = self.paths.get(abs) {
            return Some(claim);
        }
        self.brew_owner(abs, kind).map(|owner| Claim {
            owner,
            shared: false,
        })
    }

    /// Every package that claims `abs`, the credited one first.
    pub fn owners(&self, abs: &str, kind: EntryKind) -> Vec<PackageId> {
        let Some(claim) = self.claim(abs, kind) else {
            return Vec::new();
        };
        let mut rest = self.also.get(abs).cloned().unwrap_or_default();
        // In the order the claim rule ranks them, not the order they were read.
        rest.sort_by(|&a, &b| self.package(a).cmp(self.package(b)));
        let mut all = vec![claim.owner];
        all.extend(rest);
        all
    }

    /// A Homebrew symlink outside the Cellar — `bin/wget`, `opt/wget` — belongs
    /// to the formula it points into. Read off the live link: the caller has
    /// already made sure the tree describes this machine.
    fn brew_owner(&self, abs: &str, kind: EntryKind) -> Option<PackageId> {
        let prefix = self.brew.iter().find(|p| below(abs, &p.root).is_some())?;
        let rest = below(abs, &prefix.root)?;
        if let Some(owner) = prefix.owner_of(rest) {
            return Some(owner);
        }
        if kind != EntryKind::Symlink {
            return None;
        }
        let target = std::fs::read_link(abs).ok()?;
        let dir = abs.rsplit_once('/').map_or("/", |(dir, _)| dir);
        let resolved = parse::resolve_link(dir, &target.to_string_lossy());
        prefix.owner_of(below(&resolved, &prefix.root)?)
    }
}

/// `path` relative to `dir`, when it lies strictly below it.
fn below<'a>(path: &'a str, dir: &str) -> Option<&'a str> {
    let rest = path.strip_prefix(dir)?;
    match dir.ends_with('/') {
        true => Some(rest).filter(|r| !r.is_empty()),
        false => rest.strip_prefix('/').filter(|r| !r.is_empty()),
    }
}

struct Loader<'a> {
    sysroot: &'a Path,
    scope: String,
    /// Listed directory → its canonical form, `None` when it is not on disk.
    /// Distinct directories number in the thousands where paths number in the
    /// hundreds of thousands, so each is resolved once.
    dirs: HashMap<String, Option<String>>,
    /// The last directory looked up. Lists are grouped by directory, so this
    /// answers most lookups without hashing anything.
    last_dir: Option<(String, Option<String>)>,
    /// Per manager, so a lookup by `&str` allocates nothing: rpm repeats the
    /// package name on every one of its lines.
    by_name: HashMap<Manager, HashMap<String, PackageId>>,
    own: Ownership,
}

impl Loader<'_> {
    fn package(&mut self, manager: Manager, name: &str) -> PackageId {
        let known = self.by_name.entry(manager).or_default();
        if let Some(&id) = known.get(name) {
            return id;
        }
        let id = self.own.packages.len() as PackageId;
        self.own.packages.push(Package {
            manager,
            name: name.to_string(),
        });
        known.insert(name.to_string(), id);
        id
    }

    /// The canonical directory a listed directory resolves to.
    fn canonical_dir(&mut self, listed: &str) -> Option<String> {
        if let Some((dir, canonical)) = &self.last_dir {
            if dir == listed {
                return canonical.clone();
            }
        }
        let canonical = match self.dirs.get(listed) {
            Some(known) => known.clone(),
            None => {
                // Not found, not a directory, not permitted: in each case the
                // scan cannot have found anything there either, as this user,
                // so the entry has nothing to match and is dropped.
                let resolved = self
                    .sysroot
                    .join(listed)
                    .canonicalize()
                    .ok()
                    .map(|p| p.to_string_lossy().into_owned());
                self.dirs.insert(listed.to_string(), resolved.clone());
                resolved
            }
        };
        self.last_dir = Some((listed.to_string(), canonical.clone()));
        canonical
    }

    /// Record that `pkg` lists `listed`, an absolute path or one relative to
    /// `/` as the database spells it.
    fn claim(&mut self, pkg: PackageId, listed: &str) {
        self.own.listed += 1;
        let listed = listed.trim_start_matches('/');
        let (dir, name) = listed.rsplit_once('/').unwrap_or(("", listed));
        if name.is_empty() || name == "." || name == ".." {
            return;
        }
        let Some(dir) = self.canonical_dir(dir) else {
            return;
        };
        let key = match dir.ends_with('/') {
            true => format!("{dir}{name}"),
            false => format!("{dir}/{name}"),
        };
        let in_scope = key == self.scope || below(&key, &self.scope).is_some();
        if !in_scope {
            return;
        }

        let entry = match self.own.paths.entry(key.into_boxed_str()) {
            Entry::Vacant(v) => {
                v.insert(Claim {
                    owner: pkg,
                    shared: false,
                });
                return;
            }
            Entry::Occupied(o) => o,
        };
        let current = entry.get().owner;
        if current == pkg {
            return;
        }
        let also = self.own.also.entry(entry.key().clone()).or_default();
        if also.contains(&pkg) {
            return;
        }
        let packages = &self.own.packages;
        let claim = entry.into_mut();
        claim.shared = true;
        if packages[pkg as usize] < packages[current as usize] {
            claim.owner = pkg;
            also.push(current);
        } else {
            also.push(pkg);
        }
    }

    fn found(&mut self, manager: Manager, location: PathBuf, state: State) {
        self.own.databases.push(Database {
            manager,
            location,
            state,
        });
    }

    fn dpkg(&mut self) -> Result<()> {
        let admin = self.sysroot.join("var/lib/dpkg");
        let info = admin.join("info");
        if !info.is_dir() {
            return Ok(());
        }

        let diversions = read_optional(&admin.join("diversions"))?;
        let diversions: HashMap<&str, (&str, Option<&str>)> = parse::dpkg_diversions(&diversions)
            .into_iter()
            .map(|d| (d.from, (d.to, d.by)))
            .collect();

        let mut lists = Vec::new();
        for entry in
            std::fs::read_dir(&info).with_context(|| format!("cannot list {}", info.display()))?
        {
            let entry = entry.with_context(|| format!("cannot list {}", info.display()))?;
            let file_name = entry.file_name().to_string_lossy().into_owned();
            if let Some(name) = parse::dpkg_package(&file_name) {
                lists.push((name.to_string(), entry.path()));
            }
        }
        // Sorted so that a run reads them in the same order as the last one;
        // the claim rule does not depend on it, but a debugger should not have
        // to wonder.
        lists.sort();

        for (name, path) in &lists {
            let text = read_text(path)?;
            let pkg = self.package(Manager::Dpkg, name);
            // A diversion names the package without its architecture.
            let base = name.split(':').next().unwrap_or(name);
            for listed in parse::dpkg_paths(&text) {
                // Another package's file at a diverted path is installed where
                // the diversion sends it; the diverting package keeps its own.
                let listed = match diversions.get(listed) {
                    Some(&(to, by)) if by != Some(base) => to,
                    _ => listed,
                };
                self.claim(pkg, listed);
            }
        }
        let packages = lists.len();
        self.found(Manager::Dpkg, info, State::Read { packages });
        Ok(())
    }

    fn rpm(&mut self, rpm: &OsString) -> Result<()> {
        let Some(location) = RPM_DATABASES
            .iter()
            .map(|db| self.sysroot.join(db))
            .find(|db| non_empty_dir(db))
        else {
            return Ok(());
        };

        let mut command = std::process::Command::new(rpm);
        command.args(["-qa", "--qf", parse::RPM_QUERY]);
        if self.sysroot != Path::new("/") {
            command.arg("--root").arg(self.sysroot);
        }
        let output = match command.output() {
            Ok(output) => output,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                let reason = format!(
                    "the database is binary and `{}` is not installed to read it",
                    rpm.to_string_lossy()
                );
                self.found(Manager::Rpm, location, State::Unreadable { reason });
                return Ok(());
            }
            Err(e) => {
                let reason = format!("cannot run `{}`: {e}", rpm.to_string_lossy());
                self.found(Manager::Rpm, location, State::Unreadable { reason });
                return Ok(());
            }
        };
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let reason = format!(
                "`{}` failed ({}): {}",
                rpm.to_string_lossy(),
                output.status,
                stderr.lines().next().unwrap_or("no message")
            );
            self.found(Manager::Rpm, location, State::Unreadable { reason });
            return Ok(());
        }

        let text = String::from_utf8_lossy(&output.stdout);
        let first = self.own.packages.len();
        for (name, path) in parse::rpm_lines(&text) {
            let pkg = self.package(Manager::Rpm, name);
            self.claim(pkg, path);
        }
        let packages = self.own.packages.len() - first;
        self.found(Manager::Rpm, location, State::Read { packages });
        Ok(())
    }

    fn pacman(&mut self) -> Result<()> {
        let local = self.sysroot.join("var/lib/pacman/local");
        if !local.is_dir() {
            return Ok(());
        }
        let mut entries = Vec::new();
        for entry in
            std::fs::read_dir(&local).with_context(|| format!("cannot list {}", local.display()))?
        {
            let path = entry
                .with_context(|| format!("cannot list {}", local.display()))?
                .path();
            // `ALPM_DB_VERSION` sits beside the package directories.
            if path.is_dir() {
                entries.push(path);
            }
        }
        entries.sort();

        for dir in &entries {
            let desc = read_text(&dir.join("desc"))?;
            let name = parse::pacman_name(&desc)
                .with_context(|| format!("no %NAME% in {}", dir.join("desc").display()))?;
            let pkg = self.package(Manager::Pacman, name);
            let files = read_text(&dir.join("files"))?;
            for listed in parse::pacman_files(&files) {
                self.claim(pkg, listed);
            }
        }
        let packages = entries.len();
        self.found(Manager::Pacman, local, State::Read { packages });
        Ok(())
    }

    fn apk(&mut self) -> Result<()> {
        let installed = self.sysroot.join("lib/apk/db/installed");
        if !installed.is_file() {
            return Ok(());
        }
        let text = read_text(&installed)?;
        let first = self.own.packages.len();
        for (name, path) in parse::apk_installed(&text) {
            let pkg = self.package(Manager::Apk, name);
            self.claim(pkg, &path);
        }
        let packages = self.own.packages.len() - first;
        self.found(Manager::Apk, installed, State::Read { packages });
        Ok(())
    }

    fn homebrew(&mut self) -> Result<()> {
        for prefix in BREW_PREFIXES {
            let prefix = self.sysroot.join(prefix);
            let cellar = prefix.join("Cellar");
            if !cellar.is_dir() {
                continue;
            }
            let root = prefix
                .canonicalize()
                .with_context(|| format!("cannot resolve {}", prefix.display()))?
                .to_string_lossy()
                .into_owned();
            if self.own.brew.iter().any(|known| known.root == root) {
                continue;
            }
            let formulae = self.brew_names(&cellar, Manager::Homebrew)?;
            let casks = self.brew_names(&prefix.join("Caskroom"), Manager::HomebrewCask)?;
            let packages = formulae.len() + casks.len();
            self.own.brew.push(BrewPrefix {
                root,
                formulae,
                casks,
            });
            self.found(Manager::Homebrew, prefix, State::Read { packages });
        }
        Ok(())
    }

    /// One package per directory in `Cellar` or `Caskroom`.
    fn brew_names(&mut self, area: &Path, manager: Manager) -> Result<HashMap<String, PackageId>> {
        let entries = match std::fs::read_dir(area) {
            Ok(entries) => entries,
            // No casks installed, no Caskroom.
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(HashMap::new()),
            Err(e) => return Err(e).with_context(|| format!("cannot list {}", area.display())),
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry.with_context(|| format!("cannot list {}", area.display()))?;
            if entry.path().is_dir() {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        names.sort();
        Ok(names
            .into_iter()
            .map(|name| {
                let id = self.package(manager, &name);
                (name, id)
            })
            .collect())
    }
}

/// The file's text, with any byte that is not UTF-8 replaced: the tree's names
/// went through the same conversion, so the two still compare equal.
fn read_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Like [`read_text`], with a missing file meaning empty.
fn read_optional(path: &Path) -> Result<String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
    }
}

fn non_empty_dir(path: &Path) -> bool {
    std::fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_some())
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

    let root = tree.root_path().to_string_lossy().into_owned();
    let mut abs = root.clone();
    let separator = !root.ends_with('/');
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
        let Some(claim) = own.claim(&abs, node.kind) else {
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
