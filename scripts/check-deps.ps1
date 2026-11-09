# Checks that everything needed to build, test and package core-pulse is present.
#
# Usage:
#   .\scripts\check-deps.ps1              # report only
#   .\scripts\check-deps.ps1 -Json        # machine-readable
#
# The toolchain lives outside the repository (see scripts\_common.ps1), so a
# moved checkout fails loudly here instead of halfway through a release.

param(
    [switch]$Json
)

$ErrorActionPreference = "Stop"

. (Join-Path $PSScriptRoot "_common.ps1")

$root = Get-RepoRoot
$tools = Resolve-ToolsRoot
$checks = @()

function Add-Check {
    param(
        [string]$Name,
        [bool]$Ok,
        [string]$Detail,
        [string]$FixHint
    )

    $script:checks += [pscustomobject]@{
        name = $Name
        ok = $Ok
        detail = $Detail
        fix = if ($Ok) { "" } else { $FixHint }
    }
}

function Find-MsvcLink {
    $bases = @()

    $vswhere = "C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path $vswhere) {
        $install = & $vswhere -latest -products * `
            -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
            -property installationPath 2>$null
        if ($install) { $bases += $install.Trim() }
    }

    if ($tools) {
        $local = Join-Path $tools "msvc"
        if (Test-Path $local) { $bases += $local }
    }

    foreach ($base in $bases) {
        $toolsDir = Join-Path $base "VC\Tools\MSVC"
        if (-not (Test-Path $toolsDir)) { continue }

        $versions = Get-ChildItem $toolsDir -Directory -ErrorAction SilentlyContinue |
            Sort-Object Name -Descending
        foreach ($version in $versions) {
            $link = Join-Path $version.FullName "bin\Hostx64\x64\link.exe"
            if (Test-Path $link) { return $link }
        }
    }

    return $null
}

function Find-WindowsSdk {
    $sdkRoot = "C:\Program Files (x86)\Windows Kits\10"
    $libRoot = Join-Path $sdkRoot "Lib"
    if (-not (Test-Path $libRoot)) { return $null }

    $versions = Get-ChildItem $libRoot -Directory -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending
    foreach ($version in $versions) {
        $kernel = Join-Path $version.FullName "um\x64\kernel32.lib"
        $ucrt = Join-Path $version.FullName "ucrt\x64\ucrt.lib"
        if ((Test-Path $kernel) -and (Test-Path $ucrt)) { return $version.Name }
    }

    return $null
}

function Get-CargoHome {
    if ($env:CARGO_HOME) { return $env:CARGO_HOME }
    if ($tools) {
        $preferred = Join-Path $tools "rust\cargo"
        if (Test-Path $preferred) { return $preferred }
    }
    return (Join-Path $env:USERPROFILE ".cargo")
}

# ---------- Rust toolchain ----------

$cargo = $null
try {
    $cargo = Set-ToolchainEnvironment
    $cargoVersion = (& $cargo --version 2>&1 | Out-String).Trim()
    Add-Check "cargo" ([bool]$cargoVersion) "$cargo :: $cargoVersion" `
        "Install Rust into <tools>\rust, or set CORE_PULSE_CARGO."
}
catch {
    Add-Check "cargo" $false $_.Exception.Message `
        "Install Rust into <tools>\rust, or set CORE_PULSE_CARGO."
}

if ($cargo) {
    $cargoBin = Split-Path -Parent $cargo
    $rustc = Join-Path $cargoBin "rustc.exe"
    if (Test-Path $rustc) {
        $rustcVersion = (& $rustc --version 2>&1 | Out-String).Trim()
        Add-Check "rustc" ([bool]$rustcVersion) "$rustc :: $rustcVersion" `
            "Run: rustup default stable-x86_64-pc-windows-msvc"
    }
    else {
        Add-Check "rustc" $false "not next to cargo" "Run: rustup default stable-x86_64-pc-windows-msvc"
    }

    $rustup = Resolve-Rustup
    if ($rustup) {
        $active = (& $rustup show active-toolchain 2>&1 | Out-String).Trim()
        Add-Check "rustup toolchain" ([bool]$active) "$rustup :: $active" `
            "Run: rustup default stable-x86_64-pc-windows-msvc"

        $targets = (& $rustup target list --installed 2>&1 | Out-String)
        $hasTarget = $targets -match "x86_64-pc-windows-msvc"
        Add-Check "target msvc" $hasTarget "x86_64-pc-windows-msvc" `
            "Run: rustup target add x86_64-pc-windows-msvc"

        $components = (& $rustup component list --installed 2>&1 | Out-String)
        $hasRustfmt = $components -match "rustfmt"
        $hasClippy = $components -match "clippy"
        Add-Check "rustfmt" $hasRustfmt "installed" "Run: rustup component add rustfmt"
        Add-Check "clippy" $hasClippy "installed" "Run: rustup component add clippy"
    }
    else {
        Add-Check "rustup" $false "not found" "Install rustup into <tools>\rust."
    }
}

# ---------- Native linker / SDK ----------

$link = Find-MsvcLink
if ($link) {
    Add-Check "MSVC linker" $true $link ""
}
else {
    Add-Check "MSVC linker" $false "link.exe not found" `
        "Install Visual Studio Build Tools with the C++ workload (see docs\BUILD-DEPENDENCIES.md)."
}

$sdk = Find-WindowsSdk
if ($sdk) {
    Add-Check "Windows SDK" $true "$sdk" ""
}
else {
    Add-Check "Windows SDK" $false "kernel32.lib / ucrt.lib not found" `
        "Install Windows SDK 10.0.26100 (see docs\BUILD-DEPENDENCIES.md)."
}

# ---------- Crate cache ----------

$cargoHome = Get-CargoHome
$cache = Join-Path $cargoHome "registry\cache"
$crateCount = 0
if (Test-Path $cache) {
    $crateCount = (Get-ChildItem $cache -Recurse -Filter "*.crate" -File -ErrorAction SilentlyContinue |
        Measure-Object).Count
}

$lockPath = Join-Path $root "Cargo.lock"
$lockCount = 0
if (Test-Path $lockPath) {
    $lock = Get-Content $lockPath -Raw
    $lockCount = ([regex]::Matches($lock, "(?m)^name = ")).Count
}

Add-Check "Cargo.lock" ($lockCount -gt 0) "$lockPath ($lockCount packages)" `
    "Restore Cargo.lock from version control."
Add-Check "crate cache" ($crateCount -gt 0) "$cache ($crateCount .crate files)" `
    "Run: cargo fetch --locked"

# Counting .crate files is not enough: Cargo.lock also lists the six workspace
# members, and a cache can be partially populated. Let cargo itself prove that
# every locked dependency is present.
if ($cargo -and $crateCount -gt 0) {
    & $cargo fetch --locked --offline 2>&1 | Out-Null
    $offlineOk = ($LASTEXITCODE -eq 0)
    Add-Check "offline fetch" $offlineOk "cargo fetch --locked --offline" `
        "Run: cargo fetch --locked (with network)"
}

# ---------- Packaging ----------

try {
    $makensis = Resolve-Makensis
    $nsisVersion = (& $makensis /VERSION 2>&1 | Out-String).Trim()
    Add-Check "NSIS" ([bool]$nsisVersion) "$makensis :: $nsisVersion" `
        "Extract the portable NSIS ZIP into <tools>\nsis."
}
catch {
    Add-Check "NSIS" $false $_.Exception.Message `
        "Extract the portable NSIS ZIP into <tools>\nsis."
}

# ---------- Repository ----------

foreach ($required in @(
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "crates\ui\Cargo.toml",
    "installer\core-pulse.nsi",
    "scripts\package-portable.ps1",
    "scripts\make-installer.ps1")) {
    $path = Join-Path $root $required
    Add-Check "file $required" (Test-Path $path) $path "Re-clone or restore the repository."
}

$git = Get-Command git -ErrorAction SilentlyContinue
Add-Check "git" ([bool]$git) $(if ($git) { $git.Source } else { "not found" }) `
    "Install Git for Windows."

# ---------- Report ----------

if ($Json) {
    $checks | ConvertTo-Json -Depth 3
    exit $(if ($checks.Where({ -not $_.ok }).Count -gt 0) { 1 } else { 0 })
}

Write-Host ""
Write-Host "core-pulse build dependencies" -ForegroundColor White
Write-Host ""

foreach ($check in $checks) {
    if ($check.ok) {
        Write-Host ("  [ok]   {0,-24} {1}" -f $check.name, $check.detail) -ForegroundColor Green
    }
    else {
        Write-Host ("  [MISS] {0,-24} {1}" -f $check.name, $check.detail) -ForegroundColor Red
        if ($check.fix) { Write-Host ("         fix: {0}" -f $check.fix) -ForegroundColor Yellow }
    }
}

$failed = $checks.Where({ -not $_.ok }).Count
Write-Host ""

if ($failed -gt 0) {
    Write-Host "$failed check(s) failed. See docs\BUILD-DEPENDENCIES.md." -ForegroundColor Red
    exit 1
}

Write-Host "All dependencies present." -ForegroundColor Green
