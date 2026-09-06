//! Sending a snapshot to another agent or a hub.
//!
//! The body is the same standalone SQLite file `GET /scans/{id}/download`
//! serves, so pushing and pulling are two directions of one format and neither
//! side needs a converter.

use std::path::Path;

use anyhow::{bail, Context, Result};
use spacetrace_store::ScanId;

use crate::runner::Runner;

/// Matches the level used when serving downloads.
const ZSTD_LEVEL: i32 = 3;

/// Which snapshot to send.
pub enum Selector {
    /// One specific snapshot.
    Id(ScanId),
    /// The newest snapshot of this root taken by this host.
    LatestForRoot(std::path::PathBuf),
    /// The newest snapshot in the database, whatever it is.
    Latest,
}

#[derive(Debug)]
pub struct Outcome {
    pub scan_id: ScanId,
    pub sent_bytes: usize,
    pub compressed: bool,
    pub imported: Vec<ScanId>,
    pub skipped: usize,
}

pub async fn push(
    runner: &Runner,
    base_url: &str,
    token: &str,
    selector: Selector,
    compress: bool,
) -> Result<Outcome> {
    let scan_id = resolve(runner, selector)?;

    let dir = tempfile::tempdir().context("creating a staging directory")?;
    let path = dir.path().join("snapshot.sqlite");
    runner.export(scan_id, &path)?;
    let (body, compressed) = read_body(&path, compress)?;
    let sent_bytes = body.len();

    let url = format!("{}/snapshots", base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .build()
        .context("building the HTTP client")?;

    let mut request = client
        .post(&url)
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, "application/vnd.sqlite3");
    if compressed {
        request = request.header(reqwest::header::CONTENT_ENCODING, "zstd");
    }

    let response = request
        .body(body)
        .send()
        .await
        .with_context(|| format!("posting to {url}"))?;

    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        // Pass the server's own message through: it is far more useful than a
        // bare status code when the token is wrong or the body was rejected.
        bail!("{url} returned {status}: {}", text.trim());
    }

    let parsed: ImportResponse = serde_json::from_str(&text)
        .with_context(|| format!("unexpected response body from {url}: {}", text.trim()))?;

    Ok(Outcome {
        scan_id,
        sent_bytes,
        compressed,
        imported: parsed.imported,
        skipped: parsed.skipped,
    })
}

#[derive(serde::Deserialize)]
struct ImportResponse {
    imported: Vec<ScanId>,
    #[serde(default)]
    skipped: usize,
}

fn resolve(runner: &Runner, selector: Selector) -> Result<ScanId> {
    match selector {
        Selector::Id(id) => {
            anyhow::ensure!(
                runner.scan_meta(id)?.is_some(),
                "no snapshot with id {id} to push"
            );
            Ok(id)
        }
        Selector::LatestForRoot(root) => {
            let store = runner.open_store()?;
            // Canonicalise so `/srv/` and `/srv` name the same target, matching
            // how the scanner recorded it.
            let canonical = root
                .canonicalize()
                .unwrap_or(root.clone())
                .to_string_lossy()
                .into_owned();
            let meta = store
                .latest_for(&canonical, Some(runner.host()))?
                .with_context(|| format!("no snapshot of {canonical} on this host yet"))?;
            Ok(meta.id)
        }
        Selector::Latest => {
            let scans = runner.list_scans()?;
            let newest = scans
                .first()
                .context("this agent has no snapshots to push yet")?;
            Ok(newest.id)
        }
    }
}

fn read_body(path: &Path, compress: bool) -> Result<(Vec<u8>, bool)> {
    let raw = std::fs::read(path)
        .with_context(|| format!("reading the staged snapshot {}", path.display()))?;
    if !compress {
        return Ok((raw, false));
    }
    match zstd::encode_all(raw.as_slice(), ZSTD_LEVEL) {
        Ok(packed) => Ok((packed, true)),
        // Sending it uncompressed is better than not sending it at all.
        Err(err) => {
            eprintln!("warning: compression failed ({err}); sending uncompressed");
            Ok((raw, false))
        }
    }
}
