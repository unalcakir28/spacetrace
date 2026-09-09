#!/bin/sh
# Install the spacetrace desktop app on macOS from a GitHub release.
#
#   curl -fsSL https://raw.githubusercontent.com/unalcakir28/spacetrace/main/install-desktop.sh | sh
#
# ---------------------------------------------------------------------------
# Why this exists, and why it should stop existing
#
# The .dmg on the download page is the normal way in and stays the normal way
# in. This script is a workaround for one specific problem: the app is not
# code-signed, so macOS refuses the first launch and the way past that refusal
# keeps getting narrower. Apple removed the right-click → Open shortcut in
# macOS 15, which left System Settings → Privacy & Security → Open Anyway as
# the only click-through — and on an ad-hoc-signed app that button is not
# guaranteed to appear at all.
#
# What makes this work is not a trick against Gatekeeper. Gatekeeper's
# first-launch check fires on the `com.apple.quarantine` attribute, and that
# attribute is written by the *browser*, not by the file. curl does not write
# it, so the check never runs. Measured on macOS 26.5.2: a curl-fetched .dmg
# carries no extended attributes at all, and the app copied out of it launches
# with no dialog.
#
# The honest trade: a signature is an *identity* check — Apple vouching that
# they know who published this and can revoke them. The SHA256SUMS check below
# is an *integrity* check — proof the bytes did not change in transit, and no
# proof at all of who built them. This script is therefore for people who
# already decided to trust this project, which is a smaller promise than the
# one a signature makes.
#
# **This is temporary.** The moment a Developer ID certificate exists, the .dmg
# gets signed and notarized, double-clicking works like any other app, and this
# file should be deleted rather than maintained. See docs/RELEASING.md.
# ---------------------------------------------------------------------------
set -eu

REPO="unalcakir28/spacetrace"
VERSION="${SPACETRACE_VERSION:-latest}"
APP_DIR="${SPACETRACE_APP_DIR:-/Applications}"
APP_NAME="spacetrace.app"

die() {
    echo "error: $*" >&2
    exit 1
}

[ "$(uname -s)" = "Darwin" ] ||
    die "this installer is macOS-only — Linux has no Gatekeeper to work around, so use the .deb, .rpm or .AppImage from https://spacetrace.teknobakkall.com/download/"

for tool in hdiutil ditto; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool is required but not installed"
done

# Checked before anything is downloaded. Swapping the bundle out from under a
# live process gives it half of one version and half of another, and the crash
# that follows looks like a bug in the app — but there is also no reason to
# spend a 13 MB download first only to refuse at the end.
if pgrep -f "${APP_DIR}/${APP_NAME}/Contents/MacOS/" >/dev/null 2>&1; then
    die "spacetrace is running — quit it first, then run this again"
fi

if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL "$1"; }
    fetch_to() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -qO- "$1"; }
    fetch_to() { wget -qO "$2" "$1"; }
else
    die "need curl or wget"
fi

if [ "$VERSION" = "latest" ]; then
    echo "Looking up the latest desktop release..."
    # NOT `releases/latest`. This repository holds all three components'
    # downloads, so GitHub's "latest" is whichever of them was published most
    # recently — on 9 Sept 2026 that was `hub-v0.3.0`. The desktop's releases
    # are the ones tagged `desktop-v<digit>`. The list comes back newest first.
    tag=$(fetch "https://api.github.com/repos/${REPO}/releases?per_page=100" 2>/dev/null |
        grep -o '"tag_name" *: *"desktop-v[0-9][^"]*"' |
        sed 's/.*"\(desktop-v[^"]*\)"$/\1/' | head -n 1)
    [ -n "$tag" ] || die "no tagged desktop release found"
    VERSION="${tag#desktop-}"
else
    tag="desktop-${VERSION}"
fi

asset="spacetrace-desktop-${VERSION}-macos-universal.dmg"
url="https://github.com/${REPO}/releases/download/${tag}/${asset}"

tmp=$(mktemp -d)
mnt=""
# The mount has to come down on every exit path, or a failed install leaves a
# volume attached that the next run cannot mount read-only.
cleanup() {
    [ -n "$mnt" ] && hdiutil detach "$mnt" -quiet >/dev/null 2>&1
    rm -rf "$tmp"
}
trap cleanup EXIT INT TERM

echo "Downloading ${asset}..."
fetch_to "$url" "$tmp/app.dmg" || die "download failed: $url"

# Same best-effort verification as install.sh: a missing sha256 tool should not
# make the machine uninstallable, but a *mismatch* always stops the install.
sums_url="https://github.com/${REPO}/releases/download/${tag}/SHA256SUMS"
if fetch_to "$sums_url" "$tmp/SHA256SUMS" 2>/dev/null; then
    expected=$(sed -n "s|^\([0-9a-fA-F]\{64\}\)  \(\./\)\{0,1\}${asset}$|\1|p" "$tmp/SHA256SUMS" | head -n 1)
    [ -n "$expected" ] || die "SHA256SUMS does not list ${asset}"

    if command -v shasum >/dev/null 2>&1; then
        actual=$(shasum -a 256 "$tmp/app.dmg" | cut -d' ' -f1)
    elif command -v openssl >/dev/null 2>&1; then
        actual=$(openssl dgst -sha256 "$tmp/app.dmg" | sed 's/.*= *//')
    else
        actual=""
        echo "No sha256 tool found; skipping checksum verification." >&2
    fi

    if [ -n "$actual" ]; then
        expected=$(echo "$expected" | tr 'A-F' 'a-f')
        actual=$(echo "$actual" | tr 'A-F' 'a-f')
        [ "$actual" = "$expected" ] || die "checksum mismatch for ${asset}: expected $expected, got $actual"
        echo "Checksum verified."
    fi
else
    echo "Could not fetch SHA256SUMS; installing without verification." >&2
fi

# `-nobrowse` so the volume does not appear in Finder while this runs, and
# `-readonly` because nothing here writes to the image.
mnt=$(hdiutil attach -nobrowse -readonly "$tmp/app.dmg" | grep -o '/Volumes/.*' | tail -n 1)
[ -n "$mnt" ] && [ -d "$mnt/$APP_NAME" ] || die "could not mount ${asset} or it holds no ${APP_NAME}"

if [ -w "$APP_DIR" ]; then
    sudo_cmd=""
elif command -v sudo >/dev/null 2>&1; then
    sudo_cmd="sudo"
    echo "Installing to ${APP_DIR} needs sudo."
else
    die "$APP_DIR is not writable and sudo is not available; set SPACETRACE_APP_DIR"
fi

if [ -e "$APP_DIR/$APP_NAME" ]; then
    echo "Replacing the existing ${APP_DIR}/${APP_NAME}."
    $sudo_cmd rm -rf "$APP_DIR/$APP_NAME"
fi

# ditto, not cp: it is the tool that preserves resource forks, extended
# attributes and the signature envelope of a bundle. `cp -R` has mangled app
# bundles for years.
$sudo_cmd ditto "$mnt/$APP_NAME" "$APP_DIR/$APP_NAME" || die "could not copy the app into $APP_DIR"

# Belt and braces. A curl download carries no quarantine attribute, so this is
# normally a no-op — but if a future fetch path ever adds one, the app would
# stop opening and the failure would look like the very problem this script
# exists to avoid. Announced rather than silent: it is the one line here that
# removes a macOS security marking.
if xattr -p com.apple.quarantine "$APP_DIR/$APP_NAME" >/dev/null 2>&1; then
    echo "Clearing the quarantine flag that came with the download."
    xattr -dr com.apple.quarantine "$APP_DIR/$APP_NAME" 2>/dev/null ||
        $sudo_cmd xattr -dr com.apple.quarantine "$APP_DIR/$APP_NAME"
fi

echo
echo "Installed spacetrace desktop ${VERSION} to ${APP_DIR}/${APP_NAME}."
echo
echo "This app is not code-signed. It opened without a warning because curl"
echo "does not mark downloads the way a browser does — not because anything"
echo "verified who built it. The checksum above proves the bytes are intact,"
echo "and nothing more. When the app is signed and notarized, download the"
echo ".dmg normally and delete this script."
echo
echo "Scanning outside your home folder needs Full Disk Access:"
echo "  System Settings → Privacy & Security → Full Disk Access"
