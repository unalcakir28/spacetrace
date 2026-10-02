# Scanning over ssh

`spacetrace scan --ssh` scans a machine you can `ssh` into, with nothing
installed there. The snapshot lands in the **local** database, so `scans`,
`diff`, `ls --scan`, `age --scan`, `export` and `verify` work on it exactly as
on any other snapshot.

```bash
spacetrace scan --save --ssh admin@nas.lan /volume1
spacetrace scan --save --ssh nas /var --label weekly      # a Host from ~/.ssh/config
spacetrace scan --save --ssh ssh://admin@10.0.0.5:2222 /srv -x --exclude node_modules
spacetrace scan --save --ssh nas --ssh-option Port=2222 --ssh-option IdentityFile=~/.ssh/nas /

spacetrace diff --path /var        # two snapshots of nas:/var
```

Without `--save` the summary is printed and nothing is stored, as with a local
`scan`. The walk flags (`--exclude`, `-x`, `--depth`, `--threads`,
`--no-dedupe`, `--mount-timeout`, …) are passed through to the remote scan.

The snapshot carries the **remote** machine's hostname and canonical root, so
it joins that machine's history: a later `scan --ssh` of the same path is
comparable with it, and so are snapshots of the same root that an agent
installed there later takes and you `pull`. That holds for an agent running on
the host itself; one in a container reports the container's hostname and its
own view of the path.

## What happens

1. One `ssh` connection is opened and kept as a ControlMaster. You are asked
   for a password at most once.
2. The remote reports its platform (`uname -sm`) and creates a private
   directory with `mktemp -d` in `$TMPDIR` or `/tmp`. If that filesystem is
   mounted `noexec`, it tries `$HOME` instead.
3. A spacetrace binary of **the same version as yours** is streamed into that
   directory over the ssh connection's stdin — scp and sftp do not have to be
   enabled — and run once with `-V` to prove it arrived whole.
4. It runs `scan --save` into a database beside itself. Its progress line is
   shown here when your terminal is a terminal.
5. The database is streamed back over stdout and imported through the same
   path as `spacetrace pull`: the snapshot's checksum is recomputed and a
   mismatch imports nothing.
6. The remote directory is removed, and the connection closed.

**Which binary.** If the remote is the same platform as this machine and this
binary is portable (any macOS build, a static musl Linux build), it uploads
itself. Otherwise it downloads the published release asset of this version
for the remote's platform — `spacetrace-vX.Y.Z-<target>.tar.gz` from this
repository's releases — checks it against the release's `SHA256SUMS`, and
keeps it in the cache directory (`~/Library/Caches/spacetrace` on macOS,
`$XDG_CACHE_HOME/spacetrace` or `~/.cache/spacetrace` on Linux) for next
time. Published builds exist for Linux x86_64 and aarch64 and for macOS.

A development build has no release of its own version. If its version number
happens to be a published one, the published binary is used and a note says
so; otherwise the download fails and says to build one and pass it with
`--binary FILE`. Any binary that does not answer `spacetrace X.Y.Z` with this
CLI's exact version is refused: the scan flags and the snapshot format are
only promised between equal versions.

## Nothing stays behind

The remote directory is owned by the first session, the *lease*, which stays
open for the whole run reading a pipe that only the local CLI writes to. When
that pipe closes, the lease stops a scan that is still running and removes the
directory. It closes when the run finishes, when it fails, when you press
Ctrl-C — and also when the CLI is killed outright or the network goes away,
which no cleanup on this side could cover.

The lease removes the files it knows this run creates, by name, and then the
directory with `rmdir`. It cannot remove anything it did not put there, and it
uses its own copy of the path `mktemp` printed, never one sent back to it.

Ctrl-C reaches only the local CLI (the ssh processes run in their own process
groups), which closes the lease, waits for the remote to confirm, says
`Removed … on host.` and exits with status 130. A second Ctrl-C exits at once;
the remote still cleans up.

Measured against OpenSSH in Docker, and by the tests in
`crates/cli/tests/ssh.rs`: after a finished scan, a failed one, Ctrl-C, and
`kill -9` of the CLI, the remote temporary directory was gone and no remote
spacetrace process was left.

## The trade-off

- **It needs ssh.** Outbound from here, inbound there, and an account that can
  read what you want scanned. It scans as that account: without root, the
  paths it cannot read are counted as errors, the same as a local scan.
- **It uploads a binary every run**, about 7 MB for a Linux build. Over a slow
  link that is the slowest step.
- **The remote needs room** in `/tmp` (or `$HOME`) for the binary and the
  snapshot database. 600,601 entries with very short names made a 19.7 MB
  database; real names are longer. The binary is checked against the free
  space before uploading; a database that does not fit fails the scan, and the
  directory is removed as always.
- **One scan, when you run it.** For a history taken on a schedule, install the
  [agent](AGENT.md); `scan --ssh` is for a machine you look at now and then.

## Limits

- **The remote login shell must be a POSIX shell** — sh, bash, dash, zsh, ksh
  or busybox ash. Every argument is single-quoted for one, so no byte of a path
  or a label is parsed as shell syntax; fish and csh quote differently.
- **A login shell that prints at startup** (an `echo` in `.bashrc`) corrupts the
  stream; the import then refuses the file as not being a database.
- **Error paths do not travel.** The snapshot records how many paths could not
  be read, not which ones. The clone count (macOS) is not recorded either.
- **A vanished network is noticed late on the server.** The lease cleans up
  when sshd notices the session is gone, which with sshd's defaults can take as
  long as TCP keepalive does. The client side uses `ServerAliveInterval=15`.
- **Not on Windows yet.** The interrupt handling rests on Unix process groups
  and signals, and Windows OpenSSH has no ControlMaster; from WSL it works.
- After a `kill -9` of the CLI an empty private directory for the control
  socket stays in the local temporary directory, and the master connection
  stays up for 15 seconds (`ControlPersist`).

`--ssh-option KEY=VALUE` passes `-o KEY=VALUE` to ssh. Options spacetrace sets
itself (`ControlMaster`, `ControlPath`, `ControlPersist`, `BatchMode`,
`RemoteCommand`, `RequestTTY`, …) are refused. `SPACETRACE_SSH` names the ssh
program, for a wrapper or a test.
