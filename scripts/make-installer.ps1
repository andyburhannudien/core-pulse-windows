# Builds the per-user NSIS installer.
#
# The installer never asks for UAC: RequestExecutionLevel is "user", and the
# default install directory is %LOCALAPPDATA%\Programs\core-pulse.
#
# Usage:
#   .\scripts\make-installer.ps1
#   .\scripts\make-installer.ps1 -Version 0.1.0
#
# The release binaries must already exist. Run .\build.ps1 -Target Build first,
# or use .\build.ps1 -Target Installer which does both.

param(
    [string]$Version = ""
)

$ErrorActionPreference = "Stop"

. (Join-Path $PSScriptRoot "_common.ps1")

$root = Get-RepoRoot
$null = Set-ToolchainEnvironment

if (-not $Version) { $Version = Get-ProjectVersion -Root $root }

$release = Get-ReleaseDir -Root $root
Assert-ReleaseBinaries -Root $root

$makensis = Resolve-Makensis
$artifacts = Get-ArtifactsDir -Root $root
$out = Join-Path $artifacts "core-pulse-$Version-windows-x64-setup.exe"
$icon = Join-Path $root "installer\core-pulse.ico"

$defines = @(
    "/V2",
    "/DAPP_VERSION=$Version",
    "/DOUTFILE=$out",
    "/DSRC_DIR=$release",
    "/DDOCS_DIR=$(Join-Path $root 'docs')",
    "/DREADME=$(Join-Path $root 'README.md')"
)
if (Test-Path $icon) { $defines += "/DICON=$icon" }

Push-Location $root
try {
    & $makensis @defines "installer\core-pulse.nsi"
    if ($LASTEXITCODE -ne 0) {
        throw "makensis failed with exit code $LASTEXITCODE"
    }
}
finally {
    Pop-Location
}

if (-not (Test-Path $out)) {
    throw "installer was not created: $out"
}

$hashLine = Write-Sha256 -Path $out -OutputDir $artifacts
$sizeMb = [math]::Round((Get-Item $out).Length / 1MB, 2)

Write-Host ""
Write-Host "installer" -ForegroundColor White
Write-Host "  exe    : $out ($sizeMb MB)"
Write-Host "  sha256 : $hashLine" -ForegroundColor DarkGray
