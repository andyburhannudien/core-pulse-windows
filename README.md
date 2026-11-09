# core-pulse

A Windows system monitor with hardware temperature sensing, written in Rust.

`core-pulse` is the successor to the [disk-pulse](../disk-pulse) idea: if
`disk-pulse` answers "how healthy is my disk", `core-pulse` answers "what is
happening on this machine right now, and is anything running too hot".

```
+-----------------------------+         named pipe          +-----------------------------+
|  core-pulse.exe             |  <-------------------->     |  core-pulse-helper.exe      |
|  UI (egui/eframe)           |      JSON snapshot          |  Windows Service (elevated) |
|  non-elevated               |                             |  sensor tier 3 + SMART      |
+-------------+---------------+                             +--------------+--------------+
              |                                                            |
              | ArcSwap<Snapshot>                                          | IOCTL / vendor SDK
              |                                                            |
+-------------v---------------+                             +--------------v--------------+
|  core-pulse-core            |                             |  core-pulse-sensors         |
|  NtQuerySystemInformation   |                             |  acpi / smart / nvml / igcl |
|  PDH counters               |                             |  lhm-bridge                 |
|  per-process sampling 1 Hz  |                             +-----------------------------+
+-----------------------------+
```

The UI never asks for UAC. Every read that needs administrator rights (SMART,
low-level sensors, per-process ETW networking) is performed by an optional
helper and streamed over a local named pipe. If the helper is absent the UI
still runs and **states plainly** which sensors are unavailable.

![Processes tab](docs/images/processes.png)

![Performance tab](docs/images/performance.png)

![Sensors tab](docs/images/sensors.png)

*P4 build on an i5-9500T / 8 GB / Windows 11 build 26100. Disk temperature is
waiting for an elevated helper; that status is shown honestly rather than as a
fabricated number.*

## Status

| Area | Status |
|---|---|
| Architecture docs & ADRs | **Done** |
| Cargo workspace (6 crates) | **Done** — P0 |
| CPU / memory / process collector | **Working** |
| Disk / network / GPU collectors | **Working** — cross-checked, I/O ratio 1.00× |
| Process actions (end task, tree, suspend, priority, EcoQoS) | **Working** — PID-reuse guard tested |
| Process details (cmdline, user, integrity, signature) | **Working** |
| Services / Startup / Users tabs | **Working** — 714 services, 7 startup entries, 2 sessions |
| UI (process table, performance, sensors, alerts) | **Working** — virtualized table, six live charts |
| CLI (`top`, `doctor`, `bench`, `compare`, `verify`, `action-selftest`, `services`, `startup`, `users`, `sensors`, `helper`, `helper-selftest`) | **Working** |
| Gate G0 (CPU accuracy + perf budget) | **PASS** — [P0-RESULTS.md](docs/P0-RESULTS.md) |
| Gate G1 (disk/net/GPU + real I/O) | **PASS** — [P1-RESULTS.md](docs/P1-RESULTS.md) |
| Gate G2 (process actions + PID reuse) | **PASS** — [P2-RESULTS.md](docs/P2-RESULTS.md) |
| Gate G3 (sensors + SMART) | **Partial** — [P3-RESULTS.md](docs/P3-RESULTS.md) |
| Gate G4 (helper resilience + malformed IPC) | **PASS (non-elevated)** — [P4-RESULTS.md](docs/P4-RESULTS.md) |
| Sensor framework + SMART + GPU | **Working** — disk temperatures via SMART; CPU ACPI and NVIDIA driver unavailable on this machine |
| Sensors tab | **Working** — per-provider status, explicit elevation buttons |
| Alert engine | **Working** — sustained thresholds, dedup, 5-minute cooldown |
| Elevated helper | **Working** — foreground, service, and probe modes |
| Installer | **Working** — per-user NSIS + portable ZIP (P5) |

Measured on the development machine (i5-9500T, 8 GB, build 26100):

| Metric | Result |
|---|---|
| CPU% difference vs `GetSystemTimes` | 0.49% mean (max 0.89%) |
| Disk/GPU/network difference vs independent PDH | 0.00% |
| Real I/O verification (134 MB written) | ratio **1.00×** |
| PID-reuse guard | 6/6 scenarios passed |
| Enumerating 250 processes | p95 21 ms |
| Full snapshot build | p95 45 ms |
| Application CPU while idle | **0.40% of system** (release, maximised, P4) |
| UI private bytes | **44.0 MB** (release, P4) |
| UI binary size | **8.02 MB** (release, P4) |

Documentation was written first on purpose: the hardest decision in this
project is not "how to render a table" but **where CPU temperature comes from**,
and that has to be answered before a single line of code.

## Prerequisites

- Windows 10 1809+ / Windows 11 (development target: Windows 11 build 26100)
- Rust stable, target `x86_64-pc-windows-msvc`
- MSVC Build Tools + Windows SDK (not full Visual Studio)

Setup details are in [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

## Quick start

```powershell
cargo build --workspace

# Terminal monitor
cargo run -p core-pulse-cli -- top --watch 1

# GUI (maximised); opens on the Processes tab
cargo run -p core-pulse-ui

# Jump straight to a tab (useful for screenshots)
cargo run -p core-pulse-ui -- sensors

# Sensor reality check: which providers exist on *this* machine
cargo run -p core-pulse-cli -- sensors

# Same, including values the elevated helper provides (if it is running)
cargo run -p core-pulse-cli -- sensors --helper
```

Run the helper elevated **only when you choose to**, either from the Sensors
tab ("Start helper (UAC)") or manually:

```powershell
# This session only
Start-Process .\target\debug\core-pulse-helper.exe -Verb RunAs -ArgumentList "--serve"
```

The helper protocol is read-only. It never accepts a client-supplied path or
command, so a compromised UI process cannot ask it to touch arbitrary devices.

## Packaging

The release toolchain is portable and lives outside the repository. Verify it
first, then build the two distribution formats:

```powershell
.\scripts\check-deps.ps1          # Rust, MSVC, Windows SDK, crate cache, NSIS
.\build.ps1 -Target Portable      # artifacts\core-pulse-<version>-windows-x64-portable.zip
.\build.ps1 -Target Installer     # artifacts\core-pulse-<version>-windows-x64-setup.exe
.\build.ps1                       # build + test + portable + installer
```

The installer is per-user: `RequestExecutionLevel user`, default directory
`%LOCALAPPDATA%\Programs\core-pulse`, and no UAC prompt at install or startup.
The portable ZIP needs no installer and no registry writes. Both are written to
`artifacts/` with a `SHA256SUMS.txt`; setup details are in
[docs/BUILD-DEPENDENCIES.md](docs/BUILD-DEPENDENCIES.md).

## Keyboard shortcuts

| Shortcut | Action |
|---|---|
| `Ctrl+F` | Focus process search |
| `Ctrl+1` … `Ctrl+7` | Switch to Processes / Performance / Sensors / Alerts / Services / Startup / Users |
| `Esc` | Close the detail panel or the confirmation dialog |
| Double-click a row | Open the process detail panel |

## Documentation index

| Document | Contents |
|---|---|
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Crate boundaries, thread model, data flow, `unsafe` policy |
| [docs/DATA-MODEL.md](docs/DATA-MODEL.md) | `Snapshot` schema, units, versioning, JSON examples |
| [docs/SENSORS.md](docs/SENSORS.md) | Temperature source tiers, availability states, why there are no fake numbers |
| [docs/UI-SPEC.md](docs/UI-SPEC.md) | Layout, columns, shortcuts, theme, accessibility |
| [docs/PERF.md](docs/PERF.md) | Performance budget and how it is measured in CI |
| [docs/TESTING.md](docs/TESTING.md) | Test pyramid, fixtures, fuzz, manual checklist |
| [docs/THREAT-MODEL.md](docs/THREAT-MODEL.md) | Elevated helper risks, pipe ACL, PID-reuse guard |
| [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md) | Hardware/driver matrix and known limits |
| [docs/P0-RESULTS.md](docs/P0-RESULTS.md) | P0 results: Gate G0 evidence, measurements, findings |
| [docs/P1-RESULTS.md](docs/P1-RESULTS.md) | P1 results: Gate G1 evidence, real I/O verification |
| [docs/P2-RESULTS.md](docs/P2-RESULTS.md) | P2 results: Gate G2 evidence, PID-reuse guard |
| [docs/P3-RESULTS.md](docs/P3-RESULTS.md) | P3 results: Gate G3 (partial), SMART access probe, fixture replay |
| [docs/P4-RESULTS.md](docs/P4-RESULTS.md) | P4 results: Gate G4 evidence, helper IPC, alert engine |
| [docs/ENVIRONMENT-AUDIT.md](docs/ENVIRONMENT-AUDIT.md) | Development machine audit (facts, not assumptions) |
| [docs/ROADMAP.md](docs/ROADMAP.md) | Phases P0–P5 and their gates |
| [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) | Toolchain setup, build commands, troubleshooting |
| [docs/BUILD-DEPENDENCIES.md](docs/BUILD-DEPENDENCIES.md) | Portable toolchain, crate cache, packaging, offline builds |
| [docs/ADR/](docs/ADR/README.md) | Architecture Decision Records |
| [docs/OPEN-QUESTIONS.md](docs/OPEN-QUESTIONS.md) | Decisions still awaiting an answer |

## Non-goals (v1)

- Overclocking / fan control. This application reads; it does not drive
  hardware.
- A kernel driver of our own. See [ADR-0002](docs/ADR/0002-sensor-tiering.md).
- Outbound telemetry. There is no network egress except an opt-in update check.
- Cross-platform. Windows-specific code is the core, not a `#[cfg]` branch.

## Naming and scope

`core-pulse` is its own repository at `D:\Projects\core-pulse`, a sibling of
`disk-pulse`. The reasoning and consequences are in
[ADR-0007](docs/ADR/0007-repository-layout.md).
