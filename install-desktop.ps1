# Install the spacetrace desktop app on Windows from a GitHub release.
#
#   irm https://raw.githubusercontent.com/unalcakir28/spacetrace/main/install-desktop.ps1 | iex
#
# ---------------------------------------------------------------------------
# Why this exists, and why it should stop existing
#
# The -setup.exe on the download page is the normal way in and stays the normal
# way in. This script is a workaround for one problem: the installer is not
# code-signed, so SmartScreen shows "Windows protected your PC" and the way
# past it (More info → Run anyway) looks exactly like what a user is told never
# to click.
#
# What makes this quieter is not a trick against SmartScreen. Its app-reputation
# prompt fires on the Mark-of-the-Web that a *browser* writes into the file's
# alternate data stream. Invoke-WebRequest does not write it, so the prompt has
# nothing to fire on.
#
# Said plainly, because it matters: unlike the macOS half of this pair, that
# last paragraph is reasoning, not a measurement — it was written on a Mac and
# has not been run on Windows. If SmartScreen still appears, More info → Run
# anyway is safe here and the checksum below is what actually protects you.
# Machines with Smart App Control switched on will refuse unsigned installers
# whatever the download method; that one has no workaround but a certificate.
#
# The honest trade: a signature is an *identity* check — a certificate
# authority vouching that they know who published this and can revoke them. The
# SHA256 check below is an *integrity* check: proof the bytes did not change in
# transit, and no proof at all of who built them.
#
# **This is temporary.** The moment a code signing certificate exists, the
# installer gets signed, SmartScreen quietens on its own, and this file should
# be deleted rather than maintained. See docs/RELEASING.md.
# ---------------------------------------------------------------------------

$ErrorActionPreference = 'Stop'

$Repo = 'unalcakir28/spacetrace'
$Version = if ($env:SPACETRACE_VERSION) { $env:SPACETRACE_VERSION } else { 'latest' }

# Windows PowerShell 5.1 still negotiates TLS 1.0 by default, which GitHub
# refuses outright. Without this the first request fails with a connection
# error that says nothing about the real cause.
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

function Fail($message) {
    Write-Host "error: $message" -ForegroundColor Red
    exit 1
}

if ($Version -eq 'latest') {
    Write-Host 'Looking up the latest desktop release...'
    # NOT releases/latest. This repository holds all three components'
    # downloads, so GitHub's "latest" is whichever was published most recently
    # — on 9 Sept 2026 that was hub-v0.3.0. The desktop's releases are the ones
    # tagged desktop-v<digit>. The list comes back newest first.
    $releases = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases?per_page=100" `
        -Headers @{ 'User-Agent' = 'spacetrace-installer' }
    $tag = ($releases | Where-Object { $_.tag_name -match '^desktop-v[0-9]' } |
        Select-Object -First 1).tag_name
    if (-not $tag) { Fail 'no tagged desktop release found' }
    $Version = $tag -replace '^desktop-', ''
} else {
    $tag = "desktop-$Version"
}

$asset = "spacetrace-desktop-$Version-windows-x86_64-setup.exe"
$url = "https://github.com/$Repo/releases/download/$tag/$asset"

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("spacetrace-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    $exe = Join-Path $tmp $asset

    Write-Host "Downloading $asset..."
    # -UseBasicParsing so this works on PowerShell 5.1 without Internet
    # Explorer's engine, which is absent on Server Core and on hardened images.
    Invoke-WebRequest -Uri $url -OutFile $exe -UseBasicParsing

    # Best-effort like install.sh: a missing sums file is a warning, a
    # mismatched hash is always fatal.
    $sumsUrl = "https://github.com/$Repo/releases/download/$tag/SHA256SUMS"
    $sums = $null
    try {
        $sums = (Invoke-WebRequest -Uri $sumsUrl -UseBasicParsing).Content
    } catch {
        Write-Host 'Could not fetch SHA256SUMS; installing without verification.' -ForegroundColor Yellow
    }

    if ($sums) {
        # The ./ prefix is optional as a group: some tools write it, some do not.
        $line = $sums -split "`n" | Where-Object { $_ -match "^([0-9a-fA-F]{64})\s+(\./)?$([Regex]::Escape($asset))\s*$" }
        if (-not $line) { Fail "SHA256SUMS does not list $asset" }
        $expected = $Matches[1].ToLower()
        $actual = (Get-FileHash -Path $exe -Algorithm SHA256).Hash.ToLower()
        if ($actual -ne $expected) {
            Fail "checksum mismatch for ${asset}: expected $expected, got $actual"
        }
        Write-Host 'Checksum verified.'
    }

    Write-Host 'Starting the installer...'
    # Run it visibly rather than with /S. A silent install started by a pasted
    # one-liner is exactly the shape of the thing users are warned about, and
    # the NSIS installer is per-user and needs no administrator rights anyway.
    $proc = Start-Process -FilePath $exe -PassThru -Wait
    if ($proc.ExitCode -ne 0) {
        Fail "the installer exited with code $($proc.ExitCode)"
    }

    Write-Host ''
    Write-Host "Installed spacetrace desktop $Version."
    Write-Host ''
    Write-Host 'This installer is not code-signed. Any quiet it got here came from'
    Write-Host 'how it was downloaded, not from anything verifying who built it.'
    Write-Host 'The checksum above proves the bytes are intact, and nothing more.'
    Write-Host 'When the installer is signed, download it normally and delete this'
    Write-Host 'script.'
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
