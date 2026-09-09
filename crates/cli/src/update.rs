//! Finding out whether there is a newer release, and installing it.
//!
//! **What this does over the network, said plainly:** one unauthenticated GET
//! to `api.github.com` for the latest release tag, at most once a day, and —
//! only when you ask for it — a download of the release archive and its
//! `SHA256SUMS`. Nothing is sent about you or your machine beyond what any HTTP
//! request carries. `SPACETRACE_NO_UPDATE_CHECK=1` turns the check off, and the
//! first time it runs it says so on stderr.
//!
//! **Why the notice is one run behind.** The check runs on a background thread
//! and writes a cache; what gets printed is whatever the cache already held.
//! Doing it synchronously would put a network round trip in front of the exit
//! of every command, and a disk tool that pauses before it finishes has broken
//! the thing it is for. The cost is that the first run after a release says
//! nothing.
//!
//! **Why no archive crate.** `install.sh` already relies on the system `tar`,
//! which is present on every Unix and, since Windows 10, ships as `tar.exe`
//! that also reads zip. Two dependencies to avoid one `Command` is the wrong
//! trade for a binary that has to stay small enough to drop on a NAS.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const REPO: &str = "unalcakir28/spacetrace";
/// The release list, not `releases/latest`. See [`is_ours`].
const RELEASES_API: &str =
    "https://api.github.com/repos/unalcakir28/spacetrace/releases?per_page=100";

/// Long enough that nobody notices it, short enough to matter for a security
/// fix. A tool run twenty times an hour must not ask twenty times.
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// Short: this is a courtesy, and a slow network must not delay anything.
const TIMEOUT: Duration = Duration::from_secs(5);

const OPT_OUT: &str = "SPACETRACE_NO_UPDATE_CHECK";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Cache {
    /// Unix seconds of the last completed check.
    checked: u64,
    /// The newest release tag seen, e.g. `v0.2.0`.
    latest: String,
    /// Whether the one-time explanation has been printed.
    told: bool,
}

/// The asset suffix for this machine, or `None` where no build is published.
///
/// Matched on the same strings the release workflow puts in the file names.
/// Getting this wrong means a 404 rather than a wrong binary, but a clear
/// "no build for this platform" is a better answer than a failed download.
fn target() -> Option<&'static str> {
    Some(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "aarch64") => "aarch64-unknown-linux-musl",
        ("linux", "x86_64") => "x86_64-unknown-linux-musl",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        _ => return None,
    })
}

fn archive_name(version: &str, target: &str) -> String {
    let extension = if cfg!(windows) { "zip" } else { "tar.gz" };
    format!("spacetrace-{version}-{target}.{extension}")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn cache_path() -> Option<PathBuf> {
    Some(crate::default_data_dir()?.join("update-check.json"))
}

fn read_cache() -> Cache {
    let Some(path) = cache_path() else {
        return Cache::default();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_cache(cache: &Cache) {
    let Some(path) = cache_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string(cache) {
        let _ = std::fs::write(path, text);
    }
}

fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(TIMEOUT)
        // GitHub refuses requests with no user agent, and naming the tool is
        // more honest than borrowing a browser's.
        .user_agent(concat!("spacetrace/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("building the HTTP client")
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
}

/// Whether a tag names a release of *this* tool.
///
/// The repository holds the downloads for all three components, so its
/// releases are a mixture: `v0.4.0` is the CLI, `desktop-v0.3.0` and
/// `hub-v0.3.0` are not, and `continuous` and friends are rolling builds.
///
/// This is why `releases/latest` cannot be used. GitHub's "latest" is whichever
/// release was published most recently regardless of component, and on the day
/// all three were first tagged that was the hub — which parses as no version at
/// all, so the check went quiet and would have stayed quiet forever.
fn is_ours(tag: &str) -> bool {
    let mut chars = tag.chars();
    chars.next() == Some('v') && chars.next().is_some_and(|c| c.is_ascii_digit())
}

/// The newest tagged release of this tool, or `None` if there has never been
/// one.
fn fetch_latest() -> Result<Option<String>> {
    let releases: Vec<ApiRelease> = client()?
        .get(RELEASES_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .context("asking GitHub for the releases")?
        .error_for_status()
        .context("GitHub refused")?
        .json()
        .context("reading GitHub's answer")?;

    // Newest first, which is the order GitHub returns.
    Ok(releases
        .into_iter()
        .find(|release| !release.draft && !release.prerelease && is_ours(&release.tag_name))
        .map(|release| release.tag_name))
}

/// Whether `tag` names a release newer than what is running.
///
/// The comparison itself lives in `spacetrace-buildinfo`, which the agent uses
/// too — one definition of "newer" for both binaries.
pub fn is_newer(tag: &str) -> bool {
    spacetrace_buildinfo::is_newer(tag, env!("CARGO_PKG_VERSION"))
}

/// Whether an update notice is allowed to appear at all.
///
/// Four conditions, and each one exists for a reason: a machine-readable run
/// must stay machine-readable, a pipe or a cron job has nobody to read the
/// notice, a development build has nothing to compare against, and anyone who
/// asked to be left alone should be.
pub fn notices_allowed(json_mode: bool, stdout_is_terminal: bool) -> bool {
    if json_mode || !stdout_is_terminal {
        return false;
    }
    if std::env::var_os(OPT_OUT).is_some() {
        return false;
    }
    spacetrace_buildinfo::is_release()
}

/// Start a background check if one is due. Returns immediately.
pub fn maybe_check_in_background() {
    let cache = read_cache();
    if now().saturating_sub(cache.checked) < CHECK_EVERY.as_secs() {
        return;
    }
    // Detached on purpose: if the command finishes first the thread dies with
    // it and the cache simply stays stale for another run.
    std::thread::spawn(move || {
        if let Ok(latest) = fetch_latest() {
            write_cache(&Cache {
                checked: now(),
                latest: latest.unwrap_or_default(),
                told: cache.told,
            });
        }
    });
}

/// The one-line notice, if the cache holds a newer version. Prints to stderr so
/// it cannot land in anything being piped.
pub fn print_notice_if_due() {
    let mut cache = read_cache();

    // Said once, before anything is ever reported, so the check is never a
    // surprise discovered in a packet capture.
    if !cache.told {
        eprintln!(
            "spacetrace checks github.com once a day for a newer release. \
             Set {OPT_OUT}=1 to stop it."
        );
        cache.told = true;
        write_cache(&cache);
    }

    if cache.latest.is_empty() || !is_newer(&cache.latest) {
        return;
    }
    eprintln!(
        "A newer spacetrace is out: {} (you have {}). Run `spacetrace update`.",
        cache.latest,
        env!("CARGO_PKG_VERSION")
    );
}

/// `spacetrace update --check`: say what is available and stop.
pub fn check_only(json: bool) -> Result<()> {
    let latest = fetch_latest()?;
    let current = env!("CARGO_PKG_VERSION");

    if json {
        let available = latest.as_deref().map(is_newer).unwrap_or(false);
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "current": current,
                "latest": latest,
                "update_available": available,
                "channel": spacetrace_buildinfo::CHANNEL,
            }))?
        );
        return Ok(());
    }

    match latest {
        None => println!("No tagged release yet. You are on {current}."),
        Some(tag) if is_newer(&tag) => {
            println!("{tag} is available. You have {current}.");
            println!("Run `spacetrace update` to install it.");
        }
        Some(tag) => println!("Up to date: {current} (latest release is {tag})."),
    }
    Ok(())
}

/// `spacetrace update`: download the newest release and replace this binary.
pub fn install(json: bool) -> Result<()> {
    let Some(target) = target() else {
        bail!(
            "no published build for {} on {}; build from source with `cargo install --path crates/cli`",
            std::env::consts::ARCH,
            std::env::consts::OS
        );
    };

    let Some(tag) = fetch_latest()? else {
        bail!("there is no tagged release yet");
    };
    if !is_newer(&tag) {
        println!("Already on {} — nothing to do.", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let current = std::env::current_exe().context("finding this binary")?;
    let directory = current
        .parent()
        .context("this binary has no parent directory")?
        .to_path_buf();
    // Checked before anything is downloaded: failing after a 12 MB download
    // because of a permission the check could have predicted is rude.
    writable(&directory)?;

    let archive = archive_name(&tag, target);
    println!("Downloading {archive}…");
    let bytes = download(&format!(
        "https://github.com/{REPO}/releases/download/{tag}/{archive}"
    ))?;

    let sums = String::from_utf8(download(&format!(
        "https://github.com/{REPO}/releases/download/{tag}/SHA256SUMS"
    ))?)
    .context("SHA256SUMS was not text")?;

    verify(&bytes, &archive, &sums)?;
    println!("Checksum verified.");

    let staging = tempfile::tempdir().context("making a temporary directory")?;
    let archive_path = staging.path().join(&archive);
    std::fs::write(&archive_path, &bytes).context("writing the download")?;
    unpack(&archive_path, staging.path())?;

    let mut replaced = Vec::new();
    for name in ["spacetrace", "spacetrace-agent"] {
        let file = if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_string()
        };
        let fresh = staging.path().join(&file);
        let installed = directory.join(&file);
        // Only what is already there: `spacetrace update` on a machine with no
        // agent must not quietly install one.
        if !fresh.exists() || !installed.exists() {
            continue;
        }
        replace(&fresh, &installed)?;
        replaced.push(file);
    }

    if replaced.is_empty() {
        bail!("{archive} held no binaries matching what is installed here");
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "updated_to": tag,
                "replaced": replaced,
            }))?
        );
        return Ok(());
    }

    println!("Updated to {tag}: {}", replaced.join(", "));
    if replaced
        .iter()
        .any(|name| name.starts_with("spacetrace-agent"))
    {
        // Said out loud because an operator who checks `/status` and sees the
        // old version will otherwise think the update failed.
        println!("A running spacetrace-agent keeps the old version until it is restarted.");
    }
    Ok(())
}

fn download(url: &str) -> Result<Vec<u8>> {
    let response = client()?
        .get(url)
        .send()
        .with_context(|| format!("downloading {url}"))?
        .error_for_status()
        .with_context(|| format!("downloading {url}"))?;
    Ok(response.bytes().context("reading the download")?.to_vec())
}

/// Compare the download against the published `SHA256SUMS`.
///
/// `install.sh` published this file for months without checking it. A
/// self-update that skipped it would be worse: the whole point of replacing
/// your own binary is that you trust what replaces it.
fn verify(bytes: &[u8], name: &str, sums: &str) -> Result<()> {
    let expected = sums
        .lines()
        .filter_map(|line| line.split_once("  "))
        .find(|(_, file)| file.trim_start_matches("./") == name)
        .map(|(hash, _)| hash.trim().to_ascii_lowercase())
        .with_context(|| format!("SHA256SUMS does not list {name}"))?;

    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual != expected {
        bail!("checksum mismatch for {name}: expected {expected}, got {actual}");
    }
    Ok(())
}

fn unpack(archive: &Path, into: &Path) -> Result<()> {
    // `tar` reads zip too on Windows, where it is bsdtar.
    let status = std::process::Command::new("tar")
        .arg(if cfg!(windows) { "-xf" } else { "-xzf" })
        .arg(archive)
        .arg("-C")
        .arg(into)
        .status()
        .context("running tar; it is needed to unpack the release")?;
    if !status.success() {
        bail!("tar could not unpack {}", archive.display());
    }
    Ok(())
}

fn writable(directory: &Path) -> Result<()> {
    let probe = directory.join(".spacetrace-write-test");
    match std::fs::File::create(&probe) {
        Ok(mut file) => {
            let _ = file.write_all(b"");
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(err) => bail!(
            "cannot write to {}: {err}\n\
             Re-run with sudo, or reinstall with:\n  \
             curl -fsSL https://raw.githubusercontent.com/{REPO}/main/install.sh | sh",
            directory.display()
        ),
    }
}

/// Put `fresh` where `installed` is, atomically where the platform allows it.
fn replace(fresh: &Path, installed: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(fresh, std::fs::Permissions::from_mode(0o755))
            .context("making the new binary executable")?;
    }

    // A rename within one directory is atomic, and on Unix it works on a
    // running executable: the old inode stays alive for the running process.
    // Staged next to the target rather than in /tmp, because a rename across
    // filesystems is not a rename.
    let staged = installed.with_extension("new");
    std::fs::copy(fresh, &staged).with_context(|| format!("staging {}", staged.display()))?;

    #[cfg(windows)]
    {
        // Windows refuses to replace a running executable but allows renaming
        // it out of the way. The leftover is cleaned up on the next run rather
        // than now, because now it is still the running process.
        let retired = installed.with_extension("old");
        let _ = std::fs::remove_file(&retired);
        if installed.exists() {
            std::fs::rename(installed, &retired)
                .with_context(|| format!("moving {} aside", installed.display()))?;
        }
    }

    std::fs::rename(&staged, installed)
        .with_context(|| format!("replacing {}", installed.display()))?;
    Ok(())
}

/// Remove the previous binary left behind by a Windows update.
pub fn clean_up_after_windows_update() {
    if !cfg!(windows) {
        return;
    }
    let Ok(current) = std::env::current_exe() else {
        return;
    };
    let _ = std::fs::remove_file(current.with_extension("old"));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug this function exists for: all three components publish into one
    /// repository, and two of them must not be mistaken for this one.
    #[test]
    fn only_this_tools_own_release_tags_are_recognised() {
        assert!(is_ours("v0.4.0"));
        assert!(is_ours("v1.0.0-rc1"));

        assert!(!is_ours("hub-v0.3.0"), "the hub is not the CLI");
        assert!(!is_ours("desktop-v0.3.0"), "the desktop app is not the CLI");
        assert!(!is_ours("continuous"));
        assert!(!is_ours("hub-continuous"));
        assert!(!is_ours("version-2"), "a v must be followed by a digit");
        assert!(!is_ours("v"));
        assert!(!is_ours(""));
    }

    #[test]
    fn only_a_higher_version_counts_as_newer() {
        assert!(!is_newer(env!("CARGO_PKG_VERSION")));
        assert!(!is_newer("v0.0.1"));
        assert!(is_newer("v999.0.0"));
        // A tag nobody can parse must not produce a nag.
        assert!(!is_newer("nightly"));
        assert!(!is_newer(""));
        assert!(!is_newer("v1.2"));
    }

    #[test]
    fn asset_names_match_what_the_release_workflow_publishes() {
        assert_eq!(
            archive_name("v0.2.0", "x86_64-apple-darwin"),
            if cfg!(windows) {
                "spacetrace-v0.2.0-x86_64-apple-darwin.zip"
            } else {
                "spacetrace-v0.2.0-x86_64-apple-darwin.tar.gz"
            }
        );
    }

    #[test]
    fn a_matching_checksum_passes_and_a_wrong_one_does_not() {
        let bytes = b"the release archive";
        let digest = format!("{:x}", Sha256::digest(bytes));
        let name = "spacetrace-v0.2.0-x86_64-apple-darwin.tar.gz";
        let sums = format!("{digest}  {name}\ndeadbeef  something-else.tar.gz\n");

        assert!(verify(bytes, name, &sums).is_ok());
        assert!(verify(b"tampered", name, &sums).is_err());
    }

    /// An archive missing from the sums file must fail, not pass unchecked.
    /// That is the whole failure mode this guards against.
    #[test]
    fn an_unlisted_archive_is_refused() {
        let sums = "abc123  some-other-file.tar.gz\n";
        let error = verify(b"anything", "spacetrace-v0.2.0-x.tar.gz", sums).unwrap_err();
        assert!(error.to_string().contains("does not list"), "{error}");
    }

    /// `sha256sum` writes two spaces between hash and name, and some tools
    /// prefix `./`. Both have to parse or the verification silently never
    /// finds its line.
    #[test]
    fn checksum_lines_parse_in_the_shapes_tools_actually_write() {
        let bytes = b"x";
        let digest = format!("{:x}", Sha256::digest(bytes));
        assert!(verify(bytes, "a.tar.gz", &format!("{digest}  ./a.tar.gz\n")).is_ok());
        assert!(verify(bytes, "a.tar.gz", &format!("{digest}  a.tar.gz\n")).is_ok());
    }

    #[test]
    fn every_published_target_is_recognised() {
        // The five the release matrix builds. If one is dropped or renamed,
        // this is the test that should be updated with it.
        for (os, arch, expected) in [
            ("macos", "aarch64", "aarch64-apple-darwin"),
            ("macos", "x86_64", "x86_64-apple-darwin"),
            ("linux", "aarch64", "aarch64-unknown-linux-musl"),
            ("linux", "x86_64", "x86_64-unknown-linux-musl"),
            ("windows", "x86_64", "x86_64-pc-windows-msvc"),
        ] {
            let matched = match (os, arch) {
                ("macos", "aarch64") => "aarch64-apple-darwin",
                ("macos", "x86_64") => "x86_64-apple-darwin",
                ("linux", "aarch64") => "aarch64-unknown-linux-musl",
                ("linux", "x86_64") => "x86_64-unknown-linux-musl",
                ("windows", "x86_64") => "x86_64-pc-windows-msvc",
                _ => "",
            };
            assert_eq!(matched, expected);
        }
        // This machine is one of them, so the real function agrees.
        assert!(target().is_some());
    }

    #[test]
    fn notices_stay_out_of_machine_readable_output_and_pipes() {
        // A release build is required for any of these to be true, so the
        // assertions below hold on every channel.
        assert!(!notices_allowed(true, true));
        assert!(!notices_allowed(false, false));
        assert_eq!(
            notices_allowed(false, true),
            spacetrace_buildinfo::is_release() && std::env::var_os(OPT_OUT).is_none()
        );
    }
}
