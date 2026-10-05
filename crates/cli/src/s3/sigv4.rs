//! AWS Signature Version 4, and the HMAC-SHA256 it is built from.
//!
//! Written here rather than taken from a crate: the whole of it is a canonical
//! string, four nested HMACs and a hex digest, and `sha2` is already in the
//! workspace for `spacetrace update`. The AWS SDK would pull in an async
//! runtime and a few hundred thousand lines to sign one kind of GET.
//!
//! What keeps a hand-written signer honest is the test vectors below, copied
//! verbatim from AWS' own suite and from the S3 documentation. A signer that
//! is wrong in one byte is wrong for every request, so passing them is close
//! to all-or-nothing evidence.

use sha2::{Digest, Sha256};

/// `SHA-256("")`, the payload hash of every request without a body.
pub const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// An access key, its secret, and the session token temporary keys come with.
///
/// `Debug` is written by hand so that formatting one — in a log line, a panic,
/// an `{:?}` in an error — can never print the secret or the token.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("access_key_id", &"<redacted>")
            .field("secret_access_key", &"<redacted>")
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// One request as the signer sees it.
///
/// `path` is the decoded path — `/bucket/key with space` — and is encoded
/// here, once. S3 is the one service that does not double-encode the path, and
/// the S3 examples below are what pin that down.
pub struct Request<'a> {
    pub method: &'a str,
    pub path: &'a str,
    /// Decoded names and values; encoded and sorted here.
    pub query: &'a [(String, String)],
    /// Every header to sign, `host` and `x-amz-date` included. Names in any case.
    pub headers: &'a [(String, String)],
    pub payload_sha256: &'a str,
}

/// Where and when a signature is valid: `20150830/us-east-1/s3/aws4_request`.
pub struct Scope<'a> {
    /// `YYYYMMDDTHHMMSSZ`, the same value as the `x-amz-date` header.
    pub amz_date: &'a str,
    pub region: &'a str,
    pub service: &'a str,
}

impl Scope<'_> {
    fn date(&self) -> &str {
        // Validated by the caller building the stamp; `get` keeps a malformed
        // one from panicking here.
        self.amz_date.get(..8).unwrap_or(self.amz_date)
    }

    fn credential_scope(&self) -> String {
        format!(
            "{}/{}/{}/aws4_request",
            self.date(),
            self.region,
            self.service
        )
    }
}

/// The `Authorization` header value for `request`.
pub fn authorization(
    credentials: &Credentials,
    scope: &Scope<'_>,
    request: &Request<'_>,
) -> String {
    let (signed_headers, canonical) = canonical_request(request);
    let to_sign = string_to_sign(scope, &canonical);
    let signature = signature(&credentials.secret_access_key, scope, &to_sign);
    format!(
        "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={signed_headers}, Signature={signature}",
        credentials.access_key_id,
        scope.credential_scope(),
    )
}

/// The canonical request, and the signed-headers list that goes with it.
pub fn canonical_request(request: &Request<'_>) -> (String, String) {
    let mut query: Vec<(String, String)> = request
        .query
        .iter()
        .map(|(k, v)| (uri_encode(k, true), uri_encode(v, true)))
        .collect();
    // Sorted after encoding, by name and then by value: the suite's
    // `get-vanilla-query-order-*` cases are exactly this.
    query.sort();
    let query = query
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");

    let mut headers: Vec<(String, String)> = request
        .headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), collapse_spaces(value)))
        .collect();
    headers.sort();
    let signed: Vec<&str> = headers.iter().map(|(name, _)| name.as_str()).collect();
    let signed = signed.join(";");
    let canonical_headers: String = headers
        .iter()
        .map(|(name, value)| format!("{name}:{value}\n"))
        .collect();

    let canonical = format!(
        "{}\n{}\n{query}\n{canonical_headers}\n{signed}\n{}",
        request.method,
        uri_encode(request.path, false),
        request.payload_sha256,
    );
    (signed, canonical)
}

pub fn string_to_sign(scope: &Scope<'_>, canonical_request: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256\n{}\n{}\n{}",
        scope.amz_date,
        scope.credential_scope(),
        sha256_hex(canonical_request.as_bytes())
    )
}

pub fn signature(secret: &str, scope: &Scope<'_>, string_to_sign: &str) -> String {
    let k_date = hmac_sha256(format!("AWS4{secret}").as_bytes(), scope.date().as_bytes());
    let k_region = hmac_sha256(&k_date, scope.region.as_bytes());
    let k_service = hmac_sha256(&k_region, scope.service.as_bytes());
    let k_signing = hmac_sha256(&k_service, b"aws4_request");
    hex(&hmac_sha256(&k_signing, string_to_sign.as_bytes()))
}

/// RFC 2104 HMAC over SHA-256, block size 64.
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut block = [0u8; BLOCK];
    if key.len() > BLOCK {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block.map(|b| b ^ 0x36));
    inner.update(message);
    let mut outer = Sha256::new();
    outer.update(block.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// The headers to send with `request`, `Authorization` last: its own, plus
/// `x-amz-date` and the session token when the keys carry one, all signed.
/// `host` is signed and left out of the result, because the HTTP client
/// writes it from the same URL parts it was signed from.
pub fn signed_headers(
    credentials: &Credentials,
    scope: &Scope<'_>,
    request: &Request<'_>,
) -> Vec<(String, String)> {
    let mut headers = request.headers.to_vec();
    headers.push(("x-amz-date".to_string(), scope.amz_date.to_string()));
    if let Some(token) = &credentials.session_token {
        headers.push(("x-amz-security-token".to_string(), token.clone()));
    }
    let authorization = authorization(
        credentials,
        scope,
        &Request {
            headers: &headers,
            ..*request
        },
    );
    headers.retain(|(name, _)| !name.eq_ignore_ascii_case("host"));
    headers.push(("authorization".to_string(), authorization));
    headers
}

/// SHA-1, FIPS 180-4. Not for anything secret: botocore names the files in
/// `~/.aws/sso/cache` by the SHA-1 of the session name or start URL, and this
/// is how those files are found. `sha2` has no SHA-1 and the `sha1` crate
/// would be a new dependency for some forty lines.
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for block in padded.chunks_exact(64) {
        let mut w = [0u32; 80];
        for (i, word) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, &word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (slot, v) in h.iter_mut().zip([a, b, c, d, e]) {
            *slot = slot.wrapping_add(v);
        }
    }

    let mut out = [0u8; 20];
    for (chunk, v) in out.chunks_exact_mut(4).zip(h) {
        chunk.copy_from_slice(&v.to_be_bytes());
    }
    out
}

pub fn sha1_hex(data: &[u8]) -> String {
    hex(&sha1(data))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// RFC 3986 percent-encoding as SigV4 defines it: every byte outside
/// `A-Z a-z 0-9 - _ . ~` becomes `%XX`, upper case. `/` is kept in a path and
/// encoded everywhere else.
pub fn uri_encode(s: &str, encode_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        let unreserved = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~');
        if unreserved || (b == b'/' && !encode_slash) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Trim, and fold runs of spaces into one — the suite's `get-header-value-trim`
/// shows this applies inside quotes too.
fn collapse_spaces(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for word in value.split(' ').filter(|w| !w.is_empty()) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// `YYYYMMDDTHHMMSSZ` for a Unix time, in UTC.
pub fn amz_date(unix: i64) -> String {
    let (y, m, d, hh, mm, ss) = crate::fmt::civil(unix);
    format!("{y:04}{m:02}{d:02}T{hh:02}{mm:02}{ss:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// RFC 4231 §4.2, §4.3 and §4.7: the short key, the ASCII key, and the key
    /// longer than a block that has to be hashed first — the one branch a
    /// hand-written HMAC most often gets wrong.
    /// <https://www.rfc-editor.org/rfc/rfc4231.txt>
    #[test]
    fn hmac_matches_rfc_4231() {
        assert_eq!(
            hex(&hmac_sha256(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(
            hex(&hmac_sha256(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
        assert_eq!(unhex("0b0c"), vec![0x0b, 0x0c], "the test helper itself");
    }

    // ------------------------------------------------ the AWS SigV4 suite
    //
    // AWS' "Signature Version 4 test suite", as vendored by botocore under
    // tests/unit/auth/aws4_testsuite/<case>/<case>.{req,creq,authz}:
    // https://github.com/boto/botocore/tree/develop/tests/unit/auth/aws4_testsuite
    // (the page AWS once hosted it on, docs.aws.amazon.com/general/latest/gr/
    // signature-v4-test-suite.html, is gone). Credentials, date, region and
    // service are the ones that suite fixes for every case.

    const SUITE_SECRET: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";
    const SUITE_DATE: &str = "20150830T123600Z";

    fn suite_authz(
        method: &str,
        path: &str,
        query: &[(&str, &str)],
        extra: &[(&str, &str)],
    ) -> String {
        let query: Vec<(String, String)> = query
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let mut headers = vec![
            ("Host".to_string(), "example.amazonaws.com".to_string()),
            ("X-Amz-Date".to_string(), SUITE_DATE.to_string()),
        ];
        headers.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        let credentials = Credentials {
            access_key_id: "AKIDEXAMPLE".into(),
            secret_access_key: SUITE_SECRET.into(),
            session_token: None,
        };
        let scope = Scope {
            amz_date: SUITE_DATE,
            region: "us-east-1",
            service: "service",
        };
        authorization(
            &credentials,
            &scope,
            &Request {
                method,
                path,
                query: &query,
                headers: &headers,
                payload_sha256: EMPTY_SHA256,
            },
        )
    }

    fn suite_expected(signed: &str, signature: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
             SignedHeaders={signed}, Signature={signature}"
        )
    }

    #[test]
    fn suite_get_vanilla() {
        assert_eq!(
            suite_authz("GET", "/", &[], &[]),
            suite_expected(
                "host;x-amz-date",
                "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
            )
        );
    }

    /// `GET /?Param1=value2&Param1=Value1`: one name twice, sorted by value,
    /// and upper case sorts before lower.
    #[test]
    fn suite_get_vanilla_query_order_key() {
        assert_eq!(
            suite_authz(
                "GET",
                "/",
                &[("Param1", "value2"), ("Param1", "Value1")],
                &[]
            ),
            suite_expected(
                "host;x-amz-date",
                "eedbc4e291e521cf13422ffca22be7d2eb8146eecf653089df300a15b2382bd1"
            )
        );
    }

    #[test]
    fn suite_get_vanilla_query_order_key_case() {
        assert_eq!(
            suite_authz(
                "GET",
                "/",
                &[("Param2", "value2"), ("Param1", "value1")],
                &[]
            ),
            suite_expected(
                "host;x-amz-date",
                "b97d918cfa904a5beff61c982a1b6f458b799221646efd99d3219ec94cdf2500"
            )
        );
    }

    #[test]
    fn suite_get_vanilla_query_order_value() {
        assert_eq!(
            suite_authz(
                "GET",
                "/",
                &[("Param1", "value2"), ("Param1", "value1")],
                &[]
            ),
            suite_expected(
                "host;x-amz-date",
                "5772eed61e12b33fae39ee5e7012498b51d56abc0abb7c60486157bd471c4694"
            )
        );
    }

    #[test]
    fn suite_get_unreserved() {
        let unreserved = "-._~0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
        assert_eq!(
            suite_authz("GET", &format!("/{unreserved}"), &[], &[]),
            suite_expected(
                "host;x-amz-date",
                "07ef7494c76fa4850883e2b006601f940f8a34d404d0cfa977f52a65bbf5f24f"
            )
        );
        assert_eq!(
            suite_authz("GET", "/", &[(unreserved, unreserved)], &[]),
            suite_expected(
                "host;x-amz-date",
                "9c3e54bfcdf0b19771a7f523ee5669cdf59bc7cc0884027167c21bb143a40197"
            ),
            "get-vanilla-query-unreserved"
        );
    }

    /// `GET /ሴ`: a multi-byte character, encoded byte by byte, upper-case hex.
    #[test]
    fn suite_get_utf8() {
        assert_eq!(
            suite_authz("GET", "/ሴ", &[], &[]),
            suite_expected(
                "host;x-amz-date",
                "8318018e0b0f223aa2bbf98705b62bb787dc9c0e678f255a891fd03141be5d85"
            )
        );
        assert_eq!(
            suite_authz("GET", "/", &[("ሴ", "bar")], &[]),
            suite_expected(
                "host;x-amz-date",
                "2cdec8eed098649ff3a119c94853b13c643bcf08f8b0a1d91e12c9027818dd04"
            ),
            "get-vanilla-utf8-query"
        );
    }

    #[test]
    fn suite_get_vanilla_empty_query_key() {
        assert_eq!(
            suite_authz("GET", "/", &[("Param1", "value1")], &[]),
            suite_expected(
                "host;x-amz-date",
                "a67d582fa61cc504c4bae71f336f98b97f1ea3c7a6bfe1b6e45aec72011b9aeb"
            )
        );
    }

    #[test]
    fn suite_post_vanilla() {
        assert_eq!(
            suite_authz("POST", "/", &[], &[]),
            suite_expected(
                "host;x-amz-date",
                "5da7c1a2acd57cee7505fc6676e4e544621c30862966e37dddb68e92efbe5d6b"
            )
        );
    }

    #[test]
    fn suite_get_header_value_trim() {
        assert_eq!(
            suite_authz(
                "GET",
                "/",
                &[],
                &[("My-Header1", " value1"), ("My-Header2", " \"a   b   c\"")]
            ),
            suite_expected(
                "host;my-header1;my-header2;x-amz-date",
                "acc3ed3afb60bb290fc8d2dd0098b9911fcaa05412b367055dee359757a9c736"
            )
        );
    }

    // ------------------------------------------- the S3 documentation
    //
    // "Signature Calculations for the Authorization Header: Transferring
    // Payload in a Single Chunk", examples GET Object, PUT Object, GET Bucket
    // Lifecycle and Get Bucket (List Objects):
    // https://docs.aws.amazon.com/AmazonS3/latest/API/sig-v4-header-based-auth.html
    // That URL now redirects to the API index; the values were copied from
    // the archived page,
    // https://web.archive.org/web/20250102090230/https://docs.aws.amazon.com/AmazonS3/latest/API/sig-v4-header-based-auth.html
    // These are the ones that prove the S3-specific details: the
    // `x-amz-content-sha256` header, a path encoded once (`$` → `%24`), and a
    // query parameter with no value (`lifecycle=`).

    const DOC_DATE: &str = "20130524T000000Z";

    fn doc_signature(
        method: &str,
        path: &str,
        query: &[(&str, &str)],
        extra: &[(&str, &str)],
        payload: &str,
    ) -> (String, String) {
        let query: Vec<(String, String)> = query
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let mut headers = vec![
            (
                "Host".to_string(),
                "examplebucket.s3.amazonaws.com".to_string(),
            ),
            ("x-amz-date".to_string(), DOC_DATE.to_string()),
            ("x-amz-content-sha256".to_string(), payload.to_string()),
        ];
        headers.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        let request = Request {
            method,
            path,
            query: &query,
            headers: &headers,
            payload_sha256: payload,
        };
        let scope = Scope {
            amz_date: DOC_DATE,
            region: "us-east-1",
            service: "s3",
        };
        let (_, canonical) = canonical_request(&request);
        let to_sign = string_to_sign(&scope, &canonical);
        (
            sha256_hex(canonical.as_bytes()),
            signature("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY", &scope, &to_sign),
        )
    }

    #[test]
    fn s3_doc_get_object() {
        let (creq_hash, sig) = doc_signature(
            "GET",
            "/test.txt",
            &[],
            &[("Range", "bytes=0-9")],
            EMPTY_SHA256,
        );
        // The hash of the canonical request is the last line of StringToSign
        // in the document; checking it separately says which half is wrong.
        assert_eq!(
            creq_hash,
            "7344ae5b7ee6c3e7e6b0fe0640412a37625d1fbfff95c48bbb2dc43964946972"
        );
        assert_eq!(
            sig,
            "f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    #[test]
    fn s3_doc_put_object() {
        let payload = sha256_hex(b"Welcome to Amazon S3.");
        assert_eq!(
            payload,
            "44ce7dd67c959e0d3524ffac1771dfbba87d2b6b4b4e99e42034a8b803f8b072"
        );
        let (creq_hash, sig) = doc_signature(
            "PUT",
            "/test$file.text",
            &[],
            &[
                ("Date", "Fri, 24 May 2013 00:00:00 GMT"),
                ("x-amz-storage-class", "REDUCED_REDUNDANCY"),
            ],
            &payload,
        );
        assert_eq!(
            creq_hash,
            "9e0e90d9c76de8fa5b200d8c849cd5b8dc7a3be3951ddb7f6a76b4158342019d"
        );
        assert_eq!(
            sig,
            "98ad721746da40c64f1a55b78f14c238d841ea1380cd77a1b5971af0ece108bd"
        );
    }

    #[test]
    fn s3_doc_get_bucket_lifecycle() {
        let (creq_hash, sig) = doc_signature("GET", "/", &[("lifecycle", "")], &[], EMPTY_SHA256);
        assert_eq!(
            creq_hash,
            "9766c798316ff2757b517bc739a67f6213b4ab36dd5da2f94eaebf79c77395ca"
        );
        assert_eq!(
            sig,
            "fea454ca298b7da1c68078a5d1bdbfbbe0d65c699e0f91ac7a200a0136783543"
        );
    }

    #[test]
    fn s3_doc_list_objects() {
        let (creq_hash, sig) = doc_signature(
            "GET",
            "/",
            &[("max-keys", "2"), ("prefix", "J")],
            &[],
            EMPTY_SHA256,
        );
        assert_eq!(
            creq_hash,
            "df57d21db20da04d7fa30298dd4488ba3a2b47ca3a489c74750e0f1e7df1b9b7"
        );
        assert_eq!(
            sig,
            "34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7"
        );
    }

    #[test]
    fn the_whole_authorization_header_has_the_documented_shape() {
        let credentials = Credentials {
            access_key_id: "AKIAIOSFODNN7EXAMPLE".into(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into(),
            session_token: None,
        };
        let headers = vec![
            (
                "host".to_string(),
                "examplebucket.s3.amazonaws.com".to_string(),
            ),
            ("x-amz-date".to_string(), DOC_DATE.to_string()),
            ("x-amz-content-sha256".to_string(), EMPTY_SHA256.to_string()),
        ];
        let query = vec![
            ("max-keys".to_string(), "2".to_string()),
            ("prefix".to_string(), "J".to_string()),
        ];
        let header = authorization(
            &credentials,
            &Scope {
                amz_date: DOC_DATE,
                region: "us-east-1",
                service: "s3",
            },
            &Request {
                method: "GET",
                path: "/",
                query: &query,
                headers: &headers,
                payload_sha256: EMPTY_SHA256,
            },
        );
        // The document writes it without spaces after the commas; both are
        // accepted, and the suite's form (with spaces) is what this emits.
        assert_eq!(
            header,
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, \
             SignedHeaders=host;x-amz-content-sha256;x-amz-date, \
             Signature=34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7"
        );
    }

    #[test]
    fn credentials_never_format_their_secrets() {
        let credentials = Credentials {
            access_key_id: "AKIAVISIBLE".into(),
            secret_access_key: "super-secret-value".into(),
            session_token: Some("session-token-value".into()),
        };
        let shown = format!("{credentials:?}");
        assert!(!shown.contains("super-secret-value"), "{shown}");
        assert!(!shown.contains("session-token-value"), "{shown}");
        assert!(!shown.contains("AKIAVISIBLE"), "{shown}");
    }

    /// FIPS 180-2 Appendix A (one block, two blocks, a million `a`), the
    /// empty string, and the lengths around the padding boundary, where a
    /// message of 55 bytes fits its length in one block and 56 does not.
    /// <https://csrc.nist.gov/CSRC/media/Projects/Cryptographic-Standards-and-Guidelines/documents/examples/SHA1.pdf>
    #[test]
    fn sha1_matches_the_published_vectors() {
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            sha1_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
        assert_eq!(
            sha1_hex(&vec![b'a'; 1_000_000]),
            "34aa973cd4c4daa4f61eeb2bdbad27316534016f"
        );
        assert_eq!(sha1_hex(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            sha1_hex(b"The quick brown fox jumps over the lazy dog"),
            "2fd4e1c67a2d28fced849ee1bb76e7391b93eb12"
        );
        assert_eq!(
            sha1_hex(&[b'a'; 55]),
            "c1c8bbdc22796e28c0e15163d20899b65621d65a"
        );
        assert_eq!(
            sha1_hex(&[b'a'; 56]),
            "c2db330f6083854c99d4b5bfb6e8f29f201be699"
        );
        assert_eq!(
            sha1_hex(&[b'a'; 64]),
            "0098ba824b5c16427bd7a1122a5a442a25ec644d"
        );
    }

    /// What the session token changes: one more signed header, the same
    /// shape otherwise, and `host` left for the HTTP client to write.
    #[test]
    fn signed_headers_carry_the_token_and_sign_it() {
        let scope = Scope {
            amz_date: DOC_DATE,
            region: "us-east-1",
            service: "sts",
        };
        let mut credentials = Credentials {
            access_key_id: "AKID".into(),
            secret_access_key: "secret".into(),
            session_token: None,
        };
        let names = |headers: &[(String, String)]| -> Vec<String> {
            headers.iter().map(|(n, _)| n.clone()).collect()
        };
        let request = |headers| Request {
            method: "POST",
            path: "/",
            query: &[],
            headers,
            payload_sha256: EMPTY_SHA256,
        };
        let host = [(
            "host".to_string(),
            "sts.us-east-1.amazonaws.com".to_string(),
        )];
        let plain = signed_headers(&credentials, &scope, &request(&host));
        assert_eq!(names(&plain), ["x-amz-date", "authorization"]);
        assert!(plain[1].1.contains("SignedHeaders=host;x-amz-date,"));

        credentials.session_token = Some("the-token".into());
        let host_and_type = [
            host[0].clone(),
            ("content-type".to_string(), "x".to_string()),
        ];
        let with_token = signed_headers(&credentials, &scope, &request(&host_and_type));
        assert_eq!(
            names(&with_token),
            [
                "content-type",
                "x-amz-date",
                "x-amz-security-token",
                "authorization"
            ]
        );
        assert_eq!(with_token[2].1, "the-token");
        assert!(with_token[3]
            .1
            .contains("SignedHeaders=content-type;host;x-amz-date;x-amz-security-token,"));
    }

    #[test]
    fn amz_dates_are_utc_stamps() {
        // 2015-08-30T12:36:00Z, the suite's date.
        assert_eq!(amz_date(1_440_938_160), SUITE_DATE);
        assert_eq!(amz_date(0), "19700101T000000Z");
    }

    #[test]
    fn encoding_keeps_only_the_unreserved_set() {
        assert_eq!(uri_encode("a b+c/d%e", true), "a%20b%2Bc%2Fd%25e");
        assert_eq!(uri_encode("a b/c", false), "a%20b/c");
        assert_eq!(uri_encode("ü", true), "%C3%BC");
    }
}
