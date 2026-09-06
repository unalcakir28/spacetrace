//! End-to-end tests against a real server on a real socket.
//!
//! These go through reqwest rather than calling handlers directly, because the
//! things most likely to break — the auth layer, status codes, content
//! encoding, the snapshot body being a genuine SQLite file — only exist once
//! the request has been through the whole stack.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use spacetrace_agent::config::Config;
use spacetrace_agent::runner::Runner;
use spacetrace_agent::serve;
use spacetrace_store::Store;
use tempfile::TempDir;

const TOKEN: &str = "test-token-8f2a";

struct Agent {
    addr: SocketAddr,
    runner: Arc<Runner>,
    /// Kept alive so the temporary directories outlive the server.
    _home: TempDir,
    _scanned: TempDir,
}

impl Agent {
    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}

fn scannable_dir() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("big.bin"), vec![0u8; 40_000]).unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("sub/small.txt"), b"hello").unwrap();
    dir
}

/// Start an agent whose single configured root is a temporary directory.
async fn start_agent(allow_adhoc: bool) -> Agent {
    let home = tempfile::tempdir().unwrap();
    let scanned = scannable_dir();

    let toml = format!(
        "db = {:?}\n\
         [server]\n\
         token = {:?}\n\
         allow_adhoc_scans = {}\n\
         [[roots]]\n\
         path = {:?}\n",
        home.path().join("snapshots.sqlite").to_string_lossy(),
        TOKEN,
        allow_adhoc,
        scanned.path().to_string_lossy(),
    );
    let config: Config = toml::from_str(&toml).unwrap();
    let runner = Arc::new(Runner::new(&config));
    let app = serve::router(Arc::clone(&runner), &config, TOKEN.to_string());

    // Port 0 lets the OS pick, so tests never collide on a fixed port.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    Agent {
        addr,
        runner,
        _home: home,
        _scanned: scanned,
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

/// Wait for a scan triggered by `POST /scans` to show up, since that endpoint
/// answers before the scan finishes on purpose.
async fn wait_for_snapshot(agent: &Agent) -> i64 {
    for _ in 0..100 {
        if let Some(first) = agent.runner.list_scans().unwrap().first() {
            return first.id;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no snapshot appeared within 5 seconds");
}

fn is_sqlite(bytes: &[u8]) -> bool {
    bytes.starts_with(b"SQLite format 3\0")
}

fn open_snapshot_bytes(bytes: &[u8], dir: &Path) -> Store {
    let path = dir.join("downloaded.sqlite");
    std::fs::write(&path, bytes).unwrap();
    Store::open(&path).unwrap()
}

// ------------------------------------------------------------------- auth

#[tokio::test]
async fn health_needs_no_token_and_leaks_nothing() {
    let agent = start_agent(false).await;
    let response = client().get(agent.url("/health")).send().await.unwrap();

    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert!(body["version"].is_string());
    // Monitoring should not be able to read the machine's layout.
    assert!(body.get("roots").is_none(), "health must not list roots");
    assert!(body.get("host").is_none(), "health must not name the host");
}

#[tokio::test]
async fn protected_routes_reject_a_missing_token() {
    let agent = start_agent(false).await;
    for path in ["/scans", "/status", "/scans/1", "/scans/1/download"] {
        let response = client().get(agent.url(path)).send().await.unwrap();
        assert_eq!(response.status(), 401, "{path} should require a token");
        assert!(
            response.headers().contains_key("www-authenticate"),
            "{path} should say how to authenticate"
        );
    }
}

#[tokio::test]
async fn a_wrong_token_is_rejected() {
    let agent = start_agent(false).await;
    for wrong in [
        "",
        "nope",
        "test-token-8f2b",  // one byte off at the end
        "Test-Token-8F2A",  // case must matter
        "test-token-8f2",   // a prefix
        "test-token-8f2ax", // the real token plus one byte
        " test-token-8f2a", // leading whitespace is not stripped
    ] {
        let response = client()
            .get(agent.url("/scans"))
            .bearer_auth(wrong)
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            401,
            "token {wrong:?} must not be accepted"
        );
    }
}

/// Not a hole in the comparison: HTTP itself strips optional trailing
/// whitespace from header values (RFC 7230 §3.2.4), so a client cannot send a
/// token with a trailing space even if it tries. Pinned so nobody "fixes" the
/// comparison to be whitespace-sensitive and breaks real clients.
#[tokio::test]
async fn trailing_whitespace_is_removed_by_the_protocol_before_we_see_it() {
    let agent = start_agent(false).await;
    let response = client()
        .get(agent.url("/scans"))
        .bearer_auth(format!("{TOKEN}   "))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn a_correct_token_is_accepted() {
    let agent = start_agent(false).await;
    let response = client()
        .get(agent.url("/scans"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let scans: Vec<serde_json::Value> = response.json().await.unwrap();
    assert!(scans.is_empty(), "a fresh agent has no snapshots");
}

// ------------------------------------------------------------------ scans

#[tokio::test]
async fn a_triggered_scan_produces_a_downloadable_snapshot() {
    let agent = start_agent(false).await;
    let root = agent._scanned.path().to_string_lossy().into_owned();

    let response = client()
        .post(agent.url("/scans"))
        .bearer_auth(TOKEN)
        .json(&serde_json::json!({ "root": root, "label": "manual" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202, "scanning is asynchronous");
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["status"], "started");

    let id = wait_for_snapshot(&agent).await;

    // The metadata endpoint sees it.
    let meta: serde_json::Value = client()
        .get(agent.url(&format!("/scans/{id}")))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(meta["id"], id);
    assert_eq!(meta["label"], "manual");
    assert_eq!(meta["files"], 2);

    // And the body really is a snapshot database.
    let response = client()
        .get(agent.url(&format!("/scans/{id}/download")))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers()["content-type"],
        "application/vnd.sqlite3"
    );
    let bytes = response.bytes().await.unwrap();
    assert!(is_sqlite(&bytes), "body must be a SQLite file");

    let work = tempfile::tempdir().unwrap();
    let store = open_snapshot_bytes(&bytes, work.path());
    let scans = store.list().unwrap();
    assert_eq!(scans.len(), 1);
    assert_eq!(scans[0].label.as_deref(), Some("manual"));
    let (tree, _) = store.load(scans[0].id).unwrap();
    assert!(tree.total_size() >= 40_000);
}

#[tokio::test]
async fn a_snapshot_can_be_downloaded_zstd_compressed() {
    let agent = start_agent(false).await;
    let root = agent._scanned.path().to_string_lossy().into_owned();
    client()
        .post(agent.url("/scans"))
        .bearer_auth(TOKEN)
        .json(&serde_json::json!({ "root": root }))
        .send()
        .await
        .unwrap();
    let id = wait_for_snapshot(&agent).await;

    // reqwest would transparently decode encodings it knows about; zstd is not
    // among the enabled features here, so the raw body arrives untouched.
    let response = client()
        .get(agent.url(&format!("/scans/{id}/download")))
        .bearer_auth(TOKEN)
        .header("accept-encoding", "zstd")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-encoding"], "zstd");

    let packed = response.bytes().await.unwrap();
    assert!(!is_sqlite(&packed), "the body should still be compressed");
    let raw = zstd::decode_all(&packed[..]).expect("body must be valid zstd");
    assert!(is_sqlite(&raw));
    assert!(
        packed.len() < raw.len(),
        "compression should actually shrink it ({} -> {})",
        raw.len(),
        packed.len()
    );
}

#[tokio::test]
async fn unknown_snapshots_are_404_not_500() {
    let agent = start_agent(false).await;
    for path in ["/scans/999", "/scans/999/download"] {
        let response = client()
            .get(agent.url(path))
            .bearer_auth(TOKEN)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 404, "{path}");
        let body: serde_json::Value = response.json().await.unwrap();
        assert!(body["error"].as_str().unwrap().contains("999"));
    }
}

#[tokio::test]
async fn an_unconfigured_root_is_refused_unless_adhoc_scans_are_enabled() {
    let agent = start_agent(false).await;
    let elsewhere = scannable_dir();

    let response = client()
        .post(agent.url("/scans"))
        .bearer_auth(TOKEN)
        .json(&serde_json::json!({ "root": elsewhere.path().to_string_lossy() }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("allow_adhoc_scans"));
    assert!(agent.runner.list_scans().unwrap().is_empty());
}

#[tokio::test]
async fn an_unconfigured_root_is_accepted_when_adhoc_scans_are_enabled() {
    let agent = start_agent(true).await;
    let elsewhere = scannable_dir();

    let response = client()
        .post(agent.url("/scans"))
        .bearer_auth(TOKEN)
        .json(&serde_json::json!({ "root": elsewhere.path().to_string_lossy() }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    wait_for_snapshot(&agent).await;
}

#[tokio::test]
async fn scanning_something_that_is_not_a_directory_is_a_client_error() {
    let agent = start_agent(true).await;
    let file = agent._scanned.path().join("big.bin");

    let response = client()
        .post(agent.url("/scans"))
        .bearer_auth(TOKEN)
        .json(&serde_json::json!({ "root": file.to_string_lossy() }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
}

#[tokio::test]
async fn status_reports_what_the_agent_is_configured_to_do() {
    let agent = start_agent(false).await;
    let status: serde_json::Value = client()
        .get(agent.url("/status"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(status["snapshots"], 0);
    assert_eq!(status["roots"].as_array().unwrap().len(), 1);
    assert!(status["scanning"].as_array().unwrap().is_empty());
    assert!(status["host"].is_string());
}

// ------------------------------------------------------------------- push

#[tokio::test]
async fn a_snapshot_pushed_between_agents_arrives_intact_and_only_once() {
    let sender = start_agent(false).await;
    let receiver = start_agent(false).await;

    // Give the sender something to push.
    sender.runner.scan_root(&sender.runner.roots()[0]).unwrap();
    let original = sender.runner.list_scans().unwrap()[0].clone();

    let base = format!("http://{}", receiver.addr);
    let outcome = spacetrace_agent::push::push(
        &sender.runner,
        &base,
        TOKEN,
        spacetrace_agent::push::Selector::Latest,
        true,
    )
    .await
    .unwrap();

    assert_eq!(outcome.scan_id, original.id);
    assert!(outcome.compressed);
    assert_eq!(outcome.imported.len(), 1);
    assert_eq!(outcome.skipped, 0);

    // The receiver's copy must be comparable with the sender's own history:
    // same host, same root, same instant.
    let landed = receiver.runner.list_scans().unwrap();
    assert_eq!(landed.len(), 1);
    assert_eq!(landed[0].host, original.host);
    assert_eq!(landed[0].root, original.root);
    assert_eq!(landed[0].started_at, original.started_at);
    assert_eq!(landed[0].total_size, original.total_size);

    // Pushing the same snapshot again must not duplicate it.
    let again = spacetrace_agent::push::push(
        &sender.runner,
        &base,
        TOKEN,
        spacetrace_agent::push::Selector::Latest,
        true,
    )
    .await
    .unwrap();
    assert!(again.imported.is_empty());
    assert_eq!(again.skipped, 1);
    assert_eq!(receiver.runner.list_scans().unwrap().len(), 1);
}

#[tokio::test]
async fn pushing_uncompressed_also_works() {
    let sender = start_agent(false).await;
    let receiver = start_agent(false).await;
    sender.runner.scan_root(&sender.runner.roots()[0]).unwrap();

    let outcome = spacetrace_agent::push::push(
        &sender.runner,
        &format!("http://{}", receiver.addr),
        TOKEN,
        spacetrace_agent::push::Selector::Latest,
        false,
    )
    .await
    .unwrap();

    assert!(!outcome.compressed);
    assert_eq!(outcome.imported.len(), 1);
}

#[tokio::test]
async fn pushing_with_a_bad_token_reports_the_servers_reason() {
    let sender = start_agent(false).await;
    let receiver = start_agent(false).await;
    sender.runner.scan_root(&sender.runner.roots()[0]).unwrap();

    let err = spacetrace_agent::push::push(
        &sender.runner,
        &format!("http://{}", receiver.addr),
        "wrong-token",
        spacetrace_agent::push::Selector::Latest,
        true,
    )
    .await
    .unwrap_err();

    let message = format!("{err:#}");
    assert!(message.contains("401"), "{message}");
    assert!(receiver.runner.list_scans().unwrap().is_empty());
}

#[tokio::test]
async fn a_body_that_is_not_a_snapshot_is_rejected_as_a_client_error() {
    let agent = start_agent(false).await;
    let response = client()
        .post(agent.url("/snapshots"))
        .bearer_auth(TOKEN)
        .body(vec![0u8; 512])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert!(agent.runner.list_scans().unwrap().is_empty());
}

#[tokio::test]
async fn a_body_claiming_to_be_zstd_but_is_not_is_rejected() {
    let agent = start_agent(false).await;
    let response = client()
        .post(agent.url("/snapshots"))
        .bearer_auth(TOKEN)
        .header("content-encoding", "zstd")
        .body(b"definitely not zstd".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("zstd"));
}

#[tokio::test]
async fn pushing_requires_a_token_on_the_receiver() {
    let agent = start_agent(false).await;
    let response = client()
        .post(agent.url("/snapshots"))
        .body(vec![0u8; 16])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
}
