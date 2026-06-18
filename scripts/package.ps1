<#
.SYNOPSIS
    Builds a release of Dinosaur and bundles it into a portable, distributable ZIP.

.DESCRIPTION
    Produces  dist/Dinosaur-<version>-win64/  containing the executable and the
    Mesa OpenGL DLLs it needs to run on machines without a usable GPU driver,
    then zips that folder to  dist/Dinosaur-<version>-win64.zip .

    The end user simply unzips and runs Dinosaur.exe — no install, no admin.

.PARAMETER Version
    Version string for the package name. Defaults to the version in Cargo.toml.

.PARAMETER SkipBuild
    Reuse the existing target/release output instead of rebuilding.

.EXAMPLE
    pwsh scripts/package.ps1
#>
[CmdletBinding()]
param(
    [string]$Version,
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'

# We build the GNU (mingw-w64) Windows target explicitly so the output is the
# same on a GNU-host dev machine and an MSVC-host CI runner.
$Target = 'x86_64-pc-windows-gnu'

# Repo layout: <repo>/scripts/package.ps1 , <repo>/app , <repo>/dist
$RepoRoot = Split-Path $PSScriptRoot -Parent
$AppDir   = Join-Path $RepoRoot 'app'
$RelDir   = Join-Path $AppDir   "target\$Target\release"
$DistDir  = Join-Path $RepoRoot 'dist'

# Resolve version from Cargo.toml if not supplied.
if (-not $Version) {
    $cargo = Get-Content (Join-Path $AppDir 'Cargo.toml') -Raw
    if ($cargo -match '(?m)^\s*version\s*=\s*"([^"]+)"') { $Version = $Matches[1] }
    else { $Version = '0.0.0' }
}

Write-Host "Packaging Dinosaur v$Version" -ForegroundColor Cyan

# 1. Build (unless skipped). Kill any running instance so the exe isn't locked.
if (-not $SkipBuild) {
    Get-Process Dinosaur -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Seconds 1
    Push-Location $AppDir
    try { cargo build --release --target $Target } finally { Pop-Location }
}

# 2. Stage the required files.
$files = @(
    'Dinosaur.exe',
    'opengl32.dll',
    'libgallium_wgl.dll'
)

$stageName = "Dinosaur-$Version-win64"
$stageDir  = Join-Path $DistDir $stageName
if (Test-Path $stageDir) { Remove-Item $stageDir -Recurse -Force }
New-Item -ItemType Directory -Path $stageDir -Force | Out-Null

foreach ($f in $files) {
    $src = Join-Path $RelDir $f
    if (-not (Test-Path $src)) { throw "Missing build artifact: $src (did the build succeed?)" }
    Copy-Item $src -Destination $stageDir
}

# Optional: include a short readme so users know how to run it.
@"
Dinosaur $Version — large-file viewer
=====================================

To run:  double-click Dinosaur.exe
Open a file from the toolbar, drag & drop, or:  Dinosaur.exe path\to\file.csv

Keep all files in this folder together — the .dll files are required for
the app to render on machines without a dedicated GPU.
"@ | Set-Content (Join-Path $stageDir 'README.txt') -Encoding UTF8

# 3. Zip it.
$zipPath = Join-Path $DistDir "$stageName.zip"
if (Test-Path $zipPath) { Remove-Item $zipPath -Force }
Compress-Archive -Path (Join-Path $stageDir '*') -DestinationPath $zipPath

$zipMB = [math]::Round((Get-Item $zipPath).Length / 1MB, 1)
Write-Host "Created $zipPath ($zipMB MB)" -ForegroundColor Green
