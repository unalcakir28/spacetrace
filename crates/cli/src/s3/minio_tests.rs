//! Against a real S3 server: MinIO, or anything else that speaks the API.
//!
//! Skipped, saying why, unless `SPACETRACE_TEST_S3_ENDPOINT` names one — CI
//! has no Docker, and a test that fails for want of a server teaches everyone
//! to ignore it. To run it:
//!
//! ```sh
//! # Any MinIO binary will do. Its Docker Hub image and dl.min.io downloads
//! # were gone when this was written (October 2026), so build one:
//! #   GOBIN=$PWD go install github.com/minio/minio@latest
//! MINIO_ROOT_USER=minioadmin MINIO_ROOT_PASSWORD=minioadmin ./minio server /tmp/minio-data &
//! SPACETRACE_TEST_S3_ENDPOINT=http://127.0.0.1:9000 \
//! SPACETRACE_TEST_S3_ACCESS_KEY=minioadmin SPACETRACE_TEST_S3_SECRET_KEY=minioadmin \
//!   cargo test -p spacetrace-cli s3::minio_tests -- --nocapture
//! ```
//!
//! It creates a bucket of its own, fills it through the same signer the
//! listing uses, and deletes what it made at the end. The signer is checked
//! by the server here, not by a vector: MinIO verifies every signature.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use spacetrace_scan_core::{EntryKind, ScanProgress, ScanStats};
use spacetrace_store::{Integrity, Store};

use super::client::{Bucket, Client};
use super::config::{Endpoint, Settings, DEFAULT_REGION};
use super::listing::DEFAULT_WORKERS;
use super::sigv4::Credentials;
use super::{scan_with, S3Url};

struct Server {
    endpoint: Endpoint,
    credentials: Credentials,
}

fn server() -> Option<Server> {
    let Ok(endpoint) = std::env::var("SPACETRACE_TEST_S3_ENDPOINT") else {
        eprintln!(
            "skipped: SPACETRACE_TEST_S3_ENDPOINT is not set, so there is no S3 server to test \
             against (see crates/cli/src/s3/minio_tests.rs to start one)"
        );
        return None;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    Some(Server {
        endpoint: Endpoint::parse(&endpoint).expect("SPACETRACE_TEST_S3_ENDPOINT"),
        credentials: Credentials {
            access_key_id: var("SPACETRACE_TEST_S3_ACCESS_KEY", "minioadmin"),
            secret_access_key: var("SPACETRACE_TEST_S3_SECRET_KEY", "minioadmin"),
            session_token: None,
        },
    })
}

impl Server {
    fn settings(&self, credentials: Option<Credentials>) -> Settings {
        Settings {
            credentials: credentials.map(Into::into),
            region: DEFAULT_REGION.into(),
            endpoint: Some(self.endpoint.clone()),
        }
    }

    fn signed(&self) -> Settings {
        self.settings(Some(self.credentials.clone()))
    }
}

/// A bucket that exists for one test and is emptied and removed after it,
/// even when an assertion fails half way.
struct TestBucket {
    client: Client,
    bucket: Bucket,
    keys: Vec<String>,
}

impl TestBucket {
    fn create(server: &Server, tag: &str) -> TestBucket {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let name = format!(
            "spacetrace-{tag}-{}-{}",
            std::process::id(),
            nanos % 1_000_000_000
        );
        let client = Client::new(server.signed()).unwrap();
        let bucket = Bucket { name };
        let made = client.send("PUT", &bucket, &[], Vec::new()).unwrap();
        assert_eq!(made.status, 200, "creating the bucket: {}", made.body);
        TestBucket {
            client,
            bucket,
            keys: Vec::new(),
        }
    }

    fn put(&mut self, key: &str, bytes: usize) {
        let body = vec![b'x'; bytes];
        let put = self
            .client
            .send_key("PUT", &self.bucket, Some(key), &[], body)
            .unwrap();
        assert_eq!(put.status, 200, "PUT {key:?}: {}", put.body);
        self.keys.push(key.to_string());
    }

    fn url(&self) -> S3Url {
        S3Url::parse(&format!("s3://{}", self.bucket.name)).unwrap()
    }
}

impl Drop for TestBucket {
    fn drop(&mut self) {
        for key in &self.keys {
            let _ = self
                .client
                .send_key("DELETE", &self.bucket, Some(key), &[], Vec::new());
        }
        let _ = self.client.send("DELETE", &self.bucket, &[], Vec::new());
    }
}

/// The keys every test fills its bucket with, and the size of each.
fn tricky_keys() -> Vec<(&'static str, usize)> {
    vec![
        ("top.txt", 11),
        ("dir/a b.txt", 13),
        ("dir/plus+sign.txt", 17),
        ("dir/100%.txt", 19),
        ("dir/ünï©ødé/ファイル.txt", 23),
        ("dir/deeper/still/deepest.bin", 29),
        ("ctl\u{1}char.txt", 31),
        ("xml&<>\"'.txt", 37),
        ("marker/", 0),
    ]
}

#[test]
fn a_real_bucket_lists_into_a_tree_that_survives_the_store() {
    let Some(server) = server() else { return };
    let mut bucket = TestBucket::create(&server, "list");
    let mut expected_bytes = 0u64;
    for (key, size) in tricky_keys() {
        bucket.put(key, size);
        expected_bytes += size as u64;
    }
    // Past one page of 1000, and not a multiple of it, so the last page is a
    // partial one reached through a continuation token.
    for i in 0..2500 {
        bucket.put(&format!("many/{:02}/obj-{i:05}", i % 7), 3);
        expected_bytes += 3;
    }

    let progress = Arc::new(ScanProgress::default());
    let scan = scan_with(
        &bucket.url(),
        server.signed(),
        None,
        1,
        Arc::clone(&progress),
    )
    .unwrap();
    let tree = &scan.tree;

    assert_eq!(scan.pages, 3, "2509 keys at 1000 a page, one stream");
    assert_eq!(scan.stats.objects, 2509);
    assert_eq!(scan.stats.bytes, expected_bytes);
    assert_eq!(scan.stats.folder_markers, 1);
    assert_eq!(tree.total_alloc(), expected_bytes);
    assert_eq!(
        tree.total_size(),
        expected_bytes,
        "the marker holds nothing"
    );
    assert_eq!(tree.node(tree.root()).files, 2508);
    assert_eq!(
        progress.files.load(Ordering::Relaxed),
        2508,
        "the counter moved"
    );
    assert_eq!(progress.bytes.load(Ordering::Relaxed), expected_bytes);
    assert_eq!(scan.host, server.endpoint.authority);

    for (key, size) in tricky_keys() {
        let path = key.trim_end_matches('/');
        let id = tree
            .find(path)
            .unwrap_or_else(|| panic!("{key:?} is not in the tree"));
        let node = tree.node(id);
        match key.ends_with('/') {
            true => assert_eq!((node.kind, node.children_len), (EntryKind::Dir, 0), "{key}"),
            false => assert_eq!(
                (node.kind, node.size),
                (EntryKind::File, size as u64),
                "{key}"
            ),
        }
        assert!(node.mtime > 1_700_000_000, "{key} has a LastModified");
    }
    let many = tree.node(tree.find("many").unwrap());
    assert_eq!((many.files, many.dirs), (2500, 7));

    let stats = ScanStats {
        files: u64::from(tree.node(tree.root()).files),
        dirs: u64::from(tree.node(tree.root()).dirs),
        errors: 0,
        hardlinks_deduped: 0,
        clones_deduped: 0,
        error_samples: Vec::new(),
        duration_ms: scan.duration_ms,
        capacity: None,
        ..Default::default()
    };
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("s.sqlite")).unwrap();
    let id = store.save(tree, &stats, &scan.host, None).unwrap();
    let (loaded, meta) = store.load(id).unwrap();
    assert_eq!(meta.root, bucket.url().root());
    assert_eq!(loaded.total_alloc(), expected_bytes);
    assert_eq!(loaded.len(), tree.len());
    assert_eq!(store.verify(id).unwrap(), Integrity::Intact);

    // The same bucket in pages of 7 is the same tree: pagination adds
    // nothing and loses nothing.
    let small = scan_with(
        &bucket.url(),
        server.signed(),
        Some(7),
        1,
        Arc::new(ScanProgress::default()),
    )
    .unwrap();
    assert_eq!(small.pages, 2509_u64.div_ceil(7));
    assert_eq!(small.tree.total_alloc(), expected_bytes);
    assert_eq!(small.tree.len(), tree.len());

    // And listed by many workers at once, the same tree id for id, the same
    // totals, the same counters — on MinIO's own reading of `start-after`.
    for (page, workers) in [(None, DEFAULT_WORKERS), (Some(7), 3), (Some(100), 16)] {
        let progress = Arc::new(ScanProgress::default());
        let parallel = scan_with(
            &bucket.url(),
            server.signed(),
            page,
            workers,
            Arc::clone(&progress),
        )
        .unwrap();
        let what = format!("page {page:?}, {workers} workers");
        assert_eq!(super::arena(&parallel.tree), super::arena(tree), "{what}");
        assert_eq!(parallel.stats, scan.stats, "{what}");
        assert_eq!(progress.files.load(Ordering::Relaxed), 2508, "{what}");
        assert_eq!(
            progress.bytes.load(Ordering::Relaxed),
            expected_bytes,
            "{what}"
        );
    }
}

#[test]
fn a_prefix_lists_one_folder_and_nothing_beside_it() {
    let Some(server) = server() else { return };
    let mut bucket = TestBucket::create(&server, "prefix");
    bucket.put("photos/a.jpg", 5);
    bucket.put("photos/2024/b.jpg", 7);
    bucket.put("photos2024/not-this.jpg", 1000);
    let url = S3Url::parse(&format!("s3://{}/photos", bucket.bucket.name)).unwrap();
    let scan = scan_with(
        &url,
        server.signed(),
        None,
        DEFAULT_WORKERS,
        Arc::new(ScanProgress::default()),
    )
    .unwrap();
    assert_eq!(scan.tree.total_size(), 12);
    assert!(scan.tree.find("2024/b.jpg").is_some());
}

/// A role assumed through the server's own STS, end to end: the profile files
/// in a home of their own, the chain picking `role_arn` + `source_profile`,
/// a signed AssumeRole, and a listing signed with the temporary keys. MinIO
/// refuses a temporary key without its session token, so a listing that
/// works is the proof that `x-amz-security-token` went out.
#[test]
fn a_profile_role_assumed_through_sts_lists_the_bucket() {
    let Some(server) = server() else { return };
    let mut bucket = TestBucket::create(&server, "role");
    bucket.put("a/b.txt", 5);
    bucket.put("c.txt", 7);

    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(home.path().join(".aws")).unwrap();
    std::fs::write(
        home.path().join(".aws").join("credentials"),
        format!(
            "[base]\naws_access_key_id = {}\naws_secret_access_key = {}\n",
            server.credentials.access_key_id, server.credentials.secret_access_key
        ),
    )
    .unwrap();
    std::fs::write(
        home.path().join(".aws").join("config"),
        "[profile reader]\nrole_arn = arn:aws:iam::123456789012:role/reader\n\
         source_profile = base\nrole_session_name = spacetrace-test\nduration_seconds = 900\n",
    )
    .unwrap();
    let endpoint = format!("{}://{}", server.endpoint.scheme, server.endpoint.authority);
    let env: std::collections::HashMap<&str, String> = [
        ("HOME", home.path().display().to_string()),
        ("AWS_ENDPOINT_URL", endpoint),
        ("AWS_EC2_METADATA_DISABLED", "true".to_string()),
    ]
    .into_iter()
    .collect();
    let flags = super::config::Flags {
        profile: Some("reader".into()),
        ..Default::default()
    };
    let settings = super::config::resolve(&flags, &|k| env.get(k).cloned()).unwrap();
    assert!(
        matches!(
            settings.credentials,
            Some(super::credentials::Provider::AssumeRole(_))
        ),
        "{:?}",
        settings.credentials
    );
    let fetched = settings
        .credentials
        .as_ref()
        .unwrap()
        .fetch(&ScanProgress::default())
        .unwrap();
    assert_ne!(
        fetched.credentials.access_key_id, server.credentials.access_key_id,
        "temporary keys, not the source's"
    );
    let token = fetched
        .credentials
        .session_token
        .clone()
        .expect("a session token");
    let lifetime = fetched.expires.unwrap() - crate::fmt::unix_now();
    assert!(
        (800..=900).contains(&lifetime),
        "duration_seconds was asked for: {lifetime}"
    );

    let scan = scan_with(
        &bucket.url(),
        settings,
        None,
        DEFAULT_WORKERS,
        Arc::new(ScanProgress::default()),
    )
    .unwrap();
    assert_eq!(scan.stats.objects, 2);
    assert_eq!(scan.tree.total_size(), 12);

    // And without the token the same temporary key is refused, so the
    // listing above did carry it.
    let tokenless = Credentials {
        session_token: None,
        ..fetched.credentials.clone()
    };
    let refused = scan_with(
        &bucket.url(),
        server.settings(Some(tokenless)),
        None,
        DEFAULT_WORKERS,
        Arc::new(ScanProgress::default()),
    )
    .map(|_| ())
    .unwrap_err();
    let text = format!("{refused:#}");
    assert!(text.contains("refused the listing"), "{text}");
    assert!(!text.contains(&token), "{text}");
}

/// Refusals must say what went wrong in S3's words, and must not echo a
/// secret on the way.
#[test]
fn refusals_are_reported_in_the_servers_words_without_secrets() {
    let Some(server) = server() else { return };
    let bucket = TestBucket::create(&server, "refuse");

    let anonymous = scan_with(
        &bucket.url(),
        server.settings(None),
        None,
        DEFAULT_WORKERS,
        Arc::new(ScanProgress::default()),
    )
    .map(|_| ())
    .unwrap_err();
    let text = format!("{anonymous:#}");
    assert!(text.contains("AccessDenied"), "{text}");
    assert!(text.contains("Unsigned requests"), "{text}");

    let wrong = Credentials {
        secret_access_key: "definitely-not-the-secret-0123456789".into(),
        ..server.credentials.clone()
    };
    let refused = scan_with(
        &bucket.url(),
        server.settings(Some(wrong)),
        None,
        DEFAULT_WORKERS,
        Arc::new(ScanProgress::default()),
    )
    .map(|_| ())
    .unwrap_err();
    let text = format!("{refused:#}");
    assert!(text.contains("SignatureDoesNotMatch"), "{text}");
    assert!(!text.contains("definitely-not-the-secret"), "{text}");
    assert!(
        !text.contains(&server.credentials.secret_access_key),
        "{text}"
    );

    let missing = S3Url::parse("s3://spacetrace-no-such-bucket-here").unwrap();
    let text = format!(
        "{:#}",
        scan_with(
            &missing,
            server.signed(),
            None,
            DEFAULT_WORKERS,
            Arc::new(ScanProgress::default())
        )
        .map(|_| ())
        .unwrap_err()
    );
    assert!(text.contains("NoSuchBucket"), "{text}");
}
