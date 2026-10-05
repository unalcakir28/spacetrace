//! The package databases' own file formats, as pure functions over text.
//!
//! Nothing here touches the disk: reading the files and resolving the folders
//! they name happen in `load.rs`, crediting the bytes in `mod.rs`. Keeping the
//! formats apart is what lets every one of them be tested against text copied
//! off a real system, without that system.

/// The package a dpkg `.list` file describes, from its file name.
///
/// A `Multi-Arch: same` package carries its architecture (`libc6:arm64.list`),
/// and the name keeps it: two architectures of one package are two packages
/// with two file lists, and `dpkg -S` prints them the same way.
pub fn dpkg_package(file_name: &str) -> Option<&str> {
    file_name
        .strip_suffix(".list")
        .filter(|name| !name.is_empty())
}

/// The paths in a dpkg `.list`, one absolute path per line.
///
/// Directories are listed too and are not marked as such; they are filtered
/// out on the tree side, where the kind is known. `/.` is the first line of
/// every list and names the root itself. Lines are not trimmed, because a path
/// may end in a space.
pub fn dpkg_paths(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .filter(|line| line.starts_with('/') && *line != "/.")
}

/// One entry of `/var/lib/dpkg/diversions`.
#[derive(Debug, PartialEq, Eq)]
pub struct Diversion<'a> {
    pub from: &'a str,
    pub to: &'a str,
    /// The package that made the diversion, which keeps its own file at
    /// `from`. `None` for a local one (`:`), which diverts every package.
    pub by: Option<&'a str>,
}

/// `/var/lib/dpkg/diversions`: three lines per entry — the diverted path, where
/// it went, and who diverted it.
///
/// A trailing incomplete entry is dropped rather than guessed at; dpkg writes
/// the file whole, so one only appears if it was cut off.
pub fn dpkg_diversions(text: &str) -> Vec<Diversion<'_>> {
    let lines: Vec<&str> = text.lines().collect();
    lines
        .chunks_exact(3)
        .map(|entry| Diversion {
            from: entry[0],
            to: entry[1],
            by: (entry[2] != ":").then_some(entry[2]),
        })
        .collect()
}

/// The package name in a pacman `desc` file: the line after `%NAME%`.
///
/// Read from the file rather than from the directory name, because the
/// directory is `<name>-<version>-<release>` and a name may itself contain
/// dashes (`ca-certificates-mozilla-3.117-1`), so splitting it is a guess.
pub fn pacman_name(desc: &str) -> Option<&str> {
    let mut lines = desc.lines();
    lines.find(|line| *line == "%NAME%")?;
    lines.next().filter(|name| !name.is_empty())
}

/// The files in a pacman `files` file: the `%FILES%` section, relative to `/`.
///
/// Directories end in `/` and are skipped here, since only files carry bytes.
/// The section ends at a blank line; `%BACKUP%` follows it and lists the same
/// configuration files again with a checksum.
pub fn pacman_files(files: &str) -> impl Iterator<Item = &str> {
    files
        .lines()
        .skip_while(|line| *line != "%FILES%")
        .skip(1)
        .take_while(|line| !line.is_empty() && !line.starts_with('%'))
        .filter(|line| !line.ends_with('/'))
}

/// Every `(package, path)` in Alpine's `/lib/apk/db/installed`, paths relative
/// to `/`.
///
/// One block per package, separated by blank lines. `P:` names the package,
/// `F:` opens a folder and every `R:` after it is a file in that folder, until
/// the next `F:`. The other fields — checksums, permissions, dependencies — say
/// nothing about where a file is.
pub fn apk_installed(text: &str) -> Vec<(&str, String)> {
    let mut out = Vec::new();
    let mut package: Option<&str> = None;
    let mut folder = "";
    for line in text.lines() {
        if line.is_empty() {
            package = None;
            folder = "";
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        match key {
            "P" => package = Some(value),
            "F" => folder = value,
            "R" => {
                // A file before the package's name would be a corrupt block;
                // crediting it to the previous package would be a wrong answer.
                let Some(name) = package else { continue };
                let path = if folder.is_empty() {
                    value.to_string()
                } else {
                    format!("{folder}/{value}")
                };
                out.push((name, path));
            }
            _ => {}
        }
    }
    out
}

/// The query handed to `rpm`, one line per file: name, a tab, the path.
///
/// `%{=NAME}` and not `%{NAME}`: inside `[...]` rpm iterates arrays in step,
/// and a plain scalar there is an array of one, which rpm rejects with "array
/// iterator used with different sized arrays" for every package with more than
/// one file. The `=` repeats it.
pub const RPM_QUERY: &str = "[%{=NAME}\t%{FILENAMES}\n]";

/// The output of `rpm -qa --qf RPM_QUERY`.
pub fn rpm_lines(text: &str) -> impl Iterator<Item = (&str, &str)> {
    text.lines().filter_map(|line| {
        let (name, path) = line.split_once('\t')?;
        (!name.is_empty() && path.starts_with('/')).then_some((name, path))
    })
}

/// macOS's `/usr/share/firmlinks`: one per line, the folder as the system
/// shows it (absolute), a tab, and where it is on the data volume (relative to
/// `/System/Volumes/Data`). Both are returned relative, without slashes at
/// either end.
pub fn firmlinks(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (shown, at) = line.split_once('\t')?;
            let shown = shown.trim_matches('/');
            let at = at.trim_matches('/');
            (!shown.is_empty() && !at.is_empty()).then(|| (shown.to_string(), at.to_string()))
        })
        .collect()
}

/// Where a symlink points, resolved against the directory it sits in, without
/// touching the disk.
///
/// Lexical on purpose: Homebrew's links (`bin/wget -> ../Cellar/wget/1.25/bin/
/// wget`) are read to learn which formula they belong to, and following them
/// through the filesystem would answer for the target's target instead.
pub fn resolve_link(link_dir: &str, target: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let base = if target.starts_with('/') {
        ""
    } else {
        link_dir
    };
    for segment in base.split('/').chain(target.split('/')) {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            segment => parts.push(segment),
        }
    }
    format!("/{}", parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every fixture below is text copied from a real system on 2 October
    // 2026, shortened but not reworded: debian:stable (trixie) and
    // debian:bookworm, archlinux:latest, alpine:latest (3.24.1) and
    // fedora:latest (44).

    #[test]
    fn a_dpkg_list_names_its_package_and_keeps_the_architecture() {
        assert_eq!(dpkg_package("coreutils.list"), Some("coreutils"));
        assert_eq!(dpkg_package("libc6:arm64.list"), Some("libc6:arm64"));
        assert_eq!(dpkg_package("coreutils.md5sums"), None);
        assert_eq!(dpkg_package(".list"), None);
    }

    #[test]
    fn a_dpkg_list_yields_every_path_but_the_root() {
        let list = "/.\n/usr\n/usr/bin\n/usr/bin/[\n/usr/bin/arch\n/usr/share/doc/a b \n";
        let paths: Vec<&str> = dpkg_paths(list).collect();
        assert_eq!(
            paths,
            [
                "/usr",
                "/usr/bin",
                "/usr/bin/[",
                "/usr/bin/arch",
                "/usr/share/doc/a b "
            ],
            "the trailing space is part of the name"
        );
    }

    #[test]
    fn diversions_come_in_threes_and_a_colon_means_local() {
        // bookworm's own file, plus one local diversion and a cut-off tail.
        let text = "/usr/share/man/man1/sh.1.gz\n/usr/share/man/man1/sh.distrib.1.gz\ndash\n\
                    /bin/sh\n/bin/sh.distrib\ndash\n\
                    /etc/issue\n/etc/issue.orig\n:\n\
                    /usr/bin/half\n/usr/bin/half.d\n";
        let diversions = dpkg_diversions(text);
        assert_eq!(diversions.len(), 3, "the incomplete entry is dropped");
        assert_eq!(
            diversions[1],
            Diversion {
                from: "/bin/sh",
                to: "/bin/sh.distrib",
                by: Some("dash")
            }
        );
        assert_eq!(diversions[2].by, None);
    }

    #[test]
    fn pacman_takes_the_name_from_desc_not_from_the_directory() {
        let desc = "%NAME%\nca-certificates-mozilla\n\n%VERSION%\n3.117-1\n\n%BASE%\nnss\n";
        assert_eq!(pacman_name(desc), Some("ca-certificates-mozilla"));
        assert_eq!(pacman_name("%VERSION%\n1-1\n"), None);
    }

    #[test]
    fn pacman_files_are_the_files_section_without_directories_or_backups() {
        let files = "%FILES%\nusr/\nusr/bin/\nusr/bin/[\nusr/bin/b2sum\nusr/share/doc/x y\n\n\
                     %BACKUP%\netc/pacman.conf\t(null)\n";
        let paths: Vec<&str> = pacman_files(files).collect();
        assert_eq!(paths, ["usr/bin/[", "usr/bin/b2sum", "usr/share/doc/x y"]);
    }

    #[test]
    fn apk_files_belong_to_the_folder_above_them() {
        let installed = "C:Q17hOhjufinXWHIBdAPVnASE2s2WM=\nP:alpine-baselayout\nV:3.7.2-r1\n\
                         A:aarch64\nD:alpine-baselayout-data=3.7.2-r1 /bin/sh\n\
                         F:dev\nF:etc\nR:motd\nZ:Q1SLkS9hBidUbPwwrw+XR0Whv3ww8=\n\
                         F:etc/crontabs\nR:root\na:0:0:600\nZ:Q1vfk1apUWI4yLJGhhNRd0kJixfvY=\n\
                         \n\
                         C:Q1abc=\nP:busybox\nV:1.37.0-r30\nF:bin\nR:busybox\nF:usr/bin\nR:a:b\n";
        let pairs = apk_installed(installed);
        assert_eq!(
            pairs,
            [
                ("alpine-baselayout", "etc/motd".to_string()),
                ("alpine-baselayout", "etc/crontabs/root".to_string()),
                ("busybox", "bin/busybox".to_string()),
                // Split at the first colon only: a file name may hold one.
                ("busybox", "usr/bin/a:b".to_string()),
            ]
        );
    }

    #[test]
    fn an_apk_file_before_any_package_is_not_credited_to_anyone() {
        assert!(apk_installed("F:etc\nR:stray\n").is_empty());
    }

    #[test]
    fn rpm_lines_split_at_the_first_tab() {
        let out = "libgcc\t/lib64/libgcc_s-16-20260819.so.1\nlibgcc\t/usr/lib/.build-id\n\
                   bash\t/usr/bin/bash\n\nerror: something\n";
        let pairs: Vec<_> = rpm_lines(out).collect();
        assert_eq!(
            pairs,
            [
                ("libgcc", "/lib64/libgcc_s-16-20260819.so.1"),
                ("libgcc", "/usr/lib/.build-id"),
                ("bash", "/usr/bin/bash"),
            ]
        );
    }

    /// Copied from macOS 27.0.1 (26A434), shortened.
    #[test]
    fn firmlinks_are_pairs_of_relative_paths() {
        let text = "/AppleInternal\tAppleInternal\n/Applications\tApplications\n\
                    /System/Library/Caches\tSystem/Library/Caches\n/usr/local\tusr/local\n\
                    \n/broken\n";
        assert_eq!(
            firmlinks(text),
            [
                ("AppleInternal", "AppleInternal"),
                ("Applications", "Applications"),
                ("System/Library/Caches", "System/Library/Caches"),
                ("usr/local", "usr/local"),
            ]
            .map(|(a, b)| (a.to_string(), b.to_string()))
        );
    }

    #[test]
    fn a_link_resolves_against_its_own_directory() {
        assert_eq!(
            resolve_link(
                "/opt/homebrew/bin",
                "../Cellar/python@3.11/3.11.16/bin/2to3-3.11"
            ),
            "/opt/homebrew/Cellar/python@3.11/3.11.16/bin/2to3-3.11"
        );
        assert_eq!(
            resolve_link("/opt/homebrew/bin", "/opt/homebrew/Cellar/x/1/bin/x"),
            "/opt/homebrew/Cellar/x/1/bin/x"
        );
        assert_eq!(
            resolve_link("/a/b", "../../../../c"),
            "/c",
            "cannot climb above /"
        );
        assert_eq!(resolve_link("/a", "./b/./c"), "/a/b/c");
    }
}
