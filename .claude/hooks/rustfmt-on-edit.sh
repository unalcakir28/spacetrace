#!/usr/bin/env bash
# CI runs `cargo fmt --all --check` as its own job, so one unformatted line costs
# a full red build for something a machine can fix. Formatting the single file
# just edited takes milliseconds; `cargo fmt --all` would rewrite files this
# session never opened and bury the real diff.
set -uo pipefail

path=$(jq -r '.tool_input.file_path // empty' 2>/dev/null)
[ -n "$path" ] || exit 0

case "$path" in
*.rs) ;;
*) exit 0 ;;
esac

[ -f "$path" ] || exit 0

# Edition comes from [workspace.package]; rustfmt invoked directly does not read
# Cargo.toml. Failure is ignored on purpose: a file that is mid-edit and does not
# parse yet is normal, and a hook error there would be noise, not information.
rustfmt --edition 2021 "$path" >/dev/null 2>&1 || true
exit 0
