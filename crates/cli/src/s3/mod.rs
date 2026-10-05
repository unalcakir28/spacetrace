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
mod listing;
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
/// Before any object reaches the tree, while a parallel listing looks for
/// folders, `rows_done` counts the pages that search reads.
/// A cancel stops it before the next request and returns
/// `ErrorKind::Interrupted` and no tree (invariant 5).
pub fn scan(url: &S3Url, flags: &Flags, progress: Arc<ScanProgress>) -> Result<S3Scan> {
    let settings = config::resolve(flags, &|name| std::env::var(name).ok())?;
    let workers = workers(flags.threads, &settings);
    scan_with(url, settings, None, workers, progress)
}

/// Listing requests at once: `--threads` if given, up to
/// `listing::MAX_WORKERS`, else what suits the endpoint.
fn workers(threads: Option<u16>, settings: &config::Settings) -> usize {
    match threads {
        Some(n) => usize::from(n).clamp(1, listing::MAX_WORKERS),
        None => settings.default_workers(),
    }
}

fn scan_with(
    url: &S3Url,
    settings: config::Settings,
    page_size: Option<u32>,
    workers: usize,
    progress: Arc<ScanProgress>,
) -> Result<S3Scan> {
    let started = Instant::now();
    let host = client::identity(&settings);
    let mut http = client::Client::new(settings)?;
    http.page_size = page_size;
    let bucket = client::Bucket {
        name: url.bucket.clone(),
    };

    let root = url.root();
    let mut keys = keys::KeyTree::new(&root, &url.prefix);
    // The tree is built here, on this thread, from objects in key order —
    // however many workers fetched them.
    let listed = listing::list(
        &mut http,
        &bucket,
        &url.prefix,
        listing::Plan::workers(workers),
        &progress,
        &mut |objects| {
            let (mut files, mut bytes) = (0, 0);
            for object in &objects {
                keys.insert(object)?;
                files += u64::from(!object.key.ends_with('/'));
                bytes += object.size;
            }
            client::count_page(&progress, files, bytes, keys.folders());
            Ok(())
        },
    )?;
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
        pages: listed,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

/// A lock that outlives a panic on another thread: what it guards is never
/// left half-written, and one failed request must not take the listing down.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
/// Every entry of the arena, in arena order: names, links, sizes, times.
/// Two of these equal means the same tree down to the ids.
fn arena(tree: &Tree) -> Vec<String> {
    (0..tree.len() as u32)
        .map(|id| {
            let n = tree.node(id);
            format!(
                "{id} {:?} p{} c{}+{} {:?} s{} a{} o{} m{} f{} d{}",
                tree.name(id),
                n.parent,
                n.children_start,
                n.children_len,
                n.kind,
                n.size,
                n.alloc,
                n.own_alloc,
                n.mtime,
                n.files,
                n.dirs
            )
        })
        .collect()
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
        let scan = scan_with(&url, anonymous(&endpoint), None, 1, Arc::default()).unwrap();
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
        slow_issuer(lifetime_s, std::time::Duration::ZERO)
    }

    /// `issuer`, taking `delay` over every answer.
    fn slow_issuer(
        lifetime_s: i64,
        delay: std::time::Duration,
    ) -> (Served, Arc<Mutex<HashMap<String, i64>>>) {
        let valid_until = Arc::new(Mutex::new(HashMap::new()));
        let ledger = Arc::clone(&valid_until);
        let server = serve(move |_| {
            std::thread::sleep(delay);
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
        let scan = scan_with(&url, renewing(&container, &bucket), None, 1, Arc::default()).unwrap();
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
        let scan = scan_with(&url, renewing(&container, &bucket), None, 1, Arc::default()).unwrap();
        assert_eq!(scan.stats.objects, 2);
        assert_eq!(*refusals.lock().unwrap(), 1);
        let seen = bucket.seen();
        assert_eq!(seen[0].header("x-amz-security-token"), Some("token-1"));
        assert_eq!(seen[1].header("x-amz-security-token"), Some("token-2"));
        assert_eq!(container.seen().len(), 2);
    }

    // ----------------------------------------- one stream or many, one tree

    /// What one listing produced, for comparing one against another.
    fn outcome(scan: &S3Scan, progress: &ScanProgress) -> (Vec<String>, KeyStats, u64, u64) {
        use std::sync::atomic::Ordering::Relaxed;
        (
            arena(&scan.tree),
            scan.stats.clone(),
            progress.files.load(Relaxed),
            progress.bytes.load(Relaxed),
        )
    }

    fn list_stand_in(
        keys: &[(String, u64)],
        prefix: &str,
        page: u32,
        workers: usize,
    ) -> (S3Scan, Arc<ScanProgress>) {
        let server = client::test_server::bucket(keys.to_vec());
        let url = S3Url::parse(&format!("s3://b/{prefix}")).unwrap();
        let progress = Arc::new(ScanProgress::default());
        let scan = scan_with(
            &url,
            anonymous(&server.address),
            Some(page),
            workers,
            Arc::clone(&progress),
        )
        .unwrap();
        (scan, progress)
    }

    /// The keys that make a tree hard to get right — markers, an object
    /// beside a folder of its name, empty and dot segments, names that sort
    /// either side of `/` — inside folders big enough to be split.
    fn awkward_bucket() -> Vec<(String, u64)> {
        let mut keys: Vec<(String, u64)> = [
            ("a", 1),
            ("a/b", 2),
            ("a.txt", 3),
            ("a0", 4),
            ("a//double", 5),
            ("/lead", 6),
            ("./dot", 7),
            ("x/../up", 8),
            ("marker/", 9),
            ("heavy/", 10),
            ("heavy/inside", 11),
            ("sp ace/plus+sign/100%", 12),
            ("ünï/ファイル", 13),
        ]
        .iter()
        .map(|(k, s)| (k.to_string(), *s))
        .collect();
        for svc in 0..6 {
            for day in 0..5 {
                for part in 0..7 {
                    keys.push((format!("logs/svc-{svc}/d{day}/p{part}.gz"), part + 1));
                }
            }
            keys.push((format!("logs/svc-{svc}.manifest"), 100));
        }
        for user in 0..40 {
            keys.push((format!("users/u{user:02}/photo.jpg"), user));
            keys.push((format!("users/u{user:02}/docs/cv.pdf"), 2 * user));
        }
        for n in 0..60 {
            keys.push((format!("flat/{n:03}"), 1));
        }
        keys
    }

    /// The claim the whole design rests on: however many workers, whatever
    /// the page size, the tree is the one a single stream builds — the same arena, id for id —
    /// and so are the totals and the counters.
    #[test]
    fn any_number_of_workers_builds_the_tree_one_stream_builds() {
        let keys = awkward_bucket();
        for prefix in ["", "logs"] {
            let (one, progress) = list_stand_in(&keys, prefix, 1000, 1);
            let expected = outcome(&one, &progress);
            assert!(
                expected.0.len() > 100,
                "{prefix:?}: {} entries",
                expected.0.len()
            );
            for page in [1000, 7, 2] {
                for workers in [2, 3, 16] {
                    let (scan, progress) = list_stand_in(&keys, prefix, page, workers);
                    assert_eq!(
                        outcome(&scan, &progress),
                        expected,
                        "prefix {prefix:?}, page {page}, workers {workers}"
                    );
                }
            }
        }
    }

    /// Random buckets over an alphabet that puts `.`, `/` and `0` next to
    /// each other in byte order, which is where a split goes wrong if it is
    /// going to.
    #[test]
    fn random_buckets_list_the_same_in_parallel() {
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let alphabet = ["a", "b", "/", ".", "0", "-", "/", "c"];
        for round in 0..40 {
            let count = 20 + next() % 300;
            let keys: Vec<(String, u64)> = (0..count)
                .map(|_| {
                    let len = 1 + next() % 12;
                    let key: String = (0..len)
                        .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
                        .collect();
                    (key, next() % 1000)
                })
                .collect();
            let (one, progress) = list_stand_in(&keys, "", 1000, 1);
            let expected = outcome(&one, &progress);
            let page = [1, 3, 10, 1000][round % 4];
            let workers = [2, 4, 16][round % 3];
            let (scan, progress) = list_stand_in(&keys, "", page, workers);
            assert_eq!(
                outcome(&scan, &progress),
                expected,
                "round {round}: {count} keys, page {page}, workers {workers}"
            );
        }
    }

    /// A bucket that fits in one page costs one request, as it did before
    /// there were workers; a flat one is listed as one stream after the
    /// first page, plus the few pages discovery reads before it gives up on
    /// expanding it.
    #[test]
    fn splitting_costs_nothing_where_it_cannot_pay() {
        let small: Vec<(String, u64)> = (0..50).map(|n| (format!("d{}/k{n}", n % 5), 1)).collect();
        let (scan, _) = list_stand_in(&small, "", 1000, 16);
        assert_eq!(scan.pages, 1);

        let flat: Vec<(String, u64)> = (0..30_000).map(|n| (format!("k{n:06}"), 1)).collect();
        let (one, _) = list_stand_in(&flat, "", 1000, 1);
        let (many, _) = list_stand_in(&flat, "", 1000, 16);
        assert_eq!(one.pages, 30);
        assert!(
            many.pages <= one.pages + 11,
            "{} requests for {} pages",
            many.pages,
            one.pages
        );
        assert_eq!(arena(&many.tree), arena(&one.tree));
    }

    /// Many small folders under one parent are listed as a few long ranges,
    /// not one request each.
    #[test]
    fn neighbouring_folders_are_listed_as_one_range() {
        let keys: Vec<(String, u64)> = (0..4000)
            .map(|n| (format!("users/u{:04}/f{}", n / 2, n % 2), 1))
            .collect();
        let (one, _) = list_stand_in(&keys, "", 1000, 1);
        let (many, _) = list_stand_in(&keys, "", 1000, 16);
        assert_eq!(arena(&many.tree), arena(&one.tree));
        assert!(
            many.pages < 40,
            "{} requests for 2000 folders of 2 keys",
            many.pages
        );
    }

    /// A hostile endpoint can hand out a fresh token with every empty page
    /// forever; the repeated-token check cannot see that, and no counter
    /// moves for anyone watching. Bounded instead.
    #[test]
    fn an_endless_run_of_empty_pages_is_refused() {
        let responses = (0..=listing::MAX_EMPTY_PAGES)
            .map(|i| (200, "OK", page(&[], Some(&format!("token-{i}")))))
            .collect();
        let (endpoint, server) = client::test_server::canned(responses);
        let url = S3Url::parse("s3://b").unwrap();
        let err = scan_with(&url, anonymous(&endpoint), None, 1, Arc::default())
            .map(|_| ())
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("empty pages in a row"),
            "{err:#}"
        );
        assert_eq!(
            server.join().unwrap().len(),
            listing::MAX_EMPTY_PAGES as usize + 1
        );
    }

    // ------------------------------------ cancelled, refused, held back

    /// The bucket stand-in, with `hook` given each request and its number
    /// first: an answer of its own, or `None` for the bucket's.
    fn bucket_with(
        keys: &[(String, u64)],
        hook: impl Fn(usize, &client::test_server::Seen) -> Option<client::test_server::Reply>
            + Send
            + Sync
            + 'static,
    ) -> Served {
        let mut keys = keys.to_vec();
        keys.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        let count = std::sync::atomic::AtomicUsize::new(0);
        serve(move |request| {
            let n = count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            hook(n, request)
                .unwrap_or_else(|| client::test_server::list_objects(&keys, &request.target))
        })
    }

    fn interrupted(err: &anyhow::Error) -> bool {
        err.downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.kind() == std::io::ErrorKind::Interrupted)
    }

    /// Invariant 5 the way the repository tests it, with workers: cancelled
    /// before the start, nothing is asked, no counter moves.
    #[test]
    fn a_listing_cancelled_before_it_starts_asks_nothing_with_workers() {
        let server = client::test_server::bucket(awkward_bucket());
        let progress = Arc::new(ScanProgress::default());
        progress.cancel();
        let url = S3Url::parse("s3://b").unwrap();
        let err = scan_with(
            &url,
            anonymous(&server.address),
            Some(7),
            16,
            Arc::clone(&progress),
        )
        .map(|_| ())
        .unwrap_err();
        assert!(interrupted(&err), "{err:#}");
        assert_eq!(progress.files.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert_eq!(server.seen().len(), 0);
    }

    /// Cancelled midway — on the server's count of requests, not a timer —
    /// in discovery or in the ranges: every worker stops within the request
    /// it had in flight, the call returns, and no tree comes back.
    #[test]
    fn a_listing_cancelled_midway_returns_no_tree() {
        let keys = awkward_bucket();
        for workers in [1, 4, 16] {
            for at in [1, 3, 10, 30] {
                let progress = Arc::new(ScanProgress::default());
                let cancel = Arc::clone(&progress);
                let server = bucket_with(&keys, move |n, _| {
                    if n == at {
                        cancel.cancel();
                    }
                    None
                });
                let url = S3Url::parse("s3://b").unwrap();
                let err = scan_with(
                    &url,
                    anonymous(&server.address),
                    Some(7),
                    workers,
                    Arc::clone(&progress),
                )
                .map(|_| ())
                .unwrap_err();
                let what = format!("{workers} workers, cancelled at request {at}");
                assert!(interrupted(&err), "{what}: {err:#}");
                let sent = server.seen().len();
                assert!(sent <= at + workers, "{what}: {sent} requests");
            }
        }
    }

    /// A range the server refuses ends the listing with the server's word,
    /// whichever worker met it, and leaves no other waiting for it.
    #[test]
    fn a_refused_range_ends_the_listing_with_the_refusal() {
        let keys = awkward_bucket();
        for workers in [2, 16] {
            let server = bucket_with(&keys, |n, request| {
                let range = n > 0 && !request.target.contains("delimiter=");
                range.then(|| {
                    (
                        403,
                        "<Error><Code>AccessDenied</Code><Message>Access Denied</Message></Error>"
                            .into(),
                    )
                })
            });
            let url = S3Url::parse("s3://b").unwrap();
            let err = scan_with(
                &url,
                anonymous(&server.address),
                Some(7),
                workers,
                Arc::default(),
            )
            .map(|_| ())
            .unwrap_err();
            assert!(
                format!("{err:#}").contains("AccessDenied"),
                "{workers} workers: {err:#}"
            );
        }
    }

    /// Invariant 8 through discovery: a counter moves with every page it
    /// reads, so a level of slow folders is not taken for a stall, and the
    /// folder count never runs backwards — discovery leaves it to the tree.
    #[test]
    fn discovery_moves_a_counter_every_page_and_none_backwards() {
        use std::sync::atomic::Ordering::Relaxed;
        // Forty folders: the root's listing finds them in four pages of ten,
        // and that is enough for two workers.
        let keys: Vec<(String, u64)> = (0..2000)
            .map(|n| (format!("d{:02}/k{n:04}", n % 40), 1))
            .collect();
        let progress = Arc::new(ScanProgress::default());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let server = {
            let (progress, seen) = (Arc::clone(&progress), Arc::clone(&seen));
            bucket_with(&keys, move |_, request| {
                lock(&seen).push((
                    request.target.contains("delimiter="),
                    progress.dirs.load(Relaxed),
                    progress.rows_done.load(Relaxed),
                ));
                None
            })
        };
        let url = S3Url::parse("s3://b").unwrap();
        scan_with(
            &url,
            anonymous(&server.address),
            Some(10),
            2,
            Arc::clone(&progress),
        )
        .unwrap();
        let seen = lock(&seen);
        let dirs: Vec<u64> = seen.iter().map(|s| s.1).collect();
        assert!(dirs.windows(2).all(|w| w[0] <= w[1]), "{dirs:?}");
        let discovery: Vec<u64> = seen.iter().filter(|s| s.0).map(|s| s.2).collect();
        assert_eq!(discovery.len(), 4);
        assert_eq!(discovery, [0, 1, 2, 3], "one more for every page read");
    }

    /// A folder discovery cannot expand ends the listing, and the workers
    /// stop taking folders instead of expanding the rest for nothing.
    #[test]
    fn a_failed_expansion_stops_the_other_workers() {
        let keys: Vec<(String, u64)> = (0..40)
            .flat_map(|a| (0..3).map(move |b| (format!("d{a:02}/s{b}/k"), 1)))
            .collect();
        let server = bucket_with(&keys, |_, request| {
            let delimited = request.target.contains("delimiter=");
            if delimited && request.target.contains("prefix=d00") {
                return Some((
                    403,
                    "<Error><Code>AccessDenied</Code><Message>Access Denied</Message></Error>"
                        .into(),
                ));
            }
            if delimited && request.target.contains("prefix=d") {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            None
        });
        let url = S3Url::parse("s3://b").unwrap();
        let err = scan_with(
            &url,
            anonymous(&server.address),
            Some(10),
            16,
            Arc::default(),
        )
        .map(|_| ())
        .unwrap_err();
        assert!(format!("{err:#}").contains("AccessDenied"), "{err:#}");
        let expansions = server
            .seen()
            .iter()
            .filter(|r| r.target.contains("delimiter=") && r.target.contains("prefix=d"))
            .count();
        assert!(
            expansions <= 20,
            "{expansions} of 40 folders expanded after the first failed"
        );
    }

    /// `--threads` for a bucket is capped: past the point where more
    /// requests at once stopped paying, each worker is one more connection
    /// and one more thread for nothing.
    #[test]
    fn listing_workers_are_capped() {
        let aws = config::Settings {
            credentials: None,
            region: config::DEFAULT_REGION.into(),
            endpoint: None,
        };
        let minio = anonymous("http://127.0.0.1:9000");
        assert_eq!(workers(None, &aws), listing::DEFAULT_WORKERS);
        assert_eq!(workers(None, &minio), 1);
        assert_eq!(workers(Some(3), &minio), 3);
        assert_eq!(workers(Some(u16::MAX), &aws), listing::MAX_WORKERS);
        assert_eq!(workers(Some(0), &aws), 1);
    }

    /// The cap on empty pages holds inside a range as it does in one stream.
    #[test]
    fn an_endless_run_of_empty_pages_is_refused_inside_a_range() {
        let keys: Vec<(String, u64)> = (0..50)
            .map(|n| (format!("d{}/k{n:02}", n % 10), 1))
            .collect();
        let range = |n: usize, request: &client::test_server::Seen| {
            n > 0 && !request.target.contains("delimiter=")
        };
        let server = bucket_with(&keys, move |n, request| {
            range(n, request).then(|| {
                (
                    200,
                    format!(
                        "<ListBucketResult><IsTruncated>true</IsTruncated>\
                         <NextContinuationToken>e{n}</NextContinuationToken></ListBucketResult>"
                    ),
                )
            })
        });
        let url = S3Url::parse("s3://b").unwrap();
        let err = scan_with(&url, anonymous(&server.address), Some(5), 2, Arc::default())
            .map(|_| ())
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("empty pages in a row"),
            "{err:#}"
        );
        let empties = server
            .seen()
            .iter()
            .enumerate()
            .filter(|(n, r)| range(*n, r))
            .count();
        assert!(empties > listing::MAX_EMPTY_PAGES as usize, "{empties}");
    }

    /// However slow the caller, workers fetch no more than the cap ahead of
    /// it, and the listing still finishes with the same keys in the same
    /// order: the paused ranges and the one being taken do not deadlock.
    /// Watched from the caller's side, as requests made ahead of the pages it
    /// has taken.
    #[test]
    fn workers_stop_fetching_ahead_of_a_slow_caller() {
        // Forty folders, enough that discovery leaves them to the ranges.
        let keys: Vec<(String, u64)> = (0..3000)
            .map(|n| (format!("d{:02}/k{n:04}", n % 40), 1))
            .collect();
        let mut expected: Vec<String> = keys.iter().map(|(k, _)| k.clone()).collect();
        expected.sort();
        let run = |max_buffered| {
            let server = client::test_server::bucket(keys.clone());
            let mut http = client::Client::new(anonymous(&server.address)).unwrap();
            http.page_size = Some(10);
            let mut got = Vec::new();
            let mut taken = 0usize;
            let mut lead = 0usize;
            listing::list(
                &mut http,
                &client::Bucket { name: "b".into() },
                "",
                listing::Plan {
                    workers: 8,
                    max_buffered,
                },
                &ScanProgress::default(),
                &mut |objects| {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                    taken += 1;
                    lead = lead.max(server.seen().len().saturating_sub(taken));
                    got.extend(objects.into_iter().map(|o| o.key));
                    Ok(())
                },
            )
            .unwrap();
            (got, lead)
        };
        let (got, lead) = run(30);
        assert_eq!(got, expected);
        // Twice the cap, the second cap, at ten keys a page; a page in
        // flight for each worker and the one being taken; and discovery's
        // requests, made before anything was taken (five here).
        let bound = 2 * 30 / 10 + (8 + 1) + 5;
        assert!(lead <= bound, "{lead} requests ahead with a cap of 30");
        let (got, uncapped) = run(usize::MAX);
        assert_eq!(got, expected);
        assert!(
            uncapped > bound,
            "uncapped, the workers ran only {uncapped} ahead; the test proves nothing"
        );
    }

    /// Keys renewed while many workers share them: one fetch per renewal,
    /// not one per worker, and none of them lapsed on the way. The issuer is
    /// slow so that workers arrive while a renewal is under way.
    #[test]
    fn keys_shared_by_workers_are_renewed_once_each_time() {
        let keys = awkward_bucket();
        let (container, valid_until) = slow_issuer(3, std::time::Duration::from_millis(300));
        let refusals = Arc::new(Mutex::new(0u32));
        let counted = Arc::clone(&refusals);
        let bucket = bucket_with(&keys, move |_, request| {
            let token = request.header("x-amz-security-token").unwrap_or("");
            let alive = valid_until
                .lock()
                .unwrap()
                .get(token)
                .is_some_and(|until| request.at_ms < *until);
            if !alive {
                *counted.lock().unwrap() += 1;
                return Some((
                    400,
                    "<Error><Code>ExpiredToken</Code><Message>expired</Message></Error>".into(),
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
            None
        });
        let url = S3Url::parse("s3://b").unwrap();
        let progress = Arc::new(ScanProgress::default());
        let scan = scan_with(
            &url,
            renewing(&container, &bucket),
            Some(7),
            4,
            Arc::clone(&progress),
        )
        .unwrap();
        let (one, one_progress) = list_stand_in(&keys, "", 1000, 1);
        assert_eq!(outcome(&scan, &progress), outcome(&one, &one_progress));
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
        // Keys living two to three seconds are renewed a second or more
        // apart; workers renewing each for itself would be milliseconds apart.
        let fetched: Vec<i64> = container.seen().iter().map(|r| r.at_ms).collect();
        let closest = fetched.windows(2).map(|w| w[1] - w[0]).min().unwrap();
        assert!(
            closest >= 500,
            "two renewals {closest} ms apart: {fetched:?}"
        );
    }

    /// Through an endpoint the listing is one stream unless `--threads` asks
    /// for more: on MinIO, ranges side by side are slower than one. Asked,
    /// the same endpoint is split.
    #[test]
    fn an_endpoint_lists_in_one_stream_unless_asked_for_more() {
        let keys: Vec<(String, u64)> = (0..2500)
            .map(|n| (format!("d{:02}/k{n:04}", n % 50), 1))
            .collect();
        let url = S3Url::parse("s3://b").unwrap();
        let listed = |threads: Option<u16>| {
            let server = client::test_server::bucket(keys.clone());
            let flags = config::Flags {
                endpoint: Some(server.address.clone()),
                region: Some("us-east-1".into()),
                no_sign_request: true,
                threads,
                ..Default::default()
            };
            let scan = scan(&url, &flags, Arc::default()).unwrap();
            let delimited = server
                .seen()
                .iter()
                .filter(|r| r.target.contains("delimiter="))
                .count();
            (scan.pages, delimited)
        };
        assert_eq!(
            listed(None),
            (3, 0),
            "one stream: three pages, nothing split"
        );
        let (pages, delimited) = listed(Some(4));
        assert!(
            delimited > 0,
            "asked for 4, the bucket was split ({pages} requests)"
        );
    }

    /// A folder with too many objects of its own is read once by discovery:
    /// not probed again at every level below, and not read again by the
    /// range after it, which starts after the last key read.
    #[test]
    fn a_folder_discovery_stops_expanding_is_read_once() {
        let mut keys: Vec<(String, u64)> =
            (0..12_000).map(|n| (format!("flat/k{n:05}"), 1)).collect();
        for x in 0..10 {
            for y in 0..10 {
                keys.push((format!("deep/x{x}/y{y}/z"), 1));
            }
        }
        let (one, one_progress) = list_stand_in(&keys, "", 1000, 1);
        let server = client::test_server::bucket(keys);
        let progress = Arc::new(ScanProgress::default());
        let url = S3Url::parse("s3://b").unwrap();
        let many = scan_with(
            &url,
            anonymous(&server.address),
            None,
            16,
            Arc::clone(&progress),
        )
        .unwrap();
        assert_eq!(outcome(&many, &progress), outcome(&one, &one_progress));
        let seen = server.seen();
        let probes = seen
            .iter()
            .filter(|r| r.target.contains("delimiter=") && r.target.contains("prefix=flat"))
            .count();
        assert_eq!(probes, 11, "eleven pages, read once, three levels deep");
        let flat_keys_read: usize = seen
            .iter()
            .filter(|r| r.target.contains("prefix=flat") && !r.target.contains("delimiter="))
            .count();
        assert_eq!(
            flat_keys_read, 1,
            "the 1000 keys after the probe, in one page"
        );
    }
}
