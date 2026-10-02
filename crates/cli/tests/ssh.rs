//! `scan --ssh`, end to end, with real processes and no network.
//!
//! `SPACETRACE_SSH` points at a shim that does what ssh would do once
//! connected: hand the command line to a POSIX shell. Everything else is real —
//! the remote scripts, the upload over stdin, the scan by an uploaded copy of
//! the test-built binary, the download, the import, and the remote directory
//! that has to be gone afterwards. The "remote" temporary directory is a test
//! directory of its own, so "nothing left behind" is a directory listing.
//!
//! What this cannot cover is OpenSSH itself: the ControlMaster, `-tt`, and a
//! real network. Those were run against an sshd in Docker; see docs/SSH.md.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_spacetrace");

/// Stands in for ssh: skip the options, take the destination, run the command
/// line through `sh -c` — which is what the remote login shell does with it.
/// The master and its `-O exit` have nothing to connect, so they succeed.
const SHIM: &str = r#"#!/bin/sh
echo "invoked" >> "$SHIM_LOG"
for a; do
  case $a in
    -N|-O) exit 0 ;;
  esac
done
while [ $# -gt 0 ]; do
  case $1 in
    --) shift; break ;;
  esac
  shift
done
shift
TMPDIR=$FAKE_REMOTE_TMP exec sh -c "$1"
"#;

struct Rig {
    _dir: tempfile::TempDir,
    root: PathBuf,
    shim: PathBuf,
    remote_tmp: PathBuf,
    log: PathBuf,
    db: PathBuf,
}

impl Rig {
    fn new() -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let shim = root.join("ssh-shim");
        write_executable(&shim, SHIM);
        let remote_tmp = root.join("remote-tmp");
        std::fs::create_dir(&remote_tmp).unwrap();
        Rig {
            log: root.join("shim.log"),
            db: root.join("local.sqlite"),
            shim,
            remote_tmp,
            root,
            _dir: dir,
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(BIN);
        cmd.arg("--db")
            .arg(&self.db)
            .args(args)
            .current_dir(&self.root)
            .env("SPACETRACE_SSH", &self.shim)
            .env("FAKE_REMOTE_TMP", &self.remote_tmp)
            .env("SHIM_LOG", &self.log)
            .env("SPACETRACE_NO_UPDATE_CHECK", "1")
            // Never the user's real cache or data directory.
            .env("SPACETRACE_HOME", self.root.join("home"));
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    /// What is left in the remote's temporary directory.
    fn leftovers(&self) -> Vec<String> {
        std::fs::read_dir(&self.remote_tmp)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }

    fn scans(&self) -> Vec<serde_json::Value> {
        let out = self.run(&["--json", "scans"]);
        assert!(out.status.success(), "{}", stderr(&out));
        serde_json::from_slice(&out.stdout).unwrap()
    }

    /// Wait for the remote scan to be running, and return its pid.
    fn remote_scan_pid(&self) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            for entry in std::fs::read_dir(&self.remote_tmp).unwrap() {
                let pid_file = entry.unwrap().path().join("scan.pid");
                if let Ok(text) = std::fs::read_to_string(&pid_file) {
                    if let Ok(pid) = text.trim().parse() {
                        return pid;
                    }
                }
            }
            assert!(Instant::now() < deadline, "the remote scan never started");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn write_executable(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A fixture with known sizes, a nested folder and a hardlink.
fn fixture(at: &Path) -> PathBuf {
    let tree = at.join("tree");
    std::fs::create_dir_all(tree.join("a/b")).unwrap();
    std::fs::write(tree.join("a/one"), vec![1u8; 1000]).unwrap();
    std::fs::write(tree.join("a/b/two"), vec![2u8; 20_000]).unwrap();
    std::fs::write(tree.join("three"), vec![3u8; 300]).unwrap();
    std::fs::hard_link(tree.join("three"), tree.join("a/three-again")).unwrap();
    tree
}

/// A stand-in remote binary: answers `-V` as the real one does, and turns
/// every scan into a minute of waiting, so a run can be stopped in the middle.
fn slow_binary(at: &Path) -> PathBuf {
    let path = at.join("slow-spacetrace");
    write_executable(
        &path,
        &format!("#!/bin/sh\ncase \"$1\" in -V) exec '{BIN}' -V ;; esac\nexec sleep 60\n"),
    );
    path
}

fn alive(pid: i32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
}

fn signal(child: &Child, name: &str) {
    let status = Command::new("kill")
        .arg(format!("-{name}"))
        .arg(child.id().to_string())
        .status()
        .unwrap();
    assert!(status.success());
}

fn wait_with_deadline(child: &mut Child, seconds: u64) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("the CLI did not finish within {seconds}s");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_snapshot_taken_over_ssh_matches_a_local_scan_of_the_same_tree() {
    let rig = Rig::new();
    let tree = fixture(&rig.root);
    let tree_arg = tree.to_str().unwrap();

    let out = rig.run(&[
        "scan", "--save", "--label", "over ssh", "--ssh", "nas", "--binary", BIN, tree_arg,
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        rig.leftovers().is_empty(),
        "the remote temporary directory must be gone: {:?}",
        rig.leftovers()
    );

    let out = rig.run(&["scan", "--save", tree_arg]);
    assert!(out.status.success(), "{}", stderr(&out));

    let scans = rig.scans();
    assert_eq!(scans.len(), 2);
    let (local, remote) = (&scans[0], &scans[1]);
    assert_eq!(remote["label"], "over ssh");
    // The remote's own canonical root and hostname — which for this shim is
    // this machine, so they must equal what a local scan records.
    assert_eq!(remote["root"], local["root"]);
    assert_eq!(remote["host"], local["host"]);
    for field in [
        "total_size",
        "total_alloc",
        "files",
        "dirs",
        "hardlinks_deduped",
    ] {
        assert_eq!(remote[field], local[field], "{field} differs");
    }
    assert_eq!(remote["total_size"], 1000 + 20_000 + 300);
    assert_eq!(remote["hardlinks_deduped"], 1);

    // And it is an ordinary snapshot from here on.
    let remote_id = remote["id"].as_i64().unwrap().to_string();
    let local_id = local["id"].as_i64().unwrap().to_string();
    let out = rig.run(&["--json", "diff", "--from", &remote_id, "--to", &local_id]);
    assert!(out.status.success(), "{}", stderr(&out));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["delta"], 0, "{report}");

    let out = rig.run(&["--json", "ls", "--scan", &remote_id]);
    assert!(out.status.success(), "{}", stderr(&out));
    let listing: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(listing["entries"][0]["name"], "a");

    let out = rig.run(&["verify", &remote_id]);
    assert!(out.status.success(), "{}", stderr(&out));
}

/// Without `--save` nothing is stored, the same as a local `scan`.
#[test]
fn without_save_the_scan_is_printed_and_not_stored() {
    let rig = Rig::new();
    let tree = fixture(&rig.root);

    let out = rig.run(&[
        "--json",
        "scan",
        "--ssh",
        "nas",
        "--binary",
        BIN,
        tree.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(summary["total_size"], 21_300);
    assert!(summary["scan_id"].is_null());
    assert!(rig.scans().is_empty());
    assert!(rig.leftovers().is_empty());
}

#[test]
fn a_failing_remote_scan_still_removes_the_remote_directory() {
    let rig = Rig::new();
    let missing = rig.root.join("no-such-dir");

    let out = rig.run(&[
        "scan",
        "--save",
        "--ssh",
        "nas",
        "--binary",
        BIN,
        missing.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let text = stderr(&out);
    assert!(text.contains("cannot scan"), "the remote's reason: {text}");
    assert!(text.contains("Removed "), "the cleanup is reported: {text}");
    assert!(rig.leftovers().is_empty(), "{:?}", rig.leftovers());
    assert!(rig.scans().is_empty(), "a failed scan imports nothing");
}

#[test]
fn ctrl_c_mid_scan_stops_the_remote_scan_and_removes_its_directory() {
    let rig = Rig::new();
    let slow = slow_binary(&rig.root);
    let mut child = rig
        .command(&[
            "scan",
            "--save",
            "--ssh",
            "nas",
            "--binary",
            slow.to_str().unwrap(),
            "/",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let remote_scan = rig.remote_scan_pid();
    assert!(alive(remote_scan));
    signal(&child, "INT");

    let status = wait_with_deadline(&mut child, 60);
    let mut text = String::new();
    std::io::Read::read_to_string(child.stderr.as_mut().unwrap(), &mut text).unwrap();
    assert_eq!(status.code(), Some(130), "{text}");
    assert!(text.contains("Removed "), "{text}");
    assert!(text.contains("nothing was imported"), "{text}");
    assert!(rig.leftovers().is_empty(), "{:?}", rig.leftovers());
    assert!(
        !alive(remote_scan),
        "the remote scan must have been stopped"
    );
    assert!(rig.scans().is_empty());
}

/// The case no handler on this side can cover: the CLI is gone without a
/// chance to clean up. The remote notices its lease close and cleans up
/// by itself.
#[test]
fn a_cli_killed_outright_still_leaves_nothing_on_the_remote() {
    let rig = Rig::new();
    let slow = slow_binary(&rig.root);
    let mut child = rig
        .command(&[
            "scan",
            "--ssh",
            "nas",
            "--binary",
            slow.to_str().unwrap(),
            "/",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let remote_scan = rig.remote_scan_pid();
    signal(&child, "KILL");
    child.wait().unwrap();

    let deadline = Instant::now() + Duration::from_secs(30);
    while !rig.leftovers().is_empty() || alive(remote_scan) {
        assert!(
            Instant::now() < deadline,
            "left behind after the CLI died: {:?}, scan alive: {}",
            rig.leftovers(),
            alive(remote_scan)
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Every argument crosses a shell on the other side. None of these may be
/// executed, split or altered on the way.
#[test]
fn hostile_names_reach_the_remote_scan_unchanged() {
    let rig = Rig::new();
    let name = "-it's \"$(touch PWNED)\" `touch PWNED2` $HOME ;x\nsecond line";
    let tree = rig.root.join(name);
    std::fs::create_dir(&tree).unwrap();
    std::fs::write(tree.join("ten"), b"0123456789").unwrap();
    std::fs::create_dir(tree.join("-$(touch PWNED3)")).unwrap();
    std::fs::write(tree.join("-$(touch PWNED3)/skipped"), b"xx").unwrap();
    let label = "$(touch PWNED4) it's `id`";

    let out = rig.run(&[
        "scan",
        "--save",
        "--label",
        label,
        "--exclude=-$(touch PWNED3)",
        "--ssh",
        "nas",
        "--binary",
        BIN,
        tree.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", stderr(&out));

    let scans = rig.scans();
    assert_eq!(scans[0]["root"], tree.to_str().unwrap());
    assert_eq!(scans[0]["label"], label);
    // The exclude arrived as a name, not as a flag: the excluded folder's two
    // bytes are not counted.
    assert_eq!(scans[0]["total_size"], 10);

    for evidence in ["PWNED", "PWNED2", "PWNED4"] {
        for place in [&rig.root, &tree, &rig.remote_tmp] {
            assert!(
                !place.join(evidence).exists(),
                "{evidence} appeared in {}",
                place.display()
            );
        }
    }
    assert!(rig.leftovers().is_empty());
}

#[test]
fn a_destination_that_could_be_an_ssh_option_runs_nothing() {
    let rig = Rig::new();
    for destination in ["--ssh=-oProxyCommand=touch PWNED", "--ssh=-p"] {
        let out = rig.run(&["scan", destination, "--binary", BIN, "/"]);
        assert!(!out.status.success());
        assert!(stderr(&out).contains("refusing"), "{}", stderr(&out));
    }
    assert!(!rig.log.exists(), "ssh must not have been started at all");
    assert!(!rig.root.join("PWNED").exists());
}

#[test]
fn a_reserved_ssh_option_is_refused_before_connecting() {
    let rig = Rig::new();
    let out = rig.run(&[
        "scan",
        "--ssh",
        "nas",
        "--ssh-option",
        "ControlPath=/tmp/elsewhere",
        "/",
    ]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("ControlPath"), "{}", stderr(&out));
    assert!(!rig.log.exists());
}

/// Another version's flags and snapshot format are not promised to match, and
/// the failure would otherwise surface as something unrelated mid-scan.
#[test]
fn a_binary_of_another_version_is_refused_and_cleaned_up() {
    let rig = Rig::new();
    let other = rig.root.join("old-spacetrace");
    write_executable(&other, "#!/bin/sh\necho 'spacetrace 0.0.1'\n");

    let out = rig.run(&[
        "scan",
        "--ssh",
        "nas",
        "--binary",
        other.to_str().unwrap(),
        "/",
    ]);
    assert!(!out.status.success());
    let text = stderr(&out);
    assert!(text.contains("spacetrace 0.0.1"), "{text}");
    assert!(text.contains("same version"), "{text}");
    assert!(rig.leftovers().is_empty());
}
