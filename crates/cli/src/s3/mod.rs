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
mod keys;
mod sigv4;
mod xml;

#[cfg(test)]
mod minio_tests;

use std::path::PathBuf;
use std::sync::Arc;
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
    let tree = Tree::from_nested(PathBuf::from(&root), root_node);
    Ok(S3Scan {
        tree,
        stats,
        host,
        region: http.region().to_string(),
        pages,
        duration_ms: started.elapsed().as_millis() as u64,
    })
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
}
