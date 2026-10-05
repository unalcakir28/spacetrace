//! Each test builds a small system in a temporary directory — real files, real
//! symlinks, the databases in their real formats at their real places — and
//! points the loader at it the way `dpkg --root` would be.

use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use spacetrace_scan_core::{scan, ScanOptions, ScanProgress};

use super::*;

/// A sysroot: a temporary directory standing in for `/`.
struct System {
    dir: tempfile::TempDir,
}

impl System {
    fn new() -> System {
        System {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    /// The path the databases spell as `/<rel>`, which is how every list
    /// inside the sysroot has to name it.
    fn file(&self, rel: &str, bytes: usize) {
        let path = self.root().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![b'x'; bytes]).unwrap();
    }

    fn dir(&self, rel: &str) {
        std::fs::create_dir_all(self.root().join(rel)).unwrap();
    }

    fn link(&self, rel: &str, target: &str) {
        let path = self.root().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        symlink(target, path).unwrap();
    }

    /// The merged-/usr layout every current Debian, Arch and Fedora ships:
    /// the old top-level directories are relative symlinks into `/usr`.
    fn merge_usr(&self) {
        self.dir("usr/bin");
        self.dir("usr/lib");
        self.link("bin", "usr/bin");
        self.link("lib", "usr/lib");
        self.link("sbin", "usr/bin");
    }

    fn dpkg(&self, package: &str, paths: &[&str]) {
        let mut text = String::from("/.\n");
        for path in paths {
            text.push_str(path);
            text.push('\n');
        }
        self.file_text(&format!("var/lib/dpkg/info/{package}.list"), &text);
    }

    fn file_text(&self, rel: &str, text: &str) {
        let path = self.root().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn sources(&self) -> Sources {
        Sources {
            sysroot: self.root().to_path_buf(),
            // Never the host's: a test that found a real rpm would be reading
            // this machine's packages.
            rpm: OsString::from("spacetrace-test-no-such-rpm"),
            rpm_timeout: Duration::from_secs(5),
            receipt_home: None,
            // No mounts inside a temporary directory unless a test says so.
            mounts: Mounts::none(),
            mount_timeout: Some(Duration::from_secs(5)),
            probe: probe_mount,
            progress: Arc::default(),
        }
    }

    /// The canonical spelling of `rel`, which is how the mount table and the
    /// loader both spell it.
    fn canonical(&self, rel: &str) -> PathBuf {
        self.root().canonicalize().unwrap().join(rel)
    }

    /// Load for the scope `rel` with `sources`, without scanning.
    fn load(&self, sources: &Sources, rel: &str) -> Ownership {
        Ownership::load(sources, &self.canonical(rel))
    }

    /// Scan `rel` and report on it, the way `spacetrace pkgs` does.
    fn report(&self, rel: &str) -> (Report, Ownership, Tree) {
        let (tree, _) = scan(
            self.root().join(rel),
            ScanOptions::default(),
            Arc::new(ScanProgress::default()),
        )
        .unwrap();
        let own = Ownership::load(&self.sources(), tree.root_path());
        let report = report(&tree, &own, SizeBasis::Logical, 50);
        (report, own, tree)
    }
}

fn size_of(report: &Report, name: &str) -> Option<u64> {
    report
        .packages
        .iter()
        .find(|p| p.name == name)
        .map(|p| p.size)
}

/// The case that makes or breaks this on a modern system. bookworm's
/// coreutils lists `/bin/ls`; the scan of `/usr` finds `/usr/bin/ls`, because
/// `/bin` is a symlink and the scanner does not follow it. As strings they
/// never meet.
#[test]
fn a_file_listed_under_a_merged_directory_is_found_under_usr() {
    let sys = System::new();
    sys.merge_usr();
    sys.file("usr/bin/ls", 1000);
    sys.file("usr/lib/libc.so.6", 300);
    sys.dpkg("coreutils", &["/bin", "/bin/ls"]);
    sys.dpkg("libc6:arm64", &["/lib", "/lib/libc.so.6"]);

    let (report, _, tree) = sys.report("usr");

    assert_eq!(size_of(&report, "coreutils"), Some(1000));
    assert_eq!(size_of(&report, "libc6:arm64"), Some(300));
    assert_eq!(report.unowned.size, 0, "{:?}", report.unowned_parts);
    assert_eq!(report.owned.size, tree.total_size());
}

/// Arch goes one further: `/usr/sbin` is itself a symlink to `bin`, and a
/// package may still list a path through it.
#[test]
fn a_symlink_inside_usr_is_resolved_too() {
    let sys = System::new();
    sys.merge_usr();
    sys.link("usr/sbin", "bin");
    sys.file("usr/bin/ldconfig", 50);
    sys.file_text(
        "var/lib/pacman/local/glibc-2.42-1/desc",
        "%NAME%\nglibc\n\n%VERSION%\n2.42-1\n",
    );
    sys.file_text(
        "var/lib/pacman/local/glibc-2.42-1/files",
        "%FILES%\nusr/\nusr/sbin/\nusr/sbin/ldconfig\n\n%BACKUP%\n",
    );
    // The links themselves, as archlinux:latest's `filesystem` lists them:
    // without the trailing slash a directory would have.
    sys.file_text(
        "var/lib/pacman/local/filesystem-2025.10.12-1/desc",
        "%NAME%\nfilesystem\n",
    );
    sys.file_text(
        "var/lib/pacman/local/filesystem-2025.10.12-1/files",
        "%FILES%\nbin\nlib\nsbin\nusr/\nusr/bin/\nusr/sbin\n\n",
    );
    sys.file_text("var/lib/pacman/local/ALPM_DB_VERSION", "9\n");

    let (report, own, _) = sys.report("usr");

    assert_eq!(size_of(&report, "glibc"), Some(50));
    assert_eq!(size_of(&report, "filesystem"), Some(3), "the `bin` link");
    assert_eq!(report.unowned.files, 0, "{:?}", report.unowned_parts);
    assert!(matches!(
        own.databases()[0].state,
        State::Read { packages: 2 }
    ));
}

/// A package that ships a symlink owns the link, not what it points at.
/// Resolving the whole path would credit the link's bytes to the target's
/// package and leave the link itself unowned.
#[test]
fn a_shipped_symlink_is_owned_as_a_link() {
    let sys = System::new();
    sys.merge_usr();
    sys.file("usr/bin/python3.13", 4000);
    sys.link("usr/bin/python3", "python3.13");
    sys.dpkg("python3-minimal", &["/usr/bin/python3"]);
    sys.dpkg("python3.13-minimal", &["/usr/bin/python3.13"]);

    let (report, own, _) = sys.report("usr");

    assert_eq!(size_of(&report, "python3.13-minimal"), Some(4000));
    let link = own.owners(
        &canonical_name(&sys.root().join("bin/python3"))
            .unwrap()
            .to_string_lossy(),
        EntryKind::Symlink,
    );
    assert_eq!(
        link.iter()
            .map(|&id| own.package(id).name.as_str())
            .collect::<Vec<_>>(),
        ["python3-minimal"],
        "asked through /bin, answered for the link in /usr/bin"
    );
    assert_eq!(report.unowned.files, 0);
}

/// The other half of the answer, and usually the interesting one.
#[test]
fn unowned_bytes_are_reported_as_pieces_that_add_up() {
    let sys = System::new();
    sys.merge_usr();
    sys.file("usr/bin/ls", 100);
    sys.file("usr/bin/my-script", 7);
    sys.file("usr/local/lib/node_modules/a/index.js", 2000);
    sys.file("usr/local/lib/node_modules/b/index.js", 3000);
    sys.dir("usr/local/empty");
    sys.dpkg("coreutils", &["/usr/bin/ls"]);

    let (report, _, tree) = sys.report("usr");

    assert_eq!(report.owned.size, 100);
    assert_eq!(report.unowned.size, 5007);
    assert_eq!(report.owned.size + report.unowned.size, tree.total_size());
    let parts: Vec<(&str, u64)> = report
        .unowned_parts
        .iter()
        .map(|p| (p.path.as_str(), p.size))
        .collect();
    // `local` whole, because nothing in it is packaged; `my-script` alone,
    // because its folder also holds `ls`. Not `local/lib` as well: the pieces
    // never overlap.
    assert_eq!(parts, [("local", 5000), ("bin/my-script", 7)]);
    let sum: u64 = report.unowned_parts.iter().map(|p| p.size).sum();
    assert_eq!(sum, report.unowned.size);
}

#[test]
fn everything_unowned_is_one_piece_the_root() {
    let sys = System::new();
    sys.file("home/me/a", 10);
    sys.file("home/me/b/c", 20);
    sys.dpkg("base-files", &["/etc/issue"]);

    let (report, _, _) = sys.report("home");

    assert_eq!(report.owned.files, 0);
    assert_eq!(report.unowned_parts.len(), 1);
    assert_eq!(report.unowned_parts[0].path, "");
    assert_eq!(report.unowned_parts[0].size, 30);
}

/// Two packages listing one file: the bytes go to one of them, chosen by
/// name and not by which list was read first, and the overlap is counted.
#[test]
fn a_contested_file_is_credited_once_and_the_overlap_is_said() {
    let sys = System::new();
    sys.file("usr/share/x/common", 500);
    sys.file("usr/share/x/only-b", 5);
    sys.dpkg(
        "b-pkg",
        &["/usr/share/x", "/usr/share/x/common", "/usr/share/x/only-b"],
    );
    sys.dpkg("a-pkg", &["/usr/share/x", "/usr/share/x/common"]);

    let (report, own, _) = sys.report("usr");

    assert_eq!(size_of(&report, "a-pkg"), Some(500));
    assert_eq!(size_of(&report, "b-pkg"), Some(5));
    assert_eq!(report.owned.size, 505, "counted once, not twice");
    assert_eq!(
        report.shared,
        Usage {
            size: 500,
            files: 1
        }
    );

    let common = canonical_name(&sys.root().join("usr/share/x/common")).unwrap();
    let names: Vec<_> = own
        .owners(&common.to_string_lossy(), EntryKind::File)
        .into_iter()
        .map(|id| own.package(id).name.clone())
        .collect();
    assert_eq!(names, ["a-pkg", "b-pkg"], "the lookup names both");
}

/// apk's database is in install order, not by name, so here the claimants do
/// arrive out of order — and the answer must not notice.
#[test]
fn the_credited_claimant_does_not_depend_on_read_order() {
    let sys = System::new();
    sys.file("etc/shared.conf", 40);
    sys.file_text(
        "lib/apk/db/installed",
        "P:zeta\nF:etc\nR:shared.conf\n\nP:mid\nF:etc\nR:shared.conf\n\n\
         P:alpha\nF:etc\nR:shared.conf\n",
    );

    let (report, own, _) = sys.report("etc");

    assert_eq!(size_of(&report, "alpha"), Some(40));
    assert_eq!(report.shared, Usage { size: 40, files: 1 });
    let path = canonical_name(&sys.root().join("etc/shared.conf")).unwrap();
    let names: Vec<_> = own
        .owners(&path.to_string_lossy(), EntryKind::File)
        .into_iter()
        .map(|id| own.package(id).name.clone())
        .collect();
    assert_eq!(names, ["alpha", "mid", "zeta"]);
}

/// The same path reached twice from one package — `/bin/ls` and
/// `/usr/bin/ls` in one list, as transitional Debian packages do — is one
/// claim, not a contest with itself.
#[test]
fn a_package_listing_one_file_twice_does_not_share_it_with_itself() {
    let sys = System::new();
    sys.merge_usr();
    sys.file("usr/bin/ls", 10);
    sys.dpkg("coreutils", &["/bin/ls", "/usr/bin/ls"]);

    let (report, _, _) = sys.report("usr");

    assert_eq!(size_of(&report, "coreutils"), Some(10));
    assert_eq!(report.shared, Usage::default());
}

/// bookworm's own diversion: dash moves anyone else's `/bin/sh` to
/// `/bin/sh.distrib` and keeps `/bin/sh` for itself.
#[test]
fn a_diverted_file_belongs_where_the_diversion_put_it() {
    let sys = System::new();
    sys.merge_usr();
    sys.file("usr/bin/sh", 30);
    sys.file("usr/bin/sh.distrib", 900);
    sys.dpkg("dash", &["/bin/sh"]);
    sys.dpkg("bash", &["/bin/sh"]);
    sys.file_text(
        "var/lib/dpkg/diversions",
        "/bin/sh\n/bin/sh.distrib\ndash\n",
    );

    let (report, _, _) = sys.report("usr");

    assert_eq!(size_of(&report, "dash"), Some(30));
    assert_eq!(size_of(&report, "bash"), Some(900));
    assert_eq!(report.shared, Usage::default(), "nobody contests /bin/sh");
}

#[test]
fn apk_paths_are_relative_to_the_root() {
    let sys = System::new();
    sys.file("bin/busybox", 800);
    sys.file("etc/motd", 3);
    sys.file_text(
        "lib/apk/db/installed",
        "C:Q1=\nP:busybox\nV:1.37.0-r30\nF:bin\nR:busybox\n\n\
         C:Q2=\nP:alpine-baselayout\nF:etc\nR:motd\nZ:Q1SLkS9hBidUbPwwrw+XR0Whv3ww8=\n",
    );

    let (report, own, _) = sys.report("");

    assert_eq!(size_of(&report, "busybox"), Some(800));
    assert_eq!(size_of(&report, "alpine-baselayout"), Some(3));
    assert!(own
        .databases()
        .iter()
        .any(|d| d.manager == Manager::Apk && matches!(d.state, State::Read { packages: 2 })));
}

/// A database that is there and cannot be read must not look like a system
/// whose files are all unowned.
#[test]
fn an_rpm_database_without_rpm_is_reported_as_unreadable() {
    let sys = System::new();
    sys.file("usr/bin/bash", 100);
    sys.file_text("var/lib/rpm/rpmdb.sqlite", "not really");

    let (report, own, _) = sys.report("usr");

    let rpm = own
        .databases()
        .iter()
        .find(|d| d.manager == Manager::Rpm)
        .expect("the database is found even though it cannot be read");
    let State::Unreadable { reason } = &rpm.state else {
        panic!("{:?}", rpm.state);
    };
    assert!(reason.contains("not installed"), "{reason}");
    assert_eq!(report.unowned.size, 100);
}

/// No rpm database, no rpm entry — `rpm` the tool installed on Debian is not
/// an rpm system.
#[test]
fn no_database_means_no_entry() {
    let sys = System::new();
    sys.dir("var/lib/rpm");
    sys.file("usr/bin/bash", 1);
    let (_, own, _) = sys.report("usr");
    assert!(own.databases().is_empty(), "{:?}", own.databases());
}

#[test]
fn homebrew_owns_by_position_and_its_links_by_target() {
    let sys = System::new();
    let brew = "opt/homebrew";
    sys.file(&format!("{brew}/Cellar/wget/1.25.0/bin/wget"), 600);
    sys.file(
        &format!("{brew}/Cellar/wget/1.25.0/INSTALL_RECEIPT.json"),
        40,
    );
    sys.link(
        &format!("{brew}/bin/wget"),
        "../Cellar/wget/1.25.0/bin/wget",
    );
    sys.link(&format!("{brew}/opt/wget"), "../Cellar/wget/1.25.0");
    sys.file(
        &format!("{brew}/Caskroom/firefox/140.0/firefox.wrapper.sh"),
        70,
    );
    // A database a formula's service created: data, not the package.
    sys.file(&format!("{brew}/var/postgresql@17/base/1"), 8000);
    // A link brew did not make, pointing nowhere packaged.
    sys.link(&format!("{brew}/bin/mine"), "/usr/local/mine");

    let (report, _, tree) = sys.report(brew);

    let wget = report.packages.iter().find(|p| p.name == "wget").unwrap();
    assert_eq!(wget.manager, Manager::Homebrew);
    assert_eq!(wget.files, 4, "two files and two links");
    let link_bytes = tree.total_size() - 600 - 40 - 70 - 8000;
    let mine_bytes = "/usr/local/mine".len() as u64;
    assert_eq!(wget.size, 640 + link_bytes - mine_bytes);
    let cask = report
        .packages
        .iter()
        .find(|p| p.name == "firefox")
        .unwrap();
    assert_eq!((cask.manager, cask.size), (Manager::HomebrewCask, 70));
    let parts: Vec<&str> = report
        .unowned_parts
        .iter()
        .map(|p| p.path.as_str())
        .collect();
    assert_eq!(parts, ["var", "bin/mine"]);
}

/// On a filesystem that ignores case, a link an updater wrote as
/// `../cellar/wget/…` reaches the formula `Wget` as the kernel does, and is
/// the formula's. Where case is kept the link points nowhere and stays
/// unowned; the formula's own file is owned either way.
#[test]
fn homebrew_compares_folded_where_the_filesystem_ignores_case() {
    let sys = System::new();
    let brew = "opt/homebrew";
    sys.file(&format!("{brew}/Cellar/Wget/1.25.0/bin/wget"), 600);
    let target = "../cellar/wget/1.25.0/bin/wget";
    sys.link(&format!("{brew}/bin/wget"), target);
    let fold = super::load::ignores_case(sys.root());

    let (report, _, _) = sys.report(brew);

    let link = target.len() as u64;
    let expected = if fold { 600 + link } else { 600 };
    assert_eq!(size_of(&report, "Wget"), Some(expected), "folding: {fold}");
}

/// A scan of one subdirectory keeps only that subdirectory's paths: the
/// memory follows the question.
#[test]
fn only_paths_under_the_root_are_kept() {
    let sys = System::new();
    sys.merge_usr();
    sys.file("usr/bin/ls", 1);
    sys.file("usr/share/doc/coreutils/README", 1);
    sys.file("etc/issue", 1);
    sys.dpkg("coreutils", &["/bin/ls", "/usr/share/doc/coreutils/README"]);
    sys.dpkg("base-files", &["/etc/issue"]);

    let (report, own, _) = sys.report("usr/bin");

    assert_eq!(own.sizes(), (3, 1), "three listed, one under usr/bin");
    assert_eq!(size_of(&report, "coreutils"), Some(1));
    assert_eq!(size_of(&report, "base-files"), None);
}

/// `spacetrace pkgs <file>` loads with the file itself as the scope: of its
/// folder's paths only its own name counts, and is the one kept.
#[test]
fn a_scope_that_is_one_file_keeps_that_file() {
    let sys = System::new();
    sys.file("usr/bin/ls", 1);
    sys.file("usr/bin/cat", 1);
    sys.dpkg("coreutils", &["/usr/bin/ls", "/usr/bin/cat"]);

    let own = sys.load(&sys.sources(), "usr/bin/ls");

    assert_eq!(own.sizes(), (2, 1), "two listed, the scope kept");
    let ls = sys.canonical("usr/bin/ls");
    let owners = own.owners(&ls.to_string_lossy(), EntryKind::File);
    assert_eq!(owners.len(), 1);
    assert_eq!(own.package(owners[0]).name, "coreutils");
}

/// Invariant 3: a hardlinked file is counted once whichever name carries it,
/// and the total still matches the tree's.
#[test]
fn a_hardlinked_file_is_counted_once() {
    let sys = System::new();
    sys.file("usr/bin/perl5.40", 2000);
    std::fs::hard_link(
        sys.root().join("usr/bin/perl5.40"),
        sys.root().join("usr/bin/perl"),
    )
    .unwrap();
    sys.dpkg("perl-base", &["/usr/bin/perl", "/usr/bin/perl5.40"]);

    let (report, _, tree) = sys.report("usr");

    assert_eq!(tree.total_size(), 2000);
    assert_eq!(size_of(&report, "perl-base"), Some(2000));
    assert_eq!(
        report.packages[0].files, 2,
        "both names, one carrying bytes"
    );
}

/// Pieces of equal size are ordered by path, so the list is the same whatever
/// order the walk finished directories in.
#[test]
fn equal_pieces_are_ordered_by_path() {
    let sys = System::new();
    sys.file("usr/bin/ls", 1);
    for name in ["zeta", "alpha", "mid"] {
        sys.file(&format!("usr/{name}/f"), 10);
    }
    sys.dpkg("coreutils", &["/usr/bin/ls"]);

    let (report, _, _) = sys.report("usr");

    let parts: Vec<&str> = report
        .unowned_parts
        .iter()
        .map(|p| p.path.as_str())
        .collect();
    assert_eq!(parts, ["alpha", "mid", "zeta"]);
}

/// Homebrew on Linux beside dpkg: two databases, one report, each file to the
/// one that knows it.
#[test]
fn homebrew_on_linux_and_dpkg_report_together() {
    let sys = System::new();
    sys.merge_usr();
    sys.file("usr/bin/ls", 100);
    sys.dpkg("coreutils", &["/bin/ls"]);
    let brew = "home/linuxbrew/.linuxbrew";
    sys.file(&format!("{brew}/Cellar/hello/2.12.2/bin/hello"), 200);
    sys.link(
        &format!("{brew}/bin/hello"),
        "../Cellar/hello/2.12.2/bin/hello",
    );

    let (report, own, tree) = sys.report("");

    let managers: Vec<Manager> = own.databases().iter().map(|d| d.manager).collect();
    assert_eq!(managers, [Manager::Dpkg, Manager::Homebrew]);
    assert_eq!(size_of(&report, "coreutils"), Some(100));
    let hello = report.packages.iter().find(|p| p.name == "hello").unwrap();
    assert_eq!((hello.manager, hello.files), (Manager::Homebrew, 2));
    // Everything else under the sysroot — the dpkg list itself, the /bin
    // links nobody listed here — is unowned, and the figures still agree.
    assert_eq!(report.owned.size + report.unowned.size, tree.total_size());
}

// ------------------------------------------------- filesystems that hang

/// A mount whose server has gone, as the scanner's own tests stand one in:
/// there is no building a filesystem that hangs from a test, so the probe is
/// the seam. Each test that uses one has its own counter, since tests run in
/// parallel.
macro_rules! dead_probe {
    ($name:ident, $count:ident) => {
        static $count: AtomicUsize = AtomicUsize::new(0);
        fn $name(_: &Path, _: Duration) -> Option<std::io::Result<std::fs::Metadata>> {
            $count.fetch_add(1, Ordering::SeqCst);
            None
        }
    };
}

/// Invariant 7. A listed path below a mount point that the scope does not
/// sit on is never looked at — not the mount point, not anything under it, not
/// even the probe. Before this, every directory every database named was
/// canonicalised, so `pkgs /usr` stepped onto every mount the packages
/// mentioned, and a dead one hung it.
#[test]
fn a_mount_outside_the_scope_is_never_looked_at() {
    dead_probe!(never_answers, PROBES);
    let sys = System::new();
    sys.merge_usr();
    sys.file("usr/bin/ls", 10);
    sys.file("mnt/nfs/share/data", 5);
    sys.dpkg("coreutils", &["/bin/ls"]);
    sys.dpkg("nfs-data", &["/mnt/nfs/share", "/mnt/nfs/share/data"]);
    let nfs = sys.canonical("mnt/nfs");
    let mut sources = sys.sources();
    sources.mounts = Mounts::from_paths([nfs.clone()]);
    sources.probe = never_answers;

    let own = sys.load(&sources, "usr");

    let ls = sys.canonical("usr/bin/ls");
    let owners = own.owners(&ls.to_string_lossy(), EntryKind::File);
    assert_eq!(owners.len(), 1, "/bin/ls still resolves into /usr");
    let stepped_on: Vec<&PathBuf> = own.touched.iter().filter(|p| p.starts_with(&nfs)).collect();
    assert!(stepped_on.is_empty(), "looked at {stepped_on:?}");
    assert_eq!(PROBES.load(Ordering::SeqCst), 0, "not even asked");
    assert!(own.unanswered().is_empty());
}

/// The other half: a scope that reaches past the scope's own filesystem —
/// here the whole sysroot — asks the mount once, with the deadline, and goes
/// on without it.
#[test]
fn a_mount_inside_the_scope_is_asked_once_and_skipped_when_it_does_not_answer() {
    dead_probe!(never_answers, PROBES);
    let sys = System::new();
    sys.merge_usr();
    sys.file("usr/bin/ls", 10);
    sys.file("mnt/nfs/a/data", 5);
    sys.file("mnt/nfs/b/data", 5);
    sys.dpkg("coreutils", &["/bin/ls"]);
    sys.dpkg("nfs-data", &["/mnt/nfs/a/data", "/mnt/nfs/b/data"]);
    let nfs = sys.canonical("mnt/nfs");
    let mut sources = sys.sources();
    sources.mounts = Mounts::from_paths([nfs.clone()]);
    sources.probe = never_answers;

    let own = sys.load(&sources, "");

    assert_eq!(PROBES.load(Ordering::SeqCst), 1, "one probe for two paths");
    assert_eq!(own.unanswered(), std::slice::from_ref(&nfs));
    assert!(
        own.touched.iter().all(|p| !p.starts_with(&nfs)),
        "{:?}",
        own.touched
    );
    let ls = sys.canonical("usr/bin/ls");
    assert_eq!(own.owners(&ls.to_string_lossy(), EntryKind::File).len(), 1);
}

/// The databases are needed whatever the scope is, so a dead `/var` is asked
/// with the deadline and its database reported unread — not waited on.
#[test]
fn a_database_on_a_mount_that_does_not_answer_is_reported_unread() {
    dead_probe!(never_answers, PROBES);
    let sys = System::new();
    sys.merge_usr();
    sys.file("usr/bin/ls", 10);
    sys.dpkg("coreutils", &["/bin/ls"]);
    let mut sources = sys.sources();
    sources.mounts = Mounts::from_paths([sys.canonical("var")]);
    sources.probe = never_answers;

    let own = sys.load(&sources, "usr");

    let dpkg = &own.databases()[0];
    let State::Unreadable { reason } = &dpkg.state else {
        panic!("{:?}", dpkg.state);
    };
    assert!(reason.contains("did not answer in 5 s"), "{reason}");
    assert_eq!(PROBES.load(Ordering::SeqCst), 1);
}

/// `/home/linuxbrew` is only Homebrew's when the scope reaches it. A scan of
/// `/usr` used to probe it on every run, and an autofs `/home` whose server
/// was down hung every `pkgs` call.
#[test]
fn a_homebrew_prefix_unrelated_to_the_scope_is_not_looked_for() {
    dead_probe!(never_answers, PROBES);
    let sys = System::new();
    sys.merge_usr();
    sys.file("usr/bin/ls", 10);
    sys.dpkg("coreutils", &["/bin/ls"]);
    sys.dir("home/linuxbrew/.linuxbrew/Cellar/hello");
    let home = sys.canonical("home");
    let mut sources = sys.sources();
    sources.mounts = Mounts::from_paths([home.clone()]);
    sources.probe = never_answers;

    let own = sys.load(&sources, "usr");
    assert_eq!(PROBES.load(Ordering::SeqCst), 0);
    assert!(own.touched.iter().all(|p| !p.starts_with(&home)));
    assert!(own
        .databases()
        .iter()
        .all(|d| d.manager != Manager::Homebrew));

    // A scope that does include it asks, once, and moves on.
    let own = sys.load(&sources, "");
    assert_eq!(PROBES.load(Ordering::SeqCst), 1);
    assert_eq!(own.unanswered(), [home]);
}

/// Invariant 8. A stale BDB lock makes `rpm -qa` wait forever on RHEL 8; it
/// is stopped at the deadline and the database is reported unread. The
/// stand-in is a real `rpm` on disk that sleeps, started through a shell, so
/// the `sleep` outlives the shell that is killed and still holds the pipe —
/// waiting for the output to close would hang just the same.
#[test]
fn an_rpm_that_does_not_answer_is_stopped_at_the_deadline() {
    let sys = System::new();
    sys.file("usr/bin/bash", 100);
    sys.file_text("var/lib/rpm/rpmdb.sqlite", "locked");
    sys.file_text("fake/rpm", "#!/bin/sh\nsleep 30\n");
    let rpm = sys.root().join("fake/rpm");
    std::fs::set_permissions(&rpm, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut sources = sys.sources();
    sources.rpm = rpm.into_os_string();
    sources.rpm_timeout = Duration::from_secs(1);

    let started = Instant::now();
    let own = sys.load(&sources, "usr");
    let took = started.elapsed();

    assert!(took < Duration::from_secs(10), "waited {took:?}");
    let db = own
        .databases()
        .iter()
        .find(|d| d.manager == Manager::Rpm)
        .unwrap();
    let State::Unreadable { reason } = &db.state else {
        panic!("{:?}", db.state);
    };
    assert!(reason.contains("did not answer in 1 s"), "{reason}");
}

/// One damaged entry costs that entry, counted and named, and the rest of
/// the database still counts. A directory where a list should be fails to
/// read for root as well, so this holds in a container too.
#[test]
fn a_damaged_entry_is_counted_and_the_rest_still_reads() {
    let sys = System::new();
    sys.merge_usr();
    sys.file("usr/bin/ls", 10);
    sys.file("usr/bin/pacman", 20);
    sys.file("usr/bin/grep", 30);
    sys.dpkg("coreutils", &["/bin/ls"]);
    sys.dir("var/lib/dpkg/info/broken.list");
    sys.file_text("var/lib/pacman/local/pacman-7.0-1/desc", "%NAME%\npacman\n");
    sys.file_text(
        "var/lib/pacman/local/pacman-7.0-1/files",
        "%FILES%\nusr/bin/pacman\n\n",
    );
    // No `files` at all, and a `desc` with no name.
    sys.file_text("var/lib/pacman/local/grep-3.12-1/desc", "%NAME%\ngrep\n");
    sys.file_text("var/lib/pacman/local/nameless-1-1/desc", "%VERSION%\n1-1\n");
    sys.file_text("var/lib/pacman/local/nameless-1-1/files", "%FILES%\n\n");

    let (report, own, _) = sys.report("usr");

    assert_eq!(size_of(&report, "coreutils"), Some(10));
    assert_eq!(size_of(&report, "pacman"), Some(20));
    assert_eq!(size_of(&report, "grep"), None, "its files are unknown");
    let (count, samples) = own.damaged();
    assert_eq!(count, 3, "{samples:?}");
    let named: Vec<String> = samples
        .iter()
        .map(|(path, _)| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert!(named.contains(&"broken.list".to_string()), "{named:?}");
    assert!(
        samples.iter().any(|(_, why)| why.contains("%NAME%")),
        "{samples:?}"
    );
    assert!(own
        .databases()
        .iter()
        .all(|d| matches!(d.state, State::Read { .. })));
}

// ------------------------------------------------------- macOS spellings

/// Lays down the two views of a macOS data volume: the folder as the system
/// shows it and the same folder under `System/Volumes/Data`. A firmlink
/// cannot be made in a test, so the second view is a copy — which is what a
/// firmlink looks like to everything that reads the disk.
fn both_views(sys: &System, rel: &str, bytes: usize) {
    sys.file(rel, bytes);
    sys.file(&format!("System/Volumes/Data/{rel}"), bytes);
}

/// The system's own list, as macOS 27 ships it, shortened.
fn firmlinks_file(sys: &System) {
    sys.file_text(
        "usr/share/firmlinks",
        "/Applications\tApplications\n/Library\tLibrary\n/opt\topt\n\
         /System/Library/Caches\tSystem/Library/Caches\n/usr/local\tusr/local\n",
    );
}

/// A scan of `/System/Volumes/Data/Applications` finds the files the
/// databases name under `/Applications`. `realpath` does not turn one into
/// the other, so without the firmlink table everything there was unowned.
#[test]
fn a_scan_through_the_data_volume_matches_the_firmlinked_names() {
    let sys = System::new();
    firmlinks_file(&sys);
    both_views(&sys, "Applications/Tiny.app/Contents/MacOS/Tiny", 700);
    both_views(&sys, "Applications/Mine.app/Contents/MacOS/Mine", 30);
    both_views(&sys, "usr/local/bin/tool", 90);
    sys.dpkg(
        "tiny",
        &[
            "/Applications/Tiny.app",
            "/Applications/Tiny.app/Contents/MacOS/Tiny",
        ],
    );
    sys.dpkg("tool", &["/usr/local/bin/tool"]);

    for i in 0..10 {
        both_views(
            &sys,
            &format!("Applications/Mine.app/Contents/Resources/r{i}"),
            1,
        );
    }
    let (report, own, _) = sys.report("System/Volumes/Data/Applications");
    assert_eq!(size_of(&report, "tiny"), Some(700));
    assert_eq!(report.unowned.size, 40);
    assert_eq!(own.sizes().1, 2, "the scope is translated, so it keeps 2");
    // Inside one firmlinked folder, the root is translated and each file's
    // path follows from it: the table is not searched per file.
    let translated = own.firmlinks.as_ref().unwrap().translated.get();
    assert!(translated < 12, "{translated} translations for 12 files");

    // The volume itself holds several firmlinked folders, and each matches.
    let (report, _, _) = sys.report("System/Volumes/Data");
    assert_eq!(size_of(&report, "tiny"), Some(700));
    assert_eq!(size_of(&report, "tool"), Some(90));

    // One file, asked through the data volume.
    let file = sys.canonical("System/Volumes/Data/usr/local/bin/tool");
    let own = sys.load(&sys.sources(), "System/Volumes/Data/usr/local/bin/tool");
    let owners = own.owners(
        &own.listed_spelling(&file.to_string_lossy()),
        EntryKind::File,
    );
    assert_eq!(owners.len(), 1);
}

/// A scan of `/` walks both views. The firmlinked one is credited, once; the
/// second is where the scan counted the bytes again, and stays unowned rather
/// than becoming a second claim on the same package.
#[test]
fn a_scan_of_the_root_credits_each_file_once() {
    let sys = System::new();
    firmlinks_file(&sys);
    both_views(&sys, "Applications/Tiny.app/Contents/MacOS/Tiny", 700);
    sys.dpkg("tiny", &["/Applications/Tiny.app/Contents/MacOS/Tiny"]);

    let (report, _, _) = sys.report("");
    let tiny = report.packages.iter().find(|p| p.name == "tiny").unwrap();
    assert_eq!((tiny.size, tiny.files), (700, 1));
}

/// Folding is decided by the filesystem: on one that ignores case — APFS as
/// macOS formats it — a list naming `dropdownarrow.png` owns the file an
/// updater renamed to `DropDownArrow.png`, as `stat` and `pkgutil` both say.
/// On one that keeps case they are two names, and the file is unowned.
#[test]
fn a_name_that_differs_only_in_case_matches_where_the_filesystem_ignores_case() {
    let sys = System::new();
    sys.file("opt/app/Resources/DropDownArrow.png", 64);
    sys.dpkg("app", &["/opt/app/resources/dropdownarrow.png"]);
    let fold = super::load::ignores_case(sys.root());

    let (report, _, _) = sys.report("opt");

    let expected = fold.then_some(64);
    assert_eq!(size_of(&report, "app"), expected, "folding: {fold}");
    #[cfg(target_os = "macos")]
    eprintln!("this temporary directory ignores case: {fold}");
}

// ---------------------------------------------------- installer receipts

/// Receipts are read on macOS only, and these tests make them with the
/// system's own `mkbom` and `plutil`, so a real BOM and a real binary plist
/// are what the loader reads.
#[cfg(target_os = "macos")]
mod receipts {
    use super::*;
    use std::process::Command;

    enum Item<'a> {
        File(usize),
        Link(&'a str),
        Dir,
    }

    impl System {
        /// Install a package the way `installer` leaves it: its files under
        /// `<volume>/<prefix>`, and a receipt — a BOM of exactly those
        /// files, made by `mkbom` from a staging copy, and a binary plist —
        /// in `<volume>/<receipts>`. `volume` is relative to the sysroot,
        /// empty for the boot volume.
        fn install(
            &self,
            volume: &str,
            receipts: &str,
            id: &str,
            prefix: &str,
            items: &[(&str, Item)],
        ) {
            let stage = self.root().join(".stage").join(id);
            // `/` is a real prefix, and joined as written it would replace the
            // sysroot and install onto the machine running the test.
            let location = self
                .root()
                .join(volume.trim_start_matches('/'))
                .join(prefix.trim_start_matches('/'));
            assert!(location.starts_with(self.root()), "{}", location.display());
            for (rel, item) in items {
                let installed = location.join(rel);
                assert!(installed.starts_with(self.root()));
                for at in [stage.join(rel), installed] {
                    std::fs::create_dir_all(at.parent().unwrap()).unwrap();
                    match item {
                        Item::File(bytes) => std::fs::write(&at, vec![b'x'; *bytes]).unwrap(),
                        Item::Link(target) => symlink(target, &at).unwrap(),
                        Item::Dir => std::fs::create_dir_all(&at).unwrap(),
                    }
                }
            }
            let dir = self.root().join(volume).join(receipts);
            std::fs::create_dir_all(&dir).unwrap();
            let made = Command::new("mkbom")
                .arg(&stage)
                .arg(dir.join(format!("{id}.bom")))
                .status()
                .unwrap();
            assert!(made.success());
            self.plist(&dir.join(format!("{id}.plist")), id, Some(prefix));
            std::fs::remove_dir_all(self.root().join(".stage")).unwrap();
        }

        /// [`install`](System::install) on the boot volume, the receipt in
        /// `installer`'s own folder.
        fn receipt(&self, id: &str, prefix: &str, items: &[(&str, Item)]) {
            self.install("", RECEIPTS, id, prefix, items);
        }

        fn plist(&self, at: &Path, id: &str, prefix: Option<&str>) {
            let prefix = prefix.map_or(String::new(), |p| {
                format!("<key>InstallPrefixPath</key><string>{p}</string>")
            });
            std::fs::write(
                at,
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\">\
                     <dict>{prefix}<key>PackageIdentifier</key><string>{id}</string>\
                     <key>PackageVersion</key><string>1.0</string></dict></plist>\n"
                ),
            )
            .unwrap();
            let converted = Command::new("plutil")
                .args(["-convert", "binary1"])
                .arg(at)
                .status()
                .unwrap();
            assert!(converted.success());
        }
    }

    const RECEIPTS: &str = "var/db/receipts";

    /// `pkgutil --volume <sysroot>` reads `Library/Receipts` there, not
    /// `var/db/receipts` (measured), so the oracle gets a copy.
    fn pkgutil_files(sys: &System, id: &str) -> Vec<String> {
        let mirror = sys.root().join("Library/Receipts");
        std::fs::create_dir_all(&mirror).unwrap();
        for ext in ["bom", "plist"] {
            let name = format!("{id}.{ext}");
            std::fs::copy(sys.root().join(RECEIPTS).join(&name), mirror.join(&name)).unwrap();
        }
        let out = Command::new("pkgutil")
            .arg("--volume")
            .arg(sys.root())
            .args(["--files", id])
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout)
            .unwrap()
            .split_terminator('\n')
            .map(str::to_string)
            .collect()
    }

    /// The three places a package lands — the volume root, `Applications`,
    /// and nowhere in particular (an empty prefix) — each credited with
    /// exactly what `pkgutil` lists for it, measured with `stat`. Folders the
    /// BOM lists carry nothing; the symlink carries its own size.
    #[test]
    fn each_receipt_owns_what_pkgutil_lists_relative_to_its_location() {
        let sys = System::new();
        sys.receipt(
            "org.example.tool",
            "/",
            &[
                ("usr", Item::Dir),
                ("usr/local", Item::Dir),
                ("usr/local/bin", Item::Dir),
                ("usr/local/lib/tool/core", Item::File(1000)),
                ("usr/local/bin/tool", Item::Link("../lib/tool/core")),
            ],
        );
        sys.receipt(
            "com.example.tiny",
            "Applications",
            &[
                ("Tiny App.app/Contents/MacOS/Tiny App", Item::File(700)),
                ("Tiny App.app/Contents/Info.plist", Item::File(70)),
            ],
        );
        sys.receipt(
            "com.example.nowhere",
            "",
            &[("Library/Example/data", Item::File(5))],
        );
        sys.file("Applications/Mine.app/Contents/MacOS/Mine", 3);

        let (report, own, tree) = sys.report("");

        for id in [
            "org.example.tool",
            "com.example.tiny",
            "com.example.nowhere",
        ] {
            let location = match id {
                "com.example.tiny" => "Applications",
                _ => "",
            };
            let (mut size, mut files) = (0, 0);
            for rel in pkgutil_files(&sys, id) {
                let meta = std::fs::symlink_metadata(sys.root().join(location).join(&rel)).unwrap();
                if !meta.is_dir() {
                    size += meta.len();
                    files += 1;
                }
            }
            let package = report.packages.iter().find(|p| p.name == id).unwrap();
            assert_eq!(package.manager, Manager::Pkgutil);
            assert_eq!((package.size, package.files), (size, files), "{id}");
        }
        assert_eq!(size_of(&report, "com.example.tiny"), Some(770));
        let link = "../lib/tool/core".len() as u64;
        assert_eq!(size_of(&report, "org.example.tool"), Some(1000 + link));
        assert!(report
            .unowned_parts
            .iter()
            .any(|p| p.path == "Applications/Mine.app" && p.size == 3));
        assert_eq!(report.owned.size + report.unowned.size, tree.total_size());
        let receipts: Vec<_> = own
            .databases()
            .iter()
            .filter(|d| d.manager == Manager::Pkgutil)
            .collect();
        assert_eq!(receipts.len(), 1);
        assert!(matches!(receipts[0].state, State::Read { packages: 3 }));
    }

    /// A scan of `/usr/local` has no use for a package installed into
    /// `Applications`: its BOM is not even read — this one is garbage, and
    /// is not reported as damaged. Nor for the part of one installed at `/`
    /// that lies in `opt`, whose paths are never built and still count as
    /// listed: eight paths, `usr` to `opt/x/b`.
    #[test]
    fn a_receipt_installed_elsewhere_is_not_read() {
        let sys = System::new();
        sys.receipt(
            "org.example.tool",
            "/",
            &[
                ("usr/local/bin/tool", Item::File(10)),
                ("opt/x/a", Item::File(1)),
                ("opt/x/b", Item::File(1)),
            ],
        );
        sys.dir("Applications");
        let dir = sys.root().join(RECEIPTS);
        std::fs::write(dir.join("com.example.app.bom"), b"garbage").unwrap();
        sys.plist(
            &dir.join("com.example.app.plist"),
            "com.example.app",
            Some("Applications"),
        );

        let (report, own, _) = sys.report("usr/local");
        assert_eq!(size_of(&report, "org.example.tool"), Some(10));
        assert_eq!(own.damaged().0, 0, "{:?}", own.damaged().1);
        assert_eq!(own.sizes().0, 8, "every path the receipt holds is listed");

        // The same garbage, inside the scope, is damage.
        let (_, own, _) = sys.report("");
        assert_eq!(own.damaged().0, 1, "{:?}", own.damaged().1);
    }

    /// One damaged receipt costs that receipt. A cut-off BOM, a BOM whose
    /// plist is missing (`pkgutil` says `location: (null)`), a plist that is
    /// not one, a plist with no install location and one naming no package
    /// are each counted and named; a plist with no BOM —
    /// `InstallHistory.plist` — is not a receipt at all; and the good receipt
    /// beside them still counts.
    #[test]
    fn a_damaged_receipt_is_counted_and_the_rest_still_reads() {
        let sys = System::new();
        sys.receipt(
            "org.example.good",
            "/",
            &[("usr/local/bin/good", Item::File(10))],
        );
        sys.receipt(
            "org.example.cut",
            "/",
            &[("usr/local/bin/cut", Item::File(20))],
        );
        let dir = sys.root().join(RECEIPTS);
        let bom = std::fs::read(dir.join("org.example.cut.bom")).unwrap();
        std::fs::write(dir.join("org.example.cut.bom"), &bom[..bom.len() / 2]).unwrap();
        std::fs::write(dir.join("org.example.orphan.bom"), &bom).unwrap();
        std::fs::write(dir.join("org.example.mangled.bom"), &bom).unwrap();
        std::fs::write(dir.join("org.example.mangled.plist"), b"bplist00 nonsense").unwrap();
        std::fs::write(dir.join("org.example.nowhere.bom"), &bom).unwrap();
        sys.plist(
            &dir.join("org.example.nowhere.plist"),
            "org.example.nowhere",
            None,
        );
        std::fs::write(dir.join("org.example.anonymous.bom"), &bom).unwrap();
        sys.plist(&dir.join("org.example.anonymous.plist"), "", Some("/"));
        sys.plist(&dir.join("InstallHistory.plist"), "history", Some("/"));

        let (report, own, _) = sys.report("usr");

        assert_eq!(size_of(&report, "org.example.good"), Some(10));
        assert_eq!(size_of(&report, "org.example.cut"), None);
        let (count, samples) = own.damaged();
        assert_eq!(count, 5, "{samples:?}");
        let named: Vec<String> = samples
            .iter()
            .map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        for name in [
            "org.example.cut.bom",
            "org.example.orphan.plist",
            "org.example.mangled.plist",
            "org.example.nowhere.plist",
            "org.example.anonymous.plist",
        ] {
            assert!(named.contains(&name.to_string()), "{name} in {named:?}");
        }
        let receipts = own
            .databases()
            .iter()
            .find(|d| d.manager == Manager::Pkgutil)
            .unwrap();
        assert!(matches!(receipts.state, State::Read { packages: 6 }));
    }

    /// A package installed for one user records itself in that home, its
    /// paths relative to the home — what `pkgutil --volume ~` reads.
    #[test]
    fn a_receipt_in_a_home_folder_is_relative_to_that_home() {
        let sys = System::new();
        sys.install(
            "Users/me",
            "Library/Receipts",
            "com.example.mine",
            "Applications",
            &[("Mine.app/Contents/MacOS/Mine", Item::File(40))],
        );
        let mut sources = sys.sources();
        sources.receipt_home = Some("Users/me".to_string());
        let (tree, _) = scan(
            sys.root().join("Users"),
            ScanOptions::default(),
            Arc::new(ScanProgress::default()),
        )
        .unwrap();

        let own = Ownership::load(&sources, tree.root_path());
        let report = report(&tree, &own, SizeBasis::Logical, 50);
        assert_eq!(size_of(&report, "com.example.mine"), Some(40));
        assert!(own
            .databases()
            .iter()
            .any(|d| d.location.ends_with("Users/me/Library/Receipts")));

        // Without the home in the list, the receipt is not looked for.
        let own = Ownership::load(&sys.sources(), tree.root_path());
        assert!(own
            .databases()
            .iter()
            .all(|d| d.manager != Manager::Pkgutil));
    }
}
