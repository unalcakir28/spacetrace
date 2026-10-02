//! `spacetrace scan --ssh`: scan a machine that has nothing installed.
//!
//! A copy of this same version is uploaded into a private temporary directory
//! on the other side, runs one `scan --save` into a database next to itself,
//! and the database comes back over the same connection to be imported through
//! `Store::import_snapshot` — the path every pulled snapshot takes, digest
//! check included. Nothing is installed and nothing stays behind.
//!
//! **Why the system `ssh`.** It already knows the user's keys, agent, jump
//! hosts, `~/.ssh/config` and known hosts. An ssh crate would have to learn all
//! of that, and every gap would be a host that `ssh` reaches and this does not.
//!
//! **Why the remote directory is removed by the remote.** The session that
//! creates it (the *lease*) stays open for the whole run, reading a pipe that
//! only this process writes to, and removes the directory when that pipe
//! closes. It closes when the run is finished, when it fails, when it is
//! interrupted — and also when this process is killed outright or the network
//! drops, which no local `finally` can cover. The lease is the only cleanup
//! there is, and it removes the files this run put there by name and then the
//! directory with `rmdir`: it cannot delete anything it did not create, even
//! if every other check here were wrong. The path never comes back from this
//! side either; the lease holds its own copy of what `mktemp` printed.
//!
//! **Why one connection.** Five sessions would be five password prompts. The
//! first `ssh` becomes a ControlMaster (`-M -N -f`), authenticates once in the
//! foreground and then detaches into its own session; every later step is a
//! multiplexed client on its socket, which lives in a private local temporary
//! directory. Detaching matters for Ctrl-C: the master is out of the
//! terminal's process group, and the clients are put in their own, so the
//! signal reaches only this process, which can then close the lease and wait
//! for the remote to confirm the directory is gone. `ControlPersist` bounds
//! how long a master outlives a killed CLI; `-O exit` ends it otherwise.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{bail, Context, Result};

/// The ssh program to run, for a test or for a wrapper; `ssh` otherwise.
pub const SSH_ENV: &str = "SPACETRACE_SSH";

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// How often a wait checks whether the user asked to stop.
const POLL: Duration = Duration::from_millis(50);

/// What the remote directory holds, by name. The lease removes these and then
/// the directory, and nothing else: an `rm -rf` would be one wrong path away
/// from somebody's data.
const REMOTE_FILES: &str = "spacetrace scan.sqlite scan.sqlite-wal scan.sqlite-shm \
                            scan.sqlite-journal scan.pid probe";

/// Options this module sets itself. ssh takes the *first* value it sees for an
/// option, and user options come first so that they can override the defaults
/// below — which would let one of these break the protocol instead.
const RESERVED_OPTIONS: &[&str] = &[
    "ControlMaster",
    "ControlPath",
    "ControlPersist",
    "RemoteCommand",
    "RequestTTY",
    "SessionType",
    "StdinNull",
    "ForkAfterAuthentication",
    "BatchMode",
];

/// Everything `scan --ssh` needs to know.
pub struct Request<'a> {
    pub destination: &'a str,
    /// `KEY=VALUE` pairs, each passed as `-o KEY=VALUE`.
    pub options: &'a [String],
    /// A build to upload instead of the published release.
    pub binary: Option<&'a Path>,
    /// The path to scan, on the remote machine.
    pub path: &'a str,
    /// Arguments for the remote `scan`, already in `--flag=value` form.
    pub scan_args: Vec<String>,
    /// Whether to say what is happening on stderr and draw the remote progress;
    /// true only when stderr is a terminal.
    pub chatty: bool,
}

/// The snapshot database the remote wrote, now on this machine.
pub struct Fetched {
    pub file: PathBuf,
    _staging: tempfile::TempDir,
}

/// The user stopped the run. A type of its own so `main` can exit 130 rather
/// than 1, which is what a shell expects of an interrupted command.
#[derive(Debug)]
pub struct Interrupted;

impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("interrupted")
    }
}

impl std::error::Error for Interrupted {}

/// Scan `request.path` on `request.destination` and bring the snapshot back.
pub fn scan(request: &Request) -> Result<Fetched> {
    check_destination(request.destination)?;
    let user_options = user_options(request.options)?;
    let _signals = signals::Guard::install();

    let mut ssh = Ssh {
        program: std::env::var_os(SSH_ENV).unwrap_or_else(|| "ssh".into()),
        destination: request.destination.to_string(),
        options: user_options,
        control: None,
    };
    let result = ssh.start_master().and_then(|()| with_lease(&ssh, request));
    ssh.stop_master();
    result
}

fn with_lease(ssh: &Ssh, request: &Request) -> Result<Fetched> {
    let mut lease = Lease::spawn(ssh)?;
    let work = lease
        .wait_ready(ssh)
        .and_then(|remote| transfer_and_scan(ssh, request, &remote));
    let cleanup = lease.release();

    match (&cleanup, lease.dir.as_deref()) {
        (_, None) => {}
        (Cleanup::Removed, Some(dir)) => {
            // Said after a failure even when otherwise quiet: that is when
            // somebody wonders what was left on their server.
            if request.chatty || work.is_err() {
                eprintln!("Removed {dir} on {}.", ssh.destination);
            }
        }
        (Cleanup::Kept, Some(dir)) => eprintln!(
            "warning: {dir} on {} could not be removed. It holds only files this run \
             put there; remove it by hand.",
            ssh.destination
        ),
        (Cleanup::Unconfirmed, Some(dir)) => eprintln!(
            "warning: could not confirm that {dir} on {} was removed: the connection \
             ended first. The remote side removes it as soon as it notices the session \
             is gone.",
            ssh.destination
        ),
    }
    if work.as_ref().is_err_and(|e| e.is::<Interrupted>()) {
        eprintln!("Interrupted; nothing was imported.");
    }
    work
}

/// What the lease printed when it first came up.
struct RemoteInfo {
    /// `uname -sm`, e.g. `Linux aarch64`.
    platform: String,
    dir: String,
    /// Kilobytes available in `dir`, when `df` could say.
    free_kib: Option<u64>,
}

fn transfer_and_scan(ssh: &Ssh, request: &Request, remote: &RemoteInfo) -> Result<Fetched> {
    let binary = match request.binary {
        Some(path) => path.to_path_buf(),
        None => {
            let target = target_for_uname(&remote.platform).with_context(|| {
                format!(
                    "no spacetrace release is built for {:?}, which is what {} runs; \
                     build one for it and pass it with --binary",
                    remote.platform, ssh.destination
                )
            })?;
            binary_for(target, request.chatty)?
        }
    };
    let size = std::fs::metadata(&binary)
        .with_context(|| format!("cannot read {}", binary.display()))?
        .len();

    // Refused before the upload rather than discovered halfway through it.
    // The snapshot needs room too, and how much depends on what the scan
    // finds, so that case is left to the scan's own error, with the free
    // space added to the message when it is short enough to be the reason.
    if let Some(free) = remote.free_kib {
        anyhow::ensure!(
            free.saturating_mul(1024) > size + (1 << 20),
            "{} on {} has {} free; the spacetrace binary alone needs {}",
            remote.dir,
            ssh.destination,
            crate::fmt::size(free.saturating_mul(1024)),
            crate::fmt::size(size)
        );
    }

    if request.chatty {
        eprintln!(
            "Uploading {} ({}) to {}:{}…",
            binary.display(),
            crate::fmt::size(size),
            ssh.destination,
            remote.dir
        );
    }
    upload(ssh, &binary, &remote.dir)?;

    let mut args = vec![remote.dir.clone()];
    args.extend(request.scan_args.iter().cloned());
    // `--` so a path that starts with `-` is a path.
    args.push("--".into());
    args.push(request.path.to_string());
    // A terminal on the far side is what makes the remote draw its progress
    // line; it only pays when there is a terminal here to show it on.
    let tty = request.chatty;
    let status = run(ssh
        .command(SCAN_SCRIPT, &args, tty)
        .stdin(Stdio::null())
        .stdout(stderr_as_stdio()?));
    // A remote that dies mid-redraw never clears its progress line; end it
    // here, so what follows does not land on the end of it and the last count
    // stays readable.
    if tty && !matches!(status, Ok(s) if s.success()) {
        eprintln!();
    }
    let status = status?;
    if !status.success() {
        // Only when it could be the reason. 600,601 entries with one-to-four
        // character names made a 19.7 MB database (measured), so real names
        // put ten million entries somewhere near a gigabyte; above that the
        // number is noise beside the real error.
        let room = remote
            .free_kib
            .filter(|&kib| kib < 1 << 20)
            .map(|kib| {
                format!(
                    " ({} had {} free before the upload)",
                    remote.dir,
                    crate::fmt::size(kib.saturating_mul(1024))
                )
            })
            .unwrap_or_default();
        bail!(
            "the scan on {} failed ({}){room}",
            ssh.destination,
            describe(status)
        );
    }

    if request.chatty {
        eprintln!("Fetching the snapshot…");
    }
    let staging = tempfile::tempdir().context("creating a temporary directory for the snapshot")?;
    let file = staging.path().join("snapshot.sqlite");
    let out =
        std::fs::File::create(&file).with_context(|| format!("cannot write {}", file.display()))?;
    let status = run(ssh
        .command(DOWNLOAD_SCRIPT, std::slice::from_ref(&remote.dir), false)
        .stdin(Stdio::null())
        .stdout(out))?;
    anyhow::ensure!(
        status.success(),
        "fetching the snapshot from {} failed ({})",
        ssh.destination,
        describe(status)
    );
    Ok(Fetched {
        file,
        _staging: staging,
    })
}

fn upload(ssh: &Ssh, binary: &Path, dir: &str) -> Result<()> {
    let file =
        std::fs::File::open(binary).with_context(|| format!("cannot read {}", binary.display()))?;
    let mut child = ssh
        .command(UPLOAD_SCRIPT, &[dir.to_string()], false)
        .stdin(file)
        .stdout(Stdio::piped())
        .spawn()
        .with_context(|| ssh.cannot_run())?;
    let status = wait(&mut child)?;
    let mut said = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut said);
    }
    anyhow::ensure!(
        status.success(),
        "uploading {} to {} failed ({})",
        binary.display(),
        ssh.destination,
        describe(status)
    );

    // Run once before the scan, and the answer compared, because two
    // different failures look identical otherwise: a binary for the wrong
    // platform, and one of another version — whose flags and snapshot format
    // nothing here promises to understand.
    let expected = format!("spacetrace {VERSION}");
    anyhow::ensure!(
        said.lines().any(|line| line.trim() == expected),
        "the uploaded binary answers {:?}, but this is {expected}; scan --ssh needs \
         the same version on both sides",
        said.trim()
    );
    Ok(())
}

/// The binary to upload for `target`: this one if it would run there,
/// otherwise the published release of this version, from the cache when it has
/// been fetched before.
fn binary_for(target: &str, chatty: bool) -> Result<PathBuf> {
    // A Linux build is only portable when it is static. A development build
    // made with `cargo build` links glibc dynamically and fails on a host with
    // an older one — or on Alpine, with none.
    let portable = cfg!(target_os = "macos") || cfg!(target_env = "musl");
    if portable && crate::update::target() == Some(target) {
        return std::env::current_exe().context("finding this binary");
    }

    let tag = format!("v{VERSION}");
    let cache = crate::default_cache_dir()
        .context("cannot find a cache directory; pass a build with --binary")?
        .join("remote-binaries");
    let cached = cache.join(format!("{tag}-{target}")).join("spacetrace");
    if cached.is_file() {
        return Ok(cached);
    }

    if !spacetrace_buildinfo::is_release() {
        eprintln!(
            "note: this is a development build; the remote gets the published {tag}, \
             which may not match it"
        );
    }
    if chatty {
        eprintln!("Downloading the {tag} build for {target}…");
    }
    std::fs::create_dir_all(&cache)
        .with_context(|| format!("cannot create {}", cache.display()))?;
    // Unpacked beside the cache and moved into place, so an interrupted
    // download never leaves a half-written binary where the next run would
    // trust it.
    let staging = tempfile::Builder::new()
        .prefix(".partial-")
        .tempdir_in(&cache)
        .with_context(|| format!("cannot write to {}", cache.display()))?;
    let into = staging.path().to_path_buf();
    let target_owned = target.to_string();
    let tag_owned = tag.clone();
    // On its own thread so Ctrl-C does not wait for a download to finish.
    let fetch =
        std::thread::spawn(move || crate::update::fetch_release(&tag_owned, &target_owned, &into));
    while !fetch.is_finished() {
        if signals::interrupted() {
            return Err(Interrupted.into());
        }
        std::thread::sleep(POLL);
    }
    fetch
        .join()
        .map_err(|_| anyhow::anyhow!("the download thread panicked"))?
        .with_context(|| {
            format!(
                "fetching the {tag} build for {target}. A development build has no \
                 release of its own; build spacetrace for {target} and pass it with --binary"
            )
        })?;

    let fresh = staging.path().join("spacetrace");
    anyhow::ensure!(
        fresh.is_file(),
        "the {tag} archive for {target} holds no spacetrace binary"
    );
    let parent = cached.parent().expect("joined above");
    std::fs::create_dir_all(parent)
        .with_context(|| format!("cannot create {}", parent.display()))?;
    std::fs::rename(&fresh, &cached)
        .with_context(|| format!("cannot move the download to {}", cached.display()))?;
    Ok(cached)
}

/// The release target that runs on a machine whose `uname -sm` said this.
fn target_for_uname(platform: &str) -> Option<&'static str> {
    let mut words = platform.split_whitespace();
    let (os, arch) = (words.next()?, words.next()?);
    Some(match (os, arch) {
        ("Linux", "x86_64" | "amd64") => "x86_64-unknown-linux-musl",
        ("Linux", "aarch64" | "arm64") => "aarch64-unknown-linux-musl",
        ("Darwin", "x86_64") => "x86_64-apple-darwin",
        ("Darwin", "arm64" | "aarch64") => "aarch64-apple-darwin",
        _ => return None,
    })
}

// ------------------------------------------------------------------ ssh

struct Ssh {
    program: OsString,
    destination: String,
    /// `-o KEY=VALUE` pairs, flattened.
    options: Vec<String>,
    control: Option<Control>,
}

struct Control {
    socket: PathBuf,
    /// Private to this user; removing it removes the socket.
    _dir: tempfile::TempDir,
}

impl Ssh {
    fn cannot_run(&self) -> String {
        format!(
            "cannot run {:?}; scan --ssh needs OpenSSH ({SSH_ENV} names another program)",
            self.program
        )
    }

    /// Authenticate once and leave a master connection for every later step.
    fn start_master(&mut self) -> Result<()> {
        let dir = tempfile::Builder::new()
            .prefix("spacetrace-ssh-")
            .tempdir()
            .context("creating a private directory for the ssh control socket")?;
        let socket = dir.path().join("control");
        let status = Command::new(&self.program)
            .args(&self.options)
            .args(DEFAULT_OPTIONS)
            .arg("-M")
            .arg("-S")
            .arg(control_path(&socket))
            // Long enough to survive the gap between two steps, short enough
            // that a CLI killed with SIGKILL does not leave it for long.
            .args(["-o", "ControlPersist=15", "-N", "-f", "--"])
            .arg(&self.destination)
            .stdin(Stdio::null())
            .status()
            .with_context(|| self.cannot_run())?;
        // Kept before the interrupt check: a master that did come up has to
        // be told to exit, or it lingers until ControlPersist runs out.
        if status.success() {
            self.control = Some(Control { socket, _dir: dir });
        }
        if signals::interrupted() {
            return Err(Interrupted.into());
        }
        anyhow::ensure!(
            status.success(),
            "ssh could not connect to {} ({})",
            self.destination,
            describe(status)
        );
        Ok(())
    }

    fn stop_master(&mut self) {
        let Some(control) = self.control.take() else {
            return;
        };
        // Best effort: if it fails, ControlPersist ends the master shortly.
        let _ = Command::new(&self.program)
            .args(&self.options)
            .arg("-S")
            .arg(control_path(&control.socket))
            .args(["-O", "exit", "--"])
            .arg(&self.destination)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    /// One session on the master: `sh -c SCRIPT spacetrace ARGS…`.
    fn command(&self, script: &str, args: &[String], tty: bool) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.options);
        if let Some(control) = &self.control {
            cmd.arg("-S").arg(control_path(&control.socket));
            // Never fall back to a connection of its own: a client that
            // authenticates by itself would prompt for a password from a
            // process group that may not read the terminal, and hang there.
            cmd.args(["-o", "ControlMaster=no", "-o", "BatchMode=yes"]);
            // Out of the terminal's process group, so Ctrl-C reaches this
            // process alone and the cleanup below gets to run.
            #[cfg(unix)]
            std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        }
        if tty {
            // A multiplexed session with a terminal ends with "Shared
            // connection to … closed." at level INFO; ERROR drops that and
            // nothing that is a failure. After the user's options, so a
            // LogLevel of theirs still wins.
            cmd.args(["-o", "LogLevel=ERROR"]);
        }
        cmd.arg(if tty { "-tt" } else { "-T" })
            .arg("--")
            .arg(&self.destination)
            .arg(remote_command(script, args));
        cmd
    }
}

/// Appended after the user's options, so theirs win (ssh keeps the first
/// value). Keepalives bound how long a dead network can hang a step.
const DEFAULT_OPTIONS: &[&str] = &[
    "-o",
    "ServerAliveInterval=15",
    "-o",
    "ServerAliveCountMax=3",
];

/// ssh expands `%` tokens in a control path; a literal one has to be doubled.
fn control_path(socket: &Path) -> OsString {
    socket.to_string_lossy().replace('%', "%%").into()
}

/// The command line the remote login shell receives.
///
/// The script and every argument are single-quoted, so the login shell sees
/// literal words and `sh` receives the arguments as `$1`, `$2`… — no byte of a
/// path or a label is ever parsed as shell syntax. That holds for every POSIX
/// shell (sh, bash, dash, zsh, ksh, busybox ash); fish and csh have their own
/// quoting rules, and a login shell of theirs is not supported.
fn remote_command(script: &str, args: &[String]) -> String {
    let mut command = format!("sh -c {} spacetrace", sh_quote(script));
    for arg in args {
        command.push(' ');
        command.push_str(&sh_quote(arg));
    }
    command
}

/// POSIX single quoting: everything between single quotes is literal, and a
/// single quote itself is written as `'\''` — close, escaped quote, reopen.
fn sh_quote(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('\'');
    for c in text.chars() {
        if c == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(c);
        }
    }
    quoted.push('\'');
    quoted
}

/// A destination is handed to ssh after `--`, so it cannot be read as an
/// option there. Refusing a leading `-` as well closes the same hole for any
/// ssh-alike named by `SPACETRACE_SSH` that does not honour `--`.
fn check_destination(destination: &str) -> Result<()> {
    anyhow::ensure!(!destination.is_empty(), "--ssh needs a destination");
    anyhow::ensure!(
        !destination.starts_with('-'),
        "refusing the ssh destination {destination:?}: it starts with '-' and would \
         be read as an option"
    );
    anyhow::ensure!(
        !destination
            .chars()
            .any(|c| c.is_control() || c.is_whitespace()),
        "refusing the ssh destination {destination:?}: it contains whitespace or a \
         control character"
    );
    Ok(())
}

/// `--ssh-option KEY=VALUE` as `-o KEY=VALUE`.
fn user_options(given: &[String]) -> Result<Vec<String>> {
    let mut options = Vec::with_capacity(given.len() * 2);
    for option in given {
        let (key, value) = option
            .split_once('=')
            .with_context(|| format!("--ssh-option takes KEY=VALUE, not {option:?}"))?;
        anyhow::ensure!(
            !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric()),
            "--ssh-option {option:?}: {key:?} is not an ssh option name"
        );
        anyhow::ensure!(!value.is_empty(), "--ssh-option {option:?} has no value");
        anyhow::ensure!(
            !RESERVED_OPTIONS
                .iter()
                .any(|reserved| reserved.eq_ignore_ascii_case(key)),
            "--ssh-option {key} is set by spacetrace itself and cannot be changed"
        );
        options.push("-o".to_string());
        options.push(option.clone());
    }
    Ok(options)
}

/// Accept only the shape `mktemp -d …/spacetrace.XXXXXXXXXX` produces.
///
/// The lease cleans up from its own copy of the path, so this does not guard
/// the deletion. It guards the other steps, which write into whatever path
/// arrives here, against a remote that printed something else.
fn check_remote_dir(dir: &str) -> Result<()> {
    let refuse = || anyhow::anyhow!("the remote reported an unexpected directory: {dir:?}");
    if !dir.starts_with('/') || dir.chars().any(|c| c.is_control() || c == '\u{fffd}') {
        return Err(refuse());
    }
    if dir.split('/').any(|part| part == "." || part == "..") {
        return Err(refuse());
    }
    let name = dir.rsplit('/').next().unwrap_or_default();
    let Some(suffix) = name.strip_prefix("spacetrace.") else {
        return Err(refuse());
    };
    // busybox fills only the last six X with random characters and keeps the
    // rest, so the suffix is ten characters but not necessarily ten random.
    if suffix.len() != 10 || !suffix.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(refuse());
    }
    Ok(())
}

// --------------------------------------------------------------- remote side

/// The lease: create the directory, report, then hold it until stdin closes.
///
/// Every signal that could cut the cleanup short is ignored: HUP and PIPE are
/// what a vanished client delivers, and the cleanup is exactly what has to run
/// then. Lines are tagged because a login shell may print its own.
const LEASE_SCRIPT: &str = r##"trap "" HUP PIPE INT TERM
echo "platform $(uname -sm)"
dir=
for base in "${TMPDIR:-/tmp}" "$HOME"; do
  base=${base%/}
  test -n "$base" && test -d "$base" && test -w "$base" || continue
  dir=$(mktemp -d "$base/spacetrace.XXXXXXXXXX" 2>/dev/null) || { dir=; continue; }
  echo "#!/bin/sh" > "$dir/probe" && chmod 700 "$dir/probe" && "$dir/probe" 2>/dev/null && break
  echo "spacetrace: $base does not allow running programs; trying the next place" >&2
  rm -f "$dir/probe"
  rmdir "$dir"
  dir=
done
if test -z "$dir"; then
  echo "spacetrace: found no writable directory that allows running programs (tried ${TMPDIR:-/tmp} and $HOME)" >&2
  exit 3
fi
rm -f "$dir/probe"
echo "dir $dir"
echo "free $(df -Pk "$dir" 2>/dev/null | tail -n 1)"
echo ready
cat >/dev/null
pid=$(cat "$dir/scan.pid" 2>/dev/null)
if test -n "$pid"; then
  kill "$pid" 2>/dev/null
  n=0
  while kill -0 "$pid" 2>/dev/null && test "$n" -lt 20; do
    sleep 1
    n=$((n + 1))
  done
  kill -0 "$pid" 2>/dev/null && kill -9 "$pid" 2>/dev/null
fi
for name in FILES; do
  rm -f "$dir/$name"
done
if rmdir "$dir" 2>/dev/null; then echo "removed $dir"; else echo "kept $dir"; fi
"##;

/// Upload: the binary arrives on stdin, so neither scp nor sftp has to be
/// enabled on the server. Its `-V` answer is the proof it arrived whole.
const UPLOAD_SCRIPT: &str =
    r#"cat > "$1/spacetrace" && chmod 700 "$1/spacetrace" && "$1/spacetrace" -V"#;

/// The scan, with its pid written down so the lease can stop it if this side
/// goes away while it runs. The trap keeps that pid file honest when the
/// session is hung up on.
const SCAN_SCRIPT: &str = r#"dir=$1
shift
SPACETRACE_NO_UPDATE_CHECK=1 "$dir/spacetrace" --db "$dir/scan.sqlite" scan --save "$@" >/dev/null &
pid=$!
echo "$pid" > "$dir/scan.pid"
trap 'kill "$pid" 2>/dev/null; wait "$pid"; rm -f "$dir/scan.pid"; exit 129' HUP TERM
wait "$pid"
status=$?
rm -f "$dir/scan.pid"
exit "$status"
"#;

/// The database, as raw bytes on stdout. A WAL file still holding pages means
/// the database alone is not the whole snapshot, so it is refused rather than
/// sent short.
const DOWNLOAD_SCRIPT: &str = r#"if test -s "$1/scan.sqlite-wal"; then
  echo "spacetrace: the snapshot database was not closed cleanly" >&2
  exit 3
fi
exec cat "$1/scan.sqlite"
"#;

// --------------------------------------------------------------- the lease

enum Cleanup {
    Removed,
    Kept,
    Unconfirmed,
}

struct Lease {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
    dir: Option<String>,
    outcome: Option<Cleanup>,
}

impl Lease {
    fn spawn(ssh: &Ssh) -> Result<Lease> {
        let script = LEASE_SCRIPT.replace("FILES", REMOTE_FILES);
        let mut child = ssh
            .command(&script, &[], false)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .with_context(|| ssh.cannot_run())?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped above");
        let (send, lines) = mpsc::channel();
        // A thread, so every wait on the lease can also watch for Ctrl-C.
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = Vec::new();
            while matches!(reader.read_until(b'\n', &mut line), Ok(n) if n > 0) {
                let text = String::from_utf8_lossy(&line).trim_end().to_string();
                if send.send(text).is_err() {
                    return;
                }
                line.clear();
            }
        });
        Ok(Lease {
            child,
            stdin,
            lines,
            dir: None,
            outcome: None,
        })
    }

    fn wait_ready(&mut self, ssh: &Ssh) -> Result<RemoteInfo> {
        let mut platform = String::new();
        let mut free_kib = None;
        loop {
            let line = match self.lines.recv_timeout(POLL) {
                Ok(line) => line,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if signals::interrupted() {
                        return Err(Interrupted.into());
                    }
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let status = wait(&mut self.child)?;
                    bail!(
                        "could not prepare a working directory on {} ({})",
                        ssh.destination,
                        describe(status)
                    );
                }
            };
            if let Some(rest) = line.strip_prefix("platform ") {
                platform = rest.trim().to_string();
            } else if let Some(dir) = line.strip_prefix("dir ") {
                // Recorded before it is checked: whatever it is, the lease
                // made it and will remove it, and the report should name it.
                self.dir = Some(dir.to_string());
                check_remote_dir(dir)?;
            } else if let Some(df) = line.strip_prefix("free ") {
                // `df -P`: filesystem, size, used, available, capacity, mount.
                free_kib = df.split_whitespace().nth(3).and_then(|n| n.parse().ok());
            } else if line == "ready" {
                let dir = self
                    .dir
                    .clone()
                    .context("the remote never named its directory")?;
                return Ok(RemoteInfo {
                    platform,
                    dir,
                    free_kib,
                });
            }
        }
    }

    /// Close the lease and wait for the remote to report what it removed.
    ///
    /// Deliberately deaf to Ctrl-C: this *is* the cleanup. A second Ctrl-C
    /// ends the process outright (see `signals`), and the remote still
    /// cleans up, because the pipe it is reading closes with the process.
    fn release(&mut self) -> Cleanup {
        drop(self.stdin.take());
        while let Ok(line) = self.lines.recv() {
            if line.starts_with("removed ") {
                self.outcome = Some(Cleanup::Removed);
            } else if line.starts_with("kept ") {
                self.outcome = Some(Cleanup::Kept);
            }
        }
        let _ = self.child.wait();
        self.outcome.take().unwrap_or(Cleanup::Unconfirmed)
    }
}

// ----------------------------------------------------------- processes

fn run(command: &mut Command) -> Result<ExitStatus> {
    let program = command.get_program().to_owned();
    let mut child = command
        .spawn()
        .with_context(|| format!("cannot run {program:?}"))?;
    wait(&mut child)
}

/// `Child::wait`, but one that gives up when the user asks to stop.
fn wait(child: &mut Child) -> Result<ExitStatus> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if signals::interrupted() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Interrupted.into());
        }
        std::thread::sleep(POLL);
    }
}

fn describe(status: ExitStatus) -> String {
    match status.code() {
        // ssh's own failures — refused, unreachable, authentication — all
        // exit 255, and its message is already on the terminal.
        Some(255) => "ssh exited with 255; its own message is above".to_string(),
        Some(code) => format!("exit status {code}"),
        None => "killed by a signal".to_string(),
    }
}

/// What the remote prints — progress, errors — belongs on stderr here too, so
/// `--json` on stdout stays parseable.
fn stderr_as_stdio() -> Result<Stdio> {
    use std::os::fd::AsFd;
    let fd = std::io::stderr()
        .as_fd()
        .try_clone_to_owned()
        .context("duplicating stderr")?;
    Ok(Stdio::from(fd))
}

// -------------------------------------------------------------- signals

/// Ctrl-C, SIGTERM and SIGHUP turn into a flag the waits above poll.
///
/// Without this a Ctrl-C would kill the process on the spot. The remote would
/// still clean up — the lease sees its pipe close — but nobody would be told,
/// and the master connection would linger until `ControlPersist` ran out. A
/// second signal exits at once, for the case where the cleanup itself hangs.
mod signals {
    use std::sync::atomic::{AtomicBool, Ordering};

    static INTERRUPTED: AtomicBool = AtomicBool::new(false);

    const SIGNALS: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];

    extern "C" fn on_signal(_: libc::c_int) {
        // Only async-signal-safe work here: an atomic swap and `_exit`.
        if INTERRUPTED.swap(true, Ordering::SeqCst) {
            unsafe { libc::_exit(130) };
        }
    }

    pub fn interrupted() -> bool {
        INTERRUPTED.load(Ordering::SeqCst)
    }

    /// Installed for the length of one `scan --ssh` and then put back, so
    /// every other command keeps the default "Ctrl-C ends it".
    pub struct Guard {
        previous: Vec<(libc::c_int, libc::sighandler_t)>,
    }

    impl Guard {
        pub fn install() -> Guard {
            let handler = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
            let previous = SIGNALS
                .iter()
                .map(|&signal| (signal, unsafe { libc::signal(signal, handler) }))
                .collect();
            Guard { previous }
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            for &(signal, handler) in &self.previous {
                unsafe { libc::signal(signal, handler) };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed a quoted word to a real `sh` and read back what it received.
    fn through_sh(text: &str) -> String {
        let out = Command::new("sh")
            .arg("-c")
            .arg(format!("printf '%s' {}", sh_quote(text)))
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap()
    }

    #[test]
    fn quoting_hands_every_hostile_string_to_sh_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        for text in [
            "",
            "plain",
            "with space",
            "it's",
            "''",
            "'",
            "\"double\"",
            "$(touch PWNED)",
            "`touch PWNED`",
            "${HOME}",
            "a\nb",
            "trailing newline\n",
            "-rf",
            "--help",
            "back\\slash",
            "semi;colon && pipe | amp & glob * ? [x] ~ # !",
            "tab\there",
            "ünïcode — 日本",
        ] {
            assert_eq!(through_sh(text), text, "for {text:?}");
        }
        // Run from a directory of its own, so a command substitution that
        // escaped quoting would leave evidence where this looks for it.
        let out = Command::new("sh")
            .current_dir(dir.path())
            .arg("-c")
            .arg(format!(
                "printf '%s' {} {}",
                sh_quote("$(touch PWNED)"),
                sh_quote("`touch PWNED2`")
            ))
            .output()
            .unwrap();
        assert!(out.status.success());
        assert!(!dir.path().join("PWNED").exists());
        assert!(!dir.path().join("PWNED2").exists());
    }

    /// The whole command line, parsed by a login shell and then by `sh -c`,
    /// delivers each argument as one word, byte for byte.
    #[test]
    fn a_remote_command_delivers_its_arguments_as_separate_words() {
        let args: Vec<String> = ["one two", "it's", "a\nb", "-x", "$(id)", ""]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let command = remote_command(r#"for a; do printf '[%s]' "$a"; done"#, &args);
        let out = Command::new("sh").arg("-c").arg(&command).output().unwrap();
        assert_eq!(
            String::from_utf8(out.stdout).unwrap(),
            "[one two][it's][a\nb][-x][$(id)][]"
        );
    }

    #[test]
    fn a_destination_that_could_be_an_option_is_refused() {
        assert!(check_destination("-oProxyCommand=touch /tmp/x").is_err());
        assert!(check_destination("-p").is_err());
        assert!(check_destination("").is_err());
        assert!(check_destination("host name").is_err());
        assert!(check_destination("host\nname").is_err());

        assert!(check_destination("nas").is_ok());
        assert!(check_destination("admin@nas.lan").is_ok());
        assert!(check_destination("ssh://admin@nas.lan:2222").is_ok());
    }

    #[test]
    fn ssh_options_become_dash_o_pairs_and_reserved_ones_are_refused() {
        assert_eq!(
            user_options(&["Port=2222".into(), "IdentityFile=/k ey".into()]).unwrap(),
            ["-o", "Port=2222", "-o", "IdentityFile=/k ey"]
        );
        for refused in [
            "ControlPath=/tmp/x",
            "controlmaster=yes",
            "RemoteCommand=id",
            "BatchMode=no",
            "Port",
            "=2222",
            "Port=",
            "-F=x",
            "Proxy Command=x",
        ] {
            assert!(user_options(&[refused.into()]).is_err(), "{refused}");
        }
    }

    #[test]
    fn only_a_directory_mktemp_could_have_made_is_accepted() {
        for good in [
            "/tmp/spacetrace.Ab3dE9fG0h",
            "/tmp/spacetrace.XXXXbaaAeE",
            "/home/scan user/spacetrace.0123456789",
            "/var/folders/x1/abc/T/spacetrace.aaaaaaaaaa",
        ] {
            assert!(check_remote_dir(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            "tmp/spacetrace.Ab3dE9fG0h",
            "/",
            "/tmp",
            "/home/scan",
            "/tmp/spacetrace.",
            "/tmp/spacetrace.short",
            "/tmp/spacetrace.Ab3dE9fG0h/..",
            "/tmp/../home/spacetrace.Ab3dE9fG0h",
            "/tmp/./spacetrace.Ab3dE9fG0h",
            "/tmp/spacetrace.Ab3dE9fG0\n",
            "/tmp/spacetrace.Ab3dE9fG-h",
            "/tmp/other.Ab3dE9fG0h",
        ] {
            assert!(check_remote_dir(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn uname_answers_map_to_the_targets_the_release_publishes() {
        assert_eq!(
            target_for_uname("Linux x86_64"),
            Some("x86_64-unknown-linux-musl")
        );
        assert_eq!(
            target_for_uname("Linux aarch64"),
            Some("aarch64-unknown-linux-musl")
        );
        assert_eq!(
            target_for_uname("Darwin arm64"),
            Some("aarch64-apple-darwin")
        );
        assert_eq!(
            target_for_uname("Darwin x86_64"),
            Some("x86_64-apple-darwin")
        );
        // No build exists for these; the answer must be "none", not a guess.
        assert_eq!(target_for_uname("Linux armv7l"), None);
        assert_eq!(target_for_uname("FreeBSD amd64"), None);
        assert_eq!(target_for_uname(""), None);
    }

    /// The lease's file list is substituted in; a script that still said
    /// FILES would remove nothing and leave every directory behind.
    #[test]
    fn the_lease_names_every_file_the_other_steps_create() {
        let script = LEASE_SCRIPT.replace("FILES", REMOTE_FILES);
        assert!(!script.contains("FILES"));
        for name in ["spacetrace", "scan.sqlite", "scan.sqlite-wal", "scan.pid"] {
            assert!(REMOTE_FILES.split_whitespace().any(|n| n == name), "{name}");
        }
    }

    #[test]
    fn a_literal_percent_in_the_control_path_is_doubled() {
        assert_eq!(
            control_path(Path::new("/tmp/a%b/control")),
            OsString::from("/tmp/a%%b/control")
        );
    }
}
