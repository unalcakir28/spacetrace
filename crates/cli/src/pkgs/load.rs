//! Reading the package databases off the disk.
//!
//! **Nothing here steps onto a filesystem that may not answer** (invariant 7).
//! A listed path is followed one component at a time, and the walk stops at
//! the first directory that is neither above the scope nor inside it: a scan
//! of `/usr` has to look at `/bin` — it is a link into `/usr` on a merged
//! system — and at `/var` long enough to see it is a plain directory, and at
//! nothing below either. A mount point that the scope does not sit on is never
//! looked up directly, because looking up a mount point is the first syscall
//! that blocks when its server has gone: one inside the scope is asked through
//! [`probe_mount`](spacetrace_scan_core::probe_mount) with the scan's own
//! deadline, one outside it is not asked at all. The databases themselves are
//! read the same way, so a dead `/var` costs one deadline and a sentence in the
//! report, not the whole command.
//!
//! **One damaged entry costs that entry.** A list that cannot be read, a
//! pacman directory with no `files`, is counted and sampled as a scan counts
//! unreadable paths, and the rest of the database still counts.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::{
    below, parse, BrewPrefix, Claim, Database, Manager, Ownership, PackageId, Sources, State,
};

/// Where Homebrew lives when nobody moved it: Apple silicon, Intel macOS, and
/// Linux. Read from the layout rather than by running `brew --prefix`, which
/// costs a Ruby start-up and is not there for a user who is not the owner.
const BREW_PREFIXES: [&str; 3] = ["opt/homebrew", "usr/local", "home/linuxbrew/.linuxbrew"];

/// The two places an rpm database sits: the current one, and the old one,
/// which Fedora keeps as a symlink to it.
const RPM_DATABASES: [&str; 2] = ["usr/lib/sysimage/rpm", "var/lib/rpm"];

/// How many symlinks one lookup may pass through, as the kernel's `ELOOP`.
const MAX_HOPS: u32 = 40;

/// How many damaged entries are kept by name; the rest are only counted.
const SAMPLES: usize = 5;

pub(super) fn load(sources: &Sources, scope: &Path) -> Ownership {
    let root = sources
        .sysroot
        .canonicalize()
        .unwrap_or_else(|_| sources.sysroot.clone());
    let scope = scope.to_string_lossy().into_owned();
    // The mount points the scope sits on. The scan has already stood on all
    // of them, so stepping onto them again proves nothing new can hang.
    let near: HashSet<PathBuf> = Path::new(&scope)
        .ancestors()
        .filter(|a| sources.mounts.contains(a))
        .map(Path::to_path_buf)
        .collect();
    let mut loader = Loader {
        sources,
        root: root.to_string_lossy().into_owned(),
        scope,
        near,
        verdicts: HashMap::new(),
        resolved: HashMap::new(),
        dirs: HashMap::new(),
        by_name: HashMap::new(),
        own: Ownership::empty(),
    };
    loader.dpkg();
    loader.rpm();
    loader.pacman();
    loader.apk();
    loader.homebrew();
    *sources
        .progress
        .waiting_on
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = None;
    loader.own
}

struct Loader<'a> {
    sources: &'a Sources,
    /// The canonical sysroot, `/` outside the tests.
    root: String,
    scope: String,
    near: HashSet<PathBuf>,
    /// Mount point → whether it answered the probe. Asked once each.
    verdicts: HashMap<PathBuf, bool>,
    /// Absolute path as spelled → where it really is, or `None` when it is not
    /// on disk or leads nowhere near the scope. Every prefix of every directory
    /// looked up lands here, so each is resolved once.
    resolved: HashMap<String, Option<String>>,
    /// Listed directory, as the database spells it → its resolution.
    dirs: HashMap<String, Option<String>>,
    /// Per manager, so a lookup by `&str` allocates nothing: rpm repeats the
    /// package name on every one of its lines.
    by_name: HashMap<Manager, HashMap<String, PackageId>>,
    own: Ownership,
}

impl Loader<'_> {
    /// A path under the sysroot, spelled from its canonical form so that it
    /// compares with the mount table.
    fn at(&self, rel: &str) -> PathBuf {
        Path::new(&self.root).join(rel)
    }

    // ------------------------------------------------------------- reach

    /// A mount point the scope does not sit on.
    fn is_foreign_mount(&self, path: &Path) -> bool {
        self.sources.mount_timeout.is_some()
            && self.sources.mounts.contains(path)
            && !self.near.contains(path)
    }

    /// Whether a mount point answers, asked once, on a thread that can be
    /// abandoned.
    fn answers(&mut self, mount: &Path) -> bool {
        if let Some(&verdict) = self.verdicts.get(mount) {
            return verdict;
        }
        let Some(limit) = self.sources.mount_timeout else {
            return true;
        };
        self.waiting(Some(format!("waiting for {}", mount.display())));
        // An answer that is an error — permission denied — still answered.
        let verdict = (self.sources.probe)(mount, limit).is_some();
        self.waiting(None);
        if !verdict {
            self.own.unanswered.push(mount.to_path_buf());
        }
        self.verdicts.insert(mount.to_path_buf(), verdict);
        verdict
    }

    /// Whether `path` can be read without stepping onto a mount that did not
    /// answer. For the databases themselves, which are needed whatever the
    /// scope is.
    fn reachable(&mut self, path: &Path) -> bool {
        let mounts: Vec<PathBuf> = path
            .ancestors()
            .filter(|a| self.is_foreign_mount(a))
            .map(Path::to_path_buf)
            .collect();
        // Outermost first: a dead parent mount makes the inner one moot.
        mounts.iter().rev().all(|m| self.answers(m))
    }

    fn waiting(&self, what: Option<String>) {
        *self
            .sources
            .progress
            .waiting_on
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = what.map(|what| (what, Instant::now()));
    }

    /// `/usr` is related to a scope of `/usr/bin` (above it) and to one of `/`
    /// (inside it); `/var` is related to neither.
    fn related(&self, path: &str) -> bool {
        path == self.scope
            || below(path, &self.scope).is_some()
            || below(&self.scope, path).is_some()
    }

    /// Where an absolute directory path really is, following symlinks one
    /// component at a time, as `realpath` would — except that it gives up as
    /// soon as the answer cannot be near the scope, and never looks up a
    /// foreign mount point directly.
    fn resolve(&mut self, path: &str, hops: u32) -> Option<String> {
        if path == self.root {
            return Some(self.root.clone());
        }
        if let Some(known) = self.resolved.get(path) {
            return known.clone();
        }
        let resolved = self.resolve_uncached(path, hops);
        self.resolved.insert(path.to_string(), resolved.clone());
        resolved
    }

    fn resolve_uncached(&mut self, path: &str, hops: u32) -> Option<String> {
        if hops > MAX_HOPS {
            return None;
        }
        let (parent, name) = path.rsplit_once('/')?;
        let parent = match parent.len() < self.root.len() {
            true => self.root.clone(),
            false => parent.to_string(),
        };
        let base = self.resolve(&parent, hops)?;
        match name {
            "" | "." => return Some(base),
            ".." => {
                let up = base.rsplit_once('/').map_or("", |(up, _)| up);
                return Some(match up.len() < self.root.len() {
                    true => self.root.clone(),
                    false => up.to_string(),
                });
            }
            _ => {}
        }
        let here = join(&base, name);

        if self.is_foreign_mount(Path::new(&here)) {
            // A mount point is a directory, never a link, so whether it can
            // matter is known without asking it anything.
            if !self.related(&here) {
                return None;
            }
            return self.answers(Path::new(&here)).then_some(here);
        }

        self.own.looked_up += 1;
        #[cfg(test)]
        self.own.touched.push(PathBuf::from(&here));
        let meta = std::fs::symlink_metadata(&here).ok()?;
        if meta.file_type().is_symlink() {
            // Followed whether or not it looks related: this is the step that
            // takes `/bin` into `/usr`.
            let target = std::fs::read_link(&here).ok()?;
            let target = target.to_string_lossy();
            let next = match target.strip_prefix('/') {
                Some(absolute) => parse::resolve_link(&self.root, absolute),
                None => parse::resolve_link(&base, &target),
            };
            return self.resolve(&next, hops + 1);
        }
        if !meta.is_dir() || !self.related(&here) {
            return None;
        }
        Some(here)
    }

    // ------------------------------------------------------------ claims

    fn package(&mut self, manager: Manager, name: &str) -> PackageId {
        let known = self.by_name.entry(manager).or_default();
        if let Some(&id) = known.get(name) {
            return id;
        }
        let id = self.own.packages.len() as PackageId;
        self.own.packages.push(super::Package {
            manager,
            name: name.to_string(),
        });
        known.insert(name.to_string(), id);
        id
    }

    /// The real directory a listed one stands for, if anything in it can be
    /// inside the scope.
    fn listed_dir(&mut self, listed: &str) -> Option<String> {
        if let Some(known) = self.dirs.get(listed) {
            return known.clone();
        }
        let spelled = match listed.is_empty() {
            true => self.root.clone(),
            false => join(&self.root, listed),
        };
        let resolved = self.resolve(&spelled, 0);
        self.dirs.insert(listed.to_string(), resolved.clone());
        resolved
    }

    /// Record that `pkg` lists `listed`, an absolute path or one relative to
    /// `/` as the database spells it.
    fn claim(&mut self, pkg: PackageId, listed: &str) {
        self.own.listed += 1;
        self.sources.progress.listed.fetch_add(1, Ordering::Relaxed);
        let listed = listed.trim_start_matches('/');
        let (dir, name) = listed.rsplit_once('/').unwrap_or(("", listed));
        if name.is_empty() || name == "." || name == ".." {
            return;
        }
        let Some(dir) = self.listed_dir(dir) else {
            return;
        };
        let key = join(&dir, name);
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

    // ---------------------------------------------------------- outcomes

    fn found(&mut self, manager: Manager, location: PathBuf, state: State) {
        self.own.databases.push(Database {
            manager,
            location,
            state,
        });
    }

    /// A database whose filesystem did not answer: present, as far as anyone
    /// can tell, and unread.
    fn unanswered(&mut self, manager: Manager, location: PathBuf) {
        let reason = match self.sources.mount_timeout {
            Some(limit) => format!("its filesystem did not answer in {} s", limit.as_secs()),
            None => "its filesystem did not answer".to_string(),
        };
        self.found(manager, location, State::Unreadable { reason });
    }

    /// One entry of a database that could not be used. Counted, sampled, and
    /// the rest of the database goes on.
    fn damaged(&mut self, path: &Path, why: impl std::fmt::Display) {
        self.own.damaged += 1;
        if self.own.damaged_samples.len() < SAMPLES {
            self.own
                .damaged_samples
                .push((path.to_path_buf(), why.to_string()));
        }
    }

    /// A database directory's entries, sorted so that a run reads them in the
    /// same order as the last one. `Err` is the reason for the whole database.
    fn entries(&mut self, dir: &Path) -> Result<Vec<PathBuf>, String> {
        let listing = std::fs::read_dir(dir).map_err(|e| format!("cannot list it: {e}"))?;
        let mut paths = Vec::new();
        for entry in listing {
            match entry {
                Ok(entry) => paths.push(entry.path()),
                Err(e) => self.damaged(dir, format!("an entry could not be listed: {e}")),
            }
        }
        paths.sort();
        Ok(paths)
    }

    // --------------------------------------------------------- databases

    fn dpkg(&mut self) {
        let admin = self.at("var/lib/dpkg");
        let info = admin.join("info");
        if !self.reachable(&info) {
            return self.unanswered(Manager::Dpkg, info);
        }
        if !info.is_dir() {
            return;
        }

        let diversions_path = admin.join("diversions");
        let diversions = match read_optional(&diversions_path) {
            Ok(text) => text,
            Err(e) => {
                // Without it a diverted file is credited to the package it
                // was diverted from: a wrong owner for a handful of files,
                // not a reason to give up on the rest.
                self.damaged(&diversions_path, e);
                String::new()
            }
        };
        let diversions: HashMap<&str, (&str, Option<&str>)> = parse::dpkg_diversions(&diversions)
            .into_iter()
            .map(|d| (d.from, (d.to, d.by)))
            .collect();

        let entries = match self.entries(&info) {
            Ok(entries) => entries,
            Err(reason) => return self.found(Manager::Dpkg, info, State::Unreadable { reason }),
        };
        let mut packages = 0;
        for path in entries {
            let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned());
            let Some(name) = file_name.as_deref().and_then(parse::dpkg_package) else {
                continue;
            };
            packages += 1;
            let text = match read_text(&path) {
                Ok(text) => text,
                Err(e) => {
                    self.damaged(&path, e);
                    continue;
                }
            };
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
        self.found(Manager::Dpkg, info, State::Read { packages });
    }

    fn rpm(&mut self) {
        let mut location = None;
        for db in RPM_DATABASES {
            let db = self.at(db);
            if !self.reachable(&db) {
                return self.unanswered(Manager::Rpm, db);
            }
            if non_empty_dir(&db) {
                location = Some(db);
                break;
            }
        }
        let Some(location) = location else {
            return;
        };

        let rpm = self.sources.rpm.clone();
        let mut command = Command::new(&rpm);
        command.args(["-qa", "--qf", parse::RPM_QUERY]);
        if self.sources.sysroot != Path::new("/") {
            command.arg("--root").arg(&self.sources.sysroot);
        }
        let limit = self.sources.rpm_timeout;
        self.waiting(Some(format!("waiting for {}", rpm.to_string_lossy())));
        let ran = run_with_deadline(command, limit);
        self.waiting(None);
        let output = match ran {
            Ran::Finished(output) => output,
            Ran::NotFound => {
                let reason = format!(
                    "the database is binary and `{}` is not installed to read it",
                    rpm.to_string_lossy()
                );
                return self.found(Manager::Rpm, location, State::Unreadable { reason });
            }
            Ran::TimedOut => {
                // A stale lock from a killed transaction does this on RHEL 8:
                // rpm waits for it forever, and so would this.
                let reason = format!(
                    "`{}` did not answer in {} s and was stopped",
                    rpm.to_string_lossy(),
                    limit.as_secs()
                );
                return self.found(Manager::Rpm, location, State::Unreadable { reason });
            }
            Ran::Failed(e) => {
                let reason = format!("cannot run `{}`: {e}", rpm.to_string_lossy());
                return self.found(Manager::Rpm, location, State::Unreadable { reason });
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
            return self.found(Manager::Rpm, location, State::Unreadable { reason });
        }

        let text = String::from_utf8_lossy(&output.stdout);
        let first = self.own.packages.len();
        for (name, path) in parse::rpm_lines(&text) {
            let pkg = self.package(Manager::Rpm, name);
            self.claim(pkg, path);
        }
        let packages = self.own.packages.len() - first;
        self.found(Manager::Rpm, location, State::Read { packages });
    }

    fn pacman(&mut self) {
        let local = self.at("var/lib/pacman/local");
        if !self.reachable(&local) {
            return self.unanswered(Manager::Pacman, local);
        }
        if !local.is_dir() {
            return;
        }
        let entries = match self.entries(&local) {
            Ok(entries) => entries,
            Err(reason) => return self.found(Manager::Pacman, local, State::Unreadable { reason }),
        };
        let mut packages = 0;
        // `ALPM_DB_VERSION` sits beside the package directories.
        for dir in entries.into_iter().filter(|p| p.is_dir()) {
            packages += 1;
            let desc_path = dir.join("desc");
            let desc = match read_text(&desc_path) {
                Ok(desc) => desc,
                Err(e) => {
                    self.damaged(&desc_path, e);
                    continue;
                }
            };
            let Some(name) = parse::pacman_name(&desc) else {
                self.damaged(&desc_path, "no %NAME% in it");
                continue;
            };
            let pkg = self.package(Manager::Pacman, name);
            let files_path = dir.join("files");
            let files = match read_text(&files_path) {
                Ok(files) => files,
                Err(e) => {
                    self.damaged(&files_path, e);
                    continue;
                }
            };
            for listed in parse::pacman_files(&files) {
                self.claim(pkg, listed);
            }
        }
        self.found(Manager::Pacman, local, State::Read { packages });
    }

    fn apk(&mut self) {
        let installed = self.at("lib/apk/db/installed");
        if !self.reachable(&installed) {
            return self.unanswered(Manager::Apk, installed);
        }
        if !installed.is_file() {
            return;
        }
        // One file holds the whole database, so it is all or nothing.
        let text = match read_text(&installed) {
            Ok(text) => text,
            Err(e) => {
                let reason = e.to_string();
                return self.found(Manager::Apk, installed, State::Unreadable { reason });
            }
        };
        let first = self.own.packages.len();
        for (name, path) in parse::apk_installed(&text) {
            let pkg = self.package(Manager::Apk, name);
            self.claim(pkg, &path);
        }
        let packages = self.own.packages.len() - first;
        self.found(Manager::Apk, installed, State::Read { packages });
    }

    fn homebrew(&mut self) {
        for prefix in BREW_PREFIXES {
            let spelled = join(&self.root, prefix);
            // Resolved like a listed directory, so a prefix unrelated to the
            // scope — `/home/linuxbrew` for a scan of `/usr` — is never
            // looked at, and an unanswering `/home` is asked once, with a
            // deadline, only when the scope includes it.
            let Some(root) = self.resolve(&spelled, 0) else {
                continue;
            };
            if self.own.brew.iter().any(|known| known.root == root) {
                continue;
            }
            let prefix = PathBuf::from(&root);
            let cellar = prefix.join("Cellar");
            if !self.reachable(&cellar) || !cellar.is_dir() {
                continue;
            }
            let formulae = self.brew_names(&cellar, Manager::Homebrew);
            let casks = self.brew_names(&prefix.join("Caskroom"), Manager::HomebrewCask);
            let packages = formulae.len() + casks.len();
            self.own.brew.push(BrewPrefix {
                root,
                formulae,
                casks,
            });
            self.found(Manager::Homebrew, prefix, State::Read { packages });
        }
    }

    /// One package per directory in `Cellar` or `Caskroom`.
    fn brew_names(&mut self, area: &Path, manager: Manager) -> HashMap<String, PackageId> {
        // No casks installed, no Caskroom.
        if !area.exists() {
            return HashMap::new();
        }
        let entries = match self.entries(area) {
            Ok(entries) => entries,
            Err(reason) => {
                self.damaged(area, reason);
                return HashMap::new();
            }
        };
        entries
            .into_iter()
            .filter(|p| p.is_dir())
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .map(|name| {
                let id = self.package(manager, &name);
                (name, id)
            })
            .collect()
    }
}

/// `dir/name`, without doubling the slash after `/`.
fn join(dir: &str, name: &str) -> String {
    match dir.ends_with('/') {
        true => format!("{dir}{name}"),
        false => format!("{dir}/{name}"),
    }
}

enum Ran {
    Finished(std::process::Output),
    NotFound,
    TimedOut,
    Failed(std::io::Error),
}

/// Run `command` and wait at most `limit` for it, killing it past that.
///
/// The output is read on threads of its own: a child that fills a pipe nobody
/// empties never exits, and the deadline would then blame it for a stall that
/// was ours. They are not joined after a timeout — something the child left
/// behind may still hold the pipe — and simply finish whenever it closes.
fn run_with_deadline(mut command: Command, limit: Duration) -> Ran {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ran::NotFound,
        Err(e) => return Ran::Failed(e),
    };
    let (tx, rx) = mpsc::channel::<(bool, Vec<u8>)>();
    let pipes: [(bool, Option<Box<dyn Read + Send>>); 2] = [
        (
            true,
            child
                .stdout
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
        ),
        (
            false,
            child
                .stderr
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
        ),
    ];
    for (is_stdout, pipe) in pipes {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            let _ = tx.send((is_stdout, bytes));
        });
    }

    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Ran::TimedOut;
            }
            // Polled, because `wait` cannot be given a deadline and the child
            // has to stay here to be killed. Twenty milliseconds against a
            // query that takes about a second.
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Ran::Failed(e),
        }
    };

    let (mut stdout, mut stderr) = (None, None);
    while stdout.is_none() || stderr.is_none() {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok((true, bytes)) => stdout = Some(bytes),
            Ok((false, bytes)) => stderr = Some(bytes),
            Err(_) => return Ran::TimedOut,
        }
    }
    Ran::Finished(std::process::Output {
        status,
        stdout: stdout.unwrap_or_default(),
        stderr: stderr.unwrap_or_default(),
    })
}

/// The file's text, with any byte that is not UTF-8 replaced: the tree's names
/// went through the same conversion, so the two still compare equal.
fn read_text(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Like [`read_text`], with a missing file meaning empty.
fn read_optional(path: &Path) -> std::io::Result<String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e),
    }
}

fn non_empty_dir(path: &Path) -> bool {
    std::fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_some())
}
