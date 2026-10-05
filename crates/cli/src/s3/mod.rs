//! An S3 bucket — AWS or anything that speaks its API — as a scan root.
//!
//! `spacetrace scan s3://bucket/prefix` lists the bucket with ListObjectsV2,
//! draws the keys as folders, and from there it is an ordinary tree: saved,
//! listed, diffed, aged and exported by exactly the code a disk scan goes
//! through. Like `--remote`, the one thing that is different is how the tree
//! arrives.
//!
//! **In the CLI and nowhere else.** The agent is the binary that has to stay
//! small and static, and `store` and `scan-core` are what it is built from;
//! the HTTP client is already here for `--remote` and `update`. A crate of its
//! own would have one dependent, this one, and add a manifest for nothing.
//!
//! **What the totals are.** The current version of every object under the
//! prefix — and nothing else. Old versions in a versioned bucket, delete
//! markers and the parts of unfinished multipart uploads are all stored, all
//! billed, and none of them is in a ListObjectsV2 listing. The total is what
//! the bucket *shows*, not what it costs, and the output says so every time.
//!
//! **What the sizes mean.** An object has a length and no blocks, so `size`
//! and `alloc` are both that length for every object. They part only where
//! the tree has to charge bytes to a folder — a folder marker that holds data,
//! an object with a folder's name — which count in `alloc` alone, the way a
//! directory's own blocks do on a disk (see `keys.rs`). `alloc` therefore adds
//! up to every byte listed, which is the number `mc du` prints (checked
//! against MinIO to the byte).

mod client;
mod config;
mod credentials;
mod keys;
mod sigv4;
mod xml;

#[cfg(test)]
mod minio_tests;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use spacetrace_scan_core::{Phase, ScanProgress, Tree};

pub use config::Flags;
pub use keys::KeyStats;

/// `s3://bucket/prefix`, split and normalised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3Url {
    pub bucket: String,
    /// `""` for the whole bucket, otherwise ending in `/`.
    pub prefix: String,
}

impl S3Url {
    /// Whether `path` names a bucket rather than a directory.
    ///
    /// A relative directory literally called `s3:` is still reachable as
    /// `./s3:/…`; nothing else on any platform starts with `s3://`.
    pub fn is_s3(path: &std::path::Path) -> bool {
        path.to_str().is_some_and(|p| p.starts_with("s3://"))
    }

    pub fn parse(raw: &str) -> Result<S3Url> {
        let rest = raw
            .strip_prefix("s3://")
            .with_context(|| format!("{raw:?} is not an s3:// URL"))?;
        let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
        if bucket.is_empty() {
            bail!("{raw:?} names no bucket; write s3://bucket or s3://bucket/prefix");
        }
        // Lenient on case and underscores, which some S3-compatible services
        // and old AWS buckets allow; strict on anything that would change the
        // meaning of the URL the bucket is put into.
        if !bucket
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        {
            bail!("{bucket:?} is not a bucket name");
        }
        // The prefix is a folder. `s3://b/photos` lists `photos/…` and not
        // `photos2024/…`, because a scan root is a directory and a string
        // prefix that also catches the neighbours' files would be a surprise
        // in every total.
        let folder = prefix.trim_end_matches('/');
        let prefix = match folder.is_empty() {
            true => String::new(),
            false => format!("{folder}/"),
        };
        Ok(S3Url {
            bucket: bucket.to_string(),
            prefix,
        })
    }

    /// The snapshot's `root`: `s3://bucket` or `s3://bucket/prefix`, with no
    /// trailing slash, so the two spellings of one folder are one target —
    /// for `diff --path` here and for the desktop's timeline, which trims a
    /// trailing separator before grouping.
    pub fn root(&self) -> String {
        match self.prefix.strip_suffix('/') {
            Some(folder) => format!("s3://{}/{folder}", self.bucket),
            None => format!("s3://{}", self.bucket),
        }
    }
}

/// A finished listing, as a tree and what the tree cannot show.
pub struct S3Scan {
    pub tree: Tree,
    pub stats: KeyStats,
    /// The snapshot's `host`: the service, not this machine. Two machines
    /// listing one bucket are listing the same thing.
    pub host: String,
    /// The region the listing ended up signed for, after any redirect.
    pub region: String,
    pub pages: u64,
    pub duration_ms: u64,
}

/// List `url` and build its tree.
///
/// The counters on `progress` move once per page — files, bytes, folders — so
/// the CLI's progress line and its stall warning work unchanged (invariant 8).
/// A cancel stops it before the next request and returns
/// `ErrorKind::Interrupted` and no tree (invariant 5).
pub fn scan(url: &S3Url, flags: &Flags, progress: Arc<ScanProgress>) -> Result<S3Scan> {
    let settings = config::resolve(flags, &|name| std::env::var(name).ok())?;
    scan_with(url, settings, None, progress)
}

/// Consecutive pages with no keys and a token for more, before the listing is
/// refused. S3 may send an empty page while it skips past what a listing does
/// not show; a thousand in a row is not that, it is a server that will never
/// finish.
const MAX_EMPTY_PAGES: u64 = 1000;

fn scan_with(
    url: &S3Url,
    settings: config::Settings,
    page_size: Option<u32>,
    progress: Arc<ScanProgress>,
) -> Result<S3Scan> {
    let started = Instant::now();
    let host = client::identity(&settings);
    let mut http = client::Client::new(settings)?;
    http.page_size = page_size;
    let bucket = client::Bucket {
        name: url.bucket.clone(),
        prefix: url.prefix.clone(),
    };

    let root = url.root();
    let mut keys = keys::KeyTree::new(&root, &url.prefix);
    let mut token: Option<String> = None;
    let mut seen_tokens = std::collections::HashSet::new();
    let mut pages = 0u64;
    let mut empty_in_a_row = 0u64;
    loop {
        let page = http.list_page(&bucket, token.as_deref(), &progress)?;
        pages += 1;
        let (mut files, mut bytes) = (0, 0);
        for object in &page.objects {
            keys.insert(object)?;
            files += u64::from(!object.key.ends_with('/'));
            bytes += object.size;
        }
        client::count_page(&progress, files, bytes, keys.folders());

        if !page.is_truncated {
            break;
        }
        empty_in_a_row = match page.objects.is_empty() {
            true => empty_in_a_row + 1,
            false => 0,
        };
        if empty_in_a_row > MAX_EMPTY_PAGES {
            bail!(
                "s3://{} sent more than {MAX_EMPTY_PAGES} empty pages in a row, each promising more; \
                 the listing would never end",
                url.bucket
            );
        }
        let next = page.next_continuation_token.with_context(|| {
            format!(
                "s3://{} said there is more and gave no continuation token",
                url.bucket
            )
        })?;
        // A server handing back a token it already gave would make this loop
        // forever while the counters kept moving — the one hang the stall
        // warning cannot see.
        if !seen_tokens.insert(next.clone()) {
            bail!(
                "s3://{} repeated a continuation token; the listing would never end",
                url.bucket
            );
        }
        token = Some(next);
    }
    client::check_cancelled(&progress)?;

    // Building the arena is one pass over every key; on a large bucket long
    // enough to be seen, and not walking any more.
    progress.begin_rows(Phase::Finishing, 0);
    let (root_node, stats) = keys.finish();
    let tree = Tree::try_from_nested(PathBuf::from(&root), root_node)
        .with_context(|| format!("cannot build the tree of {root}"))?;
    Ok(S3Scan {
        tree,
        stats,
        host,
        region: http.region().to_string(),
        pages,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

/// A lock that outlives a panic on another thread: what it guards is never
/// left half-written, and one failed request must not take the listing down.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_split_into_bucket_and_folder() {
        let u = S3Url::parse("s3://bucket").unwrap();
        assert_eq!((u.bucket.as_str(), u.prefix.as_str()), ("bucket", ""));
        assert_eq!(u.root(), "s3://bucket");
        assert_eq!(S3Url::parse("s3://bucket/").unwrap().root(), "s3://bucket");

        let u = S3Url::parse("s3://bucket/photos/2024").unwrap();
        assert_eq!(u.prefix, "photos/2024/");
        assert_eq!(u.root(), "s3://bucket/photos/2024");
        assert_eq!(
            S3Url::parse("s3://bucket/photos/2024/").unwrap(),
            u,
            "a trailing slash names the same folder"
        );
    }

    #[test]
    fn bad_urls_are_refused_with_a_reason() {
        let refused = |raw: &str| format!("{:#}", S3Url::parse(raw).unwrap_err());
        assert!(refused("s3://").contains("names no bucket"));
        assert!(refused("s3:///x").contains("names no bucket"));
        assert!(refused("s3://a b/x").contains("not a bucket name"));
        assert!(refused("s3://a?b").contains("not a bucket name"));
        assert!(refused("http://x").contains("not an s3:// URL"));
    }

    #[test]
    fn only_s3_urls_are_taken_for_buckets() {
        use std::path::Path;
        assert!(S3Url::is_s3(Path::new("s3://b")));
        assert!(!S3Url::is_s3(Path::new("./s3://b")));
        assert!(!S3Url::is_s3(Path::new("/srv")));
    }

    fn page(keys: &[&str], next: Option<&str>) -> Vec<u8> {
        let contents: String = keys
            .iter()
            .map(|k| format!("<Contents><Key>{k}</Key><Size>1</Size></Contents>"))
            .collect();
        let tail = match next {
            Some(token) => format!(
                "<IsTruncated>true</IsTruncated><NextContinuationToken>{token}</NextContinuationToken>"
            ),
            None => "<IsTruncated>false</IsTruncated>".to_string(),
        };
        format!("<ListBucketResult>{contents}{tail}</ListBucketResult>").into_bytes()
    }

    fn anonymous(endpoint: &str) -> config::Settings {
        config::Settings {
            credentials: None,
            region: config::DEFAULT_REGION.into(),
            endpoint: Some(config::Endpoint::parse(endpoint).unwrap()),
        }
    }

    /// A few empty pages are legal — S3 can return one while it skips over
    /// what it does not list — and must not end the listing early.
    #[test]
    fn empty_pages_in_the_middle_are_followed() {
        let responses = vec![
            (200, "OK", page(&[], Some("t1"))),
            (200, "OK", page(&[], Some("t2"))),
            (200, "OK", page(&["a", "b"], Some("t3"))),
            (200, "OK", page(&["c"], None)),
        ];
        let (endpoint, server) = client::test_server::canned(responses);
        let url = S3Url::parse("s3://b").unwrap();
        let scan = scan_with(&url, anonymous(&endpoint), None, Arc::default()).unwrap();
        assert_eq!(scan.pages, 4);
        assert_eq!(scan.stats.objects, 3);
        assert_eq!(server.join().unwrap().len(), 4);
    }

    // ------------------------------------------- keys that expire midway

    use client::test_server::{serve, Served};
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// A container credentials endpoint handing out keys that live
    /// `lifetime_s` seconds, and the moment each token stops working.
    fn issuer(lifetime_s: i64) -> (Served, Arc<Mutex<HashMap<String, i64>>>) {
        let valid_until = Arc::new(Mutex::new(HashMap::new()));
        let ledger = Arc::clone(&valid_until);
        let server = serve(move |_| {
            let mut ledger = ledger.lock().unwrap();
            let n = ledger.len() + 1;
            let expires = crate::fmt::unix_now() + lifetime_s;
            ledger.insert(format!("token-{n}"), expires * 1000);
            let stamp = crate::fmt::rfc3339(expires);
            (
                200,
                format!(
                    "{{\"AccessKeyId\":\"ID{n}\",\"SecretAccessKey\":\"s{n}\",\"Token\":\"token-{n}\",\
                     \"Expiration\":\"{stamp}\"}}"
                ),
            )
        });
        (server, valid_until)
    }

    /// An S3 stand-in serving `pages` pages, `delay` apart, that answers
    /// `ExpiredToken` for a token past its time — and for the first request
    /// when `reject_first`, the way AWS does when its clock and ours differ.
    fn bucket_checking_tokens(
        pages: usize,
        delay: std::time::Duration,
        valid_until: Arc<Mutex<HashMap<String, i64>>>,
        reject_first: bool,
    ) -> (Served, Arc<Mutex<u32>>) {
        let refusals = Arc::new(Mutex::new(0u32));
        let counted = Arc::clone(&refusals);
        let first = std::sync::atomic::AtomicBool::new(reject_first);
        let server = serve(move |request| {
            let token = request.header("x-amz-security-token").unwrap_or("");
            let alive = valid_until
                .lock()
                .unwrap()
                .get(token)
                .is_some_and(|until| request.at_ms < *until);
            if !alive || first.swap(false, std::sync::atomic::Ordering::SeqCst) {
                *counted.lock().unwrap() += 1;
                return (
                    400,
                    "<Error><Code>ExpiredToken</Code><Message>The provided token has expired.</Message></Error>"
                        .into(),
                );
            }
            std::thread::sleep(delay);
            let at: usize = request
                .target
                .split("continuation-token=p")
                .nth(1)
                .and_then(|rest| rest.split('&').next())
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            let key = format!("k{at:03}");
            let next = (at + 1 < pages).then(|| format!("p{}", at + 1));
            (
                200,
                String::from_utf8(page(&[key.as_str()], next.as_deref())).unwrap(),
            )
        });
        (server, refusals)
    }

    fn renewing(container: &Served, bucket: &Served) -> config::Settings {
        config::Settings {
            credentials: Some(credentials::Provider::Container(credentials::Container {
                url: format!("{}/creds", container.address),
                token: None,
            })),
            region: config::DEFAULT_REGION.into(),
            endpoint: Some(config::Endpoint::parse(&bucket.address).unwrap()),
        }
    }

    /// Keys that live four seconds, a listing that takes about five: the keys
    /// are renewed before they lapse, so S3 never has to refuse one.
    #[test]
    fn keys_that_expire_during_a_listing_are_renewed_before_they_lapse() {
        let (container, valid_until) = issuer(4);
        let (bucket, refusals) = bucket_checking_tokens(
            15,
            std::time::Duration::from_millis(300),
            Arc::clone(&valid_until),
            false,
        );
        let url = S3Url::parse("s3://b").unwrap();
        let scan = scan_with(&url, renewing(&container, &bucket), None, Arc::default()).unwrap();
        assert_eq!(scan.stats.objects, 15);
        assert_eq!(
            *refusals.lock().unwrap(),
            0,
            "no request went out with lapsed keys"
        );
        let tokens: std::collections::HashSet<String> = bucket
            .seen()
            .iter()
            .filter_map(|r| r.header("x-amz-security-token").map(str::to_string))
            .collect();
        assert!(tokens.len() >= 2, "renewed midway: {tokens:?}");
        assert_eq!(container.seen().len(), tokens.len());
    }

    /// S3's word outranks our clock: `ExpiredToken` drops the keys and the
    /// page is asked for once more, with fresh ones.
    #[test]
    fn an_expired_token_answer_fetches_fresh_keys_and_retries() {
        let (container, valid_until) = issuer(3600);
        let (bucket, refusals) =
            bucket_checking_tokens(2, std::time::Duration::ZERO, Arc::clone(&valid_until), true);
        let url = S3Url::parse("s3://b").unwrap();
        let scan = scan_with(&url, renewing(&container, &bucket), None, Arc::default()).unwrap();
        assert_eq!(scan.stats.objects, 2);
        assert_eq!(*refusals.lock().unwrap(), 1);
        let seen = bucket.seen();
        assert_eq!(seen[0].header("x-amz-security-token"), Some("token-1"));
        assert_eq!(seen[1].header("x-amz-security-token"), Some("token-2"));
        assert_eq!(container.seen().len(), 2);
    }

    /// A hostile endpoint can hand out a fresh token with every empty page
    /// forever; the repeated-token check cannot see that, and no counter
    /// moves for anyone watching. Bounded instead.
    #[test]
    fn an_endless_run_of_empty_pages_is_refused() {
        let responses = (0..=MAX_EMPTY_PAGES)
            .map(|i| (200, "OK", page(&[], Some(&format!("token-{i}")))))
            .collect();
        let (endpoint, server) = client::test_server::canned(responses);
        let url = S3Url::parse("s3://b").unwrap();
        let err = scan_with(&url, anonymous(&endpoint), None, Arc::default())
            .map(|_| ())
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("empty pages in a row"),
            "{err:#}"
        );
        assert_eq!(server.join().unwrap().len(), MAX_EMPTY_PAGES as usize + 1);
    }
}
