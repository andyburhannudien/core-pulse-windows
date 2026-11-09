# Shared helpers for the release scripts.
#
# The toolchain is expected to live outside the repository so a machine can move
# it as one unit. Resolution is a search, not a hardcoded drive:
#
#   1. CORE_PULSE_TOOLS            - explicit override
#   2. <repo>\..\..\Tools          - sibling of the checkout (current layout)
#   3. C:\Tools, D:\Tools          - historical locations
#
# Rust itself lives in <tools>\rust (RUSTUP_HOME and CARGO_HOME), so the
# registry cache and the toolchain travel together.

$script:RepoRoot = Split-Path -Parent $PSScriptRoot

function Get-RepoRoot {
    return $script:RepoRoot
}

function Get-ProjectVersion {
    param([string]$Root)

    $cargoToml = Join-Path $Root "Cargo.toml"
    if (-not (Test-Path $cargoToml)) { return "0.1.0" }

    $content = Get-Content $cargoToml -Raw
    $match = [regex]::Match($content, '(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"')
    if ($match.Success) { return $match.Groups[1].Value }

    return "0.1.0"
}

function Resolve-ToolsRoot {
    if ($env:CORE_PULSE_TOOLS -and (Test-Path $env:CORE_PULSE_TOOLS)) {
        return (Resolve-Path $env:CORE_PULSE_TOOLS).Path
    }

    $repo = Get-RepoRoot
    $sibling = Join-Path (Split-Path -Parent (Split-Path -Parent $repo)) "Tools"
    $candidates = @($sibling, "C:\Tools", "D:\Tools")

    foreach ($candidate in $candidates) {
        if ($candidate -and (Test-Path $candidate)) { return (Resolve-Path $candidate).Path }
    }

    return $null
}

function Resolve-Cargo {
    $candidates = @()
    if ($env:CORE_PULSE_CARGO) { $candidates += $env:CORE_PULSE_CARGO }

    $tools = Resolve-ToolsRoot
    if ($tools) { $candidates += (Join-Path $tools "rust\cargo\bin\cargo.exe") }

    $candidates += (Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe")

    foreach ($candidate in $candidates) {
        if ($candidate -and (Test-Path $candidate)) { return $candidate }
    }

    $command = Get-Command cargo -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }

    throw ("cargo not found. Install Rust into the toolchain folder, or " +
           "set CORE_PULSE_CARGO to cargo.exe.")
}

function Resolve-Rustup {
    $candidates = @()
    if ($env:CORE_PULSE_RUSTUP) { $candidates += $env:CORE_PULSE_RUSTUP }

    $tools = Resolve-ToolsRoot
    if ($tools) { $candidates += (Join-Path $tools "rust\cargo\bin\rustup.exe") }

    $candidates += (Join-Path $env:USERPROFILE ".cargo\bin\rustup.exe")

    foreach ($candidate in $candidates) {
        if ($candidate -and (Test-Path $candidate)) { return $candidate }
    }

    $command = Get-Command rustup -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }

    return $null
}

function Resolve-Makensis {
    $candidates = @()
    if ($env:CORE_PULSE_MAKENSIS) { $candidates += $env:CORE_PULSE_MAKENSIS }

    $tools = Resolve-ToolsRoot
    if ($tools) {
        $candidates += (Join-Path $tools "nsis\nsis-3.13\makensis.exe")
        $candidates += (Join-Path $tools "nsis\makensis.exe")
    }

    foreach ($candidate in $candidates) {
        if ($candidate -and (Test-Path $candidate)) { return $candidate }
    }

    $command = Get-Command makensis -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }

    throw ("makensis not found. Extract the portable NSIS ZIP into <tools>\nsis, " +
           "or set CORE_PULSE_MAKENSIS to makensis.exe.")
}

function Set-ToolchainEnvironment {
    $tools = Resolve-ToolsRoot
    $cargo = Resolve-Cargo
    $cargoBin = Split-Path -Parent $cargo

    if (-not $env:RUSTUP_HOME -and $tools) {
        $rustupHome = Join-Path $tools "rust\rustup"
        if (Test-Path $rustupHome) { $env:RUSTUP_HOME = $rustupHome }
    }

    if (-not $env:CARGO_HOME -and $tools) {
        $cargoHome = Join-Path $tools "rust\cargo"
        if (Test-Path $cargoHome) { $env:CARGO_HOME = $cargoHome }
    }

    $pathParts = $env:PATH -split ";"
    if ($pathParts -notcontains $cargoBin) {
        $env:PATH = $cargoBin + ";" + $env:PATH
    }

    $env:CARGO_TERM_COLOR = "always"
    return $cargo
}

function Get-ArtifactsDir {
    param([string]$Root)

    $dir = Join-Path $Root "artifacts"
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    return $dir
}

function Get-ReleaseDir {
    param([string]$Root)

    return (Join-Path $Root "target\release")
}

function Assert-ReleaseBinaries {
    param([string]$Root)

    $release = Get-ReleaseDir -Root $Root
    $missing = @()
    foreach ($name in @("core-pulse.exe", "core-pulse-cli.exe", "core-pulse-helper.exe")) {
        if (-not (Test-Path (Join-Path $release $name))) { $missing += $name }
    }

    if ($missing.Count -gt 0) {
        throw ("Release binaries missing: " + ($missing -join ", ") +
               ". Run: .\build.ps1 -Target Build")
    }
}

function Write-Sha256 {
    param(
        [string]$Path,
        [string]$OutputDir
    )

    $hash = (Get-FileHash $Path -Algorithm SHA256).Hash
    $name = Split-Path -Leaf $Path
    $line = "$hash  $name"
    $sums = Join-Path $OutputDir "SHA256SUMS.txt"

    # Wrap in @() so a single existing line stays an array. Without it,
    # `+=` concatenates two strings and the hashes end up on one line.
    $lines = @()
    if (Test-Path $sums) {
        $lines = @(Get-Content $sums | Where-Object { $_ -notmatch "\s$([regex]::Escape($name))$" })
    }
    $lines += $line
    $lines | Set-Content $sums -Encoding ASCII

    return $line
}
