# Architecture

This document explains component boundaries, the thread model, and the data
flow. The rules here are binding: a change that violates them needs a new ADR.

## 1. Crate boundaries

```
core-pulse-core      model + collector + sampler + alerts   no UI, no winit, no egui
core-pulse-sensors   SensorProvider trait + backends        unsafe and IOCTL allowed
core-pulse-ipc       pipe protocol + serde types            used by UI and helper
core-pulse-ui        bin core-pulse.exe                     eframe, view
core-pulse-helper    bin core-pulse-helper.exe              service, elevated
core-pulse-cli       bin core-pulse-cli.exe                 headless, --json
```

Dependency rules (enforced by CI, not convention):

| Crate | May depend on | Forbidden |
|---|---|---|
| `core` | `windows`, `serde`, `parking_lot`, `tracing`, `arc-swap` | `egui`, `eframe`, `winit`, `sensors`, `ipc` |
| `sensors` | `core`, `ipc`, `windows`, `libloading` | `egui`, `eframe` |
| `ipc` | `core` (model types only), `serde`, `windows` | `sensors`, `egui` |
| `ui` | `core`, `sensors`, `ipc`, `eframe`, `egui_extras`, `egui_plot`, `arc-swap` | — |
| `helper` | `sensors`, `ipc`, `core` | `egui`, `eframe` |
| `cli` | `core`, `sensors`, `ipc`, `clap` | `egui`, `eframe` |

`core` does not know a UI exists. That is what makes `core-pulse-cli` possible
and lets the collector be tested without opening a window.

## 2. Thread model

```
main thread (winit/eframe)
  ├── renders on input and on repaint requests (~1 fps idle)
  ├── only reads ArcSwap<Snapshot> and ArcSwap<AlertState> (no locks)
  └── sends actions through direct syscalls on the click path

sampler thread (1 thread, normal priority)
  ├── every 1000 ms: run all collectors → Snapshot
  ├── feed the snapshot to AlertEngine
  ├── store Snapshot and AlertState into their ArcSwaps (sole writers)
  └── request a repaint through the egui Context

slow-refresh thread (1 thread)
  └── every 15 s: services, startup entries and sessions → SlowStore

sensor thread (1 thread)
  ├── every 2 s: probe providers, read sensors
  ├── optionally fetch from the elevated helper over the pipe
  └── store SensorReport; the sampler copies it into the next Snapshot

signature verification (inside the sampler, budgeted)
  └── at most 3 signatures per tick, only while the column is visible
```

Principles:

1. **The render loop never makes a syscall.** If a value is not in the snapshot
   yet, the UI shows a placeholder (`—`), never blocks waiting for data.
2. **One writer per state.** `ArcSwap` exists so readers need no lock; the
   writer is single, so there is no contention.
3. **Sampling does not chase rendering.** 1 Hz for numbers, a 60-point ring
   buffer for charts. No `sleep(16ms)` polling.
4. **One collector failing does not stop the others.** Each returns its data plus
   `Degradation` entries, which are attached to the snapshot and displayed.

## 3. Data flow

```
      Windows API / PDH              helper (optional)
              │                            │
              v                            v
   core::collect::*  ──────►  Snapshot  ◄──── sensors::*
              │                    │
              │                    │ ArcSwap::store
              v                    v
      cli --json          ui::render (read-only)

   Snapshot ──► AlertEngine ──► AlertState ──► ui::render
```

`Snapshot` is an immutable value that is shared. There is no
`Mutex<Vec<Process>>` on the render path. For charts, the UI keeps its own ring
buffer filled from new snapshots (detected via `snapshot.seq`), so `core` never
has to know about display history.

The alert engine is deliberately **not** inside `Snapshot`: an alert must survive
a gap in snapshot production (for example when a sensor provider stalls), and
the UI must be able to render it without the process table being current.

## 4. `unsafe` policy

- `#![deny(unsafe_op_in_unsafe_fn)]` and
  `#![warn(clippy::undocumented_unsafe_blocks)]` across the workspace.
- Every `unsafe` block must have a `// SAFETY:` comment naming the invariant that
  makes it sound (not "because this is Windows").
- `unsafe` is only allowed in `core/src/sys/`, `sensors/`, `ipc/`, and in the
  thin `ui/elevate.rs` wrapper around `ShellExecuteW`.
- Windows handles are wrapped in RAII types (`OwnedHandle`, `PdhQuery`,
  `PipeServer`, `PipeClient`) with a `Drop` that closes them. There are no
  scattered manual `CloseHandle` calls.

## 5. Error handling

Three classes, each treated differently:

| Class | Example | Treatment |
|---|---|---|
| Fatal at startup | window creation failed, font load failed | exit with a message and a non-zero code |
| Degradation | `MSAcpi_ThermalZoneTemperature` empty, NVML missing | mark `unavailable`, the UI keeps running |
| Transient | process vanished mid-enumeration, helper connection dropped | short retry or a status change, never log spam |

`anyhow` is used in binaries (`ui`, `helper`, `cli`), `thiserror` in libraries
(`core`, `sensors`, `ipc`). A library must never `panic!` on OS-provided input.

## 6. Cadence and cost

| Component | Frequency | Reason |
|---|---|---|
| Process enumeration | 1 Hz | enough for human eyes, cheap (~1–3 ms) |
| Per-core CPU | 1 Hz | time-based delta needs a stable interval |
| System memory | 1 Hz | — |
| Disk / net / GPU (PDH) | 1 Hz | PDH needs two samples for a rate |
| Slow data (services/startup/sessions) | 15 s | ~150 ms per scan, kept off the sampler |
| Sensors | 2 s | hardware sensors rarely update faster; SMART costs 5–40 ms |
| Alert evaluation | 1 Hz | runs inside the sampler on the fresh snapshot |
| Signature verification | lazily, 3 per tick | expensive (~5–20 ms), only while visible |
| SQLite history | 1-minute aggregation | limits I/O; 30-day retention (not yet implemented) |

## 7. Degradation paths

Every feature has an explicit behaviour when its source disappears. This is a
contract, not a suggestion:

| Feature | Without a source | What is shown |
|---|---|---|
| CPU temperature | no ACPI zone and no helper | "firmware exposes no ACPI thermal zone (0 instances)" |
| Disk temperature | SMART needs admin, helper not running | "needs elevation" + a Start helper button (never `0 °C`) |
| GPU temperature | no NVML/IGCL/ADLX | `—` with the missing-library reason |
| Per-process network | needs ETW (admin) | column hidden, tooltip explains why |
| Command line of another process | needs rights | `…` while unread, `—` when refused |
| Helper not installed | pipe absent | `needs elevation`, not an error |
| Helper crashed | pipe disappeared | status flips within one poll (≤ 2 s) |

Golden rule: **`None` means unknown, `0` means genuinely zero.** Never use `0`
as a stand-in for "I don't know".

## 8. Persistence

SQLite (`rusqlite` with the `bundled` feature) at
`%LOCALAPPDATA%\core-pulse\history.db` is the design (ADR-0006):

- `sample_1m(ts, cpu_avg, cpu_max, mem_used, gpu_avg, temp_cpu, temp_gpu, temp_disk)`
- `top_process_1h(hour, exe_name, cpu_ms, peak_mem)`
- `alert(ts, kind, subject, value, message)`

Writes are batched (one transaction per 60 samples), WAL mode,
`synchronous=NORMAL`. 30-day retention via periodic `DELETE` plus a weekly
`VACUUM`. Full schema in [DATA-MODEL.md](DATA-MODEL.md).

**Status:** designed, not implemented. Alerts are currently in memory only.

## 9. Observability

- `tracing` to the console (`core-pulse-cli`, `core-pulse-helper --console`) with
  a default level of `info`.
- `core-pulse-cli doctor` reports the Windows version, privilege level, PDH
  counter availability, each sensor backend, and the helper status. It is the
  first support tool when a bug is reported.
- `core-pulse-cli helper-selftest` reproduces the Gate G4 checks end to end.

## 10. Deliberately not done

- No plugin system. The attack surface is not worth the benefit in v1.
- No remote monitoring or web server. That is a different application.
- No `static mut` or global mutable state beyond one `ArcSwap` per process.
- No unbounded retry loop on enumeration; after five consecutive failures, stop
  and report in the UI.
