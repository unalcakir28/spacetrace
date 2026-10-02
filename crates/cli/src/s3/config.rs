//! Where the credentials, the region and the endpoint come from.
//!
//! The order is the AWS CLI's, so that a machine already set up for `aws s3 ls`
//! needs nothing more: a flag, then the environment, then `~/.aws/credentials`
//! and `~/.aws/config`. Only static keys are read. Single sign-on, assumed
//! roles, `credential_process` and instance metadata are each a protocol of
//! their own; a profile that needs one is named as such, with the command that
//! turns it into keys, rather than being reported as "no credentials".
//!
//! The environment is passed in as a function, not read here, so the tests
//! can run in parallel without fighting over the process environment.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use super::sigv4::Credentials;

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
    pub credentials: Option<Credentials>,
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
    // The config file writes every profile but the default as `[profile x]`.
    let from_config = match profile.as_str() {
        "default" => config_ini.section("default"),
        name => config_ini.section(&format!("profile {name}")),
    };

    let region = flags
        .region
        .clone()
        .or_else(|| var("AWS_REGION"))
        .or_else(|| var("AWS_DEFAULT_REGION"))
        .or_else(|| from_config.and_then(|s| s.get("region")))
        .unwrap_or_else(|| DEFAULT_REGION.to_string());
    // On AWS the region becomes part of the host name a signed request goes
    // to, so it is held to what a region name can be.
    if region.is_empty()
        || region.len() > 64
        || !region
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        bail!("{region:?} is not a region name");
    }

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
    // CLI: someone who typed a profile name meant that profile.
    if flags.profile.is_none() {
        if let Some(credentials) = from_env(&var)? {
            return Ok(Settings {
                credentials: Some(credentials),
                region,
                endpoint,
            });
        }
    }

    let credentials_ini = read_ini(credentials_file.as_deref())?;
    let sources = [credentials_ini.section(&profile), from_config];
    if let Some(credentials) = sources.iter().flatten().find_map(|s| static_keys(s)) {
        return Ok(Settings {
            credentials: Some(credentials),
            region,
            endpoint,
        });
    }

    let files = describe(&[credentials_file.as_deref(), config_file.as_deref()]);
    if let Some(mechanism) = sources.iter().flatten().find_map(|s| unsupported(s)) {
        bail!(
            "profile {profile:?} gets its credentials through {mechanism}, which spacetrace does \
             not implement. Export keys for it first: \
             eval \"$(aws configure export-credentials --profile {profile} --format env)\""
        );
    }
    if profile_was_named && sources.iter().all(Option::is_none) {
        bail!("there is no profile {profile:?} in {files}");
    }
    bail!(
        "no S3 credentials: set AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY, put keys for \
         profile {profile:?} in {files}, or pass --no-sign-request for a public bucket"
    )
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

fn static_keys(section: &Section) -> Option<Credentials> {
    Some(Credentials {
        access_key_id: section.get("aws_access_key_id")?,
        secret_access_key: section.get("aws_secret_access_key")?,
        session_token: section.get("aws_session_token"),
    })
}

fn unsupported(section: &Section) -> Option<&'static str> {
    [
        ("sso_session", "single sign-on"),
        ("sso_start_url", "single sign-on"),
        ("role_arn", "an assumed role"),
        ("credential_process", "credential_process"),
        ("web_identity_token_file", "a web identity token"),
        ("credential_source", "instance or container metadata"),
    ]
    .into_iter()
    .find(|(key, _)| section.get(key).is_some())
    .map(|(_, what)| what)
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
    /// on the developer's machine leaks in.
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
            World { home, env }
        }
        fn set(mut self, k: &str, v: &str) -> Self {
            self.env.insert(k.into(), v.into());
            self
        }
        fn file(self, name: &str, text: &str) -> Self {
            std::fs::write(self.home.path().join(".aws").join(name), text).unwrap();
            self
        }
        fn resolve(&self, flags: &Flags) -> Result<Settings> {
            resolve(flags, &|k| self.env.get(k).cloned())
        }
    }

    fn keys(s: &Settings) -> (String, String, Option<String>) {
        let c = s.credentials.as_ref().expect("credentials");
        (
            c.access_key_id.clone(),
            c.secret_access_key.clone(),
            c.session_token.clone(),
        )
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

        let sso = World::new().file(
            "config",
            "[profile sso]\nsso_session = corp\nregion = eu-west-1\n\n[default]\nregion=x\n",
        );
        let flags = Flags {
            profile: Some("sso".into()),
            ..Flags::default()
        };
        let text = message(&sso, &flags);
        assert!(
            text.contains("single sign-on") && text.contains("export-credentials"),
            "{text}"
        );

        let role = World::new()
            .file("credentials", "[r]\naws_secret_access_key = rolesecret\n")
            .file("config", "[profile r]\nrole_arn = arn:aws:iam::1:role/x\n");
        let flags = Flags {
            profile: Some("r".into()),
            ..Flags::default()
        };
        let text = message(&role, &flags);
        assert!(text.contains("assumed role"), "{text}");
        assert!(!text.contains("rolesecret"), "{text}");
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
