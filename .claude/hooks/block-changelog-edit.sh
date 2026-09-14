#!/usr/bin/env bash
# CHANGELOG.md is rendered from crates/changelog/changelog.json, and CI diffs the
# two (.github/workflows/ci.yml). A hand edit therefore cannot survive: either the
# next `markdown` run overwrites it, or the build goes red with a confusing error
# that points at the generated file rather than at the edit that caused it.
#
# Blocks Edit/Write only. The release procedure rewrites the file through a shell
# redirect, which is a Bash call and deliberately still allowed.
set -uo pipefail

path=$(jq -r '.tool_input.file_path // empty' 2>/dev/null)

case "$path" in
*/CHANGELOG.md | CHANGELOG.md) ;;
*) exit 0 ;;
esac

cat >&2 <<'EOF'
CHANGELOG.md is generated — do not edit it by hand.

To change what it says, edit the entry in crates/changelog/changelog.json
(all five locales: en tr it fr de), then regenerate:

  cargo run -q -p spacetrace-changelog -- fmt
  cargo run -q -p spacetrace-changelog -- markdown --component cli > CHANGELOG.md

Rules for writing an entry: crates/changelog/README.md, or use the
changelog-entry skill.
EOF
exit 2
