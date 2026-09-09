//! Stamp the binary with what it was built from.
//!
//! Until now a release carried nothing but `CARGO_PKG_VERSION`, which meant two
//! `continuous` builds a week apart were indistinguishable — you could not tell
//! a user which code they were running, and an update check had nothing to
//! compare. `strip = true` removes even the debug metadata that might have
//! hinted.
//!
//! **Environment first, git second.** CI already knows the commit and passes it
//! in; reading `.git` there would be a slower way to learn the same thing. It
//! also sidesteps the trap that makes hand-rolled build scripts lie: a build
//! script runs only when cargo thinks its inputs changed, so a script that
//! shells out to git happily reports yesterday's commit after you commit
//! today's. `rerun-if-env-changed` covers the CI path exactly, and the git path
//! is for local builds where a stale short SHA costs nothing.

use std::process::Command;

fn main() {
    // Anything not set by CI falls through to git, then to a marker. A build
    // that cannot tell you where it came from must say so, not guess.
    let sha = env_or("SPACETRACE_GIT_SHA", || {
        git(&["rev-parse", "--short=7", "HEAD"])
    });
    let date = env_or("SPACETRACE_BUILD_DATE", || {
        git(&[
            "log",
            "-1",
            "--date=format-local:%Y-%m-%dT%H:%M:%SZ",
            "--format=%cd",
        ])
    });

    // `dev` is the honest default: a build nobody published. CI overrides it
    // with `release` or `continuous`.
    let channel = std::env::var("SPACETRACE_CHANNEL").unwrap_or_else(|_| "dev".to_string());

    // A dirty working tree is worth knowing about in a bug report, but only
    // locally: CI always builds a clean checkout, so it never sets this.
    let dirty = match std::env::var("SPACETRACE_GIT_SHA") {
        Ok(_) => false,
        Err(_) => git(&["status", "--porcelain"]).is_some_and(|out| !out.is_empty()),
    };

    println!(
        "cargo:rustc-env=SPACETRACE_GIT_SHA={}",
        sha.unwrap_or_else(unknown)
    );
    println!(
        "cargo:rustc-env=SPACETRACE_BUILD_DATE={}",
        date.unwrap_or_else(unknown)
    );
    println!("cargo:rustc-env=SPACETRACE_CHANNEL={channel}");
    println!("cargo:rustc-env=SPACETRACE_GIT_DIRTY={dirty}");

    for key in [
        "SPACETRACE_GIT_SHA",
        "SPACETRACE_BUILD_DATE",
        "SPACETRACE_CHANNEL",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
    }
}

fn unknown() -> String {
    "unknown".to_string()
}

fn env_or(key: &str, fallback: impl FnOnce() -> Option<String>) -> Option<String> {
    match std::env::var(key) {
        Ok(value) if !value.trim().is_empty() => Some(value.trim().to_string()),
        _ => fallback(),
    }
}

/// `None` when git is missing or this is not a checkout — building from a
/// release tarball is a supported path, not an error.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    Some(text.trim().to_string())
}
