# Build a double-clickable Windows package for QQ-Drawer.
#
# Usage (normal PowerShell window):
#     .\scripts\build.ps1
#
# Produces, under R:\QQ-Drawer\:
#   - QQ-Drawer-Setup.exe   NSIS installer (per-user, Start Menu shortcut)
#   - QQ-Drawer.exe         portable executable (needs WebView2 Runtime)
#
# It follows the same environment rules as dev.ps1: the repo path contains a
# space (R:\Code\QQ Drawer), so CARGO_TARGET_DIR must point somewhere space-free
# or tauri-winres/windres fails. See README section 5.7.

$ErrorActionPreference = 'Stop'

$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo

foreach ($dir in @('R:\Rust\toolchain\bin', 'R:\Rust\mingw64\bin')) {
    if (Test-Path $dir) {
        $env:PATH = "$dir;$env:PATH"
    } else {
        Write-Warning "Not found: $dir -- the Rust toolchain may be missing."
    }
}

if (Test-Path 'R:\Rust\cargo') {
    $env:CARGO_HOME = 'R:\Rust\cargo'
}

$env:CARGO_TARGET_DIR = Join-Path $env:TEMP 'qq-drawer-target'
New-Item -ItemType Directory -Force -Path $env:CARGO_TARGET_DIR | Out-Null

$npm = Get-Command npm -ErrorAction SilentlyContinue
if ($null -eq $npm) {
    Write-Error 'npm is not on PATH. Install Node.js first, or add its directory to PATH.'
}
$npmCmd = $npm.Source

if (-not (Test-Path (Join-Path $repo 'node_modules'))) {
    Write-Host 'Installing frontend dependencies (npm install) ...' -ForegroundColor Cyan
    & $npmCmd install
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

Write-Host 'Building release bundle (npm run tauri build -- --bundles nsis) ...' -ForegroundColor Cyan
& $npmCmd run tauri build -- --bundles nsis
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# ---- Collect artifacts into a stable, easy-to-find folder ----
$releaseRoot = $env:CARGO_TARGET_DIR
$portable = Join-Path $releaseRoot 'release\qq-drawer.exe'
$nsisDir = Join-Path $releaseRoot 'release\bundle\nsis'

$dest = 'R:\QQ-Drawer'
New-Item -ItemType Directory -Force -Path $dest | Out-Null

if (Test-Path $portable) {
    Copy-Item $portable (Join-Path $dest 'QQ-Drawer.exe') -Force
    Write-Host "portable: $(Join-Path $dest 'QQ-Drawer.exe')"
} else {
    Write-Warning "portable exe not found: $portable"
}

# WebView2Loader.dll 要和 exe 放一起（Tauri Windows 构建会产出它）
$loader = Join-Path $releaseRoot 'release\WebView2Loader.dll'
if (Test-Path $loader) {
    Copy-Item $loader (Join-Path $dest 'WebView2Loader.dll') -Force
}

if (Test-Path $nsisDir) {
    Get-ChildItem $nsisDir -Filter '*.exe' | ForEach-Object {
        $target = Join-Path $dest 'QQ-Drawer-Setup.exe'
        Copy-Item $_.FullName $target -Force
        Write-Host "installer: $target"
    }
} else {
    Write-Warning "NSIS output not found: $nsisDir"
}

Write-Host ''
Write-Host "Done. Everything is in: $dest" -ForegroundColor Green
