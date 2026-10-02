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

mod common;
use common::{forged_snapshot, BIN};

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
cd "$HOME" || exit 255
# A connection that drops partway through the upload.
if [ -n "$CUT_UPLOAD_AT" ]; then
  case $1 in
    *"cat > spacetrace"*)
      head -c "$CUT_UPLOAD_AT" | TMPDIR=$FAKE_REMOTE_TMP sh -c "$1"
      exit $? ;;
  esac
fi
TMPDIR=$FAKE_REMOTE_TMP exec sh -c "$1"
"#;

struct Rig {
    _dir: tempfile::TempDir,
    root: PathBuf,
    shim: PathBuf,
    remote_tmp: PathBuf,
    /// The remote's `$HOME`, where an ssh session starts and where the lease
    /// falls back to when the temporary directory will not do.
    remote_home: PathBuf,
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
        let remote_home = root.join("remote-home");
        for made in [&remote_tmp, &remote_home] {
            std::fs::create_dir(made).unwrap();
            std::fs::set_permissions(made, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Rig {
            log: root.join("shim.log"),
            db: root.join("local.sqlite"),
            shim,
            remote_tmp,
            remote_home,
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
            // Never the user's real home, cache or data directory: the lease
            // may fall back to `$HOME`, and this one is a test directory.
            .env("HOME", &self.remote_home)
            .env("SPACETRACE_HOME", self.root.join("home"));
        cmd
    }

    /// What is left in the remote's home directory.
    fn home_leftovers(&self) -> Vec<String> {
        std::fs::read_dir(&self.remote_home)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
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

/// A stand-in remote binary: answers `-V` as the real one does, and does
/// `scan` with its own body. It stays the process the lease started — no
/// `exec` — because the lease signals only a pid that still names the
/// uploaded binary.
fn stand_in(at: &Path, name: &str, scan: &str) -> PathBuf {
    let path = at.join(name);
    write_executable(
        &path,
        &format!("#!/bin/sh\ncase \"$1\" in -V) exec '{BIN}' -V ;; esac\n{scan}\n"),
    );
    path
}

/// Turns every scan into a minute of waiting, so a run can be stopped in the
/// middle. The trap passes the lease's SIGTERM on to the sleep.
fn slow_binary(at: &Path) -> PathBuf {
    stand_in(
        at,
        "slow-spacetrace",
        "trap 'kill $!; exit 143' TERM\nsleep 60 &\nwait",
    )
}

/// A local database that already holds one snapshot, and its listing.
fn with_a_snapshot(rig: &Rig) -> String {
    let tree = fixture(&rig.root);
    let out = rig.run(&["scan", "--save", tree.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr(&out));
    String::from_utf8(rig.run(&["--json", "scans"]).stdout).unwrap()
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

/// A remote can send a correct digest over a broken tree. With `--save` the
/// structural check has to run before the import commits, or the local
/// database keeps a snapshot every later command trips over.
#[test]
fn a_forged_snapshot_from_the_remote_leaves_the_local_database_unchanged() {
    let rig = Rig::new();
    let before = with_a_snapshot(&rig);
    let forged = forged_snapshot(&rig.root);
    let sender = stand_in(
        &rig.root,
        "forging-spacetrace",
        r#"cp "$FORGED_DB" scan.sqlite"#,
    );

    let out = rig
        .command(&[
            "scan",
            "--save",
            "--ssh",
            "nas",
            "--binary",
            sender.to_str().unwrap(),
            "/",
        ])
        .env("FORGED_DB", &forged)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("not a usable tree"),
        "{}",
        stderr(&out)
    );
    let after = String::from_utf8(rig.run(&["--json", "scans"]).stdout).unwrap();
    assert_eq!(
        after, before,
        "the local database must be exactly as it was"
    );
    assert!(rig.leftovers().is_empty());
}

/// A base directory others can write to without the sticky bit lets them
/// rename the work directory away and put theirs in its place, so it is not
/// used; `$HOME` is. With the sticky bit — `/tmp` everywhere — it is fine.
#[test]
fn a_shared_base_without_the_sticky_bit_is_passed_over() {
    for (mode, expect_home) in [(0o777, true), (0o770, true), (0o1777, false)] {
        let rig = Rig::new();
        let tree = fixture(&rig.root);
        std::fs::set_permissions(&rig.remote_tmp, std::fs::Permissions::from_mode(mode)).unwrap();
        let ran_in_file = rig.root.join("ran-in");
        let recorder = stand_in(
            &rig.root,
            "recording-spacetrace",
            &format!(r#"pwd -P > "$RAN_IN"; exec '{BIN}' "$@""#),
        );

        let out = rig
            .command(&[
                "scan",
                "--ssh",
                "nas",
                "--binary",
                recorder.to_str().unwrap(),
            ])
            .arg(&tree)
            .env("RAN_IN", &ran_in_file)
            .output()
            .unwrap();
        assert!(out.status.success(), "mode {mode:o}: {}", stderr(&out));
        let ran_in = PathBuf::from(std::fs::read_to_string(&ran_in_file).unwrap().trim());
        let home = rig.remote_home.canonicalize().unwrap();
        assert_eq!(
            ran_in.starts_with(&home),
            expect_home,
            "mode {mode:o} ran in {}",
            ran_in.display()
        );
        assert_eq!(
            stderr(&out).contains("no sticky bit"),
            expect_home,
            "mode {mode:o}: {}",
            stderr(&out)
        );
        assert!(
            rig.leftovers().is_empty(),
            "{mode:o}: {:?}",
            rig.leftovers()
        );
        assert!(
            rig.home_leftovers().is_empty(),
            "{mode:o}: {:?}",
            rig.home_leftovers()
        );
    }
}

/// Somebody swaps the work directory for a symlink to a directory of their
/// choosing between two steps. The next step must refuse to work there, and
/// the lease must still remove the real directory under its new name.
#[test]
fn a_work_directory_swapped_between_steps_is_refused() {
    let rig = Rig::new();
    let decoy = rig.root.join("decoy");
    std::fs::create_dir(&decoy).unwrap();
    std::fs::write(decoy.join("canary"), b"untouched").unwrap();
    // The swap happens while the upload step runs `-V`, after the bytes are in.
    let swapper = rig.root.join("swapping-spacetrace");
    write_executable(
        &swapper,
        &format!(
            "#!/bin/sh\nd=$(pwd -P)\nmv \"$d\" \"$d.moved\" && ln -s \"$DECOY\" \"$d\"\n\
             exec '{BIN}' -V\n"
        ),
    );

    let out = rig
        .command(&[
            "scan",
            "--save",
            "--ssh",
            "nas",
            "--binary",
            swapper.to_str().unwrap(),
            "/",
        ])
        .env("DECOY", &decoy)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("is not the directory this run created"),
        "{}",
        stderr(&out)
    );

    let in_decoy: Vec<String> = std::fs::read_dir(&decoy)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        in_decoy,
        ["canary"],
        "nothing may be written into the decoy"
    );
    // What remains is the swapper's symlink, which is not ours to remove;
    // the real directory, renamed, is gone.
    let left = rig.leftovers();
    assert_eq!(left.len(), 1, "{left:?}");
    assert!(rig.remote_tmp.join(&left[0]).is_symlink(), "{left:?}");
    assert!(rig.scans().is_empty());
}

/// A pid file outlives its process when the wrapper is killed outright, and
/// by then the number can belong to something else of the same user's. The
/// lease must leave that process alone.
#[test]
fn a_stale_pid_file_never_gets_an_unrelated_process_killed() {
    let rig = Rig::new();
    let mut decoy = Command::new("sleep").arg("60").spawn().unwrap();
    // Leave the decoy's pid behind, and kill the wrapper that would have
    // removed the file.
    let dying = stand_in(
        &rig.root,
        "dying-spacetrace",
        "echo \"$DECOY_PID\" > scan.pid\nkill -9 $PPID\nexit 1",
    );

    let out = rig
        .command(&[
            "scan",
            "--ssh",
            "nas",
            "--binary",
            dying.to_str().unwrap(),
            "/",
        ])
        .env("DECOY_PID", decoy.id().to_string())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(rig.leftovers().is_empty(), "{:?}", rig.leftovers());
    assert!(
        decoy.try_wait().unwrap().is_none(),
        "the unrelated process must still be running"
    );
    decoy.kill().unwrap();
    decoy.wait().unwrap();
}

/// `-V` answering proves the file starts as a binary, not that all of it
/// arrived. A connection that drops partway must be reported as such.
#[test]
fn an_upload_that_arrives_short_is_refused() {
    let rig = Rig::new();
    let out = rig
        .command(&["scan", "--ssh", "nas", "--binary", BIN, "/"])
        .env("CUT_UPLOAD_AT", "1000")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let size = std::fs::metadata(BIN).unwrap().len();
    assert!(
        stderr(&out).contains(&format!("sent {size} bytes, 1000 arrived")),
        "{}",
        stderr(&out)
    );
    assert!(rig.leftovers().is_empty());
}
