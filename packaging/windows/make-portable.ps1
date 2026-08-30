#Requires -Version 5.1
<#
.SYNOPSIS
    Stages and zips the Log Lens Windows portable Artifact.

.DESCRIPTION
    The portable flavour is the same binary as the installer ships, in a zip,
    for people who cannot or will not install anything. Plan step 4.4 fixes its
    name; this script fixes its layout.

    Layout — no zipbombs, mirroring the Linux tarball's single top-level
    directory (plan 4.4). The directory is the archive's own basename:

        LogLens-<version>-windows-x86_64-portable.zip
        └── LogLens-<version>-windows-x86_64-portable/
            ├── loglens.exe      the same binary the installer ships
            └── README.txt       portable-README.txt, verbatim

    No icon file: the exe already carries the multi-resolution icon compiled in
    by build.rs, and a loose .ico beside it would do nothing on Windows.

    NO install-manifest.json. That absence is load-bearing, not an oversight —
    the marker is what tells an Installer-managed copy from a Portable one, and
    a portable copy carrying one claiming "installer" is precisely the
    confusion the Update path's directory check exists to catch. Do not add one
    here, and do not copy one in from a local installed copy while testing.

.PARAMETER Version
    The release version, without the leading "v" of the tag (e.g. 0.1.0). Used
    in both the archive name and the directory inside it.

.PARAMETER SourceExe
    The freshly built binary. Defaults to target\release\loglens.exe under the
    repository root.

.PARAMETER OutputDir
    Where the .zip is written. Defaults to dist\ under the repository root —
    the same directory loglens.iss writes the setup Artifact to, so the release
    job can upload both from one place.

.EXAMPLE
    # What the build-windows job runs, from the repository root, after
    # `cargo build --release` and after `iscc` has produced the setup exe:
    pwsh packaging\windows\make-portable.ps1 -Version 0.1.0

    # -> dist\LogLens-0.1.0-windows-x86_64-portable.zip
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string] $Version,

    [string] $SourceExe,

    [string] $OutputDir
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Resolve-Path (Join-Path $scriptDir '..\..')

if (-not $SourceExe) { $SourceExe = Join-Path $repoRoot 'target\release\loglens.exe' }
if (-not $OutputDir) { $OutputDir = Join-Path $repoRoot 'dist' }

if (-not (Test-Path -LiteralPath $SourceExe -PathType Leaf)) {
    throw "make-portable.ps1: no binary at $SourceExe — run 'cargo build --release' first, or pass -SourceExe."
}

$readme = Join-Path $scriptDir 'portable-README.txt'
if (-not (Test-Path -LiteralPath $readme -PathType Leaf)) {
    throw "make-portable.ps1: missing $readme."
}

# Artifact naming is a contract (plan 4.4). The Update check matches Release
# assets by name, so renaming this breaks self-update for every already
# installed copy.
$baseName = "LogLens-$Version-windows-x86_64-portable"
$zipPath = Join-Path $OutputDir "$baseName.zip"

# Stage in a scratch directory rather than assembling the zip entry by entry:
# Compress-Archive names entries after the directory it is handed, which is the
# whole point of staging under $baseName.
$stageRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("loglens-portable-" + [System.Guid]::NewGuid().ToString('N'))
$stageDir = Join-Path $stageRoot $baseName

try {
    New-Item -ItemType Directory -Path $stageDir -Force | Out-Null

    Copy-Item -LiteralPath $SourceExe -Destination (Join-Path $stageDir 'loglens.exe') -Force
    Copy-Item -LiteralPath $readme -Destination (Join-Path $stageDir 'README.txt') -Force

    New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null
    # A stale zip would otherwise be appended to rather than replaced.
    if (Test-Path -LiteralPath $zipPath) { Remove-Item -LiteralPath $zipPath -Force }

    Compress-Archive -Path $stageDir -DestinationPath $zipPath -CompressionLevel Optimal

    Write-Host "Wrote $zipPath"
    Write-Host "  $baseName/loglens.exe"
    Write-Host "  $baseName/README.txt"
}
finally {
    if (Test-Path -LiteralPath $stageRoot) {
        Remove-Item -LiteralPath $stageRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
