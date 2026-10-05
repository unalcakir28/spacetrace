//! Just enough XML to read a ListObjectsV2 page and an S3 error body.
//!
//! Not a general parser, and deliberately not a crate: what S3 sends is a flat
//! document of leaf elements, and the only parts of XML that matter for it are
//! the five predefined entities, numeric character references, CDATA,
//! comments, the declaration, namespace prefixes and attributes to skip over.
//! Everything else — `DOCTYPE` above all, which is the door to entity
//! expansion — is refused rather than half-understood.
//!
//! **This is a trust boundary**, like `ncdu_import`: the body came off the
//! network from a server we do not control. Nothing here indexes without a
//! check or recurses on the input, so an adversarial body is an error, never a
//! panic or a stack overflow.
//!
//! Keys are requested with `encoding-type=url`, because a key may contain
//! characters XML 1.0 cannot carry at all (U+0001 is a legal key byte and an
//! illegal XML character). Keys are decoded as they close; whether the
//! server honoured the request is only known from the response's own
//! `<EncodingType>`, which MinIO writes *after* the keys, so a response that
//! turns out not to carry it is read again with the keys taken literally.

use std::borrow::Cow;

use anyhow::{bail, Context, Result};

/// One current object version, as the listing describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Object {
    pub key: String,
    pub size: u64,
    /// `LastModified` as Unix seconds, fraction dropped.
    pub last_modified: i64,
}

/// One page of a ListObjectsV2 listing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListPage {
    pub objects: Vec<Object>,
    pub is_truncated: bool,
    pub next_continuation_token: Option<String>,
}

/// The `<Error>` body S3 answers a failed request with.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
    /// The bucket's real region, which `AuthorizationHeaderMalformed` and some
    /// redirects name.
    pub region: Option<String>,
    /// The host to use instead, which `PermanentRedirect` names.
    pub endpoint: Option<String>,
}

#[derive(Default)]
struct Fields {
    key: Option<String>,
    size: Option<u64>,
    last_modified: Option<i64>,
}

/// Parse a ListObjectsV2 response body.
pub fn parse_list(body: &str) -> Result<ListPage> {
    let read = read_list(body, true)?;
    if !read.url_encoded {
        // The server ignored `encoding-type=url`: the keys are as they are.
        return Ok(read_list(body, false)?.page);
    }
    match read.bad_key {
        Some((key, e)) => Err(e).with_context(|| format!("cannot decode the key {key:?}")),
        None => Ok(read.page),
    }
}

/// One reading of a listing.
struct Read {
    page: ListPage,
    /// The response said `<EncodingType>url</EncodingType>`.
    url_encoded: bool,
    /// The first key that would not decode, kept until the end says whether
    /// it should have.
    bad_key: Option<(String, anyhow::Error)>,
}

fn read_list(body: &str, decode: bool) -> Result<Read> {
    let mut page = ListPage::default();
    // Fields land here as each one closes and become an entry when its
    // `<Contents>` closes; a page is pushed whole, never half an object.
    let mut current = Fields::default();
    let mut url_encoded = false;
    let mut bad_key = None;
    let mut root_seen = false;
    let mut truncation_said = false;

    walk(body, |path, text| {
        match path {
            ["ListBucketResult"] => root_seen = true,
            [first, ..] if *first != "ListBucketResult" => {
                bail!("expected a ListBucketResult, found <{first}>")
            }
            ["ListBucketResult", "IsTruncated"] => {
                truncation_said = true;
                page.is_truncated = match text.trim() {
                    "true" => true,
                    "false" => false,
                    other => bail!("IsTruncated is neither true nor false: {other:?}"),
                };
            }
            ["ListBucketResult", "NextContinuationToken"] => {
                page.next_continuation_token = Some(text.to_string());
            }
            ["ListBucketResult", "EncodingType"] => {
                url_encoded = text.trim().eq_ignore_ascii_case("url");
            }
            ["ListBucketResult", "Contents"] => {
                let fields = std::mem::take(&mut current);
                let key = fields.key.context("an object in the listing has no Key")?;
                let size = fields
                    .size
                    .with_context(|| format!("the object {key:?} has no Size"))?;
                page.objects.push(Object {
                    key,
                    size,
                    // Absent rather than malformed: a time is not worth
                    // failing a listing over, and 0 is what the tree already
                    // means by "unknown".
                    last_modified: fields.last_modified.unwrap_or(0),
                });
            }
            ["ListBucketResult", "Contents", "Key"] => {
                current.key = Some(match decode {
                    false => text.to_string(),
                    true => match url_decode(text) {
                        Ok(key) => key.into_owned(),
                        Err(e) => {
                            bad_key.get_or_insert((text.to_string(), e));
                            text.to_string()
                        }
                    },
                });
            }
            ["ListBucketResult", "Contents", "Size"] => {
                current.size = Some(
                    text.trim()
                        .parse()
                        .with_context(|| format!("Size is not a byte count: {text:?}"))?,
                );
            }
            ["ListBucketResult", "Contents", "LastModified"] => {
                current.last_modified = Some(
                    parse_rfc3339(text.trim())
                        .with_context(|| format!("LastModified is not a timestamp: {text:?}"))?,
                );
            }
            // ETag, StorageClass, Owner, ChecksumAlgorithm, RestoreStatus,
            // and whatever comes next.
            _ => {}
        }
        Ok(())
    })?;

    if !root_seen {
        bail!("the response is not a ListBucketResult");
    }
    // Required rather than defaulted. "Not truncated" is the claim that the
    // listing is complete, and a page that leaves it out — a broken proxy, a
    // body cut short in a way that still parses — would otherwise be saved as
    // the whole bucket: a partial tree reporting a wrong total, which
    // invariant 5 exists to prevent.
    if !truncation_said {
        bail!("the listing does not say whether it is complete (no <IsTruncated>)");
    }
    Ok(Read {
        page,
        url_encoded,
        bad_key,
    })
}

/// Parse an `<Error>` body. `None` when the body is not one — an HTML page
/// from a proxy, an empty body on a HEAD — so the caller falls back to the
/// status line instead of inventing a reason.
pub fn parse_error(body: &str) -> Option<ErrorBody> {
    let mut error = ErrorBody::default();
    let mut root_seen = false;
    let parsed = walk(body, |path, text| {
        match path {
            ["Error"] => root_seen = true,
            [first, ..] if *first != "Error" => bail!("not an error body"),
            ["Error", "Code"] => error.code = text.trim().to_string(),
            ["Error", "Message"] => error.message = text.trim().to_string(),
            ["Error", "Region"] => error.region = Some(text.trim().to_string()),
            ["Error", "Endpoint"] => error.endpoint = Some(text.trim().to_string()),
            _ => {}
        }
        Ok(())
    });
    (parsed.is_ok() && root_seen).then_some(error)
}

/// The keys in an STS `AssumeRole` or `AssumeRoleWithWebIdentity` answer.
#[derive(Default)]
pub struct StsCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
    /// `Expiration` as Unix seconds.
    pub expiration: i64,
}

/// Read `<XResponse><XResult><Credentials>…` out of an STS answer.
///
/// The errors say which field is missing and never quote the body: what is
/// in it is a secret key.
pub fn parse_sts_credentials(body: &str) -> Result<StsCredentials> {
    let mut out = StsCredentials::default();
    let mut expiration = None;
    let parsed = walk(body, |path, text| {
        let [response, result, "Credentials", field] = path else {
            return Ok(());
        };
        if !response.ends_with("Response") || !result.ends_with("Result") {
            return Ok(());
        }
        match *field {
            "AccessKeyId" => out.access_key_id = text.trim().to_string(),
            "SecretAccessKey" => out.secret_access_key = text.trim().to_string(),
            "SessionToken" => out.session_token = text.trim().to_string(),
            "Expiration" => expiration = parse_rfc3339(text.trim()),
            _ => {}
        }
        Ok(())
    });
    // The walker's own message can quote a few characters of the text it
    // stopped at, and the text here is credentials.
    if parsed.is_err() {
        bail!("the response is not well-formed XML");
    }
    for (name, value) in [
        ("AccessKeyId", &out.access_key_id),
        ("SecretAccessKey", &out.secret_access_key),
        ("SessionToken", &out.session_token),
    ] {
        if value.is_empty() {
            bail!("the response carries no {name}");
        }
    }
    out.expiration = expiration.context("the response carries no readable Expiration")?;
    Ok(out)
}

/// The `<ErrorResponse><Error>` body STS answers a refusal with.
pub fn parse_sts_error(body: &str) -> Option<ErrorBody> {
    let mut error = ErrorBody::default();
    let mut root_seen = false;
    let parsed = walk(body, |path, text| {
        match path {
            ["ErrorResponse"] => root_seen = true,
            ["ErrorResponse", "Error", "Code"] => error.code = text.trim().to_string(),
            ["ErrorResponse", "Error", "Message"] => error.message = text.trim().to_string(),
            _ => {}
        }
        Ok(())
    });
    (parsed.is_ok() && root_seen && !error.code.is_empty()).then_some(error)
}

/// A ListObjectsV2 page is four levels deep (`ListBucketResult`, `Contents`,
/// `Owner`, `ID`). Eight times that is room for whatever AWS adds, and the
/// limit means a hostile body cannot make the stack of open names grow with
/// its size.
const MAX_DEPTH: usize = 32;

/// Walk the document, calling `visit` once per element as it closes, with the
/// path of local names from the root and the element's own text.
///
/// Iterative, with an explicit stack: the depth of the input is the server's
/// choice, and recursion would hand it the stack.
fn walk(body: &str, mut visit: impl FnMut(&[&str], &str) -> Result<()>) -> Result<()> {
    let mut stack: Vec<&str> = Vec::new();
    let mut text = String::new();
    let mut rest = body;
    let mut seen_root = false;

    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            if !rest.trim().is_empty() {
                bail!("text outside the root element");
            }
            break;
        };
        let (before, from_lt) = rest.split_at(lt);
        if !before.is_empty() {
            if stack.is_empty() && !before.trim().is_empty() {
                bail!("text outside the root element");
            }
            decode_entities(before, &mut text)?;
        }

        if let Some(after) = from_lt.strip_prefix("<?") {
            rest = skip_past(after, "?>", "a processing instruction")?;
        } else if let Some(after) = from_lt.strip_prefix("<!--") {
            rest = skip_past(after, "-->", "a comment")?;
        } else if let Some(after) = from_lt.strip_prefix("<![CDATA[") {
            let end = after.find("]]>").context("a CDATA section never ends")?;
            text.push_str(&after[..end]);
            rest = &after[end + 3..];
        } else if from_lt.starts_with("<!") {
            // DOCTYPE and its relatives. S3 never sends one, and accepting it
            // means either implementing entity declarations or silently
            // ignoring them; neither belongs on a trust boundary.
            bail!("a document type declaration is not accepted");
        } else if let Some(after) = from_lt.strip_prefix("</") {
            let end = after.find('>').context("an end tag never closes")?;
            let name = local_name(after[..end].trim_end());
            let Some(open) = stack.last() else {
                bail!("</{name}> closes nothing");
            };
            if *open != name {
                bail!("</{name}> closes <{open}>");
            }
            visit(&stack, &text)?;
            text.clear();
            stack.pop();
            rest = &after[end + 1..];
        } else {
            let after = &from_lt[1..];
            let end = tag_end(after).context("a start tag never closes")?;
            let inner = &after[..end];
            let self_closing = inner.ends_with('/');
            let inner = inner.strip_suffix('/').unwrap_or(inner);
            let name_end = inner
                .find(|c: char| c.is_ascii_whitespace())
                .unwrap_or(inner.len());
            let name = local_name(&inner[..name_end]);
            if name.is_empty() {
                bail!("an element without a name");
            }
            if stack.is_empty() {
                if seen_root {
                    bail!("a second root element <{name}>");
                }
                seen_root = true;
            }
            if stack.len() >= MAX_DEPTH {
                bail!("the document nests deeper than {MAX_DEPTH} elements");
            }
            stack.push(name);
            // The element's own text starts here; text before it belonged to
            // the parent, which only ever holds whitespace in what S3 sends.
            text.clear();
            if self_closing {
                visit(&stack, "")?;
                stack.pop();
            }
            rest = &after[end + 1..];
        }
    }

    if let Some(open) = stack.last() {
        bail!("the document ends inside <{open}>");
    }
    if !seen_root {
        bail!("the document has no root element");
    }
    Ok(())
}

fn skip_past<'a>(s: &'a str, marker: &str, what: &str) -> Result<&'a str> {
    let end = s
        .find(marker)
        .with_context(|| format!("{what} never ends"))?;
    Ok(&s[end + marker.len()..])
}

/// Where a start tag's `>` is, stepping over quoted attribute values, which
/// may legally contain one.
fn tag_end(s: &str) -> Option<usize> {
    let mut quote = None;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), _) if c == q => quote = None,
            (None, '>') => return Some(i),
            _ => {}
        }
    }
    None
}

/// `s3:Key` → `Key`. The namespace S3 uses is the default one, but a document
/// that binds it to a prefix is the same document.
fn local_name(qualified: &str) -> &str {
    qualified.rsplit(':').next().unwrap_or(qualified)
}

/// Append `raw` to `out` with its entity and character references resolved.
fn decode_entities(raw: &str, out: &mut String) -> Result<()> {
    let mut rest = raw;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        let semi = after
            .find(';')
            .filter(|&i| i <= 12)
            .context("an '&' that does not start a reference")?;
        let name = &after[..semi];
        match name {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            _ => {
                let code = if let Some(hex) = name.strip_prefix("#x").or(name.strip_prefix("#X")) {
                    u32::from_str_radix(hex, 16).ok()
                } else if let Some(dec) = name.strip_prefix('#') {
                    dec.parse::<u32>().ok()
                } else {
                    bail!("unknown entity &{name};");
                };
                // `char::from_u32` refuses surrogates and anything past
                // U+10FFFF; U+0000 is refused by XML itself.
                let c = code
                    .filter(|&c| c != 0)
                    .and_then(char::from_u32)
                    .with_context(|| format!("&{name}; is not a character"))?;
                out.push(c);
            }
        }
        rest = &after[semi + 1..];
    }
    out.push_str(rest);
    Ok(())
}

/// Undo `encoding-type=url`.
///
/// Form decoding, `+` as a space: that is what S3 writes (a key `a b` comes
/// back as `a+b`, a key `a+b` as `a%2Bb` — both checked against MinIO, see the
/// fixtures), and it is what botocore's own decoder does (`unquote_plus` in
/// `botocore.handlers.decode_list_object_v2`). Most keys have nothing to
/// decode, and come back borrowed.
pub fn url_decode(s: &str) -> Result<Cow<'_, str>> {
    if !s.contains(['%', '+']) {
        return Ok(Cow::Borrowed(s));
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' => {
                let hex = bytes
                    .get(i + 1..i + 3)
                    .and_then(|h| std::str::from_utf8(h).ok())
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                    .context("a '%' not followed by two hex digits")?;
                out.push(hex);
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out)
        .map(Cow::Owned)
        .context("the decoded key is not UTF-8")
}

/// `2026-10-02T12:51:28.817Z` → Unix seconds. RFC 3339, which is what S3 and
/// every compatible service write; an offset other than `Z` is accepted too.
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let digits = |from: usize, len: usize| -> Option<i64> {
        let part = b.get(from..from + len)?;
        part.iter()
            .all(u8::is_ascii_digit)
            .then(|| std::str::from_utf8(part).ok()?.parse().ok())?
    };
    let sep = |at: usize, allowed: &[u8]| b.get(at).is_some_and(|c| allowed.contains(c));

    if !(sep(4, b"-") && sep(7, b"-") && sep(10, b"Tt ") && sep(13, b":") && sep(16, b":")) {
        return None;
    }
    let (year, month, day) = (digits(0, 4)?, digits(5, 2)?, digits(8, 2)?);
    let (hour, minute, second) = (digits(11, 2)?, digits(14, 2)?, digits(17, 2)?);
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    let mut at = 19;
    if sep(at, b".") {
        at += 1;
        let start = at;
        while b.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        if at == start {
            return None;
        }
    }
    let offset = match b.get(at)? {
        b'Z' | b'z' if at + 1 == b.len() => 0,
        sign @ (b'+' | b'-') if at + 6 == b.len() && sep(at + 3, b":") => {
            let (oh, om) = (digits(at + 1, 2)?, digits(at + 4, 2)?);
            if oh > 23 || om > 59 {
                return None;
            }
            let secs = oh * 3600 + om * 60;
            if *sign == b'+' {
                secs
            } else {
                -secs
            }
        }
        _ => return None,
    };

    let days = crate::fmt::days_from_civil(year, month as u32, day as u32);
    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ----------------------------------------- captured from a real MinIO
    //
    // Recorded with curl against MinIO (built from source at commit
    // 7aac2a2c5b7c, 12 February 2026) on 2 October 2026, bucket `fixture`
    // with anonymous read. Saved byte for byte.

    const MINIO_URL: &str = include_str!("testdata/minio-list-url.xml");
    const MINIO_PAGE1: &str = include_str!("testdata/minio-list-page1.xml");
    const MINIO_RAW: &str = include_str!("testdata/minio-list-raw.xml");
    const MINIO_DENIED: &str = include_str!("testdata/minio-access-denied.xml");

    fn keys(page: &ListPage) -> Vec<&str> {
        page.objects.iter().map(|o| o.key.as_str()).collect()
    }

    /// The listing that motivates `encoding-type=url`: a key with U+0001 in
    /// it, which no XML 1.0 document can contain, plus every character this
    /// parser has a reason to care about.
    #[test]
    fn a_url_encoded_minio_listing_decodes_to_the_real_keys() {
        let page = parse_list(MINIO_URL).unwrap();
        assert_eq!(
            keys(&page),
            [
                "a",
                "ctl\u{1}char.txt",
                "dir/100%.txt",
                "dir/a b.txt",
                "dir/plus+sign.txt",
                "dir/ünï©ødé/ファイル.txt",
                "plain.txt",
                "xml&<>\"'.txt",
            ]
        );
        assert!(!page.is_truncated);
        assert_eq!(page.next_continuation_token, None);
        let total: u64 = page.objects.iter().map(|o| o.size).sum();
        assert_eq!(total, 7 + 18 + 18 + 17 + 23 + 39 + 15 + 18);
        // 2026-10-02T12:51:28.817Z
        assert_eq!(page.objects[0].last_modified, 1_790_945_488);
    }

    #[test]
    fn a_truncated_minio_page_carries_its_token() {
        let page = parse_list(MINIO_PAGE1).unwrap();
        assert_eq!(keys(&page), ["a", "ctl\u{1}char.txt", "dir/100%.txt"]);
        assert!(page.is_truncated);
        assert_eq!(
            page.next_continuation_token.as_deref(),
            Some("ZGlyLzEwMCUudHh0W21pbmlvX2NhY2hlOnYyLHJldHVybjpd")
        );
    }

    /// Without `encoding-type=url` the keys arrive as text and must not be
    /// decoded a second time: `100%.txt` stays `100%.txt`, and a `+` stays a
    /// plus rather than turning into a space.
    #[test]
    fn a_listing_without_url_encoding_is_taken_literally() {
        let page = parse_list(MINIO_RAW).unwrap();
        assert_eq!(
            keys(&page),
            [
                "dir/100%.txt",
                "dir/a b.txt",
                "dir/plus+sign.txt",
                "dir/ünï©ødé/ファイル.txt"
            ]
        );
    }

    #[test]
    fn a_minio_error_body_is_read() {
        let error = parse_error(MINIO_DENIED).unwrap();
        assert_eq!(error.code, "AccessDenied");
        assert_eq!(error.message, "Access Denied.");
        assert!(
            parse_list(MINIO_DENIED).is_err(),
            "an error is not a listing"
        );
    }

    // ------------------------------------------------ captured from AWS
    //
    // Anonymous requests to the public `sentinel-cogs` bucket (us-west-2) on
    // 2 October 2026. AWS writes `<EncodingType>` before the keys and MinIO
    // after them; a token with `+` and `/` in it has to survive the trip back.

    const AWS_PAGE1: &str = include_str!("testdata/aws-list-url-page1.xml");
    const AWS_REDIRECT: &str = include_str!("testdata/aws-permanent-redirect.xml");

    #[test]
    fn an_aws_page_is_read_the_same_way() {
        let page = parse_list(AWS_PAGE1).unwrap();
        assert_eq!(
            keys(&page),
            [
                "sentinel-s2-l2a-cogs/1/C/CV/2018/10/S2B_1CCV_20181004_0_L2A/AOT.tif",
                "sentinel-s2-l2a-cogs/1/C/CV/2018/10/S2B_1CCV_20181004_0_L2A/B01.tif",
                "sentinel-s2-l2a-cogs/1/C/CV/2018/10/S2B_1CCV_20181004_0_L2A/B02.tif",
            ]
        );
        assert_eq!(
            page.objects.iter().map(|o| o.size).collect::<Vec<_>>(),
            [50_510, 1_455_332, 38_149_405]
        );
        assert!(page.is_truncated);
        let token = page.next_continuation_token.unwrap();
        assert!(
            token.starts_with("1hbogu5i7+/ct6oOMJPt5aoHoOgUW1umq5cEu+"),
            "{token}"
        );
        // 2020-09-30T20:25:56.000Z
        assert_eq!(page.objects[0].last_modified, 1_601_497_556);
    }

    #[test]
    fn an_aws_redirect_names_the_endpoint_to_use() {
        let error = parse_error(AWS_REDIRECT).unwrap();
        assert_eq!(error.code, "PermanentRedirect");
        assert_eq!(
            error.endpoint.as_deref(),
            Some("sentinel-cogs.s3-us-west-2.amazonaws.com")
        );
    }

    // ------------------------------------------------------- adversarial

    fn list_of(contents: &str) -> String {
        format!(
            "<?xml version=\"1.0\"?>\n<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
             <IsTruncated>false</IsTruncated>{contents}</ListBucketResult>"
        )
    }

    fn object(key_xml: &str) -> String {
        format!(
            "<Contents><Key>{key_xml}</Key><LastModified>2026-01-01T00:00:00Z</LastModified>\
             <Size>1</Size></Contents>"
        )
    }

    fn one_key(contents: &str) -> String {
        let page = parse_list(&list_of(contents)).unwrap();
        assert_eq!(page.objects.len(), 1);
        page.objects[0].key.clone()
    }

    #[test]
    fn the_five_entities_and_numeric_references_are_resolved() {
        assert_eq!(
            one_key(&object(
                "&amp;&lt;&gt;&quot;&apos;&#65;&#x42;&#X43;&#x1F600;"
            )),
            "&<>\"'ABC😀"
        );
    }

    #[test]
    fn cdata_is_taken_verbatim_and_joins_surrounding_text() {
        assert_eq!(one_key(&object("a<![CDATA[<&>]]>b")), "a<&>b");
    }

    #[test]
    fn namespace_prefixes_comments_and_attributes_do_not_change_the_answer() {
        let xml = "<s3:ListBucketResult xmlns:s3=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
             <s3:IsTruncated>false</s3:IsTruncated>\
             <!-- a comment with <Key>bait</Key> inside -->\
             <s3:Contents a=\"x > y\" b='1'><s3:Key>real</s3:Key><s3:Size>5</s3:Size>\
             <s3:Owner><s3:ID>o</s3:ID><s3:Key>not-this-one</s3:Key></s3:Owner></s3:Contents>\
             </s3:ListBucketResult>";
        let page = parse_list(xml).unwrap();
        assert_eq!(keys(&page), ["real"]);
        assert_eq!(page.objects[0].size, 5);
        assert_eq!(page.objects[0].last_modified, 0, "absent time is unknown");
    }

    #[test]
    fn unknown_elements_are_skipped_wherever_they_are() {
        let contents = "<Future><Nested><Deeper/></Nested></Future>\
             <Contents><ChecksumAlgorithm>CRC32</ChecksumAlgorithm><Key>k</Key>\
             <RestoreStatus><IsRestoreInProgress>false</IsRestoreInProgress></RestoreStatus>\
             <Size>3</Size><StorageClass>GLACIER</StorageClass></Contents>";
        assert_eq!(one_key(contents), "k");
    }

    #[test]
    fn self_closing_and_empty_elements_mean_empty_text() {
        let page = parse_list(&list_of(
            "<Prefix/><Contents><Key></Key><Size>0</Size></Contents>",
        ))
        .unwrap();
        assert_eq!(keys(&page), [""]);
    }

    fn refused(xml: &str) -> String {
        format!("{:#}", parse_list(xml).expect_err("should be refused"))
    }

    #[test]
    fn malformed_documents_are_refused_not_guessed_at() {
        assert!(refused(&list_of(&object("a &unknown; b"))).contains("unknown entity"));
        assert!(refused(&list_of(&object("a & b"))).contains("reference"));
        assert!(refused(&list_of(&object("&#0;"))).contains("not a character"));
        assert!(refused(&list_of(&object("&#xD800;"))).contains("not a character"));
        assert!(refused(&list_of(&object("&#x110000;"))).contains("not a character"));
        assert!(refused("<ListBucketResult><Contents></ListBucketResult>").contains("closes"));
        assert!(refused("<ListBucketResult><Contents>").contains("ends inside"));
        assert!(refused("<ListBucketResult><![CDATA[x").contains("CDATA"));
        assert!(refused("<ListBucketResult><!-- x").contains("comment"));
        assert!(refused("<ListBucketResult a=\"x>").contains("never closes"));
        assert!(refused("</ListBucketResult>").contains("closes nothing"));
        assert!(refused("<ListBucketResult/><ListBucketResult/>").contains("second root"));
        assert!(refused("").contains("no root"));
        assert!(refused("hello").contains("outside"));
        assert!(refused("<Other/>").contains("ListBucketResult"));
        assert!(refused(
            "<!DOCTYPE x [<!ENTITY a \"aaaa\">]><ListBucketResult>&a;</ListBucketResult>"
        )
        .contains("document type"));
    }

    #[test]
    fn bad_field_values_are_refused() {
        assert!(refused(&list_of("<Contents><Size>1</Size></Contents>")).contains("no Key"));
        let no_size = list_of("<Contents><Key>k</Key></Contents>");
        assert!(refused(&no_size).contains("no Size"));
        let negative = list_of("<Contents><Key>k</Key><Size>-1</Size></Contents>");
        assert!(refused(&negative).contains("Size"));
        let overflow =
            list_of("<Contents><Key>k</Key><Size>18446744073709551616</Size></Contents>");
        assert!(refused(&overflow).contains("Size"));
        let bad_time = list_of(
            "<Contents><Key>k</Key><Size>1</Size><LastModified>yesterday</LastModified></Contents>",
        );
        assert!(refused(&bad_time).contains("LastModified"));
        let bad_flag = "<ListBucketResult><IsTruncated>maybe</IsTruncated></ListBucketResult>";
        assert!(refused(bad_flag).contains("IsTruncated"));
    }

    #[test]
    fn bad_url_encoding_is_refused() {
        let encoded = |key: &str| {
            format!(
                "<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>{key}</Key><Size>1</Size></Contents>\
                 <EncodingType>url</EncodingType></ListBucketResult>"
            )
        };
        assert!(refused(&encoded("a%2")).contains("two hex digits"));
        assert!(refused(&encoded("a%zz")).contains("two hex digits"));
        assert!(refused(&encoded("%FF%FE")).contains("UTF-8"));
        // And `%` at the very end, where an unchecked `[i + 1]` would panic.
        assert!(refused(&encoded("a%")).contains("two hex digits"));
    }

    /// Depth is the server's choice. A million nested elements must be an
    /// answer, not a stack overflow.
    #[test]
    fn absurd_nesting_does_not_overflow_the_stack() {
        let nested = |depth: usize| {
            let mut xml = String::from("<ListBucketResult><IsTruncated>false</IsTruncated>");
            xml.push_str(&"<x>".repeat(depth));
            xml.push_str(&"</x>".repeat(depth));
            xml.push_str("</ListBucketResult>");
            xml
        };
        assert!(refused(&nested(1_000_000)).contains("deeper than 32"));
        assert!(refused(&nested(32)).contains("deeper than 32"));
        // The root is one level; 31 more is the deepest accepted.
        assert!(parse_list(&nested(31)).unwrap().objects.is_empty());
    }

    /// A page that never says whether it is the last one must not be taken as
    /// the last one: that would save part of a bucket as all of it.
    #[test]
    fn a_page_without_is_truncated_is_refused() {
        let xml = "<ListBucketResult><Contents><Key>k</Key><Size>1</Size></Contents>\
             <NextContinuationToken>more</NextContinuationToken></ListBucketResult>";
        assert!(refused(xml).contains("IsTruncated"));
        assert!(refused("<ListBucketResult/>").contains("IsTruncated"));
    }

    #[test]
    fn error_bodies_name_the_region_and_endpoint_when_they_carry_them() {
        let error = parse_error(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Error><Code>AuthorizationHeaderMalformed</Code>\
             <Message>The authorization header is malformed; the region 'us-east-1' is wrong; \
             expecting 'eu-west-1'</Message><Region>eu-west-1</Region><RequestId>X</RequestId></Error>",
        )
        .unwrap();
        assert_eq!(error.code, "AuthorizationHeaderMalformed");
        assert_eq!(error.region.as_deref(), Some("eu-west-1"));

        let redirect = parse_error(
            "<Error><Code>PermanentRedirect</Code><Message>m</Message>\
             <Endpoint>b.s3.eu-west-1.amazonaws.com</Endpoint></Error>",
        )
        .unwrap();
        assert_eq!(
            redirect.endpoint.as_deref(),
            Some("b.s3.eu-west-1.amazonaws.com")
        );

        assert_eq!(parse_error("<html><body>Bad gateway</body></html>"), None);
        assert_eq!(parse_error(""), None);
        assert_eq!(parse_error("<Error><Code>x</Code>"), None, "truncated");
    }

    // -------------------------------------------------------------- STS
    //
    // The AssumeRole answer is MinIO's, captured with curl on 5 October 2026
    // (keys, token and time replaced); the web identity one follows the STS
    // API reference's example, which wraps it in a result element of another
    // name.

    const MINIO_ASSUME_ROLE: &str = include_str!("testdata/minio-assume-role.xml");

    #[test]
    fn an_assume_role_answer_yields_its_keys() {
        let keys = parse_sts_credentials(MINIO_ASSUME_ROLE).unwrap();
        assert_eq!(keys.access_key_id, "EXAMPLEACCESSKEYID00");
        assert_eq!(
            keys.secret_access_key,
            "example/secret+key/0000000000000000000000"
        );
        assert!(keys.session_token.starts_with("eyJhbGciOi"));
        // 2026-10-05T08:45:57Z
        assert_eq!(keys.expiration, 1_791_189_957);

        let web = "<AssumeRoleWithWebIdentityResponse xmlns=\"https://sts.amazonaws.com/doc/2011-06-15/\">\
             <AssumeRoleWithWebIdentityResult><SubjectFromWebIdentityToken>x</SubjectFromWebIdentityToken>\
             <Credentials><SessionToken>tok</SessionToken><SecretAccessKey>sec</SecretAccessKey>\
             <Expiration>2014-10-24T23:00:23Z</Expiration><AccessKeyId>AKID</AccessKeyId></Credentials>\
             </AssumeRoleWithWebIdentityResult></AssumeRoleWithWebIdentityResponse>";
        let keys = parse_sts_credentials(web).unwrap();
        assert_eq!(
            (keys.access_key_id.as_str(), keys.session_token.as_str()),
            ("AKID", "tok")
        );
    }

    /// A broken answer is reported without a word of it: the words are keys.
    #[test]
    fn a_broken_sts_answer_is_refused_without_quoting_it() {
        let cut = &MINIO_ASSUME_ROLE[..MINIO_ASSUME_ROLE.find("</SecretAccessKey>").unwrap()];
        let text = format!("{:#}", parse_sts_credentials(cut).err().unwrap());
        assert!(!text.contains("example/secret"), "{text}");

        let entity = MINIO_ASSUME_ROLE.replace("example/secret", "&secretname;");
        let text = format!("{:#}", parse_sts_credentials(&entity).err().unwrap());
        assert!(!text.contains("secretname"), "{text}");

        let no_token =
            "<AssumeRoleResponse><AssumeRoleResult><Credentials><AccessKeyId>A</AccessKeyId>\
             <SecretAccessKey>S</SecretAccessKey><Expiration>2014-10-24T23:00:23Z</Expiration>\
             </Credentials></AssumeRoleResult></AssumeRoleResponse>";
        let text = format!("{:#}", parse_sts_credentials(no_token).err().unwrap());
        assert!(text.contains("no SessionToken"), "{text}");
    }

    #[test]
    fn an_sts_error_is_read() {
        let error = parse_sts_error(
            "<ErrorResponse xmlns=\"https://sts.amazonaws.com/doc/2011-06-15/\"><Error><Type>Sender</Type>\
             <Code>AccessDenied</Code><Message>not authorized to perform: sts:AssumeRole</Message></Error>\
             <RequestId>r</RequestId></ErrorResponse>",
        )
        .unwrap();
        assert_eq!(error.code, "AccessDenied");
        assert!(error.message.contains("sts:AssumeRole"));
        assert_eq!(parse_sts_error("<Error><Code>x</Code></Error>"), None);
    }

    #[test]
    fn timestamps_parse_as_utc_seconds() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00.999Z"), Some(0));
        assert_eq!(
            parse_rfc3339("2009-10-12T17:50:30.000Z"),
            Some(1_255_369_830)
        );
        assert_eq!(
            parse_rfc3339("2009-10-12T19:50:30+02:00"),
            Some(1_255_369_830)
        );
        assert_eq!(
            parse_rfc3339("2009-10-12T15:50:30-02:00"),
            Some(1_255_369_830)
        );
        assert_eq!(parse_rfc3339("2024-02-29T00:00:00Z"), Some(1_709_164_800));
        for bad in [
            "",
            "2023-02-29T00:00:00Z",
            "2026-13-01T00:00:00Z",
            "2026-01-01T24:00:00Z",
            "2026-01-01T00:00:00",
            "2026-01-01T00:00:00.Z",
            "2026-01-01T00:00:00Zjunk",
            "2026-01-01X00:00:00Z",
            "２０２６-01-01T00:00:00Z",
        ] {
            assert_eq!(parse_rfc3339(bad), None, "{bad:?}");
        }
    }
}
