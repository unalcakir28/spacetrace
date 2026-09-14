#!/usr/bin/env bash
# CLAUDE.md: a change a user can see needs an entry in changelog.json. Nothing
# enforces that rule until a release is cut, and by then the work is tagged and
# the entry is missing from the website, the desktop What's new panel and the
# release notes at once.
#
# Warns, does not decide. Plenty of changes under crates/*/src are refactors that
# no reader can observe, so this fires at most once per session: a deliberate
# "no entry needed" must not turn Stop into a loop.
set -uo pipefail

input=$(cat)
session=$(printf '%s' "$input" | jq -r '.session_id // "unknown"' 2>/dev/null)
marker="${TMPDIR:-/tmp}/spacetrace-changelog-reminder-${session}"

[ -e "$marker" ] && exit 0

cd "${CLAUDE_PROJECT_DIR:-.}" 2>/dev/null || exit 0
git rev-parse --git-dir >/dev/null 2>&1 || exit 0

# Only library and binary sources. Tests, benches and examples live outside src/
# and none of them change what a user sees.
touched=$(git status --porcelain -- crates 2>/dev/null |
	grep -E 'crates/[^/]+/src/.*\.rs$')
[ -n "$touched" ] || exit 0

if git status --porcelain -- crates/changelog/changelog.json 2>/dev/null | grep -q .; then
	exit 0
fi

: >"$marker"

cat >&2 <<EOF
Uncommitted changes under crates/*/src, but crates/changelog/changelog.json is
untouched:

$(printf '%s\n' "$touched" | sed 's/^/  /')

If any of this changes something a user can observe — a command, a flag, output,
a number, a failure mode — write the entry now (all five locales: en tr it fr de,
rules in crates/changelog/README.md, or use the changelog-entry skill), then
regenerate CHANGELOG.md.

If it is internal only, say so in one line and stop. This reminder fires once per
session and will not ask again.
EOF
exit 2
