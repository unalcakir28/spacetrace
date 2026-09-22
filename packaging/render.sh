#!/usr/bin/env bash
# Fill the package manifests in this directory from a published release.
#
# Every manifest names a release asset and carries its SHA-256. Both come from
# the release itself rather than from a local build, because a manifest that
# describes something other than what is published is the failure mode here —
# it installs, and the binary is not the one the version claims.
#
#   ./packaging/render.sh v0.9.0
#
# Output lands in packaging/dist/, which is not checked in.
set -euo pipefail

TAG="${1:-}"
if [ -z "$TAG" ]; then
  echo "usage: $0 <tag>    e.g. $0 v0.9.0" >&2
  exit 2
fi
VERSION="${TAG#v}"

REPO="unalcakir28/spacetrace"
HERE="$(cd "$(dirname "$0")" && pwd)"
DIST="$HERE/dist"
BASE="https://github.com/$REPO/releases/download/$TAG"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# `-f` so a tag that was never published fails here, loudly, instead of
# producing manifests whose every URL is a 404.
curl -fsSL -o "$work/SHA256SUMS" "$BASE/SHA256SUMS"

# The sums file writes each name as `./spacetrace-v0.9.0-<target>.<ext>`, so
# the lookup strips that prefix rather than assuming it away.
sha_for() {
  local name="$1" line
  line="$(grep -E "[[:space:]]\.?/?${name}\$" "$work/SHA256SUMS" || true)"
  if [ -z "$line" ]; then
    echo "no checksum for $name in $TAG" >&2
    exit 1
  fi
  echo "${line%% *}"
}

SHA_DARWIN_ARM64="$(sha_for "spacetrace-$TAG-aarch64-apple-darwin.tar.gz")"
SHA_DARWIN_X86_64="$(sha_for "spacetrace-$TAG-x86_64-apple-darwin.tar.gz")"
SHA_LINUX_ARM64="$(sha_for "spacetrace-$TAG-aarch64-unknown-linux-musl.tar.gz")"
SHA_LINUX_X86_64="$(sha_for "spacetrace-$TAG-x86_64-unknown-linux-musl.tar.gz")"
SHA_WINDOWS_X86_64="$(sha_for "spacetrace-$TAG-x86_64-pc-windows-msvc.zip")"
# winget's schema wants the digest in upper case and its validator rejects it
# otherwise, which is the sort of thing that is found out in a pull request.
SHA_WINDOWS_X86_64_UPPER="$(printf '%s' "$SHA_WINDOWS_X86_64" | tr '[:lower:]' '[:upper:]')"

rm -rf "$DIST"
mkdir -p "$DIST/homebrew/Formula" "$DIST/scoop/bucket" "$DIST/winget" "$DIST/aur"

fill() {
  sed \
    -e "s|@VERSION@|$VERSION|g" \
    -e "s|@SHA_DARWIN_ARM64@|$SHA_DARWIN_ARM64|g" \
    -e "s|@SHA_DARWIN_X86_64@|$SHA_DARWIN_X86_64|g" \
    -e "s|@SHA_LINUX_ARM64@|$SHA_LINUX_ARM64|g" \
    -e "s|@SHA_LINUX_X86_64@|$SHA_LINUX_X86_64|g" \
    -e "s|@SHA_WINDOWS_X86_64_UPPER@|$SHA_WINDOWS_X86_64_UPPER|g" \
    -e "s|@SHA_WINDOWS_X86_64@|$SHA_WINDOWS_X86_64|g" \
    "$1" > "$2"
}

fill "$HERE/homebrew/spacetrace.rb" "$DIST/homebrew/Formula/spacetrace.rb"
fill "$HERE/scoop/spacetrace.json" "$DIST/scoop/bucket/spacetrace.json"
fill "$HERE/aur/PKGBUILD" "$DIST/aur/PKGBUILD"
for f in "$HERE"/winget/*.yaml; do
  fill "$f" "$DIST/winget/$(basename "$f")"
done

# A placeholder left behind means a template grew a marker the renderer does
# not know about, and the manifest would ship with `@SHA_…@` in it.
if grep -rl '@[A-Z_]*@' "$DIST" >/dev/null 2>&1; then
  echo "unfilled placeholders remain:" >&2
  grep -rn '@[A-Z_]*@' "$DIST" >&2
  exit 1
fi

echo "rendered $TAG into $DIST"
find "$DIST" -type f | sort
