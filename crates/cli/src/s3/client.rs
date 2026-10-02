//! The requests: one signed GET per page of a listing.
//!
//! Three things a single GET does not cover, and why each is here:
//!
//! * **Retries.** S3 answers load with 503 `SlowDown` and the occasional 500,
//!   and says to back off and try again. A ten-million-key listing is ten
//!   thousand requests; without retries one hiccup throws the whole listing
//!   away.
//! * **The wrong region.** A bucket lives in one region and the request has to
//!   be addressed and signed for it. AWS answers a wrong guess with 301
//!   `PermanentRedirect` or 400 `AuthorizationHeaderMalformed`, and names the
//!   right region in `x-amz-bucket-region` or the body. That is retried once,
//!   in the region named, and said on stderr so the next run can pass it.
//! * **Cancellation.** Checked before every page and during every backoff, and
//!   a cancelled listing returns `ErrorKind::Interrupted` and no tree
//!   (invariant 5).

use std::io::Read;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use spacetrace_scan_core::ScanProgress;

use super::config::{Endpoint, Settings};
use super::sigv4::{self, Scope, EMPTY_SHA256};
use super::xml::{self, ListPage};

/// A page is at most 1000 keys of at most 1024 bytes, URL-encoded: about
/// 10 MB in the worst case. Anything past this is not a listing.
const MAX_BODY_BYTES: u64 = 64 * 1024 * 1024;

/// Attempts per request, the first included. With the delays below the last
/// retry starts about 15 s after the first failure, which outlasts the
/// throttling S3 documents and does not outlast a person's patience.
const ATTEMPTS: u32 = 6;
const FIRST_BACKOFF: Duration = Duration::from_millis(500);

/// Per request. A page normally takes well under a second; this is for a
/// connection that went silent, which a retry then gets past.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// What the listing asks for and where.
pub struct Bucket {
    pub name: String,
    /// `""` or ending in `/`.
    pub prefix: String,
}

pub struct Client {
    http: reqwest::blocking::Client,
    settings: Settings,
    /// Changes at most once, when AWS names the bucket's real region.
    region: String,
    redirected: bool,
    /// Keys per page; S3's own default and maximum when `None`.
    pub page_size: Option<u32>,
    /// The first retry's delay, doubling after. A field so a test can make
    /// six attempts in milliseconds.
    first_backoff: Duration,
}

/// The service the listing went to, for the snapshot's `host` and for the
/// output. Never carries credentials.
pub fn identity(settings: &Settings) -> String {
    match &settings.endpoint {
        Some(endpoint) => endpoint.authority.clone(),
        // Region-free on purpose: a bucket name is global on AWS, and a
        // listing that was redirected to the right region is still the same
        // bucket. `diff` finds "the previous snapshot of this target" by
        // `host` + `root`, and the region is not part of what that means.
        None => "s3.amazonaws.com".to_string(),
    }
}

impl Client {
    pub fn new(settings: Settings) -> Result<Client> {
        let http = reqwest::blocking::Client::builder()
            // S3 never redirects a listing anywhere useful by itself: a 301
            // has no Location, and a 307 would be followed with a signature
            // for the wrong host. Region handling below does it properly.
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("spacetrace/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("building the HTTP client")?;
        if let Some(warning) = cleartext_warning(&settings) {
            eprintln!("{warning}");
        }
        Ok(Client {
            http,
            region: settings.region.clone(),
            settings,
            redirected: false,
            page_size: None,
            first_backoff: FIRST_BACKOFF,
        })
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    /// One page, retried and redirected as needed.
    pub fn list_page(
        &mut self,
        bucket: &Bucket,
        token: Option<&str>,
        progress: &ScanProgress,
    ) -> Result<ListPage> {
        let mut query: Vec<(String, String)> = vec![
            ("list-type".into(), "2".into()),
            ("encoding-type".into(), "url".into()),
        ];
        if !bucket.prefix.is_empty() {
            query.push(("prefix".into(), bucket.prefix.clone()));
        }
        if let Some(size) = self.page_size {
            query.push(("max-keys".into(), size.to_string()));
        }
        if let Some(token) = token {
            query.push(("continuation-token".into(), token.to_string()));
        }

        let mut attempt = 0;
        loop {
            check_cancelled(progress)?;
            attempt += 1;
            let outcome = self.send("GET", bucket, &query, Vec::new());
            let failure = match outcome {
                Ok(Response {
                    status: 200, body, ..
                }) => {
                    return xml::parse_list(&body).with_context(|| {
                        format!(
                            "{} sent a listing that cannot be read",
                            self.describe(bucket)
                        )
                    });
                }
                Ok(response) => response,
                Err(unusable) if unusable.downcast_ref::<Final>().is_some() => {
                    return Err(unusable)
                        .with_context(|| format!("{} sent a response", self.describe(bucket)));
                }
                Err(transport) => {
                    if attempt >= ATTEMPTS {
                        return Err(transport).with_context(|| {
                            format!(
                                "{} did not answer after {ATTEMPTS} attempts",
                                self.describe(bucket)
                            )
                        });
                    }
                    backoff(self.first_backoff, attempt, progress)?;
                    continue;
                }
            };

            let error = xml::parse_error(&failure.body);
            if let Some(region) = self.region_to_retry_in(&failure, error.as_ref()) {
                eprintln!(
                    "note: s3://{} is in {region}, not {}; listing it there (pass --region {region} to skip this)",
                    bucket.name, self.region
                );
                self.region = region;
                self.redirected = true;
                attempt = 0;
                continue;
            }
            if is_retryable(failure.status, error.as_ref()) && attempt < ATTEMPTS {
                backoff(self.first_backoff, attempt, progress)?;
                continue;
            }
            return Err(self.failure(bucket, &failure, error));
        }
    }

    /// A request against the bucket. `pub(super)` for the integration test,
    /// which fills a bucket through it.
    pub(super) fn send(
        &self,
        method: &str,
        bucket: &Bucket,
        query: &[(String, String)],
        body: Vec<u8>,
    ) -> Result<Response> {
        self.send_key(method, bucket, None, query, body)
    }

    pub(super) fn send_key(
        &self,
        method: &str,
        bucket: &Bucket,
        key: Option<&str>,
        query: &[(String, String)],
        body: Vec<u8>,
    ) -> Result<Response> {
        let (scheme, host, mut path) = self.address(bucket);
        if let Some(key) = key {
            if !path.ends_with('/') {
                path.push('/');
            }
            path.push_str(key);
        }
        let encoded_query = {
            let mut pairs: Vec<String> = query
                .iter()
                .map(|(k, v)| {
                    format!(
                        "{}={}",
                        sigv4::uri_encode(k, true),
                        sigv4::uri_encode(v, true)
                    )
                })
                .collect();
            pairs.sort();
            pairs.join("&")
        };
        let mut url = format!("{scheme}://{host}{}", sigv4::uri_encode(&path, false));
        if !encoded_query.is_empty() {
            url.push('?');
            url.push_str(&encoded_query);
        }

        let mut request = match method {
            "GET" => self.http.get(&url),
            "PUT" => self.http.put(&url),
            "DELETE" => self.http.delete(&url),
            other => bail!("unsupported method {other}"),
        };

        if let Some(credentials) = &self.settings.credentials {
            let payload = match body.is_empty() {
                true => EMPTY_SHA256.to_string(),
                false => sigv4::sha256_hex(&body),
            };
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let amz_date = sigv4::amz_date(now);
            let mut headers = vec![
                ("host".to_string(), host.clone()),
                ("x-amz-content-sha256".to_string(), payload.clone()),
                ("x-amz-date".to_string(), amz_date.clone()),
            ];
            if let Some(token) = &credentials.session_token {
                headers.push(("x-amz-security-token".to_string(), token.clone()));
            }
            let authorization = sigv4::authorization(
                credentials,
                &Scope {
                    amz_date: &amz_date,
                    region: &self.region,
                    service: "s3",
                },
                &sigv4::Request {
                    method,
                    path: &path,
                    query,
                    headers: &headers,
                    payload_sha256: &payload,
                },
            );
            // `host` is not set by hand: reqwest writes it from the URL, and
            // the value signed above is built from the same URL parts.
            for (name, value) in headers.into_iter().filter(|(n, _)| n != "host") {
                request = request.header(name, value);
            }
            request = request.header(reqwest::header::AUTHORIZATION, authorization);
        }
        if !body.is_empty() {
            request = request.body(body);
        }

        // `without_url`: reqwest's text ends in the whole URL, query included,
        // and the query is the prefix and the server's continuation token.
        let response = request
            .send()
            .map_err(reqwest::Error::without_url)
            .with_context(|| format!("requesting {}", redact(&url)))?;
        let status = response.status().as_u16();
        let bucket_region = response
            .headers()
            .get("x-amz-bucket-region")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let mut raw = Vec::new();
        response
            .take(MAX_BODY_BYTES + 1)
            .read_to_end(&mut raw)
            .map_err(without_url)
            .context("reading the response")?;
        // Both refusals are final. They are the server's answer rather than a
        // dropped connection, and retrying would fetch the same answer again —
        // up to six times 64 MiB of it.
        if raw.len() as u64 > MAX_BODY_BYTES {
            return Err(Final(format!(
                "the response is larger than {MAX_BODY_BYTES} bytes; refusing to buffer it"
            ))
            .into());
        }
        let body =
            String::from_utf8(raw).map_err(|_| Final("the response is not UTF-8".to_string()))?;
        Ok(Response {
            status,
            body,
            bucket_region,
        })
    }

    /// Scheme, `Host`, and the decoded path for the bucket.
    ///
    /// AWS gets virtual-hosted addressing, which is what it recommends and the
    /// only kind every new bucket is promised; a bucket whose name cannot be a
    /// TLS-matching host label — one with a dot — falls back to path style,
    /// as the AWS CLI does. Every other service gets path style, the one
    /// MinIO, R2, B2 and Wasabi all accept without DNS set up for it.
    fn address(&self, bucket: &Bucket) -> (String, String, String) {
        match &self.settings.endpoint {
            Some(Endpoint { scheme, authority }) => (
                scheme.clone(),
                authority.clone(),
                format!("/{}", bucket.name),
            ),
            None if is_host_label(&bucket.name) => (
                "https".into(),
                format!("{}.s3.{}.amazonaws.com", bucket.name, self.region),
                "/".into(),
            ),
            None => (
                "https".into(),
                format!("s3.{}.amazonaws.com", self.region),
                format!("/{}", bucket.name),
            ),
        }
    }

    fn region_to_retry_in(
        &self,
        response: &Response,
        error: Option<&xml::ErrorBody>,
    ) -> Option<String> {
        if self.redirected || !matches!(response.status, 301 | 307 | 400) {
            return None;
        }
        let named = response
            .bucket_region
            .clone()
            .or_else(|| error.and_then(|e| e.region.clone()))
            .or_else(|| error.and_then(|e| e.endpoint.as_deref().and_then(region_of_endpoint)))?;
        let named = named.trim().to_string();
        (!named.is_empty() && named != self.region && is_region_name(&named)).then_some(named)
    }

    fn describe(&self, bucket: &Bucket) -> String {
        format!("s3://{} at {}", bucket.name, identity(&self.settings))
    }

    /// The error a failed request ends the listing with: S3's own code and
    /// message, plus what to do about the common ones. Only those two fields
    /// are shown — `SignatureDoesNotMatch` bodies also carry the string that
    /// was signed, which is no business of a terminal.
    fn failure(
        &self,
        bucket: &Bucket,
        response: &Response,
        error: Option<xml::ErrorBody>,
    ) -> anyhow::Error {
        let what = self.describe(bucket);
        let Some(error) = error else {
            return anyhow::anyhow!(
                "{what} answered HTTP {} with no S3 error in the body",
                response.status
            );
        };
        let hint = match error.code.as_str() {
            "NoSuchBucket" => " There is no such bucket at this endpoint.",
            "AccessDenied" | "AllAccessDisabled" if self.settings.credentials.is_none() => {
                " Unsigned requests are not allowed here; drop --no-sign-request and supply credentials."
            }
            "AccessDenied" => {
                " These credentials may not list this bucket (s3:ListBucket). For a public bucket, try --no-sign-request."
            }
            "InvalidAccessKeyId" => " The access key id is not known to this service.",
            "SignatureDoesNotMatch" => " The secret key does not belong to this access key id.",
            "RequestTimeTooSkewed" => " This machine's clock is too far off; S3 allows 15 minutes.",
            "PermanentRedirect" | "AuthorizationHeaderMalformed" => {
                " The bucket is in another region; pass --region."
            }
            "ExpiredToken" | "InvalidToken" => " The session token has expired; fetch new credentials.",
            _ => "",
        };
        let code = for_terminal(&error.code, 64);
        let message = for_terminal(&error.message, 400);
        let message = message.trim_end_matches('.');
        anyhow::anyhow!(
            "{what} refused the listing: {code} ({}): {message}.{hint}",
            response.status,
        )
    }
}

/// A response that arrived and cannot be used. Not retried.
#[derive(Debug)]
struct Final(String);

impl std::fmt::Display for Final {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Final {}

/// An I/O error from reading a body, with the URL taken out of the reqwest
/// error inside it for the same reason as at `send`.
fn without_url(e: std::io::Error) -> std::io::Error {
    let kind = e.kind();
    match e.into_inner() {
        Some(inner) => match inner.downcast::<reqwest::Error>() {
            Ok(reqwest_error) => std::io::Error::new(kind, reqwest_error.without_url()),
            Err(other) => std::io::Error::new(kind, other),
        },
        None => std::io::Error::from(kind),
    }
}

pub struct Response {
    pub status: u16,
    pub body: String,
    bucket_region: Option<String>,
}

/// The warning for signed requests over plain HTTP to another machine, or
/// `None`. The secret itself never travels, but the access key id, a session
/// token and a signed GET anyone on the path can replay for 15 minutes do,
/// and the listing can be altered on the way back. Loopback is exempt: a
/// MinIO on this machine is the one case plain HTTP is ordinary.
fn cleartext_warning(settings: &Settings) -> Option<String> {
    let endpoint = settings.endpoint.as_ref()?;
    settings.credentials.as_ref()?;
    if endpoint.scheme != "http" {
        return None;
    }
    let host = match endpoint.authority.rsplit_once(':') {
        // `[::1]:9000` and `host:9000`; a bare IPv6 address has no port here
        // because `Endpoint::parse` brackets it.
        Some((host, port)) if port.bytes().all(|b| b.is_ascii_digit()) => host,
        _ => endpoint.authority.as_str(),
    };
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let loopback = bare == "localhost"
        || bare
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    (!loopback).then(|| {
        format!(
            "warning: signing requests to {} over plain http: the access key id, any session \
             token and a replayable signed request travel unencrypted, and the listing can be \
             altered in transit. Use https:// for anything but this machine.",
            endpoint.authority
        )
    })
}

/// Server text made safe to print: control characters escaped, so none is
/// interpreted by the terminal, and at most `limit` characters of it.
fn for_terminal(text: &str, limit: usize) -> String {
    let mut out = String::new();
    for (i, c) in text.chars().enumerate() {
        if i == limit {
            out.push('…');
            break;
        }
        match c.is_control() {
            true => out.extend(c.escape_default()),
            false => out.push(c),
        }
    }
    out
}

fn is_retryable(status: u16, error: Option<&xml::ErrorBody>) -> bool {
    let code = error.map(|e| e.code.as_str()).unwrap_or("");
    matches!(status, 500 | 502 | 503 | 504)
        || matches!(
            code,
            "SlowDown" | "InternalError" | "RequestTimeout" | "ServiceUnavailable"
        )
}

/// Sleep before attempt `attempt + 1`, in short slices so a cancel is noticed.
fn backoff(first: Duration, attempt: u32, progress: &ScanProgress) -> Result<()> {
    let delay = first * 2u32.saturating_pow(attempt.saturating_sub(1));
    let until = std::time::Instant::now() + delay;
    while std::time::Instant::now() < until {
        check_cancelled(progress)?;
        std::thread::sleep(Duration::from_millis(50).min(delay));
    }
    Ok(())
}

pub fn check_cancelled(progress: &ScanProgress) -> Result<()> {
    if progress.is_cancelled() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "the listing was cancelled",
        )
        .into());
    }
    Ok(())
}

/// A bucket name usable as one DNS label under a wildcard certificate.
fn is_host_label(name: &str) -> bool {
    (3..=63).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

/// What a region name can look like. Checked because it goes into a host name:
/// a server answering with `evil.example/` must not get to choose where the
/// next signed request goes.
fn is_region_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `bucket.s3.eu-west-1.amazonaws.com` or `s3.eu-west-1.amazonaws.com` →
/// `eu-west-1`. The legacy global form `s3.amazonaws.com` is us-east-1.
fn region_of_endpoint(endpoint: &str) -> Option<String> {
    let host = endpoint.trim().strip_suffix(".amazonaws.com")?;
    let labels: Vec<&str> = host.split('.').collect();
    let at = labels
        .iter()
        .rposition(|l| *l == "s3" || l.starts_with("s3-"))?;
    match labels.get(at + 1) {
        Some(region) => Some((*region).to_string()),
        None if labels[at] == "s3" => Some("us-east-1".to_string()),
        None => labels[at].strip_prefix("s3-").map(str::to_string),
    }
}

/// A URL with its query removed, for an error message: the query of a
/// listing holds nothing secret, but a continuation token is noise.
fn redact(url: &str) -> &str {
    url.split('?').next().unwrap_or(url)
}

/// Bump the walk's counters by one page: objects as files, bytes, folders.
pub fn count_page(progress: &ScanProgress, files: u64, bytes: u64, folders: u64) {
    progress.files.fetch_add(files, Ordering::Relaxed);
    progress.bytes.fetch_add(bytes, Ordering::Relaxed);
    progress.dirs.store(folders, Ordering::Relaxed);
}

/// A server that answers each connection with the next canned response and
/// records the request head it was sent. Real TCP and real HTTP through
/// reqwest, so the retry and redirect loops run as they run in use, not as a
/// function is called. Status 0 closes the connection without an answer.
#[cfg(test)]
pub(super) mod test_server {
    pub type Canned = (u16, &'static str, Vec<u8>);

    pub fn canned(responses: Vec<Canned>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for (status, reason, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    head.push_str(&line);
                }
                seen.push(head);
                if status == 0 {
                    continue;
                }
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/xml\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
            }
            seen
        });
        (address, handle)
    }
}

impl Client {
    #[cfg(test)]
    pub(super) fn with_quick_retries(mut self) -> Self {
        self.first_backoff = Duration::from_millis(1);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::test_server::canned;
    use super::*;

    #[test]
    fn the_region_is_read_out_of_an_endpoint() {
        assert_eq!(
            region_of_endpoint("b.s3.eu-west-1.amazonaws.com").as_deref(),
            Some("eu-west-1")
        );
        assert_eq!(
            region_of_endpoint("s3.ap-south-1.amazonaws.com").as_deref(),
            Some("ap-south-1")
        );
        assert_eq!(
            region_of_endpoint("b.s3.amazonaws.com").as_deref(),
            Some("us-east-1")
        );
        assert_eq!(
            region_of_endpoint("b.s3-us-west-2.amazonaws.com").as_deref(),
            Some("us-west-2")
        );
        assert_eq!(region_of_endpoint("example.com"), None);
    }

    #[test]
    fn a_region_has_to_look_like_one_before_it_goes_into_a_host_name() {
        assert!(is_region_name("eu-west-1"));
        assert!(!is_region_name("evil.example.com/x"));
        assert!(!is_region_name(""));
        assert!(!is_region_name(&"a".repeat(64)));
    }

    #[test]
    fn dotted_or_odd_bucket_names_are_not_host_labels() {
        assert!(is_host_label("my-bucket-1"));
        assert!(!is_host_label("my.bucket"));
        assert!(!is_host_label("My_Bucket"));
        assert!(!is_host_label("ab"));
    }

    fn client(endpoint: Option<&str>, region: &str) -> Client {
        Client::new(Settings {
            credentials: None,
            region: region.into(),
            endpoint: endpoint.map(|e| Endpoint::parse(e).unwrap()),
        })
        .unwrap()
    }

    fn bucket(name: &str) -> Bucket {
        Bucket {
            name: name.into(),
            prefix: String::new(),
        }
    }

    #[test]
    fn aws_is_addressed_virtual_hosted_and_others_path_style() {
        let aws = client(None, "eu-west-1");
        assert_eq!(
            aws.address(&bucket("photos")),
            (
                "https".into(),
                "photos.s3.eu-west-1.amazonaws.com".into(),
                "/".into()
            )
        );
        assert_eq!(
            aws.address(&bucket("my.dotted")),
            (
                "https".into(),
                "s3.eu-west-1.amazonaws.com".into(),
                "/my.dotted".into()
            )
        );
        let minio = client(Some("http://127.0.0.1:9000"), "us-east-1");
        assert_eq!(
            minio.address(&bucket("photos")),
            ("http".into(), "127.0.0.1:9000".into(), "/photos".into())
        );
        assert_eq!(identity(&aws.settings), "s3.amazonaws.com");
        assert_eq!(identity(&minio.settings), "127.0.0.1:9000");
    }

    fn response(status: u16, region_header: Option<&str>) -> Response {
        Response {
            status,
            body: String::new(),
            bucket_region: region_header.map(str::to_string),
        }
    }

    #[test]
    fn a_wrong_region_is_retried_once_in_the_one_named() {
        let mut aws = client(None, "us-east-1");
        let header = response(301, Some("eu-west-1"));
        assert_eq!(
            aws.region_to_retry_in(&header, None).as_deref(),
            Some("eu-west-1")
        );

        let body = xml::ErrorBody {
            code: "AuthorizationHeaderMalformed".into(),
            region: Some("ap-south-1".into()),
            ..Default::default()
        };
        assert_eq!(
            aws.region_to_retry_in(&response(400, None), Some(&body))
                .as_deref(),
            Some("ap-south-1")
        );
        let redirect = xml::ErrorBody {
            code: "PermanentRedirect".into(),
            endpoint: Some("b.s3.eu-central-1.amazonaws.com".into()),
            ..Default::default()
        };
        assert_eq!(
            aws.region_to_retry_in(&response(301, None), Some(&redirect))
                .as_deref(),
            Some("eu-central-1")
        );

        // The same region again, a bad name, and a second redirect are not
        // retried: each would be a loop or a request sent somewhere chosen by
        // the server.
        assert_eq!(
            aws.region_to_retry_in(&response(301, Some("us-east-1")), None),
            None
        );
        assert_eq!(
            aws.region_to_retry_in(&response(301, Some("x.evil/")), None),
            None
        );
        assert_eq!(
            aws.region_to_retry_in(&response(403, Some("eu-west-1")), None),
            None
        );
        aws.redirected = true;
        assert_eq!(aws.region_to_retry_in(&header, None), None);
    }

    /// The body AWS really sent for `sentinel-cogs` asked for in us-east-1
    /// (captured; see xml.rs). It names the legacy `s3-us-west-2` endpoint,
    /// and that has to come out as a region.
    #[test]
    fn the_captured_aws_redirect_leads_to_the_right_region() {
        let aws = client(None, "us-east-1");
        let body = include_str!("testdata/aws-permanent-redirect.xml");
        let error = xml::parse_error(body);
        let response = Response {
            status: 301,
            body: body.to_string(),
            bucket_region: None,
        };
        assert_eq!(
            aws.region_to_retry_in(&response, error.as_ref()).as_deref(),
            Some("us-west-2")
        );
    }

    #[test]
    fn throttling_and_server_errors_are_retried_and_refusals_are_not() {
        let slow = xml::ErrorBody {
            code: "SlowDown".into(),
            ..Default::default()
        };
        assert!(is_retryable(503, Some(&slow)));
        assert!(is_retryable(500, None));
        assert!(!is_retryable(403, None));
        assert!(!is_retryable(404, None));
    }

    /// Invariant 5, the way the repository tests it: cancelled before the
    /// first request, so nothing is fetched and no counter moves.
    #[test]
    fn a_cancelled_listing_sends_nothing_and_says_interrupted() {
        let progress = ScanProgress::default();
        progress.cancel();
        // An address nothing listens on: reaching the network at all would
        // turn into a different error after a timeout.
        let mut c = client(Some("http://127.0.0.1:9"), "us-east-1");
        let err = c.list_page(&bucket("b"), None, &progress).unwrap_err();
        let io = err.downcast_ref::<std::io::Error>().expect("an io::Error");
        assert_eq!(io.kind(), std::io::ErrorKind::Interrupted);
        assert_eq!(progress.files.load(Ordering::Relaxed), 0);
    }

    const ONE_KEY: &str = "<ListBucketResult><IsTruncated>false</IsTruncated>\
        <Contents><Key>k</Key><Size>5</Size></Contents></ListBucketResult>";

    fn signed_client(endpoint: &str) -> Client {
        Client::new(Settings {
            credentials: Some(sigv4::Credentials {
                access_key_id: "AKIDTEST".into(),
                secret_access_key: "secret-never-shown".into(),
                session_token: Some("token-for-the-header".into()),
            }),
            region: "us-east-1".into(),
            endpoint: Some(Endpoint::parse(endpoint).unwrap()),
        })
        .unwrap()
    }

    #[test]
    fn slow_down_is_retried_until_the_listing_comes() {
        let slow = "<Error><Code>SlowDown</Code><Message>Please reduce your request rate.</Message></Error>";
        let (endpoint, server) = canned(vec![
            (503, "Slow Down", slow.into()),
            (500, "Internal Server Error", Vec::new()),
            (200, "OK", ONE_KEY.into()),
        ]);
        let mut c = signed_client(&endpoint);
        let page = c
            .list_page(&bucket("b"), None, &ScanProgress::default())
            .unwrap();
        assert_eq!(page.objects.len(), 1);
        let seen = server.join().unwrap();
        assert_eq!(seen.len(), 3, "two retries, then the answer");
        assert!(
            seen[0].starts_with("GET /b?encoding-type=url&list-type=2 HTTP/1.1"),
            "{}",
            seen[0]
        );
        let lower = seen[0].to_ascii_lowercase();
        assert!(
            lower.contains("x-amz-security-token: token-for-the-header"),
            "{}",
            seen[0]
        );
        assert!(lower.contains("credential=akidtest/"), "{}", seen[0]);
        assert!(
            !seen[0].contains("secret-never-shown"),
            "the secret never goes on the wire"
        );
    }

    /// The wrong region on a service that is not AWS: MinIO set up with a
    /// region answers 400 `AuthorizationHeaderMalformed` and names it. The
    /// next request has to be signed for that region.
    #[test]
    fn a_named_region_is_signed_for_on_the_next_request() {
        let malformed = "<Error><Code>AuthorizationHeaderMalformed</Code><Message>the region \
             'us-east-1' is wrong; expecting 'eu-central-1'</Message><Region>eu-central-1</Region></Error>";
        let (endpoint, server) = canned(vec![
            (400, "Bad Request", malformed.into()),
            (200, "OK", ONE_KEY.into()),
        ]);
        let mut c = signed_client(&endpoint);
        c.list_page(&bucket("b"), None, &ScanProgress::default())
            .unwrap();
        assert_eq!(c.region(), "eu-central-1");
        let seen = server.join().unwrap();
        assert!(
            seen[0].contains("/us-east-1/s3/aws4_request"),
            "{}",
            seen[0]
        );
        assert!(
            seen[1].contains("/eu-central-1/s3/aws4_request"),
            "{}",
            seen[1]
        );
    }

    #[test]
    fn a_refusal_is_not_retried_and_says_what_s3_said() {
        let denied = "<Error><Code>AccessDenied</Code><Message>Access Denied</Message>\
             <StringToSign>AWS4-HMAC-SHA256 signed material</StringToSign></Error>";
        let (endpoint, server) = canned(vec![(403, "Forbidden", denied.into())]);
        let mut c = signed_client(&endpoint);
        let err = c
            .list_page(&bucket("b"), None, &ScanProgress::default())
            .unwrap_err();
        let text = format!("{err:#}");
        assert!(
            text.contains("AccessDenied (403): Access Denied."),
            "{text}"
        );
        assert!(text.contains("--no-sign-request"), "{text}");
        assert!(!text.contains("signed material"), "{text}");
        assert!(!text.contains("token-for-the-header"), "{text}");
        assert_eq!(server.join().unwrap().len(), 1, "one request, no retry");
    }

    /// reqwest's own error text ends in the full URL — prefix, continuation
    /// token and all — and used to reach the terminal that way.
    #[test]
    fn a_transport_failure_does_not_print_the_query() {
        let dropped = (0..ATTEMPTS).map(|_| (0, "", Vec::new())).collect();
        let (endpoint, server) = canned(dropped);
        let mut c = signed_client(&endpoint).with_quick_retries();
        let mut b = bucket("b");
        b.prefix = "private-prefix/".into();
        let err = c
            .list_page(&b, Some("token-from-the-server"), &ScanProgress::default())
            .unwrap_err();
        let text = format!("{err:#}");
        assert!(text.contains("did not answer after 6 attempts"), "{text}");
        assert!(!text.contains("token-from-the-server"), "{text}");
        assert!(!text.contains("private-prefix"), "{text}");
        assert!(!text.contains("continuation-token"), "{text}");
        assert_eq!(server.join().unwrap().len(), ATTEMPTS as usize);
    }

    /// A body that can never be read is the server's answer, not a hiccup:
    /// retrying it would fetch up to 6 x 64 MiB for the same refusal.
    #[test]
    fn an_unreadable_body_is_not_retried() {
        let (endpoint, server) = canned(vec![(200, "OK", vec![0xff, 0xfe, b'<'])]);
        let mut c = signed_client(&endpoint).with_quick_retries();
        let err = c
            .list_page(&bucket("b"), None, &ScanProgress::default())
            .unwrap_err();
        assert!(format!("{err:#}").contains("not UTF-8"), "{err:#}");
        assert_eq!(server.join().unwrap().len(), 1, "one request, no retry");

        let huge = vec![b' '; MAX_BODY_BYTES as usize + 1];
        let (endpoint, server) = canned(vec![(200, "OK", huge)]);
        let mut c = signed_client(&endpoint).with_quick_retries();
        let err = c
            .list_page(&bucket("b"), None, &ScanProgress::default())
            .unwrap_err();
        assert!(format!("{err:#}").contains("larger than"), "{err:#}");
        assert_eq!(server.join().unwrap().len(), 1, "one request, no retry");
    }

    /// The server's message goes to a terminal: control characters would be
    /// interpreted by it, and length is the server's to choose.
    #[test]
    fn a_server_message_is_escaped_and_bounded() {
        let long = "x".repeat(10_000);
        let body = format!(
            "<Error><Code>AccessDenied&#x1B;[2J</Code><Message>&#x1B;[31mred\nline&#7;{long}</Message></Error>"
        );
        let (endpoint, _server) = canned(vec![(403, "Forbidden", body.into_bytes())]);
        let mut c = signed_client(&endpoint);
        let err = c
            .list_page(&bucket("b"), None, &ScanProgress::default())
            .unwrap_err();
        let text = format!("{err:#}");
        assert!(!text.chars().any(|ch| ch.is_control()), "{text:?}");
        assert!(text.contains("\\u{1b}[31mred"), "{text}");
        assert!(
            text.chars().count() < 1_000,
            "{} chars",
            text.chars().count()
        );
    }

    #[test]
    fn credentials_over_plain_http_to_another_machine_are_warned_about() {
        let settings = |endpoint: &str, signed: bool| Settings {
            credentials: signed.then(|| sigv4::Credentials {
                access_key_id: "AKID".into(),
                secret_access_key: "s".into(),
                session_token: None,
            }),
            region: "us-east-1".into(),
            endpoint: Some(Endpoint::parse(endpoint).unwrap()),
        };
        assert!(cleartext_warning(&settings("http://nas.lan:9000", true)).is_some());
        assert!(cleartext_warning(&settings("http://10.0.0.5:9000", true)).is_some());
        for quiet in [
            settings("http://127.0.0.1:9000", true),
            settings("http://localhost:9000", true),
            settings("http://[::1]:9000", true),
            settings("https://nas.lan:9000", true),
            settings("http://nas.lan:9000", false),
        ] {
            assert_eq!(cleartext_warning(&quiet), None, "{:?}", quiet.endpoint);
        }
    }
}
