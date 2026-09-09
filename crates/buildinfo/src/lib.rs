//! What this binary was built from: commit, date and channel.
//!
//! A version number alone cannot answer "which build are you running". Every
//! `continuous` build reports the same `CARGO_PKG_VERSION`, so before this
//! crate two builds a month apart were indistinguishable in a bug report, and
//! an update check had nothing to compare against. The values are stamped in by
//! `build.rs`; see there for why the environment is consulted before git.
//!
//! Deliberately dependency-free. It is linked into the agent, which has to stay
//! one small static binary, and the callers already have `serde_json` if they
//! want to put these in a response.

use std::sync::OnceLock;

/// Short commit hash, or `unknown` for a build from a source tarball.
pub const GIT_SHA: &str = env!("SPACETRACE_GIT_SHA");

/// Build time, `YYYY-MM-DDTHH:MM:SSZ`, or `unknown`.
pub const BUILD_DATE: &str = env!("SPACETRACE_BUILD_DATE");

/// `release` for a tagged build, `continuous` for the tip of `main`, `dev` for
/// anything built outside CI.
pub const CHANNEL: &str = env!("SPACETRACE_CHANNEL");

const DIRTY: &str = env!("SPACETRACE_GIT_DIRTY");

/// Whether the working tree had uncommitted changes. Always false for a CI
/// build, which starts from a clean checkout.
pub fn dirty() -> bool {
    DIRTY == "true"
}

/// True when this build came from a tagged release.
///
/// The one question an update check needs answered before it says anything: a
/// `dev` or `continuous` build is not behind, it is simply not on the release
/// channel, and telling its user to upgrade would be wrong.
pub fn is_release() -> bool {
    CHANNEL == "release"
}

/// The multi-line body of `--version`, starting with the version itself so clap
/// prints `<name> <version>` on the first line as usual.
///
/// Caches on first call, so the caller can hand it to clap as a `&'static str`.
/// One binary has one version; a second call with a different argument returns
/// the first, which is a non-problem in a binary and a bug in a test.
pub fn long_version(package_version: &str) -> &'static str {
    static TEXT: OnceLock<String> = OnceLock::new();
    TEXT.get_or_init(|| describe(package_version, GIT_SHA, BUILD_DATE, CHANNEL, dirty()))
}

fn describe(version: &str, sha: &str, date: &str, channel: &str, dirty: bool) -> String {
    let suffix = if dirty { " (uncommitted changes)" } else { "" };
    format!("{version}\ncommit   {sha}{suffix}\nbuilt    {date}\nchannel  {channel}")
}

/// Whether `tag` names a release newer than `current`.
///
/// Lives here rather than in either binary because both need it and neither
/// should own it: the CLI compares synchronously before a self-update, the
/// agent compares on a background task. Parsing `1.2.3` needs no dependency,
/// which is what lets it sit in a crate that has none.
///
/// An unparseable tag is never newer. A release named something unexpected is
/// a reason to say nothing, not a reason to nag.
pub fn is_newer(tag: &str, current: &str) -> bool {
    match (parse_version(tag), parse_version(current)) {
        (Some(theirs), Some(ours)) => theirs > ours,
        _ => false,
    }
}

/// `v1.2.3` or `1.2.3` as a comparable triple.
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.trim().trim_start_matches('v').split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_line_is_the_version_alone_so_clap_can_prefix_the_name() {
        let text = describe("0.2.0", "abc1234", "2026-09-09T18:00:00Z", "release", false);
        assert_eq!(text.lines().next(), Some("0.2.0"));
    }

    #[test]
    fn an_uncommitted_tree_says_so_and_a_clean_one_stays_quiet() {
        let dirty = describe("0.2.0", "abc1234", "d", "dev", true);
        assert!(dirty.contains("(uncommitted changes)"));

        let clean = describe("0.2.0", "abc1234", "d", "dev", false);
        assert!(!clean.contains("uncommitted"));
    }

    /// The stamping has to actually happen. An `env!` that resolved to an empty
    /// string would compile and then report nothing at all.
    #[test]
    fn the_build_stamped_real_values_in() {
        assert!(!GIT_SHA.is_empty());
        assert!(!BUILD_DATE.is_empty());
        assert!(!CHANNEL.is_empty());
        assert!(
            matches!(CHANNEL, "dev" | "continuous" | "release"),
            "unexpected channel {CHANNEL}"
        );
    }

    /// Only a tagged build may claim to be one, or the update check tells
    /// people on `main` that they are out of date.
    #[test]
    fn only_the_release_channel_counts_as_a_release() {
        assert_eq!(is_release(), CHANNEL == "release");
    }

    #[test]
    fn only_a_higher_version_is_newer() {
        assert!(is_newer("v1.0.0", "0.9.9"));
        assert!(is_newer("0.10.0", "0.9.0"), "compared as numbers, not text");
        assert!(!is_newer("v0.9.0", "0.9.0"));
        assert!(!is_newer("v0.8.0", "0.9.0"));
    }

    /// A tag nobody can parse must not produce a nag.
    #[test]
    fn an_unparseable_tag_is_never_newer() {
        for tag in ["nightly", "", "v1.2", "v1.2.3.4", "latest", "v1.2.x"] {
            assert!(!is_newer(tag, "0.0.1"), "{tag} should not count as newer");
        }
    }
}
