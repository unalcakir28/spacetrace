//! Reading snapshots from an agent instead of the local database.
//!
//! The trick that keeps this small: a remote snapshot is downloaded as the
//! standalone SQLite file the agent already serves (docs/DECISIONS.md K4), so
//! everything after the download — loading, listing, diffing — runs the exact
//! same code as a local snapshot. There is no separate remote code path to keep
//! in step.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use spacetrace_store::{ScanId, ScanMeta, Store};

/// Environment variable used when a remote has no stored token.
pub const TOKEN_ENV: &str = "SPACETRACE_TOKEN";

pub struct Remote {
    base: String,
    token: String,
    client: reqwest::blocking::Client,
}

/// Written by hand rather than derived: a derived Debug would put the bearer
/// token into any log line or panic message that formats a Remote.
impl std::fmt::Debug for Remote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Remote")
            .field("base", &self.base)
            .field("token", &"<redacted>")
            .finish()
    }
}

impl Remote {
    /// `target` is either a URL or the name of a remote in `remotes.toml`.
    pub fn resolve(target: &str, token_override: Option<&str>) -> Result<Self> {
        let (base, stored_token) =
            if target.starts_with("http://") || target.starts_with("https://") {
                (target.trim_end_matches('/').to_string(), None)
            } else {
                let saved = RemotesFile::load()?;
                let entry = saved.remotes.get(target).with_context(|| {
                    format!(
                        "no remote named {target:?} in {}. Pass a URL, or add it to that file",
                        remotes_path().display()
                    )
                })?;
                (
                    entry.url.trim_end_matches('/').to_string(),
                    entry.token.clone(),
                )
            };

        let token = token_override
            .map(str::to_string)
            .or(stored_token)
            .or_else(|| {
                std::env::var(TOKEN_ENV)
                    .ok()
                    .filter(|t| !t.trim().is_empty())
            })
            .with_context(|| {
                format!(
                    "no token for {base}. Pass --token, set {TOKEN_ENV}, or store one in {}",
                    remotes_path().display()
                )
            })?;

        let client = reqwest::blocking::Client::builder()
            .build()
            .context("building the HTTP client")?;
        Ok(Remote {
            base,
            token,
            client,
        })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn list(&self) -> Result<Vec<ScanMeta>> {
        let text = self.get("/scans")?;
        serde_json::from_str(&text)
            .with_context(|| format!("unexpected /scans response from {}", self.base))
    }

    /// Metadata for one snapshot, failing with the remote's own wording.
    pub fn scan_or_fail(&self, id: ScanId) -> Result<ScanMeta> {
        let text = self.get(&format!("/scans/{id}"))?;
        serde_json::from_str(&text)
            .with_context(|| format!("unexpected /scans/{id} response from {}", self.base))
    }

    /// The newest snapshot of `root`, or the newest overall when `root` is None.
    ///
    /// When several hosts have pushed the same root, this is whichever scanned
    /// most recently; use [`Remote::last_two_for`] for comparisons, which pins
    /// both sides to one host.
    pub fn latest_for(&self, root: Option<&str>) -> Result<ScanMeta> {
        let scans = self.list()?;
        match root {
            None => scans
                .into_iter()
                .next()
                .with_context(|| format!("{} has no snapshots yet", self.base)),
            Some(root) => scans
                .into_iter()
                .find(|s| s.root == root)
                .with_context(|| format!("{} has no snapshot of {root}", self.base)),
        }
    }

    /// The two newest snapshots of `root` **taken by the same host**.
    ///
    /// An agent can hold snapshots pushed from several machines, and two
    /// different machines' views of `/var` are not comparable. The newest
    /// matching snapshot picks the host; anything else is reported rather than
    /// silently mixed in.
    pub fn last_two_for(&self, root: &str) -> Result<Vec<ScanMeta>> {
        let all = self.list()?;
        let of_root: Vec<ScanMeta> = all.into_iter().filter(|s| s.root == root).collect();
        let newest = of_root
            .first()
            .with_context(|| format!("{} has no snapshot of {root}", self.base))?;
        let host = newest.host.clone();

        let others: Vec<&str> = {
            let mut hosts: Vec<&str> = of_root
                .iter()
                .map(|s| s.host.as_str())
                .filter(|h| *h != host)
                .collect();
            hosts.sort_unstable();
            hosts.dedup();
            hosts
        };
        if !others.is_empty() {
            eprintln!(
                "note: {} also has snapshots of {root} from {}; comparing {host} only",
                self.base,
                others.join(", ")
            );
        }

        let matching: Vec<ScanMeta> = of_root
            .into_iter()
            .filter(|s| s.host == host)
            .take(2)
            .collect();
        anyhow::ensure!(
            matching.len() == 2,
            "need two snapshots of {root} from {host} on {} to compare (found {})",
            self.base,
            matching.len()
        );
        Ok(matching)
    }

    /// Download one snapshot into `dest` and open it.
    pub fn fetch(&self, id: ScanId, dest: &Path) -> Result<Store> {
        let url = format!("{}/scans/{id}/download", self.base);
        let response = self
            .client
            .get(&url)
            .bearer_auth(&self.token)
            .header(reqwest::header::ACCEPT_ENCODING, "zstd")
            .send()
            .with_context(|| format!("requesting {url}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            bail!("{url} returned {status}: {}", body.trim());
        }

        let compressed = response
            .headers()
            .get(reqwest::header::CONTENT_ENCODING)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("zstd"));

        // Read one byte past the cap so a body that would overrun it is an
        // error rather than a silently truncated file.
        let mut body = Vec::new();
        response
            .take(MAX_SNAPSHOT_BYTES + 1)
            .read_to_end(&mut body)
            .with_context(|| format!("reading the snapshot body from {url}"))?;
        anyhow::ensure!(
            body.len() as u64 <= MAX_SNAPSHOT_BYTES,
            "{url} sent more than {MAX_SNAPSHOT_BYTES} bytes; refusing to buffer it"
        );

        let raw = if compressed {
            decode_zstd_bounded(&body, MAX_SNAPSHOT_BYTES)
                .with_context(|| format!("{url} sent a body that is not valid zstd"))?
        } else {
            body
        };

        std::fs::write(dest, &raw)
            .with_context(|| format!("writing the downloaded snapshot to {}", dest.display()))?;
        Store::open(dest).with_context(|| format!("{url} did not send a usable snapshot database"))
    }

    fn get(&self, path: &str) -> Result<String> {
        let url = format!("{}{path}", self.base);
        let response = self
            .client
            .get(&url)
            .bearer_auth(&self.token)
            .send()
            .with_context(|| format!("requesting {url}"))?;
        let status = response.status();
        let text = response.text().unwrap_or_default();
        if !status.is_success() {
            // The agent puts a human-readable reason in the body; a bare status
            // code would send people digging through server logs.
            bail!("{url} returned {status}: {}", text.trim());
        }
        Ok(text)
    }
}

/// Refuse to buffer an unbounded body from a host we do not control. A snapshot
/// costs roughly 50 bytes per filesystem entry, so this still allows a root of
/// well over ten million files.
const MAX_SNAPSHOT_BYTES: u64 = 1024 * 1024 * 1024;

/// Decompress with the same cap, so a small compressed body cannot expand into
/// an arbitrarily large one. `--remote` points at a machine the user trusts, but
/// "trusted" is not "allowed to decide how much memory we allocate".
fn decode_zstd_bounded(data: &[u8], limit: u64) -> std::io::Result<Vec<u8>> {
    let decoder = zstd::stream::Decoder::new(data)?;
    let mut out = Vec::new();
    decoder.take(limit + 1).read_to_end(&mut out)?;
    if out.len() as u64 > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("decompressed snapshot exceeds the {limit} byte limit"),
        ));
    }
    Ok(out)
}

#[derive(Debug, Default, serde::Deserialize)]
struct RemotesFile {
    #[serde(default)]
    remotes: std::collections::BTreeMap<String, RemoteEntry>,
}

#[derive(Debug, serde::Deserialize)]
struct RemoteEntry {
    url: String,
    #[serde(default)]
    token: Option<String>,
}

impl RemotesFile {
    fn load() -> Result<Self> {
        let path = remotes_path();
        if !path.exists() {
            return Ok(RemotesFile::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }
}

/// `$XDG_CONFIG_HOME/spacetrace/remotes.toml`, or the platform equivalent.
pub fn remotes_path() -> PathBuf {
    if let Ok(explicit) = std::env::var("SPACETRACE_REMOTES") {
        return PathBuf::from(explicit);
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default();

    let dir = if cfg!(target_os = "macos") {
        home.join("Library/Application Support/spacetrace")
    } else if cfg!(target_os = "windows") {
        std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or(home)
            .join("spacetrace")
    } else {
        std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".config"))
            .join("spacetrace")
    };
    dir.join("remotes.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remotes_file_parses() {
        let parsed: RemotesFile = toml::from_str(
            r#"
[remotes.prod]
url = "https://nas.example.com:7878"
token = "abc"

[remotes.staging]
url = "http://10.0.0.5:7878"
"#,
        )
        .unwrap();
        assert_eq!(parsed.remotes.len(), 2);
        assert_eq!(parsed.remotes["prod"].token.as_deref(), Some("abc"));
        assert!(parsed.remotes["staging"].token.is_none());
    }

    #[test]
    fn an_empty_remotes_file_is_valid() {
        let parsed: RemotesFile = toml::from_str("").unwrap();
        assert!(parsed.remotes.is_empty());
    }

    #[test]
    fn a_url_target_needs_no_remotes_file_but_still_needs_a_token() {
        // Guard against a token leaking in from the developer's environment.
        let saved = std::env::var(TOKEN_ENV).ok();
        std::env::remove_var(TOKEN_ENV);

        let err = Remote::resolve("https://example.com", None).unwrap_err();
        assert!(err.to_string().contains("no token"), "{err}");

        let ok = Remote::resolve("https://example.com/", Some("t")).unwrap();
        assert_eq!(ok.base(), "https://example.com", "trailing slash trimmed");

        if let Some(v) = saved {
            std::env::set_var(TOKEN_ENV, v);
        }
    }

    #[test]
    fn an_unknown_remote_name_names_the_file_to_edit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remotes.toml");
        std::fs::write(&path, "").unwrap();
        std::env::set_var("SPACETRACE_REMOTES", &path);

        let err = Remote::resolve("nosuch", Some("t")).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("nosuch"), "{message}");
        assert!(message.contains("remotes.toml"), "{message}");

        std::env::remove_var("SPACETRACE_REMOTES");
    }
}
