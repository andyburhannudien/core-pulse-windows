# P4 results — helper, alerts, UI

**Date:** 2026-11-04
**Machine:** i5-9500T, 6 cores / 6 threads, 8 GB, Intel UHD 630, two SATA HDDs,
Windows 11 IoT Enterprise LTSC 2024 build 26100, running **non-admin**.

## Scope

P4 delivers four things:

1. An elevated helper with a read-only named-pipe protocol, runnable in the
   foreground or as a Windows Service.
2. A sensor merge layer on the client side that never fabricates values.
3. An alert engine with sustained thresholds, deduplication and cooldown.
4. A redesigned UI that reads like a technical instrument panel instead of a
   form.

History (SQLite) remains open; see "Not done" at the end.

## Gate G4

| Criterion | Result | Evidence |
|---|---|---|
| Helper killed mid-run → UI does not hang, status changes in < 2 s | **PASS** | `helper-selftest`: death detected in **0 ms**, classified `NotRunning` |
| Automatic reconnect when the helper returns | **PASS** | Client opens a fresh connection per poll round; 5 consecutive calls to a restarted helper all succeeded |
| Pipe rejects messages > 1 MB without crashing | **PASS** | Oversized frame (2,000,000 B header) rejected; helper kept serving |
| Pipe rejects invalid JSON without crashing | **PASS** | `not json` payload rejected; helper kept serving |
| Application runs normally without elevated features on a non-admin account | **PASS** | All screenshots and every test below were taken non-elevated |

`helper-selftest` (release build) output:

```
spawn: target\release\core-pulse-helper.exe
OK   pipe ready
OK   hello + sensors: version 0.1.0 elevated false (0 readings)
OK   frame >1 MiB rejected, helper still alive
OK   invalid JSON rejected, helper still alive
OK   helper death detected in 0 ms (NotRunning)

G4 helper-selftest: PASS
```

`action-selftest` (PID-reuse guard, against real processes):

```
  [PASS] correct identity → terminate succeeds
  [PASS] stale identity → rejected — PID 16088, expected 1793786780672 ms, actual 1793786785672 ms
  [PASS] after rejection the process is still alive
  [PASS] non-existent PID → rejected
  [PASS] critical process (PID 4) → rejected
  [PASS] wrong identity rejected for a non-destructive action

G2: PASS (all scenarios behaved as expected)
```

## What the helper actually provides on this machine

The helper was started without elevation (UAC approval is not automatable in
this environment). That is a *useful* case: it proves the client reports
"needs administrator" rather than inventing data.

```
$ core-pulse-cli helper
expected helper: target\debug\core-pulse-helper.exe
connected: version 0.1.0 (PID 11004), elevated: false

Provider status inside the helper:
  acpi-pdh       unavailable: firmware exposes no ACPI thermal zone (0 instances)
  smart          needs administrator
  gpu-vendor     unavailable: neither nvml.dll nor igcl.dll found
  helper         needs administrator
```

The `helper` provider reports `needs administrator` because the helper itself
is not elevated — exactly the honest signal we want. Elevating the helper is a
user action (button or `Start-Process -Verb RunAs`), never automatic.

## Helper architecture as built

| Concern | Implementation |
|---|---|
| Transport | Named pipe `\\.\pipe\core-pulse-v1`, message framing with a 1 MiB cap |
| Squatting guard | First instance uses `FILE_FLAG_FIRST_PIPE_INSTANCE` |
| ACL | `D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;AU)` — SYSTEM and Administrators full, Authenticated Users read/write |
| Client validation | WTS console session user compared against the client token SID, plus integrity level ≥ Medium |
| Server validation | Client reads the pipe server PID, resolves its image path, and requires `core-pulse-helper.exe` (override with `CORE_PULSE_HELPER_PATH`) |
| Read timeout | 2 s server-side; 1.5 s client-side with `CancelIoEx` on expiry |
| Request budget | 32 requests per connection, then the connection is closed |
| Protocol | `Hello { proto_version }` → `Hello`; `GetSensors` → `Sensors`; `GetSmart { disk_index }` → `Smart`; index range 0–63 |
| Service mode | `--service` SCM dispatcher, auto-start, quoted binary path, `SERVICE_ACCEPT_STOP \| SERVICE_ACCEPT_SHUTDOWN`, pipe wake on stop |

The server creates the **next** pipe instance *before* closing the current one.
Without that, a client connecting in the gap saw `ERROR_FILE_NOT_FOUND` and the
UI briefly believed the helper had died. This was found by the self-test, not by
inspection.

## Client behaviour when the helper dies

The sensor poller runs every two seconds and opens one short-lived connection
per round. When the helper disappears:

- the connection fails immediately (`WaitNamedPipeW` returns, `CreateFileW`
  fails with `ERROR_FILE_NOT_FOUND`),
- the round is classified `HelperError::NotRunning`,
- the report gains `helper: NeedsElevation`, and
- local readings are left untouched.

Nothing blocks, nothing retries in a loop, and no stale value is kept as if it
were current.

Measured: a CLI run with the helper killed 3.1 s in continued through all four
requested samples and exited normally after 7.4 s.

## Alert engine

`core-pulse-core::alerts` is a pure state machine over `Snapshot`:

| Rule | Value |
|---|---|
| CPU warning / critical | 90% / 97%, sustained 5 s |
| Memory warning / critical | 90% / 97%, sustained 5 s |
| Disk temperature (SMART) warning / critical | 55 °C / 60 °C, sustained 10 s |
| Other temperatures warning / critical | 80 °C / 85 °C, sustained 10 s |
| Free space warning / critical | < 5% / < 2%, sustained 60 s, volumes ≥ 1 GB only |
| Cooldown after clearing | 5 minutes |

Properties guaranteed by tests:

- a one-second spike produces **no** alert,
- a sustained condition raises exactly one event,
- an active condition is never re-notified,
- clearing emits one `Cleared` event and starts the cooldown,
- an alert cannot be raised from missing data (no sensor → no candidate),
- a volume smaller than 1 GB is ignored.

The engine lives in the sampler thread; the UI reads an `ArcSwap<AlertState>`,
so the render loop never blocks and never owns alert state. Both the banner at
the top of the content area and the Alerts tab read the same snapshot.

## UI rework

The UI was rebuilt around a fixed navigation rail, a command bar, and a status
strip. It is still `egui`/`glow`, still one binary, and still non-elevated.

| Element | Before | Now |
|---|---|---|
| Navigation | Horizontal tab row | 196 px left rail with icons, labels and live counters |
| Theme | egui default dark | Near-black surfaces, hairline borders, one accent colour, Consolas for all numerals |
| Process table | 8 columns, name only | 11 columns including executable path, leading status dot, inline CPU bar |
| Performance | 4 sparklines in one card | Six KPI tiles, four charts, per-core bars, memory breakdown, volume usage bars, disk and GPU cards |
| Sensors | One card of rows | Three KPI tiles, temperature list with per-sensor meters, provider status with reasons, explicit elevation buttons |
| Alerts | did not exist | Banner + tab with severity cards and a 20-event log |
| Detail panel | Single column | Two columns, path middle-truncated, CPU time as a duration, process age and start clock |
| Language | Indonesian | English throughout |

Values that do not exist still render as `—` with a reason. The "Max temp" tile
initially showed `-340282346638528859811704183484516925440 °C` because it folded
from `f32::MIN`; `f32::MIN` is finite, so an `is_finite()` guard did not catch
an empty sensor list. Fixed by using `reduce(f32::max)`, which yields `None`.
That class of bug is exactly why ADR-0008 forbids placeholder numbers.

## Measurements after the rework

| Metric | Value |
|---|---|
| Unit + integration tests | **167** (core 73, sensors 41+3, ipc 17, helper 9, ui 24) |
| `cargo clippy --workspace --all-targets` | 0 warnings |
| `cargo fmt --all` | clean |
| Release build time | 3 min 35 s |
| Release binary sizes | UI 8.02 MB, CLI 2.55 MB, helper 1.00 MB |
| Maximum process count observed | 216 |
| Helper round trip | < 10 ms on an idle machine |
| Helper death detection | 0 ms (`NotRunning`, no timeout wait) |

The debug binary was used for screenshots; release numbers above are from
`cargo build --release`.

## Findings from the review pass

A second review of the P4 build (2026-11-05) found and fixed four real defects:

| Finding | Severity | Evidence | Fix |
|---|---|---|---|
| A client that connected and sent nothing blocked the **only** pipe instance forever; every other client was refused | **High** | Reproduced: silent client held the pipe, `core-pulse-cli helper` failed with "helper is busy or not responding" for 12+ s | Server reads now poll `PeekNamedPipe` with a 2 s deadline instead of blocking in `ReadFile`. Recovery measured at ~2 s |
| The bold font was inserted at index 0 of the `Proportional` family, making **every label bold** | Medium | Visible in screenshots: all body text was heavy | Bold moved to a dedicated named family; `theme::bold()` is the only way to opt in |
| `bold_mono` referenced a font family (`Hack-Bold`) that does not exist, so emphasised numerals could fail to lay out | Medium | epaint ships only `Hack` regular | Registers `consolab.ttf` as a real bold monospace family with a safe fallback |
| Nav glyphs mixed monochrome text characters with emoji-rendered `⚙` and `▶`, so two rail items appeared in purple/blue | Low but very visible | 3× zoom of the rail | Icons are now painted geometry: identical weight, colour and size; no font dependency |

Also tightened in the same pass:

- Five ad-hoc text sizes (9.0/9.5/10.0/10.5/11.0) collapsed into a named type
  scale, so `TextEdit`, table cells and headings cannot drift apart.
- `—` (unknown) and `…` (not read yet) now come from one constant each, so the
  ADR-0008 distinction cannot be lost at a call site.
- Hover on the navigation rail is dimmer than selection and has no accent bar,
  so hover no longer reads as "selected".
- Added tests: duplicate/missing tab icons, hint length, and single-character
  icon constants.

**Verified correct during the same review:**

- CPU percentages match WMI independently: `wixnative` read 15.2% of system in
  core-pulse and 91% of one core in `Win32_PerfFormattedData_PerfProc_Process`
  (91 ÷ 6 = 15.2).
- The process table column count matches the header count.
- The helper still rejects oversized frames and invalid JSON and detects death
  in 0 ms after the pipe changes.

## Not done in P4

| Item | Reason |
|---|---|
| SQLite history | Designed in ADR-0006 but not implemented; the Alerts tab currently keeps events in memory only |
| Tray icon, start minimised, single instance, global hotkey | Not implemented; the window opens maximised instead |
| Elevated helper verification | Requires a manual UAC approval that cannot be automated here |
| HDD temperature reading | Blocked on the elevated helper (see [P3-RESULTS.md](P3-RESULTS.md) for the access evidence) |

## Reproduce

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets
cargo build --release
.\target\release\core-pulse-cli.exe helper-selftest

# Manual: start the helper unprivileged, then inspect it from a second shell
Start-Process .\target\release\core-pulse-helper.exe -ArgumentList "--serve"
.\target\release\core-pulse-cli.exe helper
.\target\release\core-pulse-cli.exe sensors --helper --samples 1
```
