//! Where the credentials, the region and the endpoint come from.
//!
//! The order is botocore's default chain, so that a machine already set up for
//! `aws s3 ls` needs nothing more. A flag, then the environment, then the
//! profile — `~/.aws/config` with `~/.aws/credentials` laid over it — and
//! last the container and instance metadata services:
//!
//! 1. `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` (skipped under `--profile`)
//! 2. the profile's `role_arn`, with `source_profile` or `credential_source`
//! 3. a web identity token: `AWS_WEB_IDENTITY_TOKEN_FILE` + `AWS_ROLE_ARN`, or
//!    the profile's `web_identity_token_file`
//! 4. the profile's single sign-on (`sso_session` or `sso_start_url`)
//! 5. keys in `~/.aws/credentials`
//! 6. the profile's `credential_process`
//! 7. keys in `~/.aws/config`
//! 8. container credentials (`AWS_CONTAINER_CREDENTIALS_*`)
//! 9. EC2 instance metadata, unless `AWS_EC2_METADATA_DISABLED=true`
//!
//! Only the decision is made here, from the files and the environment; the
//! fetching is `credentials.rs`. The environment is passed in as a function,
//! not read here, so the tests can run in parallel without fighting over the
//! process environment.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};

use super::credentials::{
    AssumeRole, Container, ContainerToken, InstanceMetadata, Process, Provider, Sso, Sts,
    WebIdentity,
};
use super::sigv4::{self, Credentials};

/// What the user said on the command line.
#[derive(Debug, Default, Clone)]
pub struct Flags {
    pub endpoint: Option<String>,
    pub region: Option<String>,
    pub profile: Option<String>,
    pub no_sign_request: bool,
}

/// Everything needed to address and sign a request.
#[derive(Debug, Clone)]
pub struct Settings {
    /// `None` signs nothing — a public bucket, `--no-sign-request`.
    pub credentials: Option<Provider>,
    pub region: String,
    /// `None` is AWS itself.
    pub endpoint: Option<Endpoint>,
}

/// An S3-compatible service, addressed path-style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// `http` or `https`.
    pub scheme: String,
    /// `host` or `host:port`, lower case, as the `Host` header carries it.
    pub authority: String,
}

impl Endpoint {
    pub fn parse(raw: &str) -> Result<Endpoint> {
        let url = reqwest::Url::parse(raw)
            .with_context(|| format!("--endpoint is not a URL: {raw:?}"))?;
        let scheme = url.scheme();
        if scheme != "http" && scheme != "https" {
            bail!("--endpoint must be http:// or https://, not {scheme}://");
        }
        // Refused rather than stripped: credentials in a URL would end up in
        // the snapshot's host name and every line that prints it.
        if !url.username().is_empty() || url.password().is_some() {
            bail!("--endpoint must not carry credentials; use AWS_ACCESS_KEY_ID and a profile");
        }
        if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
            bail!(
                "--endpoint is the service's address alone (e.g. http://127.0.0.1:9000); \
                 the bucket goes in the s3:// path"
            );
        }
        let host = url.host_str().context("--endpoint has no host")?;
        let authority = match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_string(),
        };
        Ok(Endpoint {
            scheme: scheme.to_string(),
            authority: authority.to_ascii_lowercase(),
        })
    }
}

/// The default region when nothing names one: the AWS CLI's, and what MinIO
/// assumes unless configured otherwise.
pub const DEFAULT_REGION: &str = "us-east-1";

pub fn resolve(flags: &Flags, env: &dyn Fn(&str) -> Option<String>) -> Result<Settings> {
    let var = |name: &str| env(name).filter(|v| !v.trim().is_empty());
    let home = var("HOME")
        .or_else(|| var("USERPROFILE"))
        .map(PathBuf::from);
    let file = |explicit: &str, default: &str| -> Option<PathBuf> {
        var(explicit)
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.join(".aws").join(default)))
    };
    let credentials_file = file("AWS_SHARED_CREDENTIALS_FILE", "credentials");
    let config_file = file("AWS_CONFIG_FILE", "config");
    // Only the config file up front, for the region. The credentials file is
    // read when nothing earlier supplied keys: a file of secrets is opened
    // when it is the answer, not on every run.
    let config_ini = read_ini(config_file.as_deref())?;

    let (profile, profile_was_named) = match (&flags.profile, var("AWS_PROFILE")) {
        (Some(p), _) => (p.clone(), true),
        (None, Some(p)) => (p, true),
        (None, None) => ("default".to_string(), false),
    };
    let from_config = config_ini.section(&config_section_name(&profile));

    let region = flags
        .region
        .clone()
        .or_else(|| var("AWS_REGION"))
        .or_else(|| var("AWS_DEFAULT_REGION"))
        .or_else(|| from_config.and_then(|s| s.get("region")))
        .unwrap_or_else(|| DEFAULT_REGION.to_string());
    // On AWS the region becomes part of the host name a signed request goes
    // to, so it is held to what a region name can be.
    check_region(&region)?;

    let endpoint = match flags
        .endpoint
        .clone()
        .or_else(|| var("AWS_ENDPOINT_URL_S3"))
        .or_else(|| var("AWS_ENDPOINT_URL"))
    {
        Some(raw) => Some(Endpoint::parse(&raw)?),
        None => None,
    };

    if flags.no_sign_request {
        return Ok(Settings {
            credentials: None,
            region,
            endpoint,
        });
    }

    // An explicit `--profile` outranks keys in the environment, as in the AWS
    // CLI: someone who typed a profile name meant that profile. `AWS_PROFILE`
    // does not, which is botocore's rule too.
    let env_allowed = flags.profile.is_none();
    if env_allowed {
        if let Some(credentials) = from_env(&var)? {
            return Ok(Settings {
                credentials: Some(Provider::Static(credentials)),
                region,
                endpoint,
            });
        }
    }

    let files = Files {
        credentials: read_ini(credentials_file.as_deref())?,
        config: config_ini,
        described: describe(&[credentials_file.as_deref(), config_file.as_deref()]),
        config_described: describe(&[config_file.as_deref()]),
    };
    let chain = Chain {
        files: &files,
        var: &var,
        region: &region,
        home: home.as_deref(),
    };
    let provider = chain.default_chain(&profile, env_allowed, profile_was_named)?;
    Ok(Settings {
        credentials: Some(provider),
        region,
        endpoint,
    })
}

fn check_region(region: &str) -> Result<()> {
    if region.is_empty()
        || region.len() > 64
        || !region
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        bail!("{region:?} is not a region name");
    }
    Ok(())
}

/// The config file writes every profile but the default as `[profile x]`.
fn config_section_name(profile: &str) -> String {
    match profile {
        "default" => "default".to_string(),
        name => format!("profile {name}"),
    }
}

/// Both files, read.
struct Files {
    config: Ini,
    credentials: Ini,
    /// "~/.aws/credentials or ~/.aws/config", for messages.
    described: String,
    config_described: String,
}

/// A profile as botocore sees it: its section in the config file with the
/// credentials file's section of the same name laid over it, key by key.
struct Profile<'a> {
    config: Option<&'a Section>,
    credentials: Option<&'a Section>,
}

impl Files {
    fn profile(&self, name: &str) -> Option<Profile<'_>> {
        let profile = Profile {
            config: self.config.section(&config_section_name(name)),
            credentials: self.credentials.section(name),
        };
        (profile.config.is_some() || profile.credentials.is_some()).then_some(profile)
    }
}

impl Profile<'_> {
    fn get(&self, key: &str) -> Option<String> {
        self.credentials
            .and_then(|s| s.get(key))
            .or_else(|| self.config.and_then(|s| s.get(key)))
    }

    fn has(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// botocore's test for "this profile holds keys of its own", which
    /// decides whether a source profile is used for its keys or for its role.
    fn has_static_keys(&self) -> bool {
        [
            "aws_access_key_id",
            "aws_secret_access_key",
            "aws_session_token",
        ]
        .iter()
        .any(|k| self.has(k))
    }

    /// A role this profile assumes itself — not one it names for a web
    /// identity token, which the web identity step handles unsigned.
    fn assumes_a_role(&self) -> bool {
        self.has("role_arn") && !self.has("web_identity_token_file")
    }
}

/// The default chain and the pieces of it a role reuses for its source.
struct Chain<'a> {
    files: &'a Files,
    var: &'a dyn Fn(&str) -> Option<String>,
    /// The region STS is asked in, as botocore asks it: the session's.
    region: &'a str,
    home: Option<&'a Path>,
}

impl Chain<'_> {
    fn default_chain(
        &self,
        name: &str,
        env_allowed: bool,
        profile_was_named: bool,
    ) -> Result<Provider> {
        let profile = self.files.profile(name);
        if profile_was_named && profile.is_none() {
            bail!("there is no profile {name:?} in {}", self.files.described);
        }
        if profile.as_ref().is_some_and(Profile::assumes_a_role) {
            return self.assume_role(name, &mut Vec::new());
        }
        if let Some(found) = self.profile_chain(name, env_allowed)? {
            return Ok(found);
        }
        if let Some(container) = self.container()? {
            return Ok(container);
        }
        let no_credentials = format!(
            "no S3 credentials: set AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY, configure \
             profile {name:?} in {}, or pass --no-sign-request for a public bucket",
            self.files.described
        );
        match self.instance_metadata(profile.as_ref(), Some(no_credentials.clone()))? {
            Some(metadata) => Ok(metadata),
            None => bail!("{no_credentials}"),
        }
    }

    /// Steps 3 to 7: what one profile can supply without assuming a role of
    /// its own. Also what a `source_profile` is read through, with the
    /// environment left out (botocore's `disable_env_vars`).
    fn profile_chain(&self, name: &str, env_allowed: bool) -> Result<Option<Provider>> {
        let profile = self.files.profile(name);
        if let Some(web) = self.web_identity(profile.as_ref(), env_allowed)? {
            return Ok(Some(web));
        }
        let Some(profile) = profile else {
            return Ok(None);
        };
        if let Some(sso) = self.sso(name, &profile)? {
            return Ok(Some(sso));
        }
        if let Some(keys) = static_keys(self.files.credentials.section(name), name)? {
            return Ok(Some(Provider::Static(keys)));
        }
        if let Some(command) = profile.get("credential_process") {
            return Ok(Some(Provider::Process(Process {
                profile: name.to_string(),
                command,
            })));
        }
        let config_section = self.files.config.section(&config_section_name(name));
        if let Some(keys) = static_keys(config_section, name)? {
            return Ok(Some(Provider::Static(keys)));
        }
        Ok(None)
    }

    /// `role_arn` with `source_profile` or `credential_source`, following a
    /// chain of source profiles to the first one with keys of its own.
    fn assume_role(&self, name: &str, visited: &mut Vec<String>) -> Result<Provider> {
        visited.push(name.to_string());
        let profile = self
            .files
            .profile(name)
            .with_context(|| format!("there is no profile {name:?} in {}", self.files.described))?;
        let role_arn = profile.get("role_arn").unwrap_or_default();
        if profile.has("mfa_serial") {
            bail!(
                "profile {name:?} assumes its role with an MFA code (mfa_serial), which spacetrace \
                 does not prompt for. Export keys for it first: \
                 eval \"$(aws configure export-credentials --profile {name} --format env)\""
            );
        }
        let source = match (
            profile.get("source_profile"),
            profile.get("credential_source"),
        ) {
            (Some(_), Some(_)) => bail!(
                "profile {name:?} has both source_profile and credential_source; a role takes one"
            ),
            (None, None) => bail!(
                "profile {name:?} has a role_arn and neither a source_profile nor a \
                 credential_source to assume it with"
            ),
            (None, Some(source)) => self.credential_source(name, &source)?,
            (Some(source), None) => self.source_profile(name, &source, visited)?,
        };
        let duration_seconds = match profile.get("duration_seconds") {
            Some(raw) => Some(raw.trim().parse::<u32>().ok().with_context(|| {
                format!("duration_seconds in profile {name:?} is not a number of seconds: {raw:?}")
            })?),
            None => None,
        };
        Ok(Provider::AssumeRole(Box::new(AssumeRole {
            profile: name.to_string(),
            source,
            role_arn,
            session_name: profile.get("role_session_name"),
            external_id: profile.get("external_id"),
            duration_seconds,
            sts: self.sts()?,
        })))
    }

    fn source_profile(
        &self,
        parent: &str,
        source: &str,
        visited: &mut Vec<String>,
    ) -> Result<Provider> {
        let Some(profile) = self.files.profile(source) else {
            bail!(
                "the source_profile {source:?} of profile {parent:?} does not exist in {}",
                self.files.described
            );
        };
        if visited.iter().any(|v| v == source) {
            // A profile may name itself, to hold its keys and its role in
            // one place; anything else seen twice is a loop.
            if source != parent || !profile.has_static_keys() {
                bail!(
                    "the source profiles of {:?} go round in a loop: {} -> {source}",
                    visited[0],
                    visited.join(" -> ")
                );
            }
        }
        if profile.has_static_keys() || !profile.assumes_a_role() {
            return self.profile_chain(source, false)?.with_context(|| {
                format!("the source_profile {source:?} of profile {parent:?} has no credentials")
            });
        }
        self.assume_role(source, visited)
    }

    fn credential_source(&self, name: &str, source: &str) -> Result<Provider> {
        let missing = |what: &str| format!("profile {name:?} takes its credentials from {what}");
        match source {
            "Environment" => from_env(self.var)?.map(Provider::Static).with_context(|| {
                missing(
                    "the environment, and AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY are not set",
                )
            }),
            "EcsContainer" => self.container()?.with_context(|| {
                missing(
                    "the container, and neither AWS_CONTAINER_CREDENTIALS_RELATIVE_URI nor \
                     AWS_CONTAINER_CREDENTIALS_FULL_URI is set",
                )
            }),
            "Ec2InstanceMetadata" => self
                .instance_metadata(self.files.profile(name).as_ref(), None)?
                .with_context(|| {
                    missing("instance metadata, and AWS_EC2_METADATA_DISABLED is true")
                }),
            other => bail!(
                "credential_source {other:?} in profile {name:?} is not one of Environment, \
                 Ec2InstanceMetadata and EcsContainer"
            ),
        }
    }

    /// The environment's token file and role, or the profile's.
    fn web_identity(
        &self,
        profile: Option<&Profile<'_>>,
        env_allowed: bool,
    ) -> Result<Option<Provider>> {
        let setting = |env: &str, key: &str| {
            env_allowed
                .then(|| (self.var)(env))
                .flatten()
                .or_else(|| profile.and_then(|p| p.get(key)))
        };
        let Some(token_file) = setting("AWS_WEB_IDENTITY_TOKEN_FILE", "web_identity_token_file")
        else {
            return Ok(None);
        };
        let role_arn = setting("AWS_ROLE_ARN", "role_arn").context(
            "a web identity token file is configured and no role to assume with it: set role_arn \
             in the profile or AWS_ROLE_ARN",
        )?;
        Ok(Some(Provider::WebIdentity(WebIdentity {
            token_file: PathBuf::from(token_file),
            role_arn,
            session_name: setting("AWS_ROLE_SESSION_NAME", "role_session_name"),
            sts: self.sts()?,
        })))
    }

    /// IAM Identity Center, either through an `[sso-session]` section or the
    /// legacy keys on the profile itself.
    fn sso(&self, name: &str, profile: &Profile<'_>) -> Result<Option<Provider>> {
        const REQUIRED: [&str; 4] = [
            "sso_start_url",
            "sso_region",
            "sso_role_name",
            "sso_account_id",
        ];
        // A profile with only `sso_session` is a token for other tools, not
        // role credentials; botocore passes it by too.
        if REQUIRED.iter().all(|k| !profile.has(k)) {
            return Ok(None);
        }
        let mut resolved: Vec<(String, String)> = REQUIRED
            .iter()
            .filter_map(|k| profile.get(k).map(|v| (k.to_string(), v)))
            .collect();
        let session = profile.get("sso_session");
        if let Some(session) = &session {
            let section = self
                .files
                .config
                .section(&format!("sso-session {session}"))
                .with_context(|| {
                    format!(
                        "profile {name:?} names sso_session {session:?}, and there is no \
                         [sso-session {session}] in {}",
                        self.files.config_described
                    )
                })?;
            for (key, value) in section.entries() {
                if profile.get(key).is_some_and(|mine| mine != value) {
                    bail!(
                        "profile {name:?} and [sso-session {session}] give different values for {key}"
                    );
                }
                resolved.retain(|(k, _)| k != key);
                resolved.push((key.to_string(), value.to_string()));
            }
        }
        let get = |key: &str| {
            resolved
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
        };
        let missing: Vec<&str> = REQUIRED
            .iter()
            .copied()
            .filter(|k| get(k).is_none())
            .collect();
        if !missing.is_empty() {
            bail!(
                "profile {name:?} is set up for single sign-on and lacks {}",
                missing.join(", ")
            );
        }
        let (Some(start_url), Some(sso_region), Some(role_name), Some(account_id)) = (
            get("sso_start_url"),
            get("sso_region"),
            get("sso_role_name"),
            get("sso_account_id"),
        ) else {
            unreachable!("every required key was just checked");
        };
        check_region(&sso_region)?;
        // botocore's cache key: the session's name when there is a session,
        // the start URL for a legacy profile.
        let key = session.as_deref().unwrap_or(&start_url);
        let home = self
            .home
            .context("there is no home directory to find ~/.aws/sso/cache in")?;
        let cache_file = home
            .join(".aws")
            .join("sso")
            .join("cache")
            .join(format!("{}.json", sigv4::sha1_hex(key.as_bytes())));
        let portal = self.service_endpoint(
            "AWS_ENDPOINT_URL_SSO",
            &format!("https://portal.sso.{sso_region}.amazonaws.com"),
        )?;
        Ok(Some(Provider::Sso(Sso {
            profile: name.to_string(),
            cache_file,
            account_id,
            role_name,
            portal,
        })))
    }

    fn container(&self) -> Result<Option<Provider>> {
        let url = match (
            (self.var)("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI"),
            (self.var)("AWS_CONTAINER_CREDENTIALS_FULL_URI"),
        ) {
            (Some(relative), _) => {
                // Anything but a path after the address would turn the
                // address into userinfo and name another host.
                if !relative.starts_with('/') {
                    bail!(
                        "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI does not start with /: {relative:?}"
                    );
                }
                let url = format!("http://169.254.170.2{relative}");
                check_container_url("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI", &url)?;
                url
            }
            (None, Some(full)) => {
                check_container_url("AWS_CONTAINER_CREDENTIALS_FULL_URI", &full)?;
                full
            }
            (None, None) => return Ok(None),
        };
        let token = (self.var)("AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE")
            .map(|path| ContainerToken::File(PathBuf::from(path)))
            .or_else(|| (self.var)("AWS_CONTAINER_AUTHORIZATION_TOKEN").map(ContainerToken::Value));
        Ok(Some(Provider::Container(Container { url, token })))
    }

    /// `None` when switched off. Settings come from the environment, then
    /// the profile, as botocore reads them.
    fn instance_metadata(
        &self,
        profile: Option<&Profile<'_>>,
        last_resort: Option<String>,
    ) -> Result<Option<Provider>> {
        let disabled = (self.var)("AWS_EC2_METADATA_DISABLED")
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("true"));
        if disabled {
            return Ok(None);
        }
        let setting =
            |env: &str, key: &str| (self.var)(env).or_else(|| profile.and_then(|p| p.get(key)));
        let endpoint = match setting(
            "AWS_EC2_METADATA_SERVICE_ENDPOINT",
            "ec2_metadata_service_endpoint",
        ) {
            Some(raw) => {
                let url = reqwest::Url::parse(raw.trim())
                    .ok()
                    .filter(|u| matches!(u.scheme(), "http" | "https"))
                    .with_context(|| {
                        format!("the instance metadata endpoint {raw:?} is not an http URL")
                    })?;
                url.as_str().trim_end_matches('/').to_string()
            }
            None => match setting(
                "AWS_EC2_METADATA_SERVICE_ENDPOINT_MODE",
                "ec2_metadata_service_endpoint_mode",
            )
            .map(|m| m.trim().to_ascii_lowercase())
            .as_deref()
            {
                None | Some("ipv4") => "http://169.254.169.254".to_string(),
                Some("ipv6") => "http://[fd00:ec2::254]".to_string(),
                Some(other) => {
                    bail!("the instance metadata endpoint mode {other:?} is neither IPv4 nor IPv6")
                }
            },
        };
        let timeout = match setting("AWS_METADATA_SERVICE_TIMEOUT", "metadata_service_timeout") {
            Some(raw) => raw
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|s| s.is_finite() && *s > 0.0 && *s <= 3600.0)
                .map(Duration::from_secs_f64)
                .with_context(|| {
                    format!("metadata_service_timeout {raw:?} is not a number of seconds")
                })?,
            None => Duration::from_secs(1),
        };
        let attempts = match setting(
            "AWS_METADATA_SERVICE_NUM_ATTEMPTS",
            "metadata_service_num_attempts",
        ) {
            Some(raw) => raw
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|n| (1..=100).contains(n))
                .with_context(|| format!("metadata_service_num_attempts {raw:?} is not a count"))?,
            None => 1,
        };
        let v1_disabled = setting("AWS_EC2_METADATA_V1_DISABLED", "ec2_metadata_v1_disabled")
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("true"));
        Ok(Some(Provider::InstanceMetadata(InstanceMetadata {
            endpoint,
            timeout,
            attempts,
            v1_disabled,
            last_resort,
        })))
    }

    /// STS in the session's region, regional endpoint, unless the
    /// environment points it elsewhere — `AWS_ENDPOINT_URL` included, which
    /// is how a MinIO set up through the environment gets its STS calls.
    fn sts(&self) -> Result<Sts> {
        Ok(Sts {
            endpoint: self.service_endpoint(
                "AWS_ENDPOINT_URL_STS",
                &format!("https://sts.{}.amazonaws.com", self.region),
            )?,
            region: self.region.to_string(),
        })
    }

    /// A service's endpoint: its own variable, then `AWS_ENDPOINT_URL`, then
    /// AWS' address for it — botocore's order for configured endpoints.
    fn service_endpoint(&self, own: &str, aws: &str) -> Result<Endpoint> {
        let (raw, from) = match ((self.var)(own), (self.var)("AWS_ENDPOINT_URL")) {
            (Some(raw), _) => (raw, own),
            (None, Some(raw)) => (raw, "AWS_ENDPOINT_URL"),
            (None, None) => return Endpoint::parse(aws),
        };
        Endpoint::parse(raw.trim().trim_end_matches('/')).with_context(|| format!("reading {from}"))
    }
}

/// botocore lets `AWS_CONTAINER_CREDENTIALS_FULL_URI` name a loopback address
/// or one of the hosts the ECS and EKS agents listen on, and nothing else:
/// an arbitrary URL would hand the authorization token to whoever owns it.
///
/// `localhost` is taken by name, as botocore takes it, rather than resolved:
/// a resolver that sends it elsewhere belongs to whoever owns the machine.
fn check_container_url(var: &str, raw: &str) -> Result<()> {
    let url = reqwest::Url::parse(raw)
        .ok()
        .filter(|u| matches!(u.scheme(), "http" | "https"))
        .with_context(|| format!("{var} is not an http URL: {raw:?}"))?;
    let host = url.host_str().unwrap_or("");
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let allowed = match bare.parse::<std::net::IpAddr>() {
        Ok(ip) => {
            ip.is_loopback()
                || ["169.254.170.2", "169.254.170.23", "fd00:ec2::23"]
                    .iter()
                    .any(|a| a.parse::<std::net::IpAddr>().ok() == Some(ip))
        }
        Err(_) => bare.eq_ignore_ascii_case("localhost"),
    };
    if !allowed {
        bail!(
            "{var} may name a loopback address, 169.254.170.2, 169.254.170.23 or \
             fd00:ec2::23, not {host}"
        );
    }
    Ok(())
}

fn from_env(var: &dyn Fn(&str) -> Option<String>) -> Result<Option<Credentials>> {
    match (var("AWS_ACCESS_KEY_ID"), var("AWS_SECRET_ACCESS_KEY")) {
        (Some(id), Some(secret)) => Ok(Some(Credentials {
            access_key_id: id,
            secret_access_key: secret,
            session_token: var("AWS_SESSION_TOKEN"),
        })),
        (None, None) => Ok(None),
        // Half a pair is a mistake worth stopping on. Falling through to a
        // profile would sign with keys the user did not mean to use.
        (Some(_), None) => bail!("AWS_ACCESS_KEY_ID is set and AWS_SECRET_ACCESS_KEY is not"),
        (None, Some(_)) => bail!("AWS_SECRET_ACCESS_KEY is set and AWS_ACCESS_KEY_ID is not"),
    }
}

/// Keys in one section. An id without its secret is an error, as in
/// botocore: it is a half-finished profile, not one that has no keys.
fn static_keys(section: Option<&Section>, profile: &str) -> Result<Option<Credentials>> {
    let Some(section) = section else {
        return Ok(None);
    };
    let Some(access_key_id) = section.get("aws_access_key_id") else {
        return Ok(None);
    };
    let secret_access_key = section.get("aws_secret_access_key").with_context(|| {
        format!("profile {profile:?} has an aws_access_key_id and no aws_secret_access_key")
    })?;
    Ok(Some(Credentials {
        access_key_id,
        secret_access_key,
        session_token: section.get("aws_session_token"),
    }))
}

fn describe(paths: &[Option<&Path>]) -> String {
    let named: Vec<String> = paths
        .iter()
        .flatten()
        .map(|p| p.display().to_string())
        .collect();
    match named.is_empty() {
        true => "~/.aws (no home directory found)".to_string(),
        false => named.join(" or "),
    }
}
// ------------------------------------------------------------------ INI

#[derive(Debug, Default)]
struct Ini {
    sections: Vec<(String, Section)>,
}

#[derive(Debug, Default)]
struct Section {
    values: Vec<(String, String)>,
}

impl Ini {
    fn section(&self, name: &str) -> Option<&Section> {
        // The last one wins, as in botocore, should a file repeat a section.
        self.sections
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, s)| s)
    }
}

impl Section {
    fn get(&self, key: &str) -> Option<String> {
        self.values
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .filter(|v| !v.is_empty())
    }

    /// Every key once, with the value `get` gives it.
    fn entries(&self) -> impl Iterator<Item = (&str, &str)> {
        self.values
            .iter()
            .enumerate()
            .filter(|(i, (key, value))| {
                !value.is_empty() && !self.values[i + 1..].iter().any(|(k, _)| k == key)
            })
            .map(|(_, (key, value))| (key.as_str(), value.as_str()))
    }
}

/// A missing file is an empty one: most machines have one of the two, or
/// neither. An unreadable file is an error, because silently skipping it
/// would report "no credentials" about a file that has them.
fn read_ini(path: Option<&Path>) -> Result<Ini> {
    let Some(path) = path else {
        return Ok(Ini::default());
    };
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(parse_ini(&text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Ini::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// The subset of INI the AWS files use: `[section]`, `key = value`, `#` and
/// `;` comments on their own line, and indented lines — the nested `s3 =`
/// blocks of the config file — which belong to the key above them and are
/// skipped, since nothing here reads them.
/// The name of a section whose header could not be read. Not a string a
/// profile name can be: it holds a NUL.
const UNNAMEABLE: &str = "\0unreadable header";

fn parse_ini(text: &str) -> Ini {
    let mut ini = Ini::default();
    for line in text.lines() {
        if line.starts_with([' ', '\t']) {
            continue;
        }
        let line = line.trim();
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        if let Some(header) = line.strip_prefix('[') {
            // Up to the last `]`, as Python's configparser (and so botocore)
            // reads it, which leaves `[work]  ; prod` named `work`. A header
            // with no `]` still opens a section, one no lookup can name:
            // otherwise the keys under it would join the section above and a
            // plain run would sign with them.
            let name = match header.rfind(']') {
                Some(end) => header[..end]
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
                None => UNNAMEABLE.to_string(),
            };
            ini.sections.push((name, Section::default()));
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if let Some((_, section)) = ini.sections.last_mut() {
            section
                .values
                .push((key.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    ini
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A home directory of its own and an environment of its own, so nothing
    /// on the developer's machine leaks in. Instance metadata starts switched
    /// off, so a test that means to reach the end of the chain says so.
    struct World {
        home: tempfile::TempDir,
        env: HashMap<String, String>,
    }

    impl World {
        fn new() -> Self {
            let home = tempfile::tempdir().unwrap();
            std::fs::create_dir(home.path().join(".aws")).unwrap();
            let mut env = HashMap::new();
            env.insert("HOME".into(), home.path().display().to_string());
            env.insert("AWS_EC2_METADATA_DISABLED".into(), "true".into());
            World { home, env }
        }
        fn set(mut self, k: &str, v: &str) -> Self {
            self.env.insert(k.into(), v.into());
            self
        }
        fn unset(mut self, k: &str) -> Self {
            self.env.remove(k);
            self
        }
        fn file(self, name: &str, text: &str) -> Self {
            std::fs::write(self.home.path().join(".aws").join(name), text).unwrap();
            self
        }
        fn resolve(&self, flags: &Flags) -> Result<Settings> {
            resolve(flags, &|k| self.env.get(k).cloned())
        }
        fn provider(&self, profile: Option<&str>) -> Provider {
            let flags = Flags {
                profile: profile.map(str::to_string),
                ..Flags::default()
            };
            self.resolve(&flags)
                .unwrap()
                .credentials
                .expect("a credentials source")
        }
        fn refusal(&self, profile: Option<&str>) -> String {
            let flags = Flags {
                profile: profile.map(str::to_string),
                ..Flags::default()
            };
            format!("{:#}", self.resolve(&flags).unwrap_err())
        }
    }

    fn static_of(p: &Provider) -> (String, String, Option<String>) {
        let Provider::Static(c) = p else {
            panic!("static keys expected, got {p:?}");
        };
        (
            c.access_key_id.clone(),
            c.secret_access_key.clone(),
            c.session_token.clone(),
        )
    }

    fn keys(s: &Settings) -> (String, String, Option<String>) {
        static_of(s.credentials.as_ref().expect("credentials"))
    }

    fn role_of(p: &Provider) -> &AssumeRole {
        let Provider::AssumeRole(role) = p else {
            panic!("an assumed role expected, got {p:?}");
        };
        role
    }
    const FILES: &str = "[default]\naws_access_key_id = DEFAULTID\naws_secret_access_key = defaultsecret\n\n\
                         [work]\naws_access_key_id=WORKID\naws_secret_access_key=worksecret\naws_session_token = tok\n";
    const CONFIG: &str = "[default]\nregion = eu-central-1\n\n[profile work]\nregion = ap-south-1\ns3 =\n  addressing_style = path\n";

    #[test]
    fn environment_keys_come_first() {
        let world = World::new()
            .file("credentials", FILES)
            .set("AWS_ACCESS_KEY_ID", "ENVID")
            .set("AWS_SECRET_ACCESS_KEY", "envsecret")
            .set("AWS_SESSION_TOKEN", "envtoken");
        let s = world.resolve(&Flags::default()).unwrap();
        assert_eq!(
            keys(&s),
            ("ENVID".into(), "envsecret".into(), Some("envtoken".into()))
        );
        assert_eq!(s.region, DEFAULT_REGION);
        assert!(s.endpoint.is_none());
    }

    #[test]
    fn an_explicit_profile_outranks_the_environment() {
        let world = World::new()
            .file("credentials", FILES)
            .file("config", CONFIG)
            .set("AWS_ACCESS_KEY_ID", "ENVID")
            .set("AWS_SECRET_ACCESS_KEY", "envsecret");
        let flags = Flags {
            profile: Some("work".into()),
            ..Flags::default()
        };
        let s = world.resolve(&flags).unwrap();
        assert_eq!(
            keys(&s),
            ("WORKID".into(), "worksecret".into(), Some("tok".into()))
        );
        assert_eq!(
            s.region, "ap-south-1",
            "the profile's region, under [profile work]"
        );
    }

    #[test]
    fn the_default_profile_and_its_region_are_used_without_any_flag() {
        let world = World::new()
            .file("credentials", FILES)
            .file("config", CONFIG);
        let s = world.resolve(&Flags::default()).unwrap();
        assert_eq!(keys(&s).0, "DEFAULTID");
        assert_eq!(s.region, "eu-central-1");
    }

    /// The region ends up in an AWS host name; one that could steer a signed
    /// request elsewhere is refused wherever it came from.
    #[test]
    fn a_region_that_is_not_a_name_is_refused() {
        let world = World::new().set("AWS_REGION", "evil.example.com/x");
        let flags = Flags {
            no_sign_request: true,
            ..Flags::default()
        };
        let err = world.resolve(&flags).unwrap_err();
        assert!(err.to_string().contains("not a region name"), "{err}");
    }

    #[test]
    fn aws_profile_selects_a_profile_and_region_flags_outrank_files() {
        let world = World::new()
            .file("credentials", FILES)
            .file("config", CONFIG)
            .set("AWS_PROFILE", "work")
            .set("AWS_DEFAULT_REGION", "sa-east-1");
        let s = world.resolve(&Flags::default()).unwrap();
        assert_eq!(keys(&s).0, "WORKID");
        assert_eq!(s.region, "sa-east-1");
        let world = world.set("AWS_REGION", "us-west-2");
        assert_eq!(
            world.resolve(&Flags::default()).unwrap().region,
            "us-west-2"
        );
        let flags = Flags {
            region: Some("eu-north-1".into()),
            ..Flags::default()
        };
        assert_eq!(world.resolve(&flags).unwrap().region, "eu-north-1");
    }

    #[test]
    fn keys_may_live_in_the_config_file_too() {
        let world = World::new().file(
            "config",
            "[profile cfg]\naws_access_key_id = CFGID\naws_secret_access_key = cfgsecret\n",
        );
        let flags = Flags {
            profile: Some("cfg".into()),
            ..Flags::default()
        };
        assert_eq!(keys(&world.resolve(&flags).unwrap()).0, "CFGID");
    }

    #[test]
    fn no_sign_request_needs_no_credentials_at_all() {
        let flags = Flags {
            no_sign_request: true,
            ..Flags::default()
        };
        let s = World::new().resolve(&flags).unwrap();
        assert!(s.credentials.is_none());
    }

    /// The point of the whole module's error handling: every failure says what
    /// to do, and none of them prints a secret it read on the way.
    #[test]
    fn failures_name_the_way_out_and_never_the_secret() {
        let message =
            |world: &World, flags: &Flags| format!("{:#}", world.resolve(flags).unwrap_err());

        let none = message(&World::new(), &Flags::default());
        assert!(
            none.contains("AWS_ACCESS_KEY_ID") && none.contains("--no-sign-request"),
            "{none}"
        );

        let half = World::new().set("AWS_ACCESS_KEY_ID", "ENVID");
        assert!(message(&half, &Flags::default()).contains("AWS_SECRET_ACCESS_KEY is not"));

        let missing = World::new().file("credentials", FILES);
        let flags = Flags {
            profile: Some("nosuch".into()),
            ..Flags::default()
        };
        assert!(message(&missing, &flags).contains("no profile \"nosuch\""));

        let role = World::new()
            .file("credentials", "[r]\naws_secret_access_key = rolesecret\n")
            .file("config", "[profile r]\nrole_arn = arn:aws:iam::1:role/x\n");
        let text = role.refusal(Some("r"));
        assert!(
            text.contains("neither a source_profile nor a credential_source"),
            "{text}"
        );
        assert!(!text.contains("rolesecret"), "{text}");

        let half_profile = World::new().file("credentials", "[h]\naws_access_key_id = HALFID\n");
        let text = half_profile.refusal(Some("h"));
        assert!(text.contains("no aws_secret_access_key"), "{text}");

        let mfa = World::new().file(
            "config",
            "[profile m]\nrole_arn = arn:aws:iam::1:role/x\nsource_profile = default\n\
             mfa_serial = arn:aws:iam::1:mfa/me\n[default]\naws_access_key_id = A\n\
             aws_secret_access_key = mfasecret\n",
        );
        let text = mfa.refusal(Some("m"));
        assert!(
            text.contains("mfa_serial") && text.contains("export-credentials"),
            "{text}"
        );
        assert!(!text.contains("mfasecret"), "{text}");
    }

    // ------------------------------------------------- the chain, in order

    const ROLE: &str = "arn:aws:iam::123456789012:role/reader";

    /// Step 2 comes before step 5: a profile with a role assumes it, even
    /// when keys sit beside it — they are what `source_profile = self` is for.
    #[test]
    fn a_role_outranks_keys_in_the_same_profile() {
        let world = World::new()
            .file(
                "credentials",
                "[base]\naws_access_key_id = BASEID\naws_secret_access_key = basesecret\n\
                 [both]\naws_access_key_id = BOTHID\naws_secret_access_key = bothsecret\n",
            )
            .file(
                "config",
                &format!(
                    "[profile both]\nrole_arn = {ROLE}\nsource_profile = base\nrole_session_name = me\n\
                     external_id = ext-1\nduration_seconds = 1800\nregion = eu-west-1\n"
                ),
            );
        let provider = world.provider(Some("both"));
        let role = role_of(&provider);
        assert_eq!(role.role_arn, ROLE);
        assert_eq!(static_of(&role.source).0, "BASEID");
        assert_eq!(role.session_name.as_deref(), Some("me"));
        assert_eq!(role.external_id.as_deref(), Some("ext-1"));
        assert_eq!(role.duration_seconds, Some(1800));
        assert_eq!(
            (
                role.sts.endpoint.authority.as_str(),
                role.sts.region.as_str()
            ),
            ("sts.eu-west-1.amazonaws.com", "eu-west-1"),
            "the regional endpoint of the session's region"
        );

        let selfish = World::new().file(
            "credentials",
            &format!(
                "[me]\naws_access_key_id = MEID\naws_secret_access_key = mesecret\n\
                 role_arn = {ROLE}\nsource_profile = me\n"
            ),
        );
        let provider = selfish.provider(Some("me"));
        assert_eq!(static_of(&role_of(&provider).source).0, "MEID");
    }

    /// A source profile with a role of its own assumes that role first, and
    /// one with keys stops the chain there even if it also has a role.
    #[test]
    fn source_profiles_chain_until_one_has_keys() {
        let world = World::new().file(
            "config",
            &format!(
                "[profile a]\nrole_arn = {ROLE}-a\nsource_profile = b\n\
                 [profile b]\nrole_arn = {ROLE}-b\nsource_profile = c\n\
                 [profile c]\naws_access_key_id = CID\naws_secret_access_key = csecret\n\
                 role_arn = {ROLE}-c\nsource_profile = nowhere\n"
            ),
        );
        let provider = world.provider(Some("a"));
        let a = role_of(&provider);
        let b = role_of(&a.source);
        assert_eq!(a.role_arn, format!("{ROLE}-a"));
        assert_eq!(b.role_arn, format!("{ROLE}-b"));
        assert_eq!(static_of(&b.source).0, "CID", "c's keys, not its role");
    }

    #[test]
    fn a_loop_of_source_profiles_is_refused() {
        let world = World::new().file(
            "config",
            &format!(
                "[profile a]\nrole_arn = {ROLE}\nsource_profile = b\n\
                 [profile b]\nrole_arn = {ROLE}\nsource_profile = a\n\
                 [profile s]\nrole_arn = {ROLE}\nsource_profile = s\n"
            ),
        );
        assert!(world.refusal(Some("a")).contains("loop: a -> b -> a"));
        assert!(
            world.refusal(Some("s")).contains("loop"),
            "naming itself without keys is a loop too"
        );
        let ghost = World::new().file(
            "config",
            &format!("[profile g]\nrole_arn = {ROLE}\nsource_profile = ghost\n"),
        );
        assert!(ghost
            .refusal(Some("g"))
            .contains("\"ghost\" of profile \"g\" does not exist"));
        let both = World::new().file(
            "config",
            &format!(
            "[profile x]\nrole_arn = {ROLE}\nsource_profile = x\ncredential_source = Environment\n"
        ),
        );
        assert!(both
            .refusal(Some("x"))
            .contains("both source_profile and credential_source"));
    }

    #[test]
    fn credential_sources_are_the_environment_and_the_metadata_services() {
        let config = format!(
            "[profile env]\nrole_arn = {ROLE}\ncredential_source = Environment\n\
             [profile ecs]\nrole_arn = {ROLE}\ncredential_source = EcsContainer\n\
             [profile ec2]\nrole_arn = {ROLE}\ncredential_source = Ec2InstanceMetadata\n\
             [profile odd]\nrole_arn = {ROLE}\ncredential_source = Laptop\n"
        );
        let world = World::new()
            .file("config", &config)
            .set("AWS_ACCESS_KEY_ID", "ENVID")
            .set("AWS_SECRET_ACCESS_KEY", "envsecret")
            .set(
                "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
                "/v2/credentials/abc",
            )
            .unset("AWS_EC2_METADATA_DISABLED");
        // `--profile` switches off the environment's keys as a step of their
        // own, not as a source a profile names.
        assert_eq!(
            static_of(&role_of(&world.provider(Some("env"))).source).0,
            "ENVID"
        );
        let provider = world.provider(Some("ecs"));
        let Provider::Container(c) = &role_of(&provider).source else {
            panic!("container credentials expected");
        };
        assert_eq!(c.url, "http://169.254.170.2/v2/credentials/abc");
        let provider = world.provider(Some("ec2"));
        let Provider::InstanceMetadata(m) = &role_of(&provider).source else {
            panic!("instance metadata expected");
        };
        assert_eq!(m.endpoint, "http://169.254.169.254");
        assert!(
            m.last_resort.is_none(),
            "a source a profile names is not a fallback"
        );
        assert!(world.refusal(Some("odd")).contains("\"Laptop\""));

        let bare = World::new().file("config", &config);
        assert!(bare.refusal(Some("env")).contains("AWS_ACCESS_KEY_ID"));
        assert!(bare
            .refusal(Some("ecs"))
            .contains("AWS_CONTAINER_CREDENTIALS"));
        assert!(bare
            .refusal(Some("ec2"))
            .contains("AWS_EC2_METADATA_DISABLED"));
    }

    /// Step 3 before step 5: on EKS the pod's token wins over a credentials
    /// file someone baked into the image. Under `--profile` the environment's
    /// token is not read, the profile's is.
    #[test]
    fn a_web_identity_token_outranks_the_credentials_file() {
        let world = World::new()
            .file("credentials", FILES)
            .file(
                "config",
                &format!(
                "[profile web]\nweb_identity_token_file = /profile/token\nrole_arn = {ROLE}-p\n"
            ),
            )
            .set("AWS_WEB_IDENTITY_TOKEN_FILE", "/env/token")
            .set("AWS_ROLE_ARN", ROLE)
            .set("AWS_ROLE_SESSION_NAME", "pod");
        let Provider::WebIdentity(web) = world.provider(None) else {
            panic!("web identity expected");
        };
        assert_eq!(web.token_file, PathBuf::from("/env/token"));
        assert_eq!(
            (web.role_arn.as_str(), web.session_name.as_deref()),
            (ROLE, Some("pod"))
        );

        assert_eq!(static_of(&world.provider(Some("work"))).0, "WORKID");
        let Provider::WebIdentity(web) = world.provider(Some("web")) else {
            panic!("the profile's web identity expected");
        };
        assert_eq!(web.token_file, PathBuf::from("/profile/token"));
        assert_eq!(web.role_arn, format!("{ROLE}-p"));

        let roleless = World::new().set("AWS_WEB_IDENTITY_TOKEN_FILE", "/env/token");
        assert!(roleless.refusal(None).contains("AWS_ROLE_ARN"));
    }

    #[test]
    fn sso_profiles_find_their_token_by_botocore_s_cache_key() {
        let world = World::new().file(
            "config",
            "[profile new]\nsso_session = corp\nsso_account_id = 111122223333\nsso_role_name = Reader\n\
             [sso-session corp]\nsso_start_url = https://corp.awsapps.com/start\nsso_region = eu-west-1\n\
             sso_registration_scopes = sso:account:access\n\
             [profile legacy]\nsso_start_url = https://corp.awsapps.com/start\nsso_region = eu-west-1\n\
             sso_account_id = 111122223333\nsso_role_name = Reader\n\
             [profile token-only]\nsso_session = corp\n",
        );
        let cache = world.home.path().join(".aws").join("sso").join("cache");
        let Provider::Sso(sso) = world.provider(Some("new")) else {
            panic!("single sign-on expected");
        };
        assert_eq!(
            sso.cache_file,
            cache.join(format!("{}.json", sigv4::sha1_hex(b"corp"))),
            "a session's token is filed under the session's name"
        );
        assert_eq!(
            (sso.account_id.as_str(), sso.role_name.as_str()),
            ("111122223333", "Reader")
        );
        assert_eq!(sso.portal.authority, "portal.sso.eu-west-1.amazonaws.com");

        let Provider::Sso(sso) = world.provider(Some("legacy")) else {
            panic!("single sign-on expected");
        };
        assert_eq!(
            sso.cache_file,
            cache.join(format!(
                "{}.json",
                sigv4::sha1_hex(b"https://corp.awsapps.com/start")
            ))
        );
        assert!(
            world
                .refusal(Some("token-only"))
                .contains("no S3 credentials"),
            "a profile with nothing but a session has no role to fetch"
        );
    }

    #[test]
    fn broken_sso_profiles_say_what_is_wrong() {
        let world = World::new().file(
            "config",
            "[profile ghost]\nsso_session = nope\nsso_account_id = 1\nsso_role_name = R\n\
             [profile partial]\nsso_start_url = https://x/start\nsso_account_id = 1\n\
             [profile clash]\nsso_session = corp\nsso_account_id = 1\nsso_role_name = R\n\
             sso_region = us-west-2\n\
             [sso-session corp]\nsso_start_url = https://corp/start\nsso_region = eu-west-1\n",
        );
        assert!(world
            .refusal(Some("ghost"))
            .contains("no [sso-session nope]"));
        let partial = world.refusal(Some("partial"));
        assert!(
            partial.contains("lacks sso_region, sso_role_name"),
            "{partial}"
        );
        assert!(world
            .refusal(Some("clash"))
            .contains("different values for sso_region"));
    }

    /// Steps 5, 6 and 7 among themselves.
    #[test]
    fn credentials_file_keys_then_the_process_then_config_keys() {
        let world = World::new()
            .file(
                "credentials",
                "[p]\naws_access_key_id = FILEID\naws_secret_access_key = s\n",
            )
            .file(
                "config",
                "[profile p]\ncredential_process = /bin/creds --for p\n\
                 [profile q]\ncredential_process = /bin/creds --for q\naws_access_key_id = CFGID\n\
                 aws_secret_access_key = s\n",
            );
        assert_eq!(static_of(&world.provider(Some("p"))).0, "FILEID");
        let Provider::Process(process) = world.provider(Some("q")) else {
            panic!("credential_process expected");
        };
        assert_eq!(process.command, "/bin/creds --for q");
    }

    #[test]
    fn container_credentials_follow_botocore_s_rules() {
        let container = |world: World| match world.provider(None) {
            Provider::Container(c) => c,
            other => panic!("container credentials expected, got {other:?}"),
        };
        let both = World::new()
            .set("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI", "/creds")
            .set(
                "AWS_CONTAINER_CREDENTIALS_FULL_URI",
                "http://127.0.0.1:1/full",
            );
        assert_eq!(
            container(both).url,
            "http://169.254.170.2/creds",
            "relative first"
        );
        for allowed in [
            "http://127.0.0.1:8080/c",
            "http://localhost/c",
            "http://[::1]:9/c",
            "http://169.254.170.23/v1/credentials",
            "http://[fd00:ec2::23]/v1/credentials",
            "https://127.0.0.2/c",
        ] {
            let world = World::new().set("AWS_CONTAINER_CREDENTIALS_FULL_URI", allowed);
            assert_eq!(container(world).url, allowed);
        }
        let world = World::new()
            .set(
                "AWS_CONTAINER_CREDENTIALS_FULL_URI",
                "http://evil.example.com/c",
            )
            .set("AWS_CONTAINER_AUTHORIZATION_TOKEN", "pod-token");
        let text = world.refusal(None);
        assert!(
            text.contains("evil.example.com") && !text.contains("pod-token"),
            "{text}"
        );

        // Appended to `http://169.254.170.2`, a value that does not start a
        // path makes the address userinfo and names another host — which
        // would be handed the token and trusted for the keys.
        for hostile in ["@evil.example/x", ".evil.example/x", ":80@evil.example/x"] {
            let world = World::new()
                .set("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI", hostile)
                .set("AWS_CONTAINER_AUTHORIZATION_TOKEN", "pod-token");
            let text = world.refusal(None);
            assert!(
                text.contains("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI")
                    && !text.contains("pod-token"),
                "{hostile}: {text}"
            );
        }

        let tokens = World::new()
            .set("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI", "/creds")
            .set("AWS_CONTAINER_AUTHORIZATION_TOKEN", "value")
            .set("AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE", "/var/run/token");
        assert!(matches!(
            container(tokens).token,
            Some(ContainerToken::File(ref p)) if p == Path::new("/var/run/token")
        ));
    }

    /// The end of the chain, and the switches botocore reads for it.
    #[test]
    fn instance_metadata_is_the_last_resort_unless_switched_off() {
        let world = World::new().unset("AWS_EC2_METADATA_DISABLED");
        let Provider::InstanceMetadata(m) = world.provider(None) else {
            panic!("instance metadata expected");
        };
        assert_eq!(m.endpoint, "http://169.254.169.254");
        assert_eq!(
            (m.timeout, m.attempts, m.v1_disabled),
            (Duration::from_secs(1), 1, false)
        );
        assert!(m
            .last_resort
            .as_deref()
            .is_some_and(|t| t.contains("no S3 credentials")));

        let tuned = World::new().unset("AWS_EC2_METADATA_DISABLED").file(
            "config",
            "[default]\nec2_metadata_service_endpoint_mode = IPv6\nmetadata_service_timeout = 2\n\
             metadata_service_num_attempts = 3\nec2_metadata_v1_disabled = true\n",
        );
        let Provider::InstanceMetadata(m) = tuned.provider(None) else {
            panic!("instance metadata expected");
        };
        assert_eq!(m.endpoint, "http://[fd00:ec2::254]");
        assert_eq!(
            (m.timeout, m.attempts, m.v1_disabled),
            (Duration::from_secs(2), 3, true)
        );
        let pointed = tuned.set(
            "AWS_EC2_METADATA_SERVICE_ENDPOINT",
            "http://127.0.0.1:1234/",
        );
        let Provider::InstanceMetadata(m) = pointed.provider(None) else {
            panic!("instance metadata expected");
        };
        assert_eq!(m.endpoint, "http://127.0.0.1:1234");

        let off = World::new().set("AWS_EC2_METADATA_DISABLED", "TRUE");
        assert!(off.refusal(None).contains("no S3 credentials"));
    }

    /// `AWS_ENDPOINT_URL` reaches STS and the SSO portal as well as S3, as it
    /// does in botocore; the service's own variable outranks it.
    #[test]
    fn configured_endpoints_reach_sts_and_the_sso_portal() {
        let world = World::new()
            .file(
                "config",
                &format!(
                    "[profile r]\nrole_arn = {ROLE}\ncredential_source = Environment\n\
                     [profile s]\nsso_start_url = https://x/start\nsso_region = eu-west-1\n\
                     sso_account_id = 1\nsso_role_name = R\n"
                ),
            )
            .set("AWS_ACCESS_KEY_ID", "ENVID")
            .set("AWS_SECRET_ACCESS_KEY", "envsecret")
            .set("AWS_ENDPOINT_URL", "http://127.0.0.1:9000");
        assert_eq!(
            role_of(&world.provider(Some("r"))).sts.endpoint.authority,
            "127.0.0.1:9000"
        );
        let Provider::Sso(sso) = world.provider(Some("s")) else {
            panic!("single sign-on expected");
        };
        assert_eq!(sso.portal.authority, "127.0.0.1:9000");
        let world = world
            .set("AWS_ENDPOINT_URL_STS", "http://127.0.0.1:9001/")
            .set("AWS_ENDPOINT_URL_SSO", "http://127.0.0.1:9002");
        assert_eq!(
            role_of(&world.provider(Some("r"))).sts.endpoint.authority,
            "127.0.0.1:9001"
        );
        let Provider::Sso(sso) = world.provider(Some("s")) else {
            panic!("single sign-on expected");
        };
        assert_eq!(sso.portal.authority, "127.0.0.1:9002");
    }
    #[test]
    fn endpoints_are_checked_and_reduced_to_an_authority() {
        let e = Endpoint::parse("http://127.0.0.1:9000").unwrap();
        assert_eq!(
            (e.scheme.as_str(), e.authority.as_str()),
            ("http", "127.0.0.1:9000")
        );
        let e = Endpoint::parse("https://ACCOUNT.r2.cloudflarestorage.com/").unwrap();
        assert_eq!(e.authority, "account.r2.cloudflarestorage.com");
        let e = Endpoint::parse("https://s3.example.com:443").unwrap();
        assert_eq!(
            e.authority, "s3.example.com",
            "the default port is not part of Host"
        );

        let refused = |raw: &str| format!("{:#}", Endpoint::parse(raw).unwrap_err());
        assert!(refused("ftp://x").contains("http"));
        assert!(refused("http://user:pw@x").contains("credentials"));
        assert!(!refused("http://user:pw@x").contains("pw"));
        assert!(refused("http://x/bucket").contains("bucket goes in"));
        assert!(refused("not a url").contains("not a URL"));
    }

    #[test]
    fn the_endpoint_can_come_from_the_environment() {
        let world = World::new()
            .set("AWS_ENDPOINT_URL", "http://generic:1")
            .set("AWS_ENDPOINT_URL_S3", "http://s3only:2");
        let flags = Flags {
            no_sign_request: true,
            ..Flags::default()
        };
        assert_eq!(
            world.resolve(&flags).unwrap().endpoint.unwrap().authority,
            "s3only:2"
        );
        let flags = Flags {
            endpoint: Some("http://flag:3".into()),
            ..flags
        };
        assert_eq!(
            world.resolve(&flags).unwrap().endpoint.unwrap().authority,
            "flag:3"
        );
    }

    /// A header with a comment after it must still open its own section. It
    /// used to fail to parse as a header at all, so `work`'s keys were
    /// appended to `[default]` above it and a plain `scan` signed with them.
    #[test]
    fn a_header_with_a_trailing_comment_still_opens_its_section() {
        let world = World::new().file(
            "credentials",
            "[default]\naws_access_key_id = DEFAULTID\naws_secret_access_key = defaultsecret\n\
             [work]  ; prod\naws_access_key_id = WORKID\naws_secret_access_key = worksecret\n\
             [profile x] # note\nregion = r\n",
        );
        let s = world.resolve(&Flags::default()).unwrap();
        assert_eq!(keys(&s).0, "DEFAULTID", "default keeps its own keys");
        let flags = Flags {
            profile: Some("work".into()),
            ..Flags::default()
        };
        assert_eq!(keys(&world.resolve(&flags).unwrap()).0, "WORKID");

        let ini = parse_ini("[profile x] # note\nregion = r\n");
        assert_eq!(
            ini.section("profile x").unwrap().get("region").as_deref(),
            Some("r")
        );
    }

    /// A header that cannot be read opens a section nobody can name, so the
    /// keys under it are lost rather than lent to the section above.
    #[test]
    fn keys_under_an_unreadable_header_join_no_section() {
        let ini = parse_ini(
            "[default]\naws_access_key_id = DEFAULTID\n[broken\naws_access_key_id = STRAY\n",
        );
        assert_eq!(
            ini.section("default")
                .unwrap()
                .get("aws_access_key_id")
                .as_deref(),
            Some("DEFAULTID")
        );
        assert_eq!(
            ini.section("default").unwrap().values.len(),
            1,
            "the stray key did not join [default]"
        );
        assert!(ini.section("[broken").is_none());
        assert!(ini.section("broken").is_none());
    }

    #[test]
    fn ini_comments_nesting_and_repeats_follow_the_aws_files() {
        let ini = parse_ini(
            "# comment\n; also\n[ profile   spaced ]\nkey = one\n  nested = skipped\nKEY=two\n\
             [dup]\na=1\n[dup]\na=2\nno equals sign here\n",
        );
        assert_eq!(
            ini.section("profile spaced").unwrap().get("key").as_deref(),
            Some("two")
        );
        assert_eq!(ini.section("profile spaced").unwrap().get("nested"), None);
        assert_eq!(ini.section("dup").unwrap().get("a").as_deref(), Some("2"));
    }
}
