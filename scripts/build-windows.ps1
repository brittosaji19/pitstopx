#requires -Version 5
<#
.SYNOPSIS
    Build the PitStopX Windows production installers (.msi + NSIS .exe).

.DESCRIPTION
    Wraps `tauri build`, ensuring the prerequisites this project needs are in
    place first:
      * Node/npm on PATH (frontend bundling + the Tauri CLI),
      * clang/LLVM on PATH (the `ring` TLS crate needs it to assemble on
        aarch64-windows),
      * a known-good Rust toolchain (the box default can miscompile `time`),
      * the generated icon set (icon.ico is required by the Windows resource
        step).
    Outputs land in src-tauri\target\release\bundle\ (msi\ and nsis\).

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1

.EXAMPLE
    # Only the NSIS installer, default debug-free release:
    .\scripts\build-windows.ps1 -Bundles nsis
#>
[CmdletBinding()]
param(
    # Rust toolchain to build with. The machine default (1.96) has a codegen
    # bug; 1.88 is verified working. Pass 'default' to use whatever is active.
    [string]$RustToolchain = '1.88.0',

    # Where Node lives if it isn't already on PATH.
    [string]$NodeDir = 'C:\Node\node-v24.16.0-win-arm64',

    # Where clang/LLVM lives if it isn't already on PATH.
    [string]$LlvmBin = 'C:\Program Files\LLVM\bin',

    # Installer formats to produce.
    [ValidateSet('nsis', 'msi')]
    [string[]]$Bundles = @('nsis', 'msi'),

    # Skip `npm install` even when node_modules is absent.
    [switch]$NoInstall
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot

function Add-ToPath([string]$dir) {
    if ($dir -and (Test-Path $dir) -and ($env:Path -notlike "*$dir*")) {
        $env:Path = "$dir;$env:Path"
    }
}

function Assert-Command([string]$name, [string]$hint) {
    if (-not (Get-Command $name -ErrorAction SilentlyContinue)) {
        throw "Required tool '$name' not found on PATH. $hint"
    }
}

Write-Host '==> Preparing environment' -ForegroundColor Cyan
Add-ToPath $NodeDir
Add-ToPath $LlvmBin

# Pin the Rust toolchain for every cargo invocation `tauri build` makes.
if ($RustToolchain -and $RustToolchain -ne 'default') {
    $env:RUSTUP_TOOLCHAIN = $RustToolchain
    Write-Host "    Rust toolchain : $RustToolchain (RUSTUP_TOOLCHAIN)"
}

Assert-Command node   'Install Node 20+ or pass -NodeDir.'
Assert-Command npm    'Install npm (ships with Node).'
Assert-Command cargo  'Install Rust (https://rustup.rs).'
Assert-Command clang  "Install LLVM or pass -LlvmBin (needed by the ring TLS crate)."

Write-Host "    node           : $(node --version)"
Write-Host "    cargo          : $(cargo --version)"
Write-Host "    clang          : $((clang --version | Select-Object -First 1))"

Push-Location $repo
try {
    # 1) Frontend dependencies (also provides the Tauri CLI via npx).
    if (-not $NoInstall -and -not (Test-Path "$repo\node_modules")) {
        Write-Host '==> Installing frontend dependencies (npm install)' -ForegroundColor Cyan
        npm install
        if ($LASTEXITCODE -ne 0) { throw 'npm install failed.' }
    }

    # 2) Icon set (icon.ico is required by tauri-build on Windows).
    if (-not (Test-Path "$repo\src-tauri\icons\icon.ico")) {
        Write-Host '==> Generating icon set' -ForegroundColor Cyan
        $srcPng = Join-Path $repo 'scripts\source-icon.png'
        node scripts\gen-source-png.mjs $srcPng
        if ($LASTEXITCODE -ne 0) { throw 'icon source generation failed.' }
        npx tauri icon $srcPng
        if ($LASTEXITCODE -ne 0) { throw 'tauri icon failed.' }
        Remove-Item $srcPng -ErrorAction SilentlyContinue
    }

    # 3) Production build. `tauri build` runs `npm run build` (Vite) first via
    #    beforeBuildCommand, then `cargo build --release` + bundling.
    $bundleArg = ($Bundles -join ',')
    Write-Host "==> Building Windows installers (bundles: $bundleArg)" -ForegroundColor Cyan
    npx tauri build --bundles $bundleArg
    if ($LASTEXITCODE -ne 0) { throw "tauri build failed (exit $LASTEXITCODE)." }
}
finally {
    Pop-Location
}

# Report artifacts.
$bundleRoot = Join-Path $repo 'src-tauri\target\release\bundle'
Write-Host ''
Write-Host '==> Done. Installers:' -ForegroundColor Green
if (Test-Path $bundleRoot) {
    Get-ChildItem -Path $bundleRoot -Recurse -Include '*.msi', '*.exe' -ErrorAction SilentlyContinue |
        ForEach-Object { Write-Host "    $($_.FullName)" }
}
else {
    Write-Warning "No bundle directory at $bundleRoot"
}
