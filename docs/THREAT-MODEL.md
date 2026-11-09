# Threat model

This application reads system information and — in helper mode — runs with
`LocalSystem` rights. That combination deserves analysis, not an assumption of
safety.

Method: lightweight STRIDE per trust boundary, focused on what the design
actually contains (not a generic threat checklist).

## 1. Assets

| Asset | Value to an attacker |
|---|---|
| `core-pulse-helper.exe` process (LocalSystem) | code execution as SYSTEM → full escalation |
| Named pipe `\\.\pipe\core-pulse-v1` | entry point into a high-privilege process |
| History database | user habits, process names, active hours (privacy) |
| Logs | paths, user names, occasionally command lines |
| Integrity of the displayed numbers | user decisions (e.g. ending the wrong process) |

## 2. Trust boundaries

```
[ non-admin user ] --pipe--> [ helper LocalSystem ] --IOCTL--> [ kernel/driver ]
        │
        └── [ core-pulse.exe ] --read--> [ Windows API ]

[ third party: LHM/HWiNFO ] --HTTP loopback--> [ core-pulse.exe ]
```

Four boundaries: pipe (low → high), syscall (user → kernel), loopback HTTP
(third party → us), and supply chain (crates.io → build).

## 3. Threats and mitigations

### T1 — Another process impersonating the helper or the client

**Scenario:** malware creates a named pipe with the same name first, so the UI
connects to it and accepts forged data (for example, all temperatures normal).
Or the reverse: malware creates a pipe server and our helper connects to it.

**Mitigations (as implemented):**

- The helper creates its first pipe instance with `FILE_FLAG_FIRST_PIPE_INSTANCE`,
  so it cannot be preceded.
- The UI verifies the pipe server's PID (`GetNamedPipeServerProcessId`) and
  compares the server executable path with the expected helper path
  (`core-pulse-helper.exe`, overridable via `CORE_PULSE_HELPER_PATH`). A
  mismatch is reported as `HelperError::Untrusted` and the data is discarded.
- When the helper runs as a `LocalSystem` service, a non-admin client cannot
  open the process to read its image path. The client then asks SCM for the
  service PID and configured binary path (`QueryServiceStatusEx` +
  `QueryServiceConfigW`) and accepts the pipe only if the PID matches the pipe
  server **and** the configured binary matches the expected helper path. A
  non-matching PID or path is `HelperError::Untrusted`.
- The helper validates the client by resolving the **console session user** via
  WTS (`WTSQuerySessionInformationW` + `LookupAccountNameW`) and comparing it
  with the client token's user SID, and requires token integrity `Medium` or
  higher.
- If validation fails, the helper closes the connection without replying, so no
  information leaks to the rejected client.

**Residual risk:** low for pipe impersonation. The path comparison is not a
signature check; a build-time hash pin is the P5 hardening step.

**Residual risk (installer layout):** the per-user installer places
`core-pulse-helper.exe` under `%LOCALAPPDATA%`, while the service runs it as
`LocalSystem`. A local user who can write that directory can replace the binary
and gain SYSTEM on the next service start. This is a pre-existing installer
issue, not introduced by the SCM fallback. Until the service image is moved to
an admin-only directory (or verified by signature/hash), do not install the
service on a multi-user machine.

### T2 — The pipe as a privilege-escalation path

**Scenario:** an attacker sends a command to the helper asking it to do
something a non-admin user may not do (read a disk, modify a service, kill a
system process).

**Design mitigation — the protocol is deliberately narrow:**

| Message | Direction | Effect |
|---|---|---|
| `Hello { proto_version }` | UI → helper | version negotiation |
| `GetSensors` | UI → helper | read sensors |
| `GetSmart { disk_index }` | UI → helper | read SMART for one disk |
| `Hello { .. }` | helper → UI | version + elevated flag |
| `Sensors { readings, degraded, statuses }` | helper → UI | data |
| `Smart { disks }` | helper → UI | data |
| `Error { code, message }` | helper → UI | — |

There is no "run command", "read file", "write registry" or "terminate process"
message. **The helper never executes anything at a client's request.** If a
future feature needs that, it requires a new ADR and a fresh analysis, not a
quiet addition.

`disk_index` is validated as an integer in the range 0–63; the helper builds the
`\\.\PhysicalDriveN` path itself and never accepts a path from the client
(preventing traversal to another device).

The helper also caps each connection at 32 requests, caps a frame at 1 MiB, and
applies a 2-second read timeout, so a malicious client cannot hold the service
open or exhaust memory with an oversized frame.

**Residual risk (documented honestly):** `FlushFileBuffers` on the server has no
timeout. A client that sends a valid request and then stops reading its response
can still stall that one connection. This was found by testing (a client that
connects and sends nothing used to block all other clients forever; the
`PeekNamedPipe`-based read fixed that case). Closing the remaining case needs an
explicit client ACK in protocol v2. Until then, a compromised local process can
degrade the helper's availability — but cannot read data, issue commands, or
elevate privileges through it.

### T3 — PID reuse (acting on the wrong process)

**Scenario:** the user presses "End task" on PID 4820. Between render and action
that process dies and PID 4820 is reused by another process — which then gets
terminated.

**Mitigations:**

- `ProcessRow` always carries `start_time_unix_ms`.
- Before `TerminateProcess`, open a handle with `OpenProcess`, call
  `GetProcessTimes` and compare the creation time. If it differs by more than
  1 second: abort and report that the process changed.
- The handle used for the action is the verified handle, not a PID reopened
  later.
- An automated test simulates this (spawn, record PID, let it die, spawn until
  the PID repeats, assert the action is refused). `action-selftest` does the
  same against real processes: 6/6 scenarios pass.

This is not a theoretical mitigation: PID reuse causes real bugs in many task
managers.

### T4 — Hostile SMART payloads

**Scenario:** a disk (or driver/USB adapter) returns a buffer of unexpected
length, odd offsets, or a string without a NUL terminator.

**Mitigations:**

- Every parser takes `&[u8]` and validates length before indexing; there is no
  `unwrap()` on a slice.
- NVMe health-log and ATA attribute offsets are covered by unit tests and
  fixture replay (`tests/fixture_replay.rs`).
- String fields from the device are truncated and stripped of control
  characters before display or logging.

### T5 — Maliciously crafted pipe messages

**Scenario:** a client sends 500 MB of JSON, deeply nested JSON, or never-ending
data to exhaust the helper's memory.

**Mitigations (as implemented):**

- The frame header is mandatory: `u32 length` + `u16 proto_version`. A
  `length > 1 MiB` is rejected **before** allocation; the server then closes the
  connection without panic.
- Oversized and malformed frames are covered by `helper-selftest`, which proves
  the helper keeps serving new clients afterwards.
- Invalid JSON is rejected by `serde_json` and mapped to an `Error` response;
  the connection closes and the server continues.
- A 2-second read timeout per message, enforced with `PeekNamedPipe` polling
  rather than a blocking `ReadFile`. Without this, a client that connected and
  sent nothing held the only pipe instance and rejected every other client
  indefinitely — reproduced during review, now fixed and covered by the
  silent-client check below.
- At most 32 requests per connection, then the server disconnects. A single
  client therefore cannot park the service indefinitely.
- The first pipe instance uses `FILE_FLAG_FIRST_PIPE_INSTANCE`, so a second
  helper cannot bind the same name.

**Verified failure mode (before the fix):** a silent client made
`core-pulse-cli helper` fail with "helper is busy or not responding" for as long
as the client stayed connected. After the fix the next client is served after at
most one read timeout (~2 s).

### T6 — Log injection / data leakage through logs

**Scenario:** a process name contains `\n` and forged text, or a command line
carries a password, and both end up in a log attached to a bug report.

**Mitigations:**

- Every value from the OS is escaped (`{:?}` for strings, not `{}`) when logged.
- Command lines are **never** written to the log; only their length.
- Default log level is `info`, with no full user paths except at `debug`.
- `core-pulse-cli doctor` collects system information without the user name and
  without the process list.

### T7 — Third-party bridge (LHM/HWiNFO)

**Scenario:** something on `localhost:8085` answers with huge JSON, or serves
HTML/a redirect to another host.

**Mitigations:**

- The address is hard-coded to `127.0.0.1`, the port is fixed, redirects are not
  followed.
- Responses are capped at 2 MB and must be `Content-Type: application/json`.
- Only known fields are read; values outside physical ranges (e.g. -273 °C or
  5000 °C) are discarded.
- Off by default; the user enables it with an explicit warning.
- Data is tagged `confidence: ThirdParty` and never drives a `Critical` alert
  without user confirmation.

### T8 — Supply chain

**Mitigations:**

- `cargo deny check` for advisories (RUSTSEC), licences and duplicate versions.
- `Cargo.lock` is committed (this is a binary, not a library).
- Minimal dependencies: `windows`, `serde`, `parking_lot`, `tracing`, `eframe`,
  `egui_extras`, `egui_plot`, `arc-swap`. No dependency pulls in dozens of
  transitive crates without a reason.
- `cargo audit` runs weekly in CI, not only at release.

### T9 — DLL hijacking

**Scenario:** `igcl.dll` or `nvml.dll` is loaded from the working directory or a
user-writable `PATH` entry.

**Mitigations:**

- `libloading` always uses an absolute path under `System32` or a known driver
  directory, and `LoadLibraryExW` with `LOAD_LIBRARY_SEARCH_SYSTEM32`.
- There is no `LoadLibrary("name.dll")` with a bare name.
- Vendor DLL signatures are checked before use (best effort; on failure the
  backend reports `NotAvailable`).

### T10 — Update channel (if one is added)

If auto-update is added later: signed manifest, HTTPS mandatory, signature
verified **before** execution, and a helper update requires separate elevation
with confirmation. For v1 there is no auto-update.

## 4. Deliberately not done (and why)

| Not done | Reason |
|---|---|
| Running arbitrary commands from the pipe | the only reason the helper exists is to read; adding execution changes the risk qualitatively |
| Storing tokens/credentials | no feature needs them |
| Reading other processes' memory | requires `PROCESS_VM_READ`, is often blocked, and is unnecessary for v1 |
| A kernel driver | see ADR-0002 |
| Telemetry | no outbound data; bug reports are manual |
| Running the UI as SYSTEM | a non-elevated UI is a principle, not a preference |

## 5. Residual risk summary

| Risk | Level after mitigation |
|---|---|
| Escalation through the pipe | low (no command verbs, strict ACL, client validation) |
| Acting on the wrong process (PID reuse) | very low (creation-time guard + tests) |
| Crash from a hostile device payload | low (validation + fixture replay) |
| Privacy leakage through logs | low (no command lines, escaping) |
| Fake sensor data from a third party | medium (opt-in, tagged, not used for critical alerts) |
| A blocked third-party driver | none for us (we ship no driver) |
