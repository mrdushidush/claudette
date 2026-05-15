# Claudette one-line installer — Windows.
#
# Usage (PowerShell):
#   iwr -useb https://raw.githubusercontent.com/mrdushidush/claudette/main/install.ps1 | iex
#
# Env overrides:
#   $env:CLAUDETTE_VERSION       Pin a version (e.g. 0.5.2). Default: latest.
#   $env:CLAUDETTE_INSTALL_DIR   Install location. Default: %LOCALAPPDATA%\Programs\claudette.
#
# What this script does, in order:
#   1. Resolves the requested tag (latest by default) from the GitHub API.
#   2. Downloads claudette-<tag>-x86_64-pc-windows-msvc.zip + .sha256 sidecar.
#   3. Verifies the SHA256 (refuses to install on mismatch).
#   4. Extracts claudette.exe into the install dir.
#   5. Prints a PATH update command if the install dir isn't on User PATH.

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Repo = 'mrdushidush/claudette'
$InstallDir =
    if ($env:CLAUDETTE_INSTALL_DIR) { $env:CLAUDETTE_INSTALL_DIR }
    else { Join-Path $env:LOCALAPPDATA 'Programs\claudette' }

function Info($msg) {
    Write-Host '::' -ForegroundColor Green -NoNewline
    Write-Host " $msg"
}
function Warn($msg) {
    Write-Host '!' -ForegroundColor Yellow -NoNewline
    Write-Host " $msg"
}
function Fail($msg) {
    Write-Host 'error:' -ForegroundColor Red -NoNewline
    Write-Host " $msg"
    exit 1
}

# Only x86_64-pc-windows-msvc is shipped — surface a friendly failure on ARM64
# rather than a 404 from Releases. ARM64 Windows users should fall back to
# `cargo install claudette` until we add an arm64-msvc build leg.
$arch = [System.Environment]::GetEnvironmentVariable('PROCESSOR_ARCHITECTURE')
if ($arch -ne 'AMD64') {
    Fail "unsupported Windows arch: $arch (only x86_64 prebuilt binaries are shipped today — try 'cargo install claudette')"
}
$Target = 'x86_64-pc-windows-msvc'

if ($env:CLAUDETTE_VERSION) {
    $Tag = 'v' + ($env:CLAUDETTE_VERSION -replace '^v','')
} else {
    Info 'resolving latest release tag...'
    try {
        $rel = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest" -UseBasicParsing
        $Tag = $rel.tag_name
    } catch {
        Fail "could not resolve latest tag from GitHub API: $($_.Exception.Message)"
    }
    if (-not $Tag) { Fail 'GitHub API returned an empty tag_name' }
}

$Stem    = "claudette-$Tag-$Target"
$Archive = "$Stem.zip"
$Url     = "https://github.com/$Repo/releases/download/$Tag/$Archive"
$ShaUrl  = "$Url.sha256"

$tmp = New-Item -ItemType Directory -Path (Join-Path $env:TEMP "claudette-install-$([guid]::NewGuid().ToString('N'))")
try {
    Info "downloading $Archive"
    try {
        Invoke-WebRequest -Uri $Url    -OutFile (Join-Path $tmp $Archive)         -UseBasicParsing
        Invoke-WebRequest -Uri $ShaUrl -OutFile (Join-Path $tmp "$Archive.sha256") -UseBasicParsing
    } catch {
        Fail "download failed: $($_.Exception.Message) (does this release exist?)"
    }

    Info 'verifying SHA256'
    $expected = ((Get-Content (Join-Path $tmp "$Archive.sha256") -Raw) -split '\s+')[0].Trim().ToLower()
    $actual   = (Get-FileHash (Join-Path $tmp $Archive) -Algorithm SHA256).Hash.ToLower()
    if ($expected -ne $actual) {
        Fail "checksum mismatch — refusing to install (expected $expected, got $actual)"
    }

    Info 'extracting'
    Expand-Archive -Path (Join-Path $tmp $Archive) -DestinationPath $tmp -Force

    if (-not (Test-Path $InstallDir)) {
        New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    }
    Copy-Item -Path (Join-Path $tmp 'claudette.exe') -Destination (Join-Path $InstallDir 'claudette.exe') -Force

    Info "installed $Tag -> $(Join-Path $InstallDir 'claudette.exe')"
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

# PATH check — read the User-scope Path, not the process-scope one (the
# process Path is built from User + System + injections and doesn't reflect
# what a fresh terminal would see).
$userPath = [System.Environment]::GetEnvironmentVariable('Path', 'User')
if (-not $userPath) { $userPath = '' }
$onPath = $false
foreach ($p in ($userPath -split ';')) {
    if ($p -and ([System.IO.Path]::GetFullPath($p) -eq [System.IO.Path]::GetFullPath($InstallDir))) {
        $onPath = $true; break
    }
}

if ($onPath) {
    Info 'next: claudette --doctor'
} else {
    Write-Host ''
    Warn "$InstallDir is not on your User PATH."
    Write-Host '  Add it with this command, then restart your terminal:'
    Write-Host ''
    Write-Host "    [System.Environment]::SetEnvironmentVariable('Path', '$InstallDir;' + [System.Environment]::GetEnvironmentVariable('Path','User'), 'User')"
    Write-Host ''
    Write-Host '  Then run: claudette --doctor'
}
