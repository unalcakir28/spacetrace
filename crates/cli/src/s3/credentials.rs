//! Keys that are not written down: fetched from a process, STS, the SSO
//! portal or a metadata service, and fetched again before they expire.
//!
//! `config.rs` decides *which* source applies, from the files and the
//! environment alone, in botocore's order; nothing there touches the network.
//! This module does the fetching, and `Keys` holds the result and renews it.
//!
//! **Renewal follows botocore's two windows**: an advisory refresh 15 minutes
//! before expiry, whose failure is reported and outlived, and a mandatory one
//! 10 minutes before, whose failure ends the listing. Both shrink for keys
//! that live less than an hour — to half and a quarter of their lifetime — so
//! fifteen-minute keys are not fetched again for every page, which is what the
//! fixed windows do to them in botocore.
//!
//! **Nothing secret is printed.** Errors name the source, the status and the
//! server's message, escaped; never a body that carries keys, and never a
//! token. The JSON documents are read as values and their fields picked by
//! hand, because a typed `serde` error quotes the value it choked on.

use std::path::PathBuf;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use spacetrace_scan_core::ScanProgress;

use super::client::{backoff, check_cancelled, http_client, read_capped, BodyError};
use super::config::Endpoint;
use super::lock;
use super::sigv4::{self, Credentials, Scope};
use super::xml;
use crate::fmt::{for_terminal, unix_now, unix_now_ms};

/// Where signing keys come from. Decided from configuration alone; `fetch` is
/// the first thing that touches a process or the network.
#[derive(Debug, Clone)]
pub enum Provider {
    /// Keys from the environment or a profile, which never expire.
    Static(Credentials),
    Process(Process),
    AssumeRole(Box<AssumeRole>),
    WebIdentity(WebIdentity),
    Sso(Sso),
    Container(Container),
    InstanceMetadata(InstanceMetadata),
}

impl From<Credentials> for Provider {
    fn from(credentials: Credentials) -> Self {
        Provider::Static(credentials)
    }
}

/// `credential_process`: a command that prints keys as JSON.
#[derive(Debug, Clone)]
pub struct Process {
    pub profile: String,
    pub command: String,
}

/// The STS endpoint a role is assumed at, and the region it signs for.
#[derive(Debug, Clone)]
pub struct Sts {
    pub endpoint: Endpoint,
    pub region: String,
}

/// `role_arn` with `source_profile` or `credential_source`.
#[derive(Debug, Clone)]
pub struct AssumeRole {
    pub profile: String,
    /// What signs the AssumeRole call: another profile's keys, possibly a
    /// role of their own, or the environment or a metadata service.
    pub source: Provider,
    pub role_arn: String,
    pub session_name: Option<String>,
    pub external_id: Option<String>,
    pub duration_seconds: Option<u32>,
    pub sts: Sts,
}

/// `AssumeRoleWithWebIdentity`: an OIDC token from a file, unsigned.
#[derive(Debug, Clone)]
pub struct WebIdentity {
    /// Read again on every fetch: the orchestrator rotates the file.
    pub token_file: PathBuf,
    pub role_arn: String,
    pub session_name: Option<String>,
    pub sts: Sts,
}

/// IAM Identity Center: the token `aws sso login` cached, traded at the
/// portal for one account's role credentials.
#[derive(Debug, Clone)]
pub struct Sso {
    pub profile: String,
    pub cache_file: PathBuf,
    pub account_id: String,
    pub role_name: String,
    /// `scheme://authority` of the portal.
    pub portal: Endpoint,
}

/// ECS, EKS Pod Identity, or anything else that serves the container
/// credentials protocol.
#[derive(Debug, Clone)]
pub struct Container {
    pub url: String,
    pub token: Option<ContainerToken>,
}

#[derive(Clone)]
pub enum ContainerToken {
    /// Read on every fetch: EKS rotates it.
    File(PathBuf),
    Value(String),
}

impl std::fmt::Debug for ContainerToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContainerToken::File(path) => f.debug_tuple("File").field(path).finish(),
            ContainerToken::Value(_) => f.write_str("Value(<redacted>)"),
        }
    }
}

/// EC2 instance metadata, IMDSv2 first.
#[derive(Debug, Clone)]
pub struct InstanceMetadata {
    /// `http://169.254.169.254` unless configured, without a trailing `/`.
    pub endpoint: String,
    pub timeout: Duration,
    pub attempts: u32,
    pub v1_disabled: bool,
    /// Set when this is the end of the chain rather than something a profile
    /// asked for: the "no credentials" message to give if nothing answers,
    /// since on a machine that is not an EC2 instance that is the real news.
    pub last_resort: Option<String>,
}

/// Keys as fetched, with the moment they stop working.
pub struct Fetched {
    pub credentials: Credentials,
    /// Unix seconds; `None` for keys that do not expire.
    pub expires: Option<i64>,
}

impl Provider {
    /// `progress` cancels the pauses between retries.
    pub fn fetch(&self, progress: &ScanProgress) -> Result<Fetched> {
        match self {
            Provider::Static(credentials) => Ok(Fetched {
                credentials: credentials.clone(),
                expires: None,
            }),
            Provider::Process(p) => run_process(p),
            Provider::AssumeRole(role) => assume_role(role, progress),
            Provider::WebIdentity(web) => assume_role_with_web_identity(web, progress),
            Provider::Sso(sso) => sso_role_credentials(sso),
            Provider::Container(c) => container_credentials(c, progress),
            Provider::InstanceMetadata(m) => instance_metadata(m, progress),
        }
    }

    /// What this source is, for an error message. Names and paths only.
    pub fn describe(&self) -> String {
        match self {
            Provider::Static(_) => "static keys".to_string(),
            Provider::Process(p) => format!("the credential_process of profile {:?}", p.profile),
            Provider::AssumeRole(role) => format!(
                "role {} (profile {:?}, through {})",
                role.role_arn, role.profile, role.sts.endpoint.authority
            ),
            Provider::WebIdentity(web) => format!(
                "role {} with the web identity token in {}",
                web.role_arn,
                web.token_file.display()
            ),
            Provider::Sso(sso) => format!(
                "single sign-on for profile {:?} (role {} in account {})",
                sso.profile, sso.role_name, sso.account_id
            ),
            Provider::Container(c) => format!("container credentials at {}", where_only(&c.url)),
            Provider::InstanceMetadata(m) => {
                format!("instance metadata at {}", where_only(&m.endpoint))
            }
        }
    }
}

/// A configured URL without its userinfo, query and fragment, which are
/// where a password or a token would sit.
fn where_only(url: &str) -> String {
    let Ok(mut url) = reqwest::Url::parse(url) else {
        return "an address that is not a URL".to_string();
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.as_str().trim_end_matches('/').to_string()
}

// ------------------------------------------------------------- renewal

const ADVISORY: i64 = 15 * 60 * 1000;
const MANDATORY: i64 = 10 * 60 * 1000;

/// The keys a listing signs with, renewed before they expire. Shared by
/// every request of a listing, from any thread.
///
/// One thread fetches at a time, and never under the lock: the others either
/// sign with the keys they have or, when there are none they may use, wait
/// on `fetched` — 50 ms at a time, so a cancel reaches them while a
/// `credential_process` is still waiting for an MFA code.
pub struct Keys {
    provider: Provider,
    state: Mutex<State>,
    fetched: Condvar,
}

#[derive(Default)]
struct State {
    held: Option<Held>,
    /// A thread is fetching, without the lock.
    fetching: bool,
    /// Fetches finished so far, so a waiter can tell that the one it waited
    /// for is over.
    finished: u64,
    /// Why the last fetch failed, for everyone who was waiting for it.
    failed: Option<String>,
}

struct Held {
    credentials: Credentials,
    /// `None` for keys that never expire.
    window: Option<Window>,
}

/// Unix milliseconds. Past `refresh_at` a renewal is tried and may fail; past
/// `renew_by` it has to succeed.
#[derive(Clone, Copy)]
struct Window {
    refresh_at: i64,
    renew_by: i64,
}

impl Keys {
    pub fn new(provider: Provider) -> Keys {
        Keys {
            provider,
            state: Mutex::new(State::default()),
            fetched: Condvar::new(),
        }
    }

    /// The keys to sign the next request with.
    ///
    /// In the advisory window one thread renews and every other one signs
    /// with the current keys, which are good for minutes yet: a renewal can
    /// take STS three tries of 30 s, or a `credential_process` waiting for an
    /// MFA code, and the listing need not stop for it. Only past the
    /// mandatory deadline, or with no keys at all, does everyone wait — for
    /// one fetch, whose failure they all get rather than each trying again.
    pub fn get(&self, progress: &ScanProgress) -> Result<Credentials> {
        let mut state = lock(&self.state);
        let mut waited_for = None;
        loop {
            check_cancelled(progress)?;
            let now = unix_now_ms();
            if let Some(held) = &state.held {
                let window = held.window;
                if window.is_none_or(|w| now < w.refresh_at) {
                    return Ok(held.credentials.clone());
                }
                if window.is_some_and(|w| now < w.renew_by) {
                    let current = held.credentials.clone();
                    if state.fetching {
                        return Ok(current);
                    }
                    state.fetching = true;
                    drop(state);
                    return self.renew_advisory(current, progress);
                }
            }
            if let (Some(seen), Some(why)) = (waited_for, &state.failed) {
                if state.finished != seen {
                    bail!("{why}");
                }
            }
            if !state.fetching {
                state.fetching = true;
                drop(state);
                return self.finish(self.fetch(progress));
            }
            waited_for.get_or_insert(state.finished);
            state = self
                .fetched
                .wait_timeout(state, Duration::from_millis(50))
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
    }

    /// Renew in the advisory window. A failure is reported once and then
    /// left until the renewal has to work.
    fn renew_advisory(&self, current: Credentials, progress: &ScanProgress) -> Result<Credentials> {
        let error = match self.finish(self.fetch(progress)) {
            Ok(fresh) => return Ok(fresh),
            Err(e) if is_interrupted(&e) => return Err(e),
            Err(e) => e,
        };
        eprintln!(
            "note: renewing the S3 credentials failed, and the current ones are still valid; \
             trying again before they expire: {error:#}"
        );
        let mut state = lock(&self.state);
        if let Some(Held {
            credentials,
            window: Some(window),
        }) = state.held.as_mut()
        {
            if *credentials == current {
                window.refresh_at = window.renew_by;
            }
        }
        Ok(current)
    }

    /// Record how a fetch ended and wake whoever waited for it.
    fn finish(&self, fetched: Result<Held>) -> Result<Credentials> {
        let mut state = lock(&self.state);
        state.fetching = false;
        state.finished += 1;
        self.fetched.notify_all();
        match fetched {
            Ok(fresh) => {
                let credentials = fresh.credentials.clone();
                state.held = Some(fresh);
                state.failed = None;
                Ok(credentials)
            }
            Err(e) => {
                state.failed = Some(format!("{e:#}"));
                Err(e)
            }
        }
    }

    /// The service said `used` has expired, whatever this clock thinks.
    /// Forget them, so the next `get` fetches anew, and say whether that can
    /// help: static keys come back the same.
    pub fn expire(&self, used: &Credentials) -> bool {
        if matches!(self.provider, Provider::Static(_)) {
            return false;
        }
        let mut state = lock(&self.state);
        // Another request may have renewed them already.
        if state.held.as_ref().is_some_and(|h| h.credentials == *used) {
            state.held = None;
        }
        true
    }

    /// Bring the renewal deadlines `ms` closer, as if that much time had
    /// passed, so a test can cross them without sleeping through them.
    #[cfg(test)]
    fn age_by(&self, ms: i64) {
        let mut state = lock(&self.state);
        if let Some(window) = state.held.as_mut().and_then(|h| h.window.as_mut()) {
            window.refresh_at -= ms;
            window.renew_by -= ms;
        }
    }

    #[cfg(test)]
    fn window(&self) -> Option<Window> {
        lock(&self.state).held.as_ref().and_then(|h| h.window)
    }

    fn fetch(&self, progress: &ScanProgress) -> Result<Held> {
        let what = || {
            format!(
                "cannot get S3 credentials from {}",
                self.provider.describe()
            )
        };
        let fetched = self.provider.fetch(progress).with_context(what)?;
        let Some(expires) = fetched.expires else {
            return Ok(Held {
                credentials: fetched.credentials,
                window: None,
            });
        };
        // The lifetime counts from now, when the keys arrived: a fetch that
        // took a minute leaves them a minute less.
        let expires_ms = expires.saturating_mul(1000);
        let lifetime = expires_ms - unix_now_ms();
        if lifetime <= 0 {
            return Err(anyhow::anyhow!(
                "the credentials it returned expired at {} (this machine's clock says it is later)",
                crate::fmt::timestamp(expires)
            ))
            .with_context(what);
        }
        Ok(Held {
            credentials: fetched.credentials,
            window: Some(Window {
                refresh_at: expires_ms - ADVISORY.min(lifetime / 2),
                renew_by: expires_ms - MANDATORY.min(lifetime / 4),
            }),
        })
    }
}

fn is_interrupted(e: &anyhow::Error) -> bool {
    e.downcast_ref::<std::io::Error>()
        .is_some_and(|e| e.kind() == std::io::ErrorKind::Interrupted)
}

// ----------------------------------------------------------------- HTTP

/// A credentials answer is a few kilobytes; this is for a server sending
/// something else.
const MAX_ANSWER_BYTES: u64 = 1024 * 1024;

/// STS and the SSO portal: like any AWS call, slow on a bad day.
const SERVICE_TIMEOUT: Duration = Duration::from_secs(30);

/// botocore's container fetcher: 2 s a try, three tries, a second apart
/// (here a second, then two).
const CONTAINER_TIMEOUT: Duration = Duration::from_secs(2);
const CONTAINER_ATTEMPTS: u32 = 3;

/// STS: three tries, 200 ms then 400 ms apart.
const STS_ATTEMPTS: u32 = 3;

struct Answer {
    status: u16,
    body: String,
}

/// Send and read the answer, telling "nothing answered" apart from an answer
/// that cannot be used, because the metadata fallbacks turn on that.
fn send(request: reqwest::blocking::RequestBuilder) -> Result<Answer, Unanswered> {
    let response = request
        .send()
        .map_err(|e| Unanswered::Transport(e.without_url()))?;
    let status = response.status().as_u16();
    let body = read_capped(response, MAX_ANSWER_BYTES).map_err(|e| match e {
        BodyError::Cut(_) => Unanswered::Unusable("the answer was cut off"),
        BodyError::TooLarge => {
            Unanswered::Unusable("the answer is larger than a credentials document")
        }
        BodyError::NotUtf8 => Unanswered::Unusable("the answer is not UTF-8"),
    })?;
    Ok(Answer { status, body })
}

/// `send`, up to `attempts` times, while nothing answers or `retry` says the
/// answer is worth asking again for. The pauses are `client::backoff`'s,
/// starting at `delay`, so a cancel during one is noticed within 50 ms.
fn send_with_retries(
    attempts: u32,
    delay: Duration,
    progress: &ScanProgress,
    build: &dyn Fn() -> reqwest::blocking::RequestBuilder,
    retry: &dyn Fn(&Answer) -> bool,
) -> Result<Answer, Unanswered> {
    let attempts = attempts.max(1);
    let mut attempt = 0;
    loop {
        attempt += 1;
        let outcome = send(build());
        let again = match &outcome {
            Ok(answer) => retry(answer),
            Err(_) => true,
        };
        if !again || attempt >= attempts {
            return outcome;
        }
        backoff(delay, attempt, Some(progress)).map_err(Unanswered::Cancelled)?;
    }
}

/// A request that got no usable answer.
enum Unanswered {
    /// Nothing answered: refused, timed out, reset.
    Transport(reqwest::Error),
    /// Something answered with what cannot be a credentials document.
    Unusable(&'static str),
    /// The listing was cancelled while waiting to ask again.
    Cancelled(anyhow::Error),
}

impl Unanswered {
    /// Nothing listens there, or nothing answered the connection at all.
    fn is_connect(&self) -> bool {
        matches!(self, Unanswered::Transport(e) if e.is_connect())
    }

    fn into_error(self) -> anyhow::Error {
        match self {
            Unanswered::Transport(e) => anyhow::Error::new(e),
            Unanswered::Unusable(why) => anyhow::anyhow!(why),
            Unanswered::Cancelled(e) => e,
        }
    }
}

/// A JSON document's string field, or `None` when it is absent, empty or not
/// a string.
fn field<'a>(value: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    value.get(name)?.as_str().filter(|s| !s.is_empty())
}

/// Keys out of a JSON document with these three field names. The message
/// names the missing field and nothing else.
fn keys_from_json(
    value: &serde_json::Value,
    names: [&str; 3],
    token_required: bool,
) -> Result<Credentials> {
    let [id, secret, token] = names;
    let access_key_id = field(value, id).with_context(|| format!("the answer has no {id}"))?;
    let secret_access_key =
        field(value, secret).with_context(|| format!("the answer has no {secret}"))?;
    let session_token = field(value, token).map(str::to_string);
    if token_required && session_token.is_none() {
        bail!("the answer has no {token}");
    }
    Ok(Credentials {
        access_key_id: access_key_id.to_string(),
        secret_access_key: secret_access_key.to_string(),
        session_token,
    })
}

/// Keys and their optional `Expiration`, out of a JSON document with these
/// three key field names: what `credential_process` prints and what the
/// container and instance metadata services serve.
fn keys_and_expiry(value: &serde_json::Value, names: [&str; 3]) -> Result<Fetched> {
    let credentials = keys_from_json(value, names, false)?;
    let expires = match field(value, "Expiration") {
        Some(text) => Some(expiry(text).context("its Expiration is not a time")?),
        None => None,
    };
    Ok(Fetched {
        credentials,
        expires,
    })
}

/// The `message` of a JSON error body, made safe to print; empty when there
/// is none.
fn error_message(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| field(&v, "message").map(|m| for_terminal(m, 400)))
        .unwrap_or_default()
}

/// An RFC 3339 expiry, and the form the AWS CLI v2 once wrote into its SSO
/// cache, `2020-06-17T10:18:08UTC`.
fn expiry(text: &str) -> Option<i64> {
    match text.strip_suffix("UTC") {
        Some(stem) => xml::parse_rfc3339(&format!("{stem}Z")),
        None => xml::parse_rfc3339(text),
    }
}

fn json(body: &str) -> Result<serde_json::Value> {
    // A syntax error's message is a position, never the text.
    let value: serde_json::Value = serde_json::from_str(body).context("the answer is not JSON")?;
    if !value.is_object() {
        bail!("the answer is not a JSON object");
    }
    Ok(value)
}

// ------------------------------------------------------ credential_process

/// What is kept of a `credential_process`'s stderr for the error message;
/// the rest is read and dropped, so the process never blocks on it.
const MAX_STDERR_BYTES: u64 = 64 * 1024;

/// No timeout: a tool waiting for an MFA code is doing its job. Ctrl-C ends
/// it with the scan, because the terminal sends SIGINT to the whole
/// foreground process group and `scan s3://` installs no handler for it.
fn run_process(p: &Process) -> Result<Fetched> {
    use std::io::Read;
    let argv = split_command(&p.command)?;
    let Some((program, args)) = argv.split_first() else {
        bail!("credential_process is empty");
    };
    // stdin stays the terminal's, so a tool that asks for an MFA code can;
    // stderr is kept for the error message, as botocore keeps it.
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("cannot run {program:?}"))?;
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let stderr = std::thread::spawn(move || {
        let mut kept = Vec::new();
        let _ = (&mut stderr).take(MAX_STDERR_BYTES).read_to_end(&mut kept);
        let _ = std::io::copy(&mut stderr, &mut std::io::sink());
        kept
    });
    // botocore reads all of it; a credentials document is a few hundred
    // bytes, and a process printing without end would be read into memory.
    let mut stdout = Vec::new();
    let read = child
        .stdout
        .take()
        .expect("stdout is piped")
        .take(MAX_ANSWER_BYTES + 1)
        .read_to_end(&mut stdout);
    if stdout.len() as u64 > MAX_ANSWER_BYTES {
        let _ = child.kill();
        let _ = child.wait();
        bail!("{program:?} printed more than 1 MiB, which no credentials document is");
    }
    read.with_context(|| format!("reading what {program:?} printed"))?;
    let status = child
        .wait()
        .with_context(|| format!("waiting for {program:?}"))?;
    let stderr = stderr.join().unwrap_or_default();
    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr);
        bail!(
            "{program:?} exited with {status}: {}",
            for_terminal(stderr.trim(), 400)
        );
    }
    let stdout = std::str::from_utf8(&stdout)
        .map_err(|_| anyhow::anyhow!("{program:?} printed something that is not UTF-8"))?;
    let value = json(stdout).with_context(|| format!("reading what {program:?} printed"))?;
    match value.get("Version").and_then(serde_json::Value::as_u64) {
        Some(1) => {}
        Some(other) => bail!("{program:?} printed Version {other}; only version 1 exists"),
        None => bail!("{program:?} printed no Version"),
    }
    keys_and_expiry(&value, ["AccessKeyId", "SecretAccessKey", "SessionToken"])
        .with_context(|| format!("reading what {program:?} printed"))
}

/// The command line into a program and its arguments, the way botocore splits
/// it: POSIX shell words on Unix, the C runtime's rules on Windows — where a
/// backslash is a path separator and not an escape.
pub fn split_command(command: &str) -> Result<Vec<String>> {
    match cfg!(windows) {
        true => split_windows(command),
        false => split_posix(command),
    }
}

/// `shlex.split`: space, tab, CR and LF separate — not any other whitespace,
/// which shlex leaves in the word — single quotes are literal, double
/// quotes let a backslash escape only `"` and `\`, and outside quotes a
/// backslash escapes whatever follows.
fn split_posix(command: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\r' | '\n' => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => word.push(c),
                        None => bail!("credential_process has an unclosed ' quote"),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\')) => word.push(c),
                            Some(c) => {
                                word.push('\\');
                                word.push(c);
                            }
                            None => bail!("credential_process has an unclosed \" quote"),
                        },
                        Some(c) => word.push(c),
                        None => bail!("credential_process has an unclosed \" quote"),
                    }
                }
            }
            '\\' => {
                in_word = true;
                match chars.next() {
                    Some(c) => word.push(c),
                    None => bail!("credential_process ends in a backslash with nothing to escape"),
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    Ok(words)
}

/// botocore's `_windows_shell_split`, which follows `CommandLineToArgvW`:
/// backslashes are literal unless they run into a `"`, where each pair is one
/// backslash and an odd one out makes the quote literal.
fn split_windows(command: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    // Distinguishes `""` (an empty argument) from nothing at all.
    let mut in_word = false;
    let mut quoted = false;
    let mut backslashes = 0usize;
    for c in command.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                word.extend(std::iter::repeat_n('\\', backslashes / 2));
                let literal = backslashes % 2 == 1;
                backslashes = 0;
                in_word = true;
                match literal {
                    true => word.push('"'),
                    false => quoted = !quoted,
                }
            }
            ' ' | '\t' if !quoted => {
                word.extend(std::iter::repeat_n('\\', backslashes));
                backslashes = 0;
                if in_word || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                in_word = false;
            }
            c => {
                word.extend(std::iter::repeat_n('\\', backslashes));
                backslashes = 0;
                in_word = true;
                word.push(c);
            }
        }
    }
    if quoted {
        bail!("credential_process has an unclosed \" quote");
    }
    word.extend(std::iter::repeat_n('\\', backslashes));
    if in_word || !word.is_empty() {
        words.push(word);
    }
    Ok(words)
}

// ------------------------------------------------------------------ STS

/// One STS call, form-encoded as botocore sends it. `signer` is `None` for
/// `AssumeRoleWithWebIdentity`, which the token authorises by itself.
fn call_sts(
    sts: &Sts,
    params: &[(&str, &str)],
    signer: Option<&Credentials>,
    progress: &ScanProgress,
) -> Result<Fetched> {
    const CONTENT_TYPE: &str = "application/x-www-form-urlencoded; charset=utf-8";
    let body: String = params
        .iter()
        .map(|(k, v)| {
            format!(
                "{}={}",
                sigv4::uri_encode(k, true),
                sigv4::uri_encode(v, true)
            )
        })
        .collect::<Vec<_>>()
        .join("&");
    let url = format!("{}://{}/", sts.endpoint.scheme, sts.endpoint.authority);
    let client = http_client(SERVICE_TIMEOUT, SERVICE_TIMEOUT, false)?;
    let build = || {
        let mut request = client
            .post(&url)
            .header(reqwest::header::CONTENT_TYPE, CONTENT_TYPE)
            .body(body.clone());
        let Some(credentials) = signer else {
            return request;
        };
        let amz_date = sigv4::amz_date(unix_now());
        let scope = Scope {
            amz_date: &amz_date,
            region: &sts.region,
            service: "sts",
        };
        let headers = sigv4::signed_headers(
            credentials,
            &scope,
            &sigv4::Request {
                method: "POST",
                path: "/",
                query: &[],
                headers: &[
                    ("host".to_string(), sts.endpoint.authority.clone()),
                    ("content-type".to_string(), CONTENT_TYPE.to_string()),
                ],
                payload_sha256: &sigv4::sha256_hex(body.as_bytes()),
            },
        );
        for (name, value) in headers.into_iter().filter(|(n, _)| n != "content-type") {
            request = request.header(name, value);
        }
        request
    };
    let retry = |answer: &Answer| {
        let code = xml::parse_sts_error(&answer.body).map(|e| e.code);
        answer.status >= 500
            || matches!(
                code.as_deref(),
                Some("Throttling" | "IDPCommunicationError")
            )
    };
    let answer = send_with_retries(
        STS_ATTEMPTS,
        Duration::from_millis(200),
        progress,
        &build,
        &retry,
    )
    .map_err(Unanswered::into_error)
    .with_context(|| format!("STS at {} did not answer", sts.endpoint.authority))?;
    if answer.status == 200 {
        let keys = xml::parse_sts_credentials(&answer.body)
            .with_context(|| format!("STS at {} answered", sts.endpoint.authority))?;
        return Ok(Fetched {
            credentials: Credentials {
                access_key_id: keys.access_key_id,
                secret_access_key: keys.secret_access_key,
                session_token: Some(keys.session_token),
            },
            expires: Some(keys.expiration),
        });
    }
    let Some(error) = xml::parse_sts_error(&answer.body) else {
        bail!(
            "STS at {} answered HTTP {} with no STS error in the body",
            sts.endpoint.authority,
            answer.status
        );
    };
    bail!(
        "STS at {} refused: {} ({}): {}",
        sts.endpoint.authority,
        for_terminal(&error.code, 64),
        answer.status,
        for_terminal(error.message.trim_end_matches('.'), 400)
    );
}

/// botocore names a session `botocore-session-<time>`; this names it after
/// the tool, which is what CloudTrail then shows.
fn default_session_name() -> String {
    format!("spacetrace-{}", unix_now())
}

fn assume_role(role: &AssumeRole, progress: &ScanProgress) -> Result<Fetched> {
    let source = role.source.fetch(progress).with_context(|| {
        format!(
            "getting the keys that assume it, from {}",
            role.source.describe()
        )
    })?;
    let session_name = role
        .session_name
        .clone()
        .unwrap_or_else(default_session_name);
    let duration = role.duration_seconds.map(|d| d.to_string());
    let mut params = vec![
        ("Action", "AssumeRole"),
        ("Version", "2011-06-15"),
        ("RoleArn", role.role_arn.as_str()),
        ("RoleSessionName", session_name.as_str()),
    ];
    if let Some(external_id) = &role.external_id {
        params.push(("ExternalId", external_id));
    }
    if let Some(duration) = &duration {
        params.push(("DurationSeconds", duration));
    }
    call_sts(&role.sts, &params, Some(&source.credentials), progress)
}

fn assume_role_with_web_identity(web: &WebIdentity, progress: &ScanProgress) -> Result<Fetched> {
    let token = std::fs::read_to_string(&web.token_file).with_context(|| {
        format!(
            "reading the web identity token {}",
            web.token_file.display()
        )
    })?;
    let session_name = web
        .session_name
        .clone()
        .unwrap_or_else(default_session_name);
    call_sts(
        &web.sts,
        &[
            ("Action", "AssumeRoleWithWebIdentity"),
            ("Version", "2011-06-15"),
            ("RoleArn", &web.role_arn),
            ("RoleSessionName", &session_name),
            ("WebIdentityToken", token.trim()),
        ],
        None,
        progress,
    )
}

// ------------------------------------------------------------------ SSO

fn sso_role_credentials(sso: &Sso) -> Result<Fetched> {
    let login = format!("run `aws sso login --profile {}`", sso.profile);
    let cached = match std::fs::read_to_string(&sso.cache_file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!(
            "there is no SSO token for it in {}; {login}",
            sso.cache_file.display()
        ),
        Err(e) => return Err(e).with_context(|| format!("reading {}", sso.cache_file.display())),
    };
    let cached = json(&cached).with_context(|| format!("reading {}", sso.cache_file.display()))?;
    let (Some(token), Some(expires_at)) =
        (field(&cached, "accessToken"), field(&cached, "expiresAt"))
    else {
        bail!(
            "the SSO token cache {} holds no usable token; {login}",
            sso.cache_file.display()
        );
    };
    let expires_at = expiry(expires_at).with_context(|| {
        format!(
            "the expiresAt in {} is not a time",
            sso.cache_file.display()
        )
    })?;
    // Expired is an error and not a cue to try the next source: whoever
    // configured single sign-on meant to list as that role, and keys found
    // somewhere further down the chain would list as somebody else.
    if expires_at <= unix_now() {
        bail!(
            "the SSO session expired at {}; {login}",
            crate::fmt::timestamp(expires_at)
        );
    }

    let url = format!(
        "{}://{}/federation/credentials?account_id={}&role_name={}",
        sso.portal.scheme,
        sso.portal.authority,
        sigv4::uri_encode(&sso.account_id, true),
        sigv4::uri_encode(&sso.role_name, true)
    );
    let client = http_client(SERVICE_TIMEOUT, SERVICE_TIMEOUT, false)?;
    let request = client.get(&url).header("x-amz-sso_bearer_token", token);
    let answer = send(request)
        .map_err(Unanswered::into_error)
        .with_context(|| format!("the SSO portal {} did not answer", sso.portal.authority))?;
    match answer.status {
        200 => {}
        401 | 403 => bail!(
            "the SSO portal no longer accepts the cached token (HTTP {}); {login}",
            answer.status
        ),
        status => bail!(
            "the SSO portal {} answered HTTP {status} {}",
            sso.portal.authority,
            error_message(&answer.body)
        ),
    }
    let value = json(&answer.body).context("reading the SSO portal's answer")?;
    let role = value
        .get("roleCredentials")
        .filter(|r| r.is_object())
        .context("the SSO portal's answer has no roleCredentials")?;
    let credentials = keys_from_json(
        role,
        ["accessKeyId", "secretAccessKey", "sessionToken"],
        true,
    )
    .context("reading the SSO portal's answer")?;
    let expiration_ms = role
        .get("expiration")
        .and_then(serde_json::Value::as_i64)
        .context("the SSO portal's answer has no expiration")?;
    Ok(Fetched {
        credentials,
        expires: Some(expiration_ms / 1000),
    })
}

// ------------------------------------------------------------ container

fn container_credentials(c: &Container, progress: &ScanProgress) -> Result<Fetched> {
    let token = match &c.token {
        None => None,
        Some(ContainerToken::Value(value)) => Some(value.clone()),
        Some(ContainerToken::File(path)) => {
            Some(std::fs::read_to_string(path).with_context(|| {
                format!(
                    "reading the container authorization token {}",
                    path.display()
                )
            })?)
        }
    };
    // botocore's rule, and a header with a line break in it would be two.
    if token.as_ref().is_some_and(|t| t.contains(['\r', '\n'])) {
        bail!("the container authorization token contains a line break");
    }
    let client = http_client(CONTAINER_TIMEOUT, CONTAINER_TIMEOUT, true)?;
    let build = || {
        let request = client.get(&c.url);
        match &token {
            Some(token) => request.header(reqwest::header::AUTHORIZATION, token),
            None => request,
        }
    };
    let answer = send_with_retries(
        CONTAINER_ATTEMPTS,
        Duration::from_secs(1),
        progress,
        &build,
        &|answer| answer.status >= 500,
    )
    .map_err(Unanswered::into_error)
    .context("the container credentials endpoint did not answer")?;
    if answer.status != 200 {
        bail!(
            "the container credentials endpoint answered HTTP {} {}",
            answer.status,
            error_message(&answer.body)
        );
    }
    metadata_keys(&answer.body).context("reading the container credentials")
}

/// The document the container and instance metadata services both serve.
fn metadata_keys(body: &str) -> Result<Fetched> {
    let value = json(body)?;
    if let Some(code) = field(&value, "Code").filter(|c| *c != "Success") {
        bail!("the answer says {}", for_terminal(code, 64));
    }
    keys_and_expiry(&value, ["AccessKeyId", "SecretAccessKey", "Token"])
}

// ------------------------------------------------------ instance metadata

const IMDS_TOKEN_TTL: &str = "21600";

/// The IMDSv2 token as a header value, which is all it is used as.
fn imds_token(body: &str) -> Result<reqwest::header::HeaderValue> {
    reqwest::header::HeaderValue::from_str(body.trim())
        .ok()
        .filter(|token| !token.is_empty())
        .context("its answer to the token request is malformed")
}

/// What IAM allows in a role name, and so all the role-name answer can
/// hold: it becomes the last segment of the next URL, where `.` and `..`
/// would navigate and anything else would need escaping.
fn is_role_name(name: &str) -> bool {
    let allowed = |c: char| c.is_ascii_alphanumeric() || "+=,.@_-".contains(c);
    (1..=64).contains(&name.len()) && name.chars().all(allowed) && name != "." && name != ".."
}

/// The errors name no endpoint: `describe()` already does, in the context
/// every one of them is wrapped in.
fn instance_metadata(m: &InstanceMetadata, progress: &ScanProgress) -> Result<Fetched> {
    let unreachable = |e: Unanswered| match &m.last_resort {
        Some(no_credentials) => anyhow::anyhow!(
            "{no_credentials} (and no instance metadata service answered at {})",
            where_only(&m.endpoint)
        ),
        None => e.into_error().context("it did not answer"),
    };
    // botocore's timeout is for the connection and for the answer each, as
    // here: a connection that is never answered is given up on after one.
    let client = http_client(m.timeout, m.timeout * 2, true)?;
    let ask = |build: &dyn Fn() -> reqwest::blocking::RequestBuilder| {
        send_with_retries(m.attempts, Duration::ZERO, progress, build, &|answer| {
            answer.status >= 500
        })
    };

    // IMDSv2 first. A 403/404/405 is a service that has no v2, and a read
    // timeout is the usual sign of a container one hop too far for the
    // token's answer to reach; both fall back to v1, as botocore does,
    // unless v1 is switched off. A connection refused, or never answered
    // within the timeout, means nothing is there at all, and asking again
    // would only cost another timeout.
    let token_url = format!("{}/latest/api/token", m.endpoint);
    let token = match ask(&|| {
        client
            .put(&token_url)
            .header("x-aws-ec2-metadata-token-ttl-seconds", IMDS_TOKEN_TTL)
    }) {
        Ok(answer) if answer.status == 200 => Some(imds_token(&answer.body)?),
        Ok(answer) if answer.status == 400 => bail!("it refused the token request (HTTP 400)"),
        Ok(_) => None,
        Err(e @ Unanswered::Cancelled(_)) => return Err(e.into_error()),
        Err(e) if e.is_connect() => return Err(unreachable(e)),
        Err(_) => None,
    };
    if token.is_none() && m.v1_disabled {
        bail!(
            "it gave no IMDSv2 token, and IMDSv1 is disabled \
             (AWS_EC2_METADATA_V1_DISABLED / ec2_metadata_v1_disabled)"
        );
    }
    let get = |path: &str| {
        let url = format!(
            "{}/latest/meta-data/iam/security-credentials/{path}",
            m.endpoint
        );
        ask(&|| {
            let request = client.get(&url);
            match &token {
                Some(token) => request.header("x-aws-ec2-metadata-token", token.clone()),
                None => request,
            }
        })
    };

    let roles = get("").map_err(unreachable)?;
    if roles.status == 404 {
        match &m.last_resort {
            Some(no_credentials) => bail!(
                "{no_credentials} (the instance metadata service at {} has no IAM role for this machine)",
                where_only(&m.endpoint)
            ),
            None => bail!("this instance has no IAM role"),
        }
    }
    if roles.status != 200 {
        bail!("it answered HTTP {} for the role name", roles.status);
    }
    let role = roles.body.lines().next().unwrap_or("").trim();
    if !is_role_name(role) {
        bail!("its answer for the role name is malformed");
    }
    let answer = get(role)
        .map_err(Unanswered::into_error)
        .context("it stopped answering")?;
    if answer.status != 200 {
        bail!(
            "it answered HTTP {} for role {role}'s credentials",
            answer.status
        );
    }
    metadata_keys(&answer.body).with_context(|| format!("reading role {role}'s credentials"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_command_lines_split_like_shlex() {
        let split = |s: &str| split_posix(s).unwrap();
        assert_eq!(
            split("aws-vault export --format=json work"),
            ["aws-vault", "export", "--format=json", "work"]
        );
        assert_eq!(
            split("  /bin/tool   'a b'  \"c d\"  "),
            ["/bin/tool", "a b", "c d"]
        );
        assert_eq!(
            split(r#"a\ b "q\"x\\y\z" 'no\escape'"#),
            ["a b", r#"q"x\y\z"#, r"no\escape"]
        );
        assert_eq!(split(r#"x''y "" ''"#), ["xy", "", ""]);
        assert!(split("").is_empty());
        // shlex separates on space, tab, CR and LF only: a no-break space,
        // an em space or a vertical tab is part of the word.
        assert_eq!(split("a\r\nb\tc"), ["a", "b", "c"]);
        assert_eq!(
            split("a\u{a0}b\u{0b}c\u{2003}d e"),
            ["a\u{a0}b\u{0b}c\u{2003}d", "e"]
        );
        for bad in ["'open", "\"open", "trailing\\"] {
            assert!(split_posix(bad).is_err(), "{bad:?}");
        }
    }

    /// Microsoft's own examples for `CommandLineToArgvW` ("Parsing C++
    /// command-line arguments"), which botocore's splitter follows.
    /// <https://learn.microsoft.com/en-us/cpp/cpp/main-function-command-line-args>
    #[test]
    fn windows_command_lines_split_like_the_c_runtime() {
        let split = |s: &str| split_windows(s).unwrap();
        assert_eq!(split(r#""a b c" d e"#), ["a b c", "d", "e"]);
        assert_eq!(split(r#""ab\"c" "\\" d"#), [r#"ab"c"#, r"\", "d"]);
        assert_eq!(split(r#"a\\\b d"e f"g h"#), [r"a\\\b", "de fg", "h"]);
        assert_eq!(split(r#"a\\\"b c d"#), [r#"a\"b"#, "c", "d"]);
        assert_eq!(split(r#"a\\\\"b c" d e"#), [r"a\\b c", "d", "e"]);
        assert_eq!(
            split(r#"C:\Tools\creds.exe --profile "my work""#),
            [r"C:\Tools\creds.exe", "--profile", "my work"]
        );
        assert_eq!(split(r#"x "" y"#), ["x", "", "y"]);
        assert!(split("").is_empty());
        assert!(split_windows("\"open").is_err());
    }

    #[test]
    fn expiries_are_rfc3339_or_the_old_sso_cache_form() {
        assert_eq!(expiry("2020-06-17T10:18:08Z"), Some(1_592_389_088));
        assert_eq!(expiry("2020-06-17T10:18:08UTC"), Some(1_592_389_088));
        assert_eq!(expiry("2020-06-17T12:18:08+02:00"), Some(1_592_389_088));
        assert_eq!(expiry("soon"), None);
    }

    #[test]
    fn a_container_token_never_formats_its_value() {
        let shown = format!("{:?}", ContainerToken::Value("token-value".into()));
        assert!(!shown.contains("token-value"), "{shown}");
    }

    // ------------------------------------------- against stand-in servers
    //
    // Real sockets and real HTTP through reqwest; the server is a local
    // stand-in for STS, the SSO portal and the metadata services, which only
    // exist inside AWS. MinIO's real STS is exercised in `minio_tests.rs`.

    use crate::s3::client::test_server::{serve, Served};

    use crate::fmt::rfc3339;

    /// A form body as a list of decoded pairs.
    fn form(body: &str) -> Vec<(String, String)> {
        body.split('&')
            .filter_map(|pair| pair.split_once('='))
            .map(|(k, v)| {
                (
                    xml::url_decode(k).unwrap().into_owned(),
                    xml::url_decode(v).unwrap().into_owned(),
                )
            })
            .collect()
    }

    fn param<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
        pairs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    fn sts_answer(action: &str, id: &str, expires: i64) -> String {
        format!(
            "<{action}Response xmlns=\"https://sts.amazonaws.com/doc/2011-06-15/\"><{action}Result>\
             <Credentials><AccessKeyId>{id}</AccessKeyId><SecretAccessKey>secret-of-{id}</SecretAccessKey>\
             <SessionToken>token-of-{id}</SessionToken><Expiration>{}</Expiration></Credentials>\
             </{action}Result></{action}Response>",
            rfc3339(expires)
        )
    }

    fn source_keys() -> Credentials {
        Credentials {
            access_key_id: "SOURCEID".into(),
            secret_access_key: "source-secret".into(),
            session_token: Some("source-token".into()),
        }
    }

    #[test]
    fn a_role_is_assumed_with_a_signed_sts_call() {
        let expires = unix_now() + 3600;
        let sts = serve(move |_| (200, sts_answer("AssumeRole", "ROLEID", expires)));
        let role = AssumeRole {
            profile: "p".into(),
            source: Provider::Static(source_keys()),
            role_arn: "arn:aws:iam::123456789012:role/reader".into(),
            session_name: Some("me".into()),
            external_id: Some("ext-1".into()),
            duration_seconds: Some(1800),
            sts: Sts {
                endpoint: Endpoint::parse(&sts.address).unwrap(),
                region: "eu-west-1".into(),
            },
        };
        let fetched = Provider::AssumeRole(Box::new(role))
            .fetch(&ScanProgress::default())
            .unwrap();
        assert_eq!(fetched.credentials.access_key_id, "ROLEID");
        assert_eq!(
            fetched.credentials.session_token.as_deref(),
            Some("token-of-ROLEID")
        );
        assert_eq!(fetched.expires, Some(expires));

        let seen = sts.seen();
        assert_eq!(seen.len(), 1);
        let request = &seen[0];
        assert_eq!(
            (request.method.as_str(), request.target.as_str()),
            ("POST", "/")
        );
        let pairs = form(&request.body);
        assert_eq!(param(&pairs, "Action"), Some("AssumeRole"));
        assert_eq!(param(&pairs, "Version"), Some("2011-06-15"));
        assert_eq!(
            param(&pairs, "RoleArn"),
            Some("arn:aws:iam::123456789012:role/reader")
        );
        assert_eq!(param(&pairs, "RoleSessionName"), Some("me"));
        assert_eq!(param(&pairs, "ExternalId"), Some("ext-1"));
        assert_eq!(param(&pairs, "DurationSeconds"), Some("1800"));
        let authorization = request.header("authorization").unwrap();
        assert!(
            authorization.contains("Credential=SOURCEID/")
                && authorization.contains("/eu-west-1/sts/aws4_request"),
            "{authorization}"
        );
        assert!(
            authorization.contains("x-amz-security-token"),
            "{authorization}"
        );
        assert_eq!(request.header("x-amz-security-token"), Some("source-token"));
        assert!(!request.body.contains("source-secret"));
    }

    #[test]
    fn an_sts_refusal_names_the_code_and_not_the_keys() {
        let sts = serve(|_| {
            (
                403,
                "<ErrorResponse><Error><Type>Sender</Type><Code>AccessDenied</Code>\
                 <Message>User is not authorized to perform: sts:AssumeRole.</Message></Error></ErrorResponse>"
                    .into(),
            )
        });
        let role = AssumeRole {
            profile: "p".into(),
            source: Provider::Static(source_keys()),
            role_arn: "arn:aws:iam::1:role/x".into(),
            session_name: None,
            external_id: None,
            duration_seconds: None,
            sts: Sts {
                endpoint: Endpoint::parse(&sts.address).unwrap(),
                region: "us-east-1".into(),
            },
        };
        let provider = Provider::AssumeRole(Box::new(role));
        let err = Keys::new(provider)
            .get(&ScanProgress::default())
            .unwrap_err();
        let text = format!("{err:#}");
        assert!(text.contains("AccessDenied (403)"), "{text}");
        assert!(text.contains("sts:AssumeRole"), "{text}");
        assert!(text.contains("role arn:aws:iam::1:role/x"), "{text}");
        assert!(
            !text.contains("source-secret") && !text.contains("source-token"),
            "{text}"
        );
        assert_eq!(sts.seen().len(), 1, "a refusal is not retried");
        let pairs = form(&sts.seen()[0].body);
        assert!(
            param(&pairs, "RoleSessionName").is_some_and(|n| n.starts_with("spacetrace-")),
            "a session name is always sent"
        );
    }

    #[test]
    fn a_web_identity_token_is_traded_unsigned_and_read_afresh() {
        let dir = tempfile::tempdir().unwrap();
        let token_file = dir.path().join("token");
        std::fs::write(&token_file, "first-jwt\n").unwrap();
        let expires = unix_now() + 3600;
        let sts = serve(move |_| {
            (
                200,
                sts_answer("AssumeRoleWithWebIdentity", "WEBID", expires),
            )
        });
        let provider = Provider::WebIdentity(WebIdentity {
            token_file: token_file.clone(),
            role_arn: "arn:aws:iam::1:role/pod".into(),
            session_name: Some("pod".into()),
            sts: Sts {
                endpoint: Endpoint::parse(&sts.address).unwrap(),
                region: "us-east-1".into(),
            },
        });
        assert_eq!(
            provider
                .fetch(&ScanProgress::default())
                .unwrap()
                .credentials
                .access_key_id,
            "WEBID"
        );
        std::fs::write(&token_file, "second-jwt").unwrap();
        provider.fetch(&ScanProgress::default()).unwrap();

        let seen = sts.seen();
        assert!(
            seen.iter().all(|r| r.header("authorization").is_none()),
            "unsigned"
        );
        let first = form(&seen[0].body);
        assert_eq!(param(&first, "Action"), Some("AssumeRoleWithWebIdentity"));
        assert_eq!(param(&first, "WebIdentityToken"), Some("first-jwt"));
        assert_eq!(param(&first, "RoleArn"), Some("arn:aws:iam::1:role/pod"));
        assert_eq!(
            param(&form(&seen[1].body), "WebIdentityToken"),
            Some("second-jwt")
        );
    }

    fn sso_world(expires_at: &str, portal: &str) -> (tempfile::TempDir, Provider) {
        let dir = tempfile::tempdir().unwrap();
        let cache_file = dir
            .path()
            .join(format!("{}.json", sigv4::sha1_hex(b"corp")));
        std::fs::write(
            &cache_file,
            format!(
                "{{\"startUrl\": \"https://corp/start\", \"region\": \"eu-west-1\", \
                 \"accessToken\": \"cached-bearer\", \"expiresAt\": \"{expires_at}\"}}"
            ),
        )
        .unwrap();
        let provider = Provider::Sso(Sso {
            profile: "dev".into(),
            cache_file,
            account_id: "111122223333".into(),
            role_name: "Read Only".into(),
            portal: Endpoint::parse(portal).unwrap(),
        });
        (dir, provider)
    }

    #[test]
    fn sso_trades_the_cached_token_at_the_portal() {
        let expires_ms = (unix_now() + 3600) * 1000;
        let portal = serve(move |_| {
            (
                200,
                format!(
                    "{{\"roleCredentials\":{{\"accessKeyId\":\"SSOID\",\"secretAccessKey\":\"sso-secret\",\
                     \"sessionToken\":\"sso-token\",\"expiration\":{expires_ms}}}}}"
                ),
            )
        });
        let (_dir, provider) = sso_world(&rfc3339(unix_now() + 600), &portal.address);
        let fetched = provider.fetch(&ScanProgress::default()).unwrap();
        assert_eq!(fetched.credentials.access_key_id, "SSOID");
        assert_eq!(
            fetched.credentials.session_token.as_deref(),
            Some("sso-token")
        );
        assert_eq!(fetched.expires, Some(expires_ms / 1000));
        let seen = portal.seen();
        assert_eq!(
            seen[0].target,
            "/federation/credentials?account_id=111122223333&role_name=Read%20Only"
        );
        assert_eq!(
            seen[0].header("x-amz-sso_bearer_token"),
            Some("cached-bearer")
        );

        // The CLI's older cache spelling of the same instant.
        let old_form = rfc3339(unix_now() + 600).replace('Z', "UTC");
        let (_dir, provider) = sso_world(&old_form, &portal.address);
        provider.fetch(&ScanProgress::default()).unwrap();
    }

    /// Expired means "log in again", never "try the next source": the next
    /// source would list as somebody else. Nothing is sent to the portal.
    #[test]
    fn an_expired_or_refused_sso_token_says_to_log_in() {
        let portal = serve(|_| {
            (
                401,
                "{\"message\":\"Session token not found or invalid\"}".into(),
            )
        });
        let (_dir, expired) = sso_world(&rfc3339(unix_now() - 60), &portal.address);
        let text = format!(
            "{:#}",
            Keys::new(expired)
                .get(&ScanProgress::default())
                .unwrap_err()
        );
        assert!(
            text.contains("expired") && text.contains("aws sso login --profile dev"),
            "{text}"
        );
        assert!(!text.contains("cached-bearer"), "{text}");
        assert!(portal.seen().is_empty());

        let (_dir, refused) = sso_world(&rfc3339(unix_now() + 600), &portal.address);
        let text = format!(
            "{:#}",
            refused.fetch(&ScanProgress::default()).err().unwrap()
        );
        assert!(
            text.contains("HTTP 401") && text.contains("aws sso login"),
            "{text}"
        );

        let (dir, missing) = sso_world(&rfc3339(unix_now() + 600), &portal.address);
        std::fs::remove_dir_all(dir.path()).unwrap();
        let text = format!(
            "{:#}",
            missing.fetch(&ScanProgress::default()).err().unwrap()
        );
        assert!(
            text.contains("no SSO token") && text.contains("aws sso login"),
            "{text}"
        );
    }

    fn metadata_answer(id: &str, expires: i64) -> String {
        format!(
            "{{\"Code\":\"Success\",\"Type\":\"AWS-HMAC\",\"AccessKeyId\":\"{id}\",\
             \"SecretAccessKey\":\"secret-of-{id}\",\"Token\":\"token-of-{id}\",\
             \"Expiration\":\"{}\"}}",
            rfc3339(expires)
        )
    }

    #[test]
    fn container_credentials_carry_the_authorization_token() {
        let expires = unix_now() + 3600;
        let server = serve(move |_| (200, metadata_answer("ECSID", expires)));
        let dir = tempfile::tempdir().unwrap();
        let token_file = dir.path().join("token");
        std::fs::write(&token_file, "pod-identity-token").unwrap();
        let provider = Provider::Container(Container {
            url: format!("{}/v1/credentials", server.address),
            token: Some(ContainerToken::File(token_file.clone())),
        });
        let fetched = provider.fetch(&ScanProgress::default()).unwrap();
        assert_eq!(fetched.credentials.access_key_id, "ECSID");
        assert_eq!(
            fetched.credentials.session_token.as_deref(),
            Some("token-of-ECSID")
        );
        assert_eq!(fetched.expires, Some(expires));
        let seen = server.seen();
        assert_eq!(seen[0].target, "/v1/credentials");
        assert_eq!(seen[0].header("authorization"), Some("pod-identity-token"));

        std::fs::write(&token_file, "token\n").unwrap();
        let text = format!(
            "{:#}",
            provider.fetch(&ScanProgress::default()).err().unwrap()
        );
        assert!(text.contains("line break"), "{text}");
        assert_eq!(server.seen().len(), 1, "a bad token is not sent");
    }

    #[test]
    fn imds_v2_takes_a_token_then_the_role_then_its_keys() {
        let expires = unix_now() + 3600;
        let server =
            serve(
                move |request| match (request.method.as_str(), request.target.as_str()) {
                    ("PUT", "/latest/api/token") => (200, "imds-session-token".into()),
                    ("GET", "/latest/meta-data/iam/security-credentials/") => {
                        (200, "reader-role\n".into())
                    }
                    ("GET", "/latest/meta-data/iam/security-credentials/reader-role") => {
                        (200, metadata_answer("EC2ID", expires))
                    }
                    _ => (404, String::new()),
                },
            );
        let provider = Provider::InstanceMetadata(InstanceMetadata {
            endpoint: server.address.clone(),
            timeout: Duration::from_secs(1),
            attempts: 1,
            v1_disabled: false,
            last_resort: None,
        });
        let fetched = provider.fetch(&ScanProgress::default()).unwrap();
        assert_eq!(fetched.credentials.access_key_id, "EC2ID");
        let seen = server.seen();
        assert_eq!(seen.len(), 3);
        assert_eq!(
            seen[0].header("x-aws-ec2-metadata-token-ttl-seconds"),
            Some("21600")
        );
        for request in &seen[1..] {
            assert_eq!(
                request.header("x-aws-ec2-metadata-token"),
                Some("imds-session-token")
            );
        }
    }

    /// A service without IMDSv2 answers the token request 403/404/405; the
    /// keys then come over v1, unless v1 is switched off.
    #[test]
    fn imds_falls_back_to_v1_only_when_allowed() {
        let expires = unix_now() + 3600;
        let server =
            serve(
                move |request| match (request.method.as_str(), request.target.as_str()) {
                    ("PUT", _) => (403, String::new()),
                    ("GET", "/latest/meta-data/iam/security-credentials/") => (200, "r".into()),
                    ("GET", "/latest/meta-data/iam/security-credentials/r") => {
                        (200, metadata_answer("V1ID", expires))
                    }
                    _ => (404, String::new()),
                },
            );
        let metadata = |v1_disabled| {
            Provider::InstanceMetadata(InstanceMetadata {
                endpoint: server.address.clone(),
                timeout: Duration::from_secs(1),
                attempts: 1,
                v1_disabled,
                last_resort: None,
            })
        };
        assert_eq!(
            metadata(false)
                .fetch(&ScanProgress::default())
                .unwrap()
                .credentials
                .access_key_id,
            "V1ID"
        );
        assert!(server.seen()[1..]
            .iter()
            .all(|r| r.header("x-aws-ec2-metadata-token").is_none()));
        let text = format!(
            "{:#}",
            metadata(true)
                .fetch(&ScanProgress::default())
                .err()
                .unwrap()
        );
        assert!(text.contains("IMDSv1 is disabled"), "{text}");
    }

    /// At the end of the chain, an address nothing answers on is the usual
    /// case — a laptop — and the message is the "no credentials" one.
    #[test]
    fn nothing_at_the_metadata_address_means_no_credentials() {
        let closed = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", listener.local_addr().unwrap())
        };
        let provider = Provider::InstanceMetadata(InstanceMetadata {
            endpoint: closed,
            timeout: Duration::from_secs(1),
            attempts: 1,
            v1_disabled: false,
            last_resort: Some("no S3 credentials: do something".into()),
        });
        let started = std::time::Instant::now();
        let text = format!(
            "{:#}",
            provider.fetch(&ScanProgress::default()).err().unwrap()
        );
        assert!(
            text.starts_with(
                "no S3 credentials: do something (and no instance metadata service answered"
            ),
            "{text}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "a refused connection is not waited on twice"
        );

        let roleless = serve(|request| match request.method.as_str() {
            "PUT" => (200, "t".into()),
            _ => (404, String::new()),
        });
        let provider = Provider::InstanceMetadata(InstanceMetadata {
            endpoint: roleless.address.clone(),
            timeout: Duration::from_secs(1),
            attempts: 1,
            v1_disabled: false,
            last_resort: Some("no S3 credentials".into()),
        });
        let text = format!(
            "{:#}",
            provider.fetch(&ScanProgress::default()).err().unwrap()
        );
        assert!(text.contains("has no IAM role"), "{text}");
    }

    /// Off EC2 nothing answers the metadata address at all. That is given up
    /// on within one timeout, as unreachable: no IMDSv1 attempt after the
    /// token request, which would only wait as long again.
    ///
    /// 192.0.2.1 is TEST-NET-1 (RFC 5737), assigned to nobody: with a route
    /// out, a connection there is never answered, which is the case this is
    /// about; with none, it is refused at once.
    #[test]
    fn a_metadata_address_that_never_answers_costs_one_timeout() {
        let provider = Provider::InstanceMetadata(InstanceMetadata {
            endpoint: "http://192.0.2.1".into(),
            timeout: Duration::from_secs(1),
            attempts: 1,
            v1_disabled: false,
            last_resort: Some("no S3 credentials".into()),
        });
        let started = std::time::Instant::now();
        let text = format!(
            "{:#}",
            provider.fetch(&ScanProgress::default()).err().unwrap()
        );
        let took = started.elapsed();
        assert!(
            text.starts_with("no S3 credentials (and no instance metadata service answered"),
            "{text}"
        );
        assert!(took < Duration::from_millis(1800), "took {took:?}");
    }

    /// A cancel while waiting to ask again ends the fetch at once, as
    /// `Interrupted`, rather than after the pause.
    #[test]
    fn a_cancel_during_a_retry_pause_is_noticed_at_once() {
        let progress = std::sync::Arc::new(ScanProgress::default());
        let cancel = std::sync::Arc::clone(&progress);
        let server = serve(move |_| {
            cancel.cancel();
            (500, String::new())
        });
        let started = std::time::Instant::now();
        let err = container_at(&server).fetch(&progress).err().unwrap();
        assert!(is_interrupted(&err), "{err:#}");
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "the second attempt was a second away"
        );
        assert_eq!(server.seen().len(), 1);
    }

    /// A connection dropped without an answer is asked again: the container
    /// agent can still be starting when the scan does.
    #[test]
    fn a_dropped_connection_is_asked_again() {
        let expires = unix_now() + 3600;
        let (address, server) = crate::s3::client::test_server::canned(vec![
            (0, "", Vec::new()),
            (200, "OK", metadata_answer("AGAINID", expires).into_bytes()),
        ]);
        let provider = Provider::Container(Container {
            url: format!("{address}/creds"),
            token: None,
        });
        let fetched = provider.fetch(&ScanProgress::default()).unwrap();
        assert_eq!(fetched.credentials.access_key_id, "AGAINID");
        assert_eq!(server.join().unwrap().len(), 2);
    }

    /// A configured URL may carry a password or a token in its userinfo or
    /// query; what an error names is where, not how.
    #[test]
    fn a_described_address_carries_no_userinfo_or_query() {
        let container = Provider::Container(Container {
            url: "http://alice:hunter2@127.0.0.1:9/creds?sig=abc#frag".into(),
            token: None,
        });
        let text = container.describe();
        assert!(text.contains("http://127.0.0.1:9/creds"), "{text}");
        for secret in ["alice", "hunter2", "sig=abc", "frag"] {
            assert!(!text.contains(secret), "{text}");
        }

        let closed = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let metadata = Provider::InstanceMetadata(InstanceMetadata {
            endpoint: format!("http://alice:hunter2@127.0.0.1:{closed}"),
            timeout: Duration::from_secs(1),
            attempts: 1,
            v1_disabled: false,
            last_resort: Some("no S3 credentials".into()),
        });
        let err = Keys::new(metadata)
            .get(&ScanProgress::default())
            .unwrap_err();
        let text = format!("{err:#}");
        assert!(text.contains(&format!("127.0.0.1:{closed}")), "{text}");
        assert!(
            !text.contains("alice") && !text.contains("hunter2"),
            "{text}"
        );
    }

    /// The role name goes into the next URL and the token into a header, so
    /// either one malformed is refused as such — not sent, and not reported
    /// as a service that did not answer.
    #[test]
    fn a_malformed_role_name_or_token_is_refused_as_malformed() {
        let expires = unix_now() + 3600;
        let cases: [(&'static str, &'static str); 6] = [
            ("token", ".."),
            ("token", "."),
            ("token", "a b"),
            ("token", "role\u{1b}[31m"),
            ("token", "r%2F.."),
            ("to\u{1}ken", "reader"),
        ];
        for (token, role) in cases {
            let server = serve(move |request| {
                match (request.method.as_str(), request.target.as_str()) {
                    ("PUT", _) => (200, token.into()),
                    ("GET", "/latest/meta-data/iam/security-credentials/") => (200, role.into()),
                    // Anything else hands out keys, so only a refusal fails.
                    _ => (200, metadata_answer("WRONGID", expires)),
                }
            });
            let provider = Provider::InstanceMetadata(InstanceMetadata {
                endpoint: server.address.clone(),
                timeout: Duration::from_secs(1),
                attempts: 1,
                v1_disabled: false,
                last_resort: None,
            });
            let outcome = provider.fetch(&ScanProgress::default());
            let text = format!("{:#}", outcome.err().expect("refused"));
            assert!(text.contains("malformed"), "{token:?} {role:?}: {text}");
            assert!(!text.contains('\u{1b}'), "{text}");
            let asked = if token == "token" { 2 } else { 1 };
            assert_eq!(
                server.seen().len(),
                asked,
                "{token:?} {role:?}: nothing asked after it"
            );
        }
    }

    /// A `credential_process` printing without end is stopped at a size no
    /// credentials document reaches, instead of being read into memory.
    #[cfg(unix)]
    #[test]
    fn a_credential_process_that_prints_too_much_is_cut_off() {
        let process = Provider::Process(Process {
            profile: "p".into(),
            command: "head -c 3000000 /dev/zero".into(),
        });
        let text = format!(
            "{:#}",
            process.fetch(&ScanProgress::default()).err().unwrap()
        );
        assert!(text.contains("more than 1 MiB"), "{text}");

        let endless = Provider::Process(Process {
            profile: "p".into(),
            command: "yes".into(),
        });
        let started = std::time::Instant::now();
        let text = format!(
            "{:#}",
            endless.fetch(&ScanProgress::default()).err().unwrap()
        );
        assert!(text.contains("more than 1 MiB"), "{text}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// Keys whose first fetch is slow: a second caller waits for it, and a
    /// cancel reaches the waiter at once rather than when the fetch ends.
    #[test]
    fn a_caller_waiting_for_another_s_fetch_can_be_cancelled() {
        let server = serve(|_| {
            std::thread::sleep(Duration::from_millis(1500));
            (200, metadata_answer("SLOWID", unix_now() + 3600))
        });
        let keys = std::sync::Arc::new(Keys::new(container_at(&server)));
        let fetcher = {
            let keys = std::sync::Arc::clone(&keys);
            std::thread::spawn(move || keys.get(&ScanProgress::default()).unwrap())
        };
        while server.seen().is_empty() {
            std::thread::sleep(Duration::from_millis(5));
        }
        let progress = std::sync::Arc::new(ScanProgress::default());
        let waiter = {
            let (keys, progress) = (
                std::sync::Arc::clone(&keys),
                std::sync::Arc::clone(&progress),
            );
            std::thread::spawn(move || {
                let outcome = keys.get(&progress);
                (outcome.map(|c| c.access_key_id), std::time::Instant::now())
            })
        };
        std::thread::sleep(Duration::from_millis(100));
        let cancelled = std::time::Instant::now();
        progress.cancel();
        let (outcome, ended) = waiter.join().unwrap();
        let err = outcome.unwrap_err();
        assert!(is_interrupted(&err), "{err:#}");
        assert!(
            ended.duration_since(cancelled) < Duration::from_millis(300),
            "the cancel took {:?}",
            ended.duration_since(cancelled)
        );
        assert_eq!(fetcher.join().unwrap().access_key_id, "SLOWID");
        assert_eq!(server.seen().len(), 1);
    }

    /// Callers that waited for a fetch that failed get its failure; they do
    /// not each ask again, which for a `credential_process` would be one MFA
    /// prompt per worker.
    #[test]
    fn a_failed_fetch_is_not_repeated_by_everyone_waiting_for_it() {
        let server = serve(|_| {
            std::thread::sleep(Duration::from_millis(300));
            (404, "{\"message\":\"no such role\"}".into())
        });
        let keys = std::sync::Arc::new(Keys::new(container_at(&server)));
        let callers: Vec<_> = (0..8)
            .map(|_| {
                let keys = std::sync::Arc::clone(&keys);
                std::thread::spawn(move || keys.get(&ScanProgress::default()).map(|_| ()))
            })
            .collect();
        for caller in callers {
            let text = format!("{:#}", caller.join().unwrap().unwrap_err());
            assert!(text.contains("no such role"), "{text}");
        }
        assert_eq!(server.seen().len(), 1, "one fetch for eight callers");
        assert!(keys.get(&ScanProgress::default()).is_err());
        assert_eq!(server.seen().len(), 2, "a later caller asks again");
    }

    /// The renewal windows are measured from when the keys arrived: a slow
    /// fetch does not stretch the lifetime they are computed from.
    #[test]
    fn the_windows_count_from_when_the_keys_arrived() {
        use std::sync::atomic::{AtomicI64, Ordering::SeqCst};
        let expires = std::sync::Arc::new(AtomicI64::new(0));
        let server = {
            let expires = std::sync::Arc::clone(&expires);
            serve(move |_| {
                std::thread::sleep(Duration::from_millis(1000));
                let at = unix_now() + 8;
                expires.store(at, SeqCst);
                (200, metadata_answer("ID", at))
            })
        };
        let keys = Keys::new(container_at(&server));
        let asked = unix_now_ms();
        keys.get(&ScanProgress::default()).unwrap();
        let window = keys.window().expect("keys that expire");
        let expires_ms = expires.load(SeqCst) * 1000;
        // Half the lifetime before expiry, the lifetime counted from the
        // arrival — which is at least a second after the asking.
        let arrived = asked + 1000;
        assert!(
            window.refresh_at >= expires_ms - (expires_ms - arrived) / 2,
            "refresh at {} for keys expiring at {expires_ms}, asked at {asked}",
            window.refresh_at
        );
    }

    /// A chain of roles that ends in a refusal names every role and none of
    /// the keys on the way, in the message and in `Debug`.
    #[test]
    fn a_chained_role_s_refusal_prints_no_keys() {
        let sts = serve(|_| {
            (
                403,
                "<ErrorResponse><Error><Code>AccessDenied</Code>\
                 <Message>not allowed</Message></Error></ErrorResponse>"
                    .into(),
            )
        });
        let sts_at = || Sts {
            endpoint: Endpoint::parse(&sts.address).unwrap(),
            region: "us-east-1".into(),
        };
        let inner = AssumeRole {
            profile: "base".into(),
            source: Provider::Static(source_keys()),
            role_arn: "arn:aws:iam::1:role/inner".into(),
            session_name: None,
            external_id: None,
            duration_seconds: None,
            sts: sts_at(),
        };
        let outer = Provider::AssumeRole(Box::new(AssumeRole {
            profile: "top".into(),
            source: Provider::AssumeRole(Box::new(inner)),
            role_arn: "arn:aws:iam::1:role/outer".into(),
            session_name: None,
            external_id: None,
            duration_seconds: None,
            sts: sts_at(),
        }));
        let shown = format!("{outer:?}");
        let err = Keys::new(outer).get(&ScanProgress::default()).unwrap_err();
        let text = format!("{err:#} {err:?}");
        assert!(
            text.contains("role/outer") && text.contains("role/inner"),
            "{text}"
        );
        for secret in ["SOURCEID", "source-secret", "source-token"] {
            assert!(!text.contains(secret), "{secret} in {text}");
            assert!(!shown.contains(secret), "{secret} in {shown}");
        }
    }

    /// Prints the file's contents: `cat` on Unix, `type` on Windows, through
    /// the platform's own command-line splitting.
    fn print_file(path: &std::path::Path) -> String {
        match cfg!(windows) {
            true => format!("cmd /C type \"{}\"", path.display()),
            false => format!("cat '{}'", path.display()),
        }
    }

    #[test]
    fn credential_process_output_is_read_as_botocore_reads_it() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("creds.json");
        let process = |command: String| {
            Provider::Process(Process {
                profile: "p".into(),
                command,
            })
        };
        let expires = unix_now() + 3600;
        std::fs::write(
            &out,
            format!(
                "{{\"Version\": 1, \"AccessKeyId\": \"PROCID\", \"SecretAccessKey\": \"proc-secret\", \
                 \"SessionToken\": \"proc-token\", \"Expiration\": \"{}\"}}",
                rfc3339(expires)
            ),
        )
        .unwrap();
        let fetched = process(print_file(&out))
            .fetch(&ScanProgress::default())
            .unwrap();
        assert_eq!(fetched.credentials.access_key_id, "PROCID");
        assert_eq!(
            fetched.credentials.session_token.as_deref(),
            Some("proc-token")
        );
        assert_eq!(fetched.expires, Some(expires));

        std::fs::write(
            &out,
            "{\"Version\": 1, \"AccessKeyId\": \"LONGID\", \"SecretAccessKey\": \"long-secret\"}",
        )
        .unwrap();
        let fetched = process(print_file(&out))
            .fetch(&ScanProgress::default())
            .unwrap();
        assert_eq!(
            fetched.expires, None,
            "no Expiration is a key that does not expire"
        );

        std::fs::write(
            &out,
            "{\"Version\": 2, \"AccessKeyId\": \"X\", \"SecretAccessKey\": \"v2-secret\"}",
        )
        .unwrap();
        let text = format!(
            "{:#}",
            process(print_file(&out))
                .fetch(&ScanProgress::default())
                .err()
                .unwrap()
        );
        assert!(text.contains("Version 2"), "{text}");
        assert!(!text.contains("v2-secret"), "{text}");

        std::fs::write(
            &out,
            "{\"Version\": 1, \"AccessKeyId\": \"X\", \"SecretAccessKey\": 7}",
        )
        .unwrap();
        let text = format!(
            "{:#}",
            process(print_file(&out))
                .fetch(&ScanProgress::default())
                .err()
                .unwrap()
        );
        assert!(text.contains("no SecretAccessKey"), "{text}");

        std::fs::write(&out, "AccessKeyId=half-secret").unwrap();
        let text = format!(
            "{:#}",
            process(print_file(&out))
                .fetch(&ScanProgress::default())
                .err()
                .unwrap()
        );
        assert!(
            text.contains("not JSON") && !text.contains("half-secret"),
            "{text}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_credential_process_shows_its_stderr_and_not_its_stdout() {
        let process = Provider::Process(Process {
            profile: "p".into(),
            command: r#"sh -c "echo printed-secret; echo 'MFA device not found' >&2; exit 3""#
                .into(),
        });
        let text = format!(
            "{:#}",
            process.fetch(&ScanProgress::default()).err().unwrap()
        );
        assert!(text.contains("MFA device not found"), "{text}");
        assert!(text.contains("exit status: 3"), "{text}");
        assert!(!text.contains("printed-secret"), "{text}");

        let missing = Provider::Process(Process {
            profile: "p".into(),
            command: "/no/such/credential-helper".into(),
        });
        assert!(format!(
            "{:#}",
            missing.fetch(&ScanProgress::default()).err().unwrap()
        )
        .contains("cannot run"));
    }

    /// One container endpoint, counting what it hands out: each fetch gets
    /// keys of its own, valid for `lifetime` seconds.
    fn issuing(lifetime: i64) -> Served {
        let issued = std::sync::atomic::AtomicU64::new(0);
        serve(move |_| {
            let n = issued.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            (
                200,
                metadata_answer(&format!("ID{n}"), unix_now() + lifetime),
            )
        })
    }

    fn container_at(server: &Served) -> Provider {
        Provider::Container(Container {
            url: format!("{}/creds", server.address),
            token: None,
        })
    }

    /// Fetched once, held, renewed past the advisory mark, and an advisory
    /// renewal that fails leaves the current keys in use until the mandatory
    /// one — which has to work.
    #[test]
    fn keys_are_held_and_renewed_in_two_windows() {
        let server = issuing(3600);
        let keys = Keys::new(container_at(&server));
        assert_eq!(
            keys.get(&ScanProgress::default()).unwrap().access_key_id,
            "ID1"
        );
        assert_eq!(
            keys.get(&ScanProgress::default()).unwrap().access_key_id,
            "ID1",
            "held, not fetched again"
        );
        assert_eq!(server.seen().len(), 1);

        // One hour: advisory at 45 minutes, mandatory at 50, as in botocore.
        keys.age_by(44 * 60 * 1000);
        assert_eq!(
            keys.get(&ScanProgress::default()).unwrap().access_key_id,
            "ID1"
        );
        keys.age_by(2 * 60 * 1000);
        assert_eq!(
            keys.get(&ScanProgress::default()).unwrap().access_key_id,
            "ID2",
            "renewed in the advisory window"
        );

        drop(server);
        keys.age_by(46 * 60 * 1000);
        assert_eq!(
            keys.get(&ScanProgress::default()).unwrap().access_key_id,
            "ID2",
            "an advisory renewal that fails keeps the keys that still work"
        );
        keys.age_by(5 * 60 * 1000);
        let text = format!("{:#}", keys.get(&ScanProgress::default()).unwrap_err());
        assert!(text.contains("container credentials"), "{text}");
    }

    /// One thread renews in the advisory window; the others sign with the
    /// keys that still work instead of waiting for it.
    #[test]
    fn an_advisory_renewal_does_not_hold_up_other_requests() {
        let issued = std::sync::atomic::AtomicU64::new(0);
        let server = serve(move |_| {
            let n = issued.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            if n > 1 {
                std::thread::sleep(Duration::from_millis(1500));
            }
            (200, metadata_answer(&format!("ID{n}"), unix_now() + 3600))
        });
        let keys = std::sync::Arc::new(Keys::new(container_at(&server)));
        let idle = ScanProgress::default();
        assert_eq!(keys.get(&idle).unwrap().access_key_id, "ID1");
        keys.age_by(46 * 60 * 1000);

        let renewer = {
            let keys = std::sync::Arc::clone(&keys);
            std::thread::spawn(move || keys.get(&ScanProgress::default()).unwrap())
        };
        while server.seen().len() < 2 {
            std::thread::sleep(Duration::from_millis(10));
        }
        let started = std::time::Instant::now();
        assert_eq!(keys.get(&idle).unwrap().access_key_id, "ID1");
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "waited {:?} for somebody else's renewal",
            started.elapsed()
        );
        assert_eq!(renewer.join().unwrap().access_key_id, "ID2");
        assert_eq!(keys.get(&idle).unwrap().access_key_id, "ID2");
        assert_eq!(server.seen().len(), 2, "one renewal, not one per caller");
    }

    /// Fifteen-minute keys are not fetched for every request, which botocore's
    /// fixed ten-minute window would do to them.
    #[test]
    fn short_lived_keys_are_renewed_at_half_their_life() {
        let server = issuing(900);
        let keys = Keys::new(container_at(&server));
        keys.get(&ScanProgress::default()).unwrap();
        keys.age_by(7 * 60 * 1000);
        keys.get(&ScanProgress::default()).unwrap();
        assert_eq!(server.seen().len(), 1, "still in the first half");
        keys.age_by(60 * 1000);
        keys.get(&ScanProgress::default()).unwrap();
        assert_eq!(server.seen().len(), 2);
    }

    #[test]
    fn keys_the_service_calls_expired_are_dropped_once() {
        let server = issuing(3600);
        let keys = Keys::new(container_at(&server));
        let first = keys.get(&ScanProgress::default()).unwrap();
        assert!(keys.expire(&first));
        let second = keys.get(&ScanProgress::default()).unwrap();
        assert_ne!(first, second);
        assert!(keys.expire(&first), "renewable, though already renewed");
        assert_eq!(
            keys.get(&ScanProgress::default()).unwrap(),
            second,
            "a stale complaint drops nothing"
        );

        let fixed = Keys::new(Provider::Static(source_keys()));
        assert!(
            !fixed.expire(&source_keys()),
            "static keys come back the same"
        );
    }

    #[test]
    fn keys_that_arrive_expired_are_refused() {
        let server = issuing(-10);
        let text = format!(
            "{:#}",
            Keys::new(container_at(&server))
                .get(&ScanProgress::default())
                .unwrap_err()
        );
        assert!(text.contains("expired at"), "{text}");
    }
}
