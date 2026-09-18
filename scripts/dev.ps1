# Start the QQ-Drawer dev environment (Vite + Tauri + Rust).
#
# Usage (run in a normal PowerShell window, NOT inside a sandbox/automation host):
#     .\scripts\dev.ps1
#
# What it does:
#   1. Prepends the R: drive Rust toolchain (rustc/cargo + mingw64) to PATH.
#   2. Points CARGO_TARGET_DIR at a space-free path. This repo lives at
#      R:\Code\QQ Drawer, and the space in that path makes tauri-winres pass an
#      unquoted path to windres -> gcc, which splits it and fails with
#      "gcc: error: Drawer\src-tauri\target\...: No such file or directory".
#      See README section 5.7.
#   3. Runs npm install on first launch.
#
# To quit: right-click the tray icon. The window is skipTaskbar + closable:false,
# so it never shows up in the taskbar or Alt+Tab.

$ErrorActionPreference = 'Stop'

$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo

# ---- Rust toolchain (installed on R: on this machine) ----
foreach ($dir in @('R:\Rust\toolchain\bin', 'R:\Rust\mingw64\bin')) {
    if (Test-Path $dir) {
        $env:PATH = "$dir;$env:PATH"
    } else {
        Write-Warning "Not found: $dir -- the Rust toolchain may be missing; the build will fail."
    }
}

if (Test-Path 'R:\Rust\cargo') {
    $env:CARGO_HOME = 'R:\Rust\cargo'
}

# ---- The important bit: target dir must not contain spaces ----
$env:CARGO_TARGET_DIR = Join-Path $env:TEMP 'qq-drawer-target'
New-Item -ItemType Directory -Force -Path $env:CARGO_TARGET_DIR | Out-Null

# ---- npm ----
$npm = Get-Command npm -ErrorAction SilentlyContinue
if ($null -eq $npm) {
    Write-Error 'npm is not on PATH. Install Node.js first, or add its directory to PATH.'
}
$npmCmd = $npm.Source

if (-not (Test-Path (Join-Path $repo 'node_modules'))) {
    Write-Host 'First run: installing frontend dependencies (npm install) ...' -ForegroundColor Cyan
    & $npmCmd install
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

Write-Host ''
Write-Host "repo:             $repo"
Write-Host "CARGO_HOME:       $env:CARGO_HOME"
Write-Host "CARGO_TARGET_DIR: $env:CARGO_TARGET_DIR"
Write-Host ''

& $npmCmd run tauri dev
exit $LASTEXITCODE
