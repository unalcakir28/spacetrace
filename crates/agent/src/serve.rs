//! The agent's HTTP API.
//!
//! HTTP+JSON rather than gRPC (docs/DECISIONS.md K3): the people who run this
//! put it behind Caddy or Traefik and debug it with `curl`. Snapshot bodies are
//! the raw SQLite file (K4), optionally zstd-compressed when the client asks.

use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Path as AxumPath, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use spacetrace_store::{ScanId, Store};

use crate::config::{Config, RootConfig};
use crate::runner::{AlreadyRunning, Runner};

/// Compression level for snapshot bodies. Level 3 is zstd's default: on the
/// measured corpus it gets within a few percent of level 19 while costing a
/// fraction of the CPU, which matters on the ARM NAS boxes this runs on.
const ZSTD_LEVEL: i32 = 3;

pub struct AppState {
    runner: Arc<Runner>,
    token: String,
    allow_adhoc_scans: bool,
    max_upload_bytes: usize,
    started: Instant,
}

pub fn router(runner: Arc<Runner>, config: &Config, token: String) -> Router {
    let state = Arc::new(AppState {
        runner,
        token,
        allow_adhoc_scans: config.server.allow_adhoc_scans,
        max_upload_bytes: config.server.max_upload_bytes,
        started: Instant::now(),
    });

    // /health stays outside the auth layer so an uptime check or a container
    // healthcheck works without handing the token to the monitoring system. It
    // deliberately reveals nothing but liveness and version.
    let public = Router::new().route("/health", get(health));

    let private = Router::new()
        .route("/scans", get(list_scans).post(trigger_scan))
        .route("/scans/{id}", get(scan_meta))
        .route("/scans/{id}/download", get(download_scan))
        .route("/snapshots", post(receive_snapshot))
        .layer(DefaultBodyLimit::max(config.server.max_upload_bytes))
        .route("/status", get(status))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_token));

    public.merge(private).with_state(state)
}

pub async fn serve(runner: Arc<Runner>, config: &Config, token: String) -> Result<()> {
    let app = router(runner, config, token);
    let listener = tokio::net::TcpListener::bind(config.server.listen)
        .await
        .with_context(|| format!("binding {}", config.server.listen))?;
    let addr = listener.local_addr()?;
    eprintln!("spacetrace-agent listening on http://{addr}");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("http server failed")?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        // systemd stops services with SIGTERM, so honouring it is what makes
        // `systemctl stop` clean rather than a kill after the timeout.
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    eprintln!("shutting down");
}

// ------------------------------------------------------------------ auth

async fn require_token(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");

    if !constant_time_eq(presented.as_bytes(), state.token.as_bytes()) {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            Json(ApiError {
                error: "missing or invalid bearer token".into(),
            }),
        )
            .into_response();
    }
    next.run(request).await
}

/// Compare without an early exit on the first differing byte. Length is not
/// hidden, which is fine: the token length is not the secret.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// -------------------------------------------------------------- handlers

#[derive(Serialize)]
struct Health {
    status: &'static str,
    version: &'static str,
}

async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

#[derive(Serialize)]
struct Status {
    version: &'static str,
    host: String,
    uptime_s: u64,
    roots: Vec<String>,
    scanning: Vec<String>,
    snapshots: usize,
}

async fn status(State(state): State<Arc<AppState>>) -> Result<Json<Status>, ApiFailure> {
    let runner = &state.runner;
    let snapshots = runner.list_scans()?.len();
    Ok(Json(Status {
        version: env!("CARGO_PKG_VERSION"),
        host: runner.host().to_string(),
        uptime_s: state.started.elapsed().as_secs(),
        roots: runner
            .roots()
            .iter()
            .map(|r| r.path.to_string_lossy().into_owned())
            .collect(),
        scanning: runner
            .in_flight_roots()
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
        snapshots,
    }))
}

async fn list_scans(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<spacetrace_store::ScanMeta>>, ApiFailure> {
    Ok(Json(state.runner.list_scans()?))
}

async fn scan_meta(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<ScanId>,
) -> Result<Json<spacetrace_store::ScanMeta>, ApiFailure> {
    match state.runner.scan_meta(id)? {
        Some(meta) => Ok(Json(meta)),
        None => Err(ApiFailure::not_found(format!("no snapshot with id {id}"))),
    }
}

/// The snapshot itself: a standalone SQLite file, which the client can open
/// with exactly the same code it uses for a local one.
async fn download_scan(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<ScanId>,
    headers: HeaderMap,
) -> Result<Response, ApiFailure> {
    if state.runner.scan_meta(id)?.is_none() {
        return Err(ApiFailure::not_found(format!("no snapshot with id {id}")));
    }

    let wants_zstd = headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|e| e.trim().starts_with("zstd")));

    let runner = Arc::clone(&state.runner);
    let (bytes, encoded) =
        tokio::task::spawn_blocking(move || export_bytes(&runner, id, wants_zstd))
            .await
            .map_err(|e| ApiFailure::internal(format!("export task failed: {e}")))??;

    let mut response = Response::builder()
        .header(header::CONTENT_TYPE, "application/vnd.sqlite3")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"spacetrace-{id}.sqlite\""),
        );
    if encoded {
        response = response.header(header::CONTENT_ENCODING, "zstd");
    }
    response
        .body(Body::from(bytes))
        .map_err(|e| ApiFailure::internal(format!("building response: {e}")))
}

/// Export to a temporary file, then read it back. Snapshots are tens of
/// megabytes at most (measured: ~50 B per entry), so holding one in memory is
/// cheaper than the machinery needed to stream a file that is being written.
fn export_bytes(
    runner: &Runner,
    id: ScanId,
    compress: bool,
) -> Result<(Vec<u8>, bool), ApiFailure> {
    let dir = tempfile::tempdir().map_err(|e| ApiFailure::internal(e.to_string()))?;
    let path = dir.path().join("snapshot.sqlite");
    runner.export(id, &path)?;
    let raw = std::fs::read(&path).map_err(|e| ApiFailure::internal(e.to_string()))?;

    if !compress {
        return Ok((raw, false));
    }
    match zstd::encode_all(raw.as_slice(), ZSTD_LEVEL) {
        Ok(packed) => Ok((packed, true)),
        // Compression is an optimisation; failing it should not fail the
        // download.
        Err(_) => Ok((raw, false)),
    }
}

#[derive(Deserialize)]
struct ScanRequest {
    root: std::path::PathBuf,
    #[serde(default)]
    label: Option<String>,
}

#[derive(Serialize)]
struct Accepted {
    status: &'static str,
    root: String,
}

/// Start a scan and return immediately.
///
/// Scanning a large root takes minutes, well past any sensible HTTP timeout, so
/// this answers 202 and lets the caller watch `GET /scans` for the new
/// snapshot. That also avoids inventing a job registry for a single-user agent.
async fn trigger_scan(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ScanRequest>,
) -> Result<(StatusCode, Json<Accepted>), ApiFailure> {
    let configured = state.runner.configured_root(&body.root).cloned();
    let root = match configured {
        Some(mut r) => {
            // An explicit label on the request overrides the configured one.
            if body.label.is_some() {
                r.label = body.label.clone();
            }
            r
        }
        None => {
            if !state.allow_adhoc_scans {
                return Err(ApiFailure::forbidden(format!(
                    "{} is not a configured root and allow_adhoc_scans is off",
                    body.root.display()
                )));
            }
            adhoc_root(body.root.clone(), body.label.clone())
        }
    };

    if !root.path.is_dir() {
        return Err(ApiFailure::bad_request(format!(
            "not a directory: {}",
            root.path.display()
        )));
    }
    if state.runner.in_flight_roots().contains(&root.path) {
        return Err(ApiFailure::conflict(format!(
            "a scan of {} is already running",
            root.path.display()
        )));
    }

    let runner = Arc::clone(&state.runner);
    let reported = root.path.to_string_lossy().into_owned();
    tokio::task::spawn_blocking(move || match runner.scan_root(&root) {
        Ok(outcome) => eprintln!(
            "scanned {} -> snapshot #{} ({} files, {} ms)",
            outcome.root, outcome.scan_id, outcome.files, outcome.duration_ms
        ),
        Err(err) if err.downcast_ref::<AlreadyRunning>().is_some() => {
            eprintln!("skipped: {err}")
        }
        Err(err) => eprintln!("scan failed: {err:#}"),
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(Accepted {
            status: "started",
            root: reported,
        }),
    ))
}

#[derive(Serialize)]
struct Imported {
    imported: Vec<ScanId>,
    /// Snapshots already present, identified by host, root and start time.
    skipped: usize,
}

/// Accept a snapshot pushed by another agent.
///
/// The body is a snapshot file exactly as `GET /scans/{id}/download` produces
/// it, so an agent can push straight to a hub with no format in between.
async fn receive_snapshot(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Imported>, ApiFailure> {
    let compressed = headers
        .get(header::CONTENT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("zstd"));

    let runner = Arc::clone(&state.runner);
    let limit = state.max_upload_bytes;
    tokio::task::spawn_blocking(move || import_bytes(&runner, &body, compressed, limit))
        .await
        .map_err(|e| ApiFailure::internal(format!("import task failed: {e}")))?
        .map(Json)
}

/// Decompress, refusing to grow past `limit` bytes.
///
/// Reads one byte past the limit so hitting it exactly is not mistaken for
/// overflow, and so a body that would keep expanding is stopped rather than
/// buffered.
pub(crate) fn decode_zstd_bounded(data: &[u8], limit: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read;

    let decoder = zstd::stream::Decoder::new(data)?;
    let mut out = Vec::new();
    decoder.take(limit as u64 + 1).read_to_end(&mut out)?;
    if out.len() > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("decompressed body exceeds the {limit} byte limit"),
        ));
    }
    Ok(out)
}

fn import_bytes(
    runner: &Runner,
    body: &[u8],
    compressed: bool,
    limit: usize,
) -> Result<Imported, ApiFailure> {
    let raw = if compressed {
        // max_upload_bytes only bounds the compressed body, so an unbounded
        // decode would let a few kilobytes of zstd expand until the agent dies.
        decode_zstd_bounded(body, limit)
            .map_err(|e| ApiFailure::bad_request(format!("body is not valid zstd: {e}")))?
    } else {
        body.to_vec()
    };

    let dir = tempfile::tempdir().map_err(|e| ApiFailure::internal(e.to_string()))?;
    let path = dir.path().join("incoming.sqlite");
    std::fs::write(&path, &raw).map_err(|e| ApiFailure::internal(e.to_string()))?;

    let mut store = runner.open_store()?;
    // A body that is not a snapshot is the sender's mistake, not ours.
    let imported = store
        .import_snapshot(&path)
        .map_err(|e| ApiFailure::bad_request(format!("{e:#}")))?;

    let total = Store::open(&path)
        .and_then(|s| s.list())
        .map(|l| l.len())
        .unwrap_or(imported.len());
    Ok(Imported {
        skipped: total.saturating_sub(imported.len()),
        imported,
    })
}

fn adhoc_root(path: std::path::PathBuf, label: Option<String>) -> RootConfig {
    let mut root = RootConfig::new(path);
    root.label = label;
    root
}

// --------------------------------------------------------------- errors

#[derive(Serialize)]
struct ApiError {
    error: String,
}

pub struct ApiFailure {
    status: StatusCode,
    message: String,
}

impl ApiFailure {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        ApiFailure {
            status,
            message: message.into(),
        }
    }
    fn not_found(m: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, m)
    }
    fn bad_request(m: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, m)
    }
    fn forbidden(m: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, m)
    }
    fn conflict(m: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, m)
    }
    fn internal(m: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, m)
    }
}

impl From<anyhow::Error> for ApiFailure {
    fn from(err: anyhow::Error) -> Self {
        if err.downcast_ref::<AlreadyRunning>().is_some() {
            return ApiFailure::conflict(err.to_string());
        }
        // `{:#}` keeps the context chain, which is what makes a failed scan
        // readable in a client that only sees the JSON body.
        ApiFailure::internal(format!("{err:#}"))
    }
}

impl IntoResponse for ApiFailure {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ApiError {
                error: self.message,
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_decompression_bomb_is_refused_instead_of_buffered() {
        // ~100 MB of zeros compresses to a few kilobytes.
        let bomb = zstd::encode_all(vec![0u8; 100 * 1024 * 1024].as_slice(), 3).unwrap();
        assert!(bomb.len() < 100_000, "sanity: the bomb should be small");

        let err = decode_zstd_bounded(&bomb, 1024).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

        // A body inside the limit still decodes intact.
        let ok = zstd::encode_all(b"hello".as_slice(), 3).unwrap();
        assert_eq!(decode_zstd_bounded(&ok, 1024).unwrap(), b"hello");
    }

    #[test]
    fn a_body_exactly_at_the_limit_is_allowed() {
        let payload = vec![7u8; 4096];
        let packed = zstd::encode_all(payload.as_slice(), 3).unwrap();
        assert_eq!(decode_zstd_bounded(&packed, 4096).unwrap(), payload);
        assert!(decode_zstd_bounded(&packed, 4095).is_err());
    }

    #[test]
    fn constant_time_eq_still_compares_correctly() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreT"));
        assert!(!constant_time_eq(b"secret", b"secre"));
        assert!(!constant_time_eq(b"", b"x"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn an_adhoc_root_inherits_the_configured_defaults() {
        let root = adhoc_root(std::path::PathBuf::from("/var/log"), Some("m".into()));
        assert_eq!(root.path, std::path::PathBuf::from("/var/log"));
        assert_eq!(root.label.as_deref(), Some("m"));
        assert!(root.dedupe_hardlinks, "must match the config-file default");
        assert!(!root.one_file_system);
        assert!(root.keep.is_none());
        assert!(root.schedule.is_none());
    }
}
