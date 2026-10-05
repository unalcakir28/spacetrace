#!/bin/sh
# Refuses an external dependency that is not an exact pin, or that a crate
# versions itself instead of taking it from [workspace.dependencies].
#
# A range means a lock regenerated from scratch can resolve to a release
# nobody read; a crate-local version means one bump has more than one place
# to go. Path entries (the workspace's own crates) are skipped.
set -eu
cd "$(dirname "$0")/.."

awk '
  /^\[/ { deps = ($0 ~ /dependencies/); ws = ($0 ~ /^\[workspace\.dependencies\]/) }
  !deps || /^[[:space:]]*#/ || /path[[:space:]]*=/ { next }
  /^[A-Za-z0-9_-]+[[:space:]]*=[[:space:]]*(\{[^}]*version[[:space:]]*=[[:space:]]*)?"/ {
    if (!ws) { print FILENAME ": versioned in the crate, take it from the workspace: " $0; bad = 1; next }
    if ($0 !~ /"=[0-9]/) { print FILENAME ": not an exact pin: " $0; bad = 1 }
  }
  END { exit bad }
' Cargo.toml crates/*/Cargo.toml
