//! Whether a newer release exists — as a fact reported on `/status`, and
//! nothing more.
//!
//! **The agent never updates itself.** It runs on someone else's server with
//! read access to their whole filesystem, and the trust that buys is the same
//! trust invariant #8 protects by refusing to delete anything. A daemon that
//! silently replaces its own binary is a larger ask than one that deletes
//! files. So this reports, and the operator runs `spacetrace update` when they
//! choose to.
//!
//! **What it does over the network, said plainly:** one unauthenticated GET to
//! `api.github.com` for the latest release tag, once when the agent starts and
//! once a day after that. Nothing about the machine is sent. Set
//! `update_check = false` in the config to switch it off entirely, and it
//! switches itself off on any build that is not a tagged release, because
//! there is nothing there to compare against.
//!
//! The check runs on its own task and `/status` reads a cached answer, so a
//! slow or unreachable GitHub delays nothing: the field is simply absent until
//! a check has succeeded.

use std::sync::{Arc, RwLock};
use std::time::Duration;

/// The release list, not `releases/latest`. See [`is_ours`].
const RELEASES_API: &str =
    "https://api.github.com/repos/unalcakir28/spacetrace/releases?per_page=100";
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const TIMEOUT: Duration = Duration::from_secs(10);

/// The newest release seen, refreshed in the background.
#[derive(Debug, Default)]
pub struct UpdateWatch {
    /// `None` until a check has succeeded. A failed check leaves the previous
    /// answer in place rather than blanking it.
    latest: RwLock<Option<String>>,
}

impl UpdateWatch {
    /// A watch that never checks anything, for `update_check = false` and for
    /// tests.
    pub fn off() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Start checking, if this build and this configuration allow it.
    ///
    /// Returns an idle watch when they do not, so callers need no branch of
    /// their own and `/status` behaves identically either way.
    pub fn start(enabled: bool) -> Arc<Self> {
        let watch = Self::off();
        if !enabled || !spacetrace_buildinfo::is_release() {
            return watch;
        }

        let task = Arc::clone(&watch);
        tokio::spawn(async move {
            loop {
                if let Some(tag) = fetch_latest().await {
                    // A poisoned lock here would mean a panic in a one-line
                    // critical section; there is nothing to recover, and
                    // taking down the agent over a version check would be
                    // absurd.
                    if let Ok(mut slot) = task.latest.write() {
                        *slot = Some(tag);
                    }
                }
                tokio::time::sleep(CHECK_EVERY).await;
            }
        });
        watch
    }

    /// The newer release, when there is one. `None` means up to date, not
    /// checked yet, or checking switched off — all three are the same thing to
    /// a reader of `/status`: no upgrade to mention.
    pub fn available(&self) -> Option<String> {
        let latest = self.latest.read().ok()?.clone()?;
        spacetrace_buildinfo::is_newer(&latest, env!("CARGO_PKG_VERSION")).then_some(latest)
    }
}

#[derive(serde::Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
}

/// Whether a tag names a release of the agent.
///
/// One repository carries the downloads for all three components, so its
/// releases are a mixture: `v0.4.0` is this binary, `desktop-v…` and `hub-v…`
/// are not, and `continuous` and friends are rolling builds.
///
/// This is why `releases/latest` cannot be used. GitHub's "latest" is whichever
/// release went out most recently regardless of component, and on the day all
/// three were first tagged that was the hub — which parses as no version at
/// all, so the check went quiet and would have stayed quiet forever.
fn is_ours(tag: &str) -> bool {
    let mut chars = tag.chars();
    chars.next() == Some('v') && chars.next().is_some_and(|c| c.is_ascii_digit())
}

/// `None` on any failure, including the 404 GitHub answers before the first
/// tagged release. A version check is a courtesy; it never becomes a log full
/// of errors about someone else's network.
async fn fetch_latest() -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        // GitHub refuses requests with no user agent, and naming the tool is
        // more honest than borrowing a browser's.
        .user_agent(concat!("spacetrace-agent/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;

    let response = client
        .get(RELEASES_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let releases: Vec<ApiRelease> = response.json().await.ok()?;
    // Newest first, which is the order GitHub returns.
    releases
        .into_iter()
        .find(|release| !release.draft && !release.prerelease && is_ours(&release.tag_name))
        .map(|release| release.tag_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug this exists for: two other components publish into the same
    /// repository and must not be mistaken for this one.
    #[test]
    fn only_the_agents_own_release_tags_are_recognised() {
        assert!(is_ours("v0.4.0"));
        assert!(!is_ours("hub-v0.3.0"));
        assert!(!is_ours("desktop-v0.3.0"));
        assert!(!is_ours("continuous"));
        assert!(!is_ours("v"));
    }

    #[test]
    fn an_idle_watch_reports_nothing() {
        assert!(UpdateWatch::off().available().is_none());
    }

    #[test]
    fn a_disabled_check_never_starts_one() {
        // No runtime is needed, which is itself the assertion: `start(false)`
        // must not reach `tokio::spawn`. It would panic here if it did.
        assert!(UpdateWatch::start(false).available().is_none());
    }

    /// The field appears only for a genuinely newer tag. Same version, older
    /// version and unparseable tags all have to stay quiet, or every agent in
    /// a fleet reports an upgrade that does not exist.
    #[test]
    fn only_a_newer_tag_is_reported() {
        let watch = UpdateWatch::off();
        let current = env!("CARGO_PKG_VERSION");

        for (tag, expected) in [
            ("v999.0.0", true),
            (current, false),
            ("v0.0.1", false),
            ("nightly", false),
        ] {
            *watch.latest.write().unwrap() = Some(tag.to_string());
            assert_eq!(
                watch.available().is_some(),
                expected,
                "{tag} against {current}"
            );
        }
    }
}
