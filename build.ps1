# Build, test, and package core-pulse.
#
# Usage:
#   .\build.ps1                     # release build + tests + portable ZIP + installer
#   .\build.ps1 -Target Build       # release build only
#   .\build.ps1 -Target Test        # tests only
#   .\build.ps1 -Target Check       # fmt + clippy
#   .\build.ps1 -Target Portable    # portable ZIP (builds release if needed)
#   .\build.ps1 -Target Installer   # NSIS installer (builds release if needed)
#   .\build.ps1 -SkipTests          # All without running tests
#   .\build.ps1 -Offline            # use only the crate cache in Tools\rust
#
# The toolchain is resolved by scripts\_common.ps1: CORE_PULSE_TOOLS, then a
# sibling Tools folder, then C:\Tools / D:\Tools. Rust lives in <tools>\rust.

param(
    [ValidateSet("Build", "Test", "Check", "Portable", "Installer", "All")]
    [string]$Target = "All",
    [switch]$SkipTests,
    [switch]$Offline
)

$ErrorActionPreference = "Stop"

. (Join-Path $PSScriptRoot "scripts\_common.ps1")

$root = Get-RepoRoot
$cargo = Set-ToolchainEnvironment
$version = Get-ProjectVersion -Root $root

if ($Offline) {
    $env:CARGO_NET_OFFLINE = "true"
}

function Invoke-Cargo {
    param([string[]]$Arguments)

    Write-Host ("cargo " + ($Arguments -join " ")) -ForegroundColor DarkGray
    & $cargo @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "cargo failed with exit code $LASTEXITCODE"
    }
}

function Invoke-Build {
    Invoke-Cargo @("build", "--release", "--workspace", "--locked")
}

function Ensure-Release {
    $release = Get-ReleaseDir -Root $root
    $missing = @()
    foreach ($name in @("core-pulse.exe", "core-pulse-cli.exe", "core-pulse-helper.exe")) {
        if (-not (Test-Path (Join-Path $release $name))) { $missing += $name }
    }

    if ($missing.Count -gt 0) {
        Write-Host ("release binaries missing: " + ($missing -join ", ")) -ForegroundColor Yellow
        Invoke-Build
    }
}

Write-Host ""
Write-Host "core-pulse $version" -ForegroundColor White
Write-Host "  repo    : $root"
Write-Host "  cargo   : $cargo"
Write-Host "  target  : $Target"
Write-Host ""

switch ($Target) {
    "Build" {
        Invoke-Build
    }
    "Test" {
        Invoke-Cargo @("test", "--workspace", "--locked")
    }
    "Check" {
        Invoke-Cargo @("fmt", "--all", "--check")
        Invoke-Cargo @("clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings")
    }
    "Portable" {
        Ensure-Release
        & (Join-Path $root "scripts\package-portable.ps1") -Version $version
        if (-not $?) { throw "portable packaging failed" }
    }
    "Installer" {
        Ensure-Release
        & (Join-Path $root "scripts\make-installer.ps1") -Version $version
        if (-not $?) { throw "installer packaging failed" }
    }
    "All" {
        Invoke-Build
        if (-not $SkipTests) {
            Invoke-Cargo @("test", "--workspace", "--locked")
        }
        & (Join-Path $root "scripts\package-portable.ps1") -Version $version
        if (-not $?) { throw "portable packaging failed" }
        & (Join-Path $root "scripts\make-installer.ps1") -Version $version
        if (-not $?) { throw "installer packaging failed" }
    }
}

Write-Host ""
Write-Host "done" -ForegroundColor Green
