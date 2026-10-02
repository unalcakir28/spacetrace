//! `pull` against an agent that sends a forged snapshot.
//!
//! The agent here is a few lines of HTTP over a real socket, answering the two
//! requests `pull` makes with the bytes a hostile agent would choose: a
//! snapshot whose digest is correct and whose tree is not.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output};

mod common;
use common::{forged_snapshot, BIN};

fn spacetrace(db: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .arg("--db")
        .arg(db)
        .args(args)
        .env("SPACETRACE_NO_UPDATE_CHECK", "1")
        .env("SPACETRACE_REMOTES", db.with_extension("remotes.toml"))
        .output()
        .unwrap()
}

/// Serve `/scans/<id>` and `/scans/<id>/download` until the test ends.
fn serve(meta: String, body: Vec<u8>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            reader.read_line(&mut request).unwrap();
            // The headers, read and ignored.
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() > 2 {
                line.clear();
            }
            let path = request.split_whitespace().nth(1).unwrap_or_default();
            let (kind, payload) = match path.ends_with("/download") {
                true => ("application/octet-stream", body.clone()),
                false => ("application/json", meta.clone().into_bytes()),
            };
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n",
                payload.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&payload);
        }
    });
    format!("http://{address}")
}

#[test]
fn a_forged_snapshot_from_an_agent_leaves_the_local_database_unchanged() {
    let work = tempfile::tempdir().unwrap();
    let local = work.path().join("local.sqlite");
    let tree = work.path().join("tree");
    std::fs::create_dir(&tree).unwrap();
    std::fs::write(tree.join("file"), b"local content").unwrap();
    assert!(
        spacetrace(&local, &["scan", "--save", tree.to_str().unwrap()])
            .status
            .success()
    );
    let before = spacetrace(&local, &["--json", "scans"]).stdout;

    let forged = forged_snapshot(work.path());
    let listing: serde_json::Value =
        serde_json::from_slice(&spacetrace(&forged, &["--json", "scans"]).stdout).unwrap();
    let meta = listing[0].clone();
    let id = meta["id"].as_i64().unwrap().to_string();
    let agent = serve(meta.to_string(), std::fs::read(&forged).unwrap());

    let out = spacetrace(
        &local,
        &["--remote", &agent, "--token", "t", "pull", "--scan", &id],
    );
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("not a usable tree"), "{text}");
    assert_eq!(
        spacetrace(&local, &["--json", "scans"]).stdout,
        before,
        "the local database must be exactly as it was"
    );
}
