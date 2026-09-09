#!/bin/sh
# Install the spacetrace agent (and CLI) from a GitHub release.
#
#   curl -fsSL https://raw.githubusercontent.com/unalcakir28/spacetrace/main/install.sh | sh
#
# Deliberately POSIX sh and deliberately boring: this runs on NAS boxes whose
# shell is busybox ash. It installs binaries and prints what to do next; it does
# not enable services or write config behind your back.
set -eu

REPO="unalcakir28/spacetrace"
VERSION="${SPACETRACE_VERSION:-latest}"
BIN_DIR="${SPACETRACE_BIN_DIR:-/usr/local/bin}"

die() {
    echo "error: $*" >&2
    exit 1
}

need() {
    command -v "$1" >/dev/null 2>&1 || die "$1 is required but not installed"
}

need uname
need tar

if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL "$1"; }
    fetch_to() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -qO- "$1"; }
    fetch_to() { wget -qO "$2" "$1"; }
else
    die "need curl or wget"
fi

os=$(uname -s)
arch=$(uname -m)

case "$os" in
    Linux) os_tag="unknown-linux-musl" ;;
    Darwin) os_tag="apple-darwin" ;;
    *) die "unsupported OS: $os (build from source: cargo build --release)" ;;
esac

case "$arch" in
    x86_64 | amd64) arch_tag="x86_64" ;;
    aarch64 | arm64) arch_tag="aarch64" ;;
    *) die "unsupported architecture: $arch (build from source: cargo build --release)" ;;
esac

target="${arch_tag}-${os_tag}"

if [ "$VERSION" = "latest" ]; then
    echo "Looking up the latest release..."
    # stderr discarded: before the first tagged release this endpoint answers
    # 404, which is an expected path handled below, not news. A real download
    # failure further down still reports itself.
    VERSION=$(fetch "https://api.github.com/repos/${REPO}/releases/latest" 2>/dev/null |
        sed -n 's/.*"tag_name" *: *"\([^"]*\)".*/\1/p' | head -n 1)
    # `releases/latest` only ever names a non-prerelease, so before the first
    # tagged release it returns nothing at all. Falling back to the rolling
    # build rather than dying is what makes the documented one-liner work from
    # day one — but it is not a release, so say so out loud.
    if [ -z "$VERSION" ]; then
        VERSION=continuous
        echo "No tagged release yet: installing the continuous build of main." >&2
        echo "It has passed CI and nothing else. Set SPACETRACE_VERSION=v… for a release." >&2
    fi
fi

asset="spacetrace-${VERSION}-${target}.tar.gz"
url="https://github.com/${REPO}/releases/download/${VERSION}/${asset}"

tmp=$(mktemp -d)
# shellcheck disable=SC2064  # $tmp must expand now, not at trap time.
trap "rm -rf '$tmp'" EXIT INT TERM

echo "Downloading ${asset}..."
fetch_to "$url" "$tmp/pkg.tar.gz" || die "download failed: $url"

# Verify against the SHA256SUMS published beside the archive.
#
# This was missing until 9 Sept 2026: the file was published every release and
# never read, so a truncated download or a tampered mirror installed silently.
# It is a best-effort check on purpose — a NAS with no sha256 tool at all
# should still be installable, and saying so is better than either failing or
# pretending the check happened.
sums_url="https://github.com/${REPO}/releases/download/${VERSION}/SHA256SUMS"
if fetch_to "$sums_url" "$tmp/SHA256SUMS" 2>/dev/null; then
    # The `./` prefix is optional as a group: some tools write it, some do not.
    expected=$(sed -n "s|^\([0-9a-fA-F]\{64\}\)  \(\./\)\{0,1\}${asset}$|\1|p" "$tmp/SHA256SUMS" | head -n 1)
    [ -n "$expected" ] || die "SHA256SUMS does not list ${asset}"

    if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$tmp/pkg.tar.gz" | cut -d' ' -f1)
    elif command -v shasum >/dev/null 2>&1; then
        actual=$(shasum -a 256 "$tmp/pkg.tar.gz" | cut -d' ' -f1)
    elif command -v openssl >/dev/null 2>&1; then
        actual=$(openssl dgst -sha256 "$tmp/pkg.tar.gz" | sed 's/.*= *//')
    else
        actual=""
        echo "No sha256 tool found; skipping checksum verification." >&2
    fi

    if [ -n "$actual" ]; then
        # Lowercased both sides: openssl and shasum disagree about case.
        expected=$(echo "$expected" | tr 'A-F' 'a-f')
        actual=$(echo "$actual" | tr 'A-F' 'a-f')
        [ "$actual" = "$expected" ] || die "checksum mismatch for ${asset}: expected $expected, got $actual"
        echo "Checksum verified."
    fi
else
    echo "Could not fetch SHA256SUMS; installing without verification." >&2
fi

tar -xzf "$tmp/pkg.tar.gz" -C "$tmp" || die "could not unpack $asset"

# Only use sudo when the destination is not already writable, so this works
# unchanged inside a container running as root.
if [ -w "$BIN_DIR" ]; then
    install_cmd=""
elif command -v sudo >/dev/null 2>&1; then
    install_cmd="sudo"
    echo "Installing to ${BIN_DIR} needs sudo."
else
    die "$BIN_DIR is not writable and sudo is not available; set SPACETRACE_BIN_DIR"
fi

installed=0
for bin in spacetrace spacetrace-agent; do
    [ -f "$tmp/$bin" ] || continue
    $install_cmd mkdir -p "$BIN_DIR"
    $install_cmd cp "$tmp/$bin" "$BIN_DIR/$bin"
    $install_cmd chmod 755 "$BIN_DIR/$bin"
    echo "Installed ${BIN_DIR}/${bin}"
    installed=$((installed + 1))
done

# An archive that unpacked but held nothing we recognise must not look like a
# successful install; the user would go on to run a command that is not there.
[ "$installed" -gt 0 ] || die "$asset contained no spacetrace binaries"

echo
echo "Installed spacetrace ${VERSION}."
echo
echo "Next steps for the agent:"
echo "  1. spacetrace-agent init > agent.toml     # starter config, then edit the roots"
echo "  2. head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \\n'   # make a token"
echo "  3. spacetrace-agent --config agent.toml check"
echo "  4. spacetrace-agent --config agent.toml serve"
echo
echo "The agent only reads your filesystem. It never deletes anything."
