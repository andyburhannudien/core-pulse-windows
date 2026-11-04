# Captures the UI tabs into docs/images.
#
# Requires the debug binary to be built:
#   cargo build -p core-pulse-ui
#
# Each tab is opened by passing the tab name as a command-line argument, so the
# capture never depends on clicking pixel coordinates.

param(
    [string]$Exe = ".\target\debug\core-pulse.exe",
    [string]$OutDir = "docs\images",
    [int]$SettleSeconds = 7
)

$ErrorActionPreference = "Stop"
$script = Join-Path $PSScriptRoot "screenshot.ps1"

if (-not (Test-Path $Exe)) {
    throw "UI binary not found at $Exe. Run: cargo build -p core-pulse-ui"
}
if (-not (Test-Path $OutDir)) {
    New-Item -ItemType Directory -Path $OutDir | Out-Null
}

function Stop-Ui {
    Get-Process -Name "core-pulse" -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 600
}

# Tab name -> output file. The tab name is also the CLI argument.
$tabs = [ordered]@{
    "processes"   = "processes.png"
    "performance" = "performance.png"
    "sensors"     = "sensors.png"
    "alerts"      = "alerts.png"
    "services"    = "services.png"
    "startup"     = "startup.png"
    "users"       = "users.png"
}

foreach ($tab in $tabs.Keys) {
    Stop-Ui
    Start-Process -FilePath $Exe -ArgumentList $tab
    Start-Sleep -Seconds $SettleSeconds
    $out = Join-Path $OutDir $tabs[$tab]
    & powershell -NoProfile -ExecutionPolicy Bypass -File $script -Output $out -MoveX 0 -MoveY 0 -ClientArea | Out-Null
    Write-Output "captured $($tabs[$tab])"
}

# Detail panel: open on the current explorer.exe so the panel has content.
Stop-Ui
$explorer = (Get-Process -Name explorer -ErrorAction SilentlyContinue | Select-Object -First 1).Id
if ($explorer) {
    Start-Process -FilePath $Exe -ArgumentList "detail:$explorer"
    Start-Sleep -Seconds $SettleSeconds
    & powershell -NoProfile -ExecutionPolicy Bypass -File $script -Output (Join-Path $OutDir "detail.png") -MoveX 0 -MoveY 0 -ClientArea | Out-Null
    Write-Output "captured detail.png"
}

Stop-Ui
Write-Output "done"
