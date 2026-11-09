# Builds the portable ZIP: no installer, no registry writes, no admin rights.
#
# Usage:
#   .\scripts\package-portable.ps1
#   .\scripts\package-portable.ps1 -Version 0.1.0
#
# The release binaries must already exist. Run .\build.ps1 -Target Build first,
# or use .\build.ps1 -Target Portable which does both.

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

$artifacts = Get-ArtifactsDir -Root $root
$stage = Join-Path $artifacts "core-pulse-$Version-windows-x64-portable"
$zip = Join-Path $artifacts "core-pulse-$Version-windows-x64-portable.zip"

if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

foreach ($name in @("core-pulse.exe", "core-pulse-cli.exe", "core-pulse-helper.exe")) {
    Copy-Item (Join-Path $release $name) (Join-Path $stage $name)
}

Copy-Item (Join-Path $root "README.md") (Join-Path $stage "README.md")

$docs = Join-Path $root "docs"
if (Test-Path $docs) {
    Copy-Item $docs (Join-Path $stage "docs") -Recurse
}

foreach ($license in @("LICENSE", "LICENSE-MIT", "LICENSE-APACHE")) {
    $path = Join-Path $root $license
    if (Test-Path $path) { Copy-Item $path (Join-Path $stage $license) }
}

$icon = Join-Path $root "installer\core-pulse.ico"
if (Test-Path $icon) { Copy-Item $icon (Join-Path $stage "core-pulse.ico") }

if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $zip -CompressionLevel Optimal

$hashLine = Write-Sha256 -Path $zip -OutputDir $artifacts
$sizeMb = [math]::Round((Get-Item $zip).Length / 1MB, 2)

Write-Host ""
Write-Host "portable package" -ForegroundColor White
Write-Host "  folder : $stage"
Write-Host "  zip    : $zip ($sizeMb MB)"
Write-Host "  sha256 : $hashLine" -ForegroundColor DarkGray
