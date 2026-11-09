# Temperature sensors

This is the hardest part of the project, and the part most easily made
dishonest. This document explains where each number comes from, what happens
when the source is missing, and why several popular approaches are deliberately
not used.

## 1. The core problem

Windows provides no user-mode API for CPU temperature. The only official path is
ACPI, and that depends on firmware. Many machines — including this project's
development machine — expose none at all.

Facts from the development machine audit (HP 85A2, i5-9500T, Windows 11 build
26100):

| Path | Class/counter | Result |
|---|---|---|
| `root\WMI` → `MSAcpi_ThermalZoneTemperature` | class exists | **0 instances** |
| PDH `\Thermal Zone Information(*)\Temperature` | counter set exists | **0 instances** |
| `Win32_PerfFormattedData_Counters_ThermalZoneInformation` | — | empty |
| `root\HP` | namespace exists | no thermal class |
| SATA disk SMART | `IOCTL_STORAGE_QUERY_PROPERTY` | present (2 HDDs) |
| NVML | no NVIDIA GPU | not applicable |
| Intel UHD 630 iGPU | IGCL not yet usable here | possibly available |

Design consequence: **the application must be designed to live without CPU
temperature.** Not as an edge case, but as the normal state on some machines.

## 2. Source tiers

```
Tier 0  ACPI / PDH thermal zone        user-mode, free, often empty
Tier 1  Disk SMART (NVMe/ATA)          user-mode, needs admin for some disks
Tier 2  GPU vendor SDK                 user-mode, needs the vendor driver
Tier 3  Bridge to an external monitor  user-mode, opt-in, third-party dependent
Tier 4  Our own kernel driver          NOT USED in v1
```

### Tier 0 — ACPI / PDH

Two paths, tried in order:

1. PDH counters `\Thermal Zone Information(*)\Temperature` and
   `\High Precision Temperature` (Kelvin × 10 / Kelvin). Cheaper than WMI and
   needs no COM.
2. `MSAcpi_ThermalZoneTemperature` via WMI, as a fallback.

If both are empty: `Degradation { area: Sensors, reason: NotAvailable }`. No
attempt is made to read BIOS registers or any other trick — that is fragile and
not portable across vendors.

Note: on many laptops the ACPI thermal zone measures the **chassis zone**, not
the CPU die. It can read 15–25 °C below die temperature. That is why
`confidence` for this path is `Derived`, and the UI labels it "ACPI zone" rather
than "CPU".

### Tier 1 — Disk SMART

Ported directly from `disk-pulse`, which already has fixture-verified parsers:

- NVMe: `IOCTL_STORAGE_QUERY_PROPERTY` with `STORAGE_PROTOCOL_SPECIFIC_DATA`,
  NVMe protocol, log page `0x02` (SMART/Health Information), bytes 1–2 =
  `Composite Temperature` in Kelvin.
- ATA: `SMART_RCV_DRIVE_DATA`, attributes `0xC2` (Temperature) and `0xBE`
  (Airflow Temperature).
- Fallback: `IOCTL_STORAGE_PREDICT_FAILURE`, then storage reliability counters.

Requires `GENERIC_READ | GENERIC_WRITE` on `\\.\PhysicalDriveN`, which usually
requires admin. `IOCTL_ATA_PASS_THROUGH` rejects a read-only handle with
`ERROR_ACCESS_DENIED` (5), even for a read-only SMART command, so the write bit
is mandatory. Without the helper: `NeedsElevation`, and the UI offers an
explicit "Start helper (UAC)" button.

Measured on this machine (see [P3-RESULTS.md](P3-RESULTS.md)): opening
`\\.\PhysicalDrive0` with `GENERIC_READ` fails with `0x0005`
(`ACCESS_DENIED`) and with access 0 with `0x0001` (`INVALID_FUNCTION`). With
`GENERIC_READ | GENERIC_WRITE` the ATA SMART command succeeds (attributes
`0xC2` and `0xBE` both return the disk temperature). Disk **model** and size are
readable without admin through `IOCTL_STORAGE_QUERY_PROPERTY`, which is why the
disk list is useful even unelevated — only the temperature needs the helper.

### Tier 2 — GPU

| Vendor | Library | Method | Note |
|---|---|---|---|
| NVIDIA | `nvml.dll` (ships with the driver) | `nvml-wrapper`, `nvmlDeviceGetTemperature` | most reliable, no extra install |
| Intel | `igcl.dll` (Intel Graphics Driver) | `libloading` + temperature domain enumeration | not verified on this machine (iGPU only) |
| AMD | `amdadlx64.dll` (Adrenalin) | `libloading` + `IADLXTemperature` | not verified |

All are loaded through `libloading` behind `Option`, never statically linked. If
a DLL is absent the backend reports `NotAvailable` and costs nothing at startup.

### Tier 3 — Bridge to an external monitor (opt-in)

For users who need CPU temperature and already run another monitor, we provide a
**read-only** bridge:

- **LibreHardwareMonitor**: HTTP `GET http://localhost:8085/data.json` (Remote
  Web Server plugin). We parse the JSON, map sensor names to `SensorReading`,
  `confidence: ThirdParty`.
- **HWiNFO**: Shared Memory Support (`Global\HWiNFO_SENS_SM2`), reading the
  struct HWiNFO documents for third-party readers.

Bridge rules:

1. **Off by default.** The user must enable it (`--bridge` or
   `CORE_PULSE_LHM_BRIDGE=1`) and accept that the data is third-party.
2. **Loopback only.** The address is hard-coded to `127.0.0.1`; it cannot be
   pointed at another host.
3. **500 ms timeout, no aggressive retry.** If the external monitor is not
   running, the bridge stays silent and reports `NotAvailable`.
4. **Never writes** to that monitor.

### Tier 4 — Our own kernel driver: rejected for v1

See [ADR-0002](ADR/0002-sensor-tiering.md). In short: reading MSRs
(`IA32_PACKAGE_THERM_STATUS` 0x1B1, `IA32_THERM_STATUS` 0x19C) requires ring-0
code. That means a driver that has to be signed, tested across Windows builds,
and updated every time Microsoft changes the rules. That is a project in itself,
not a feature of a task manager.

## 3. Why not WinRing0

Many open-source tools (OpenHardwareMonitor, older LibreHardwareMonitor) read
MSRs through `WinRing0.sys`. That approach **must not** be used:

- `WinRing0.sys` has CVE-2020-14979 (an IOCTL that leaks MSR read/write and port
  I/O primitives to any process).
- Microsoft added it to the vulnerable driver blocklist, so Defender blocks it.
- Windows 11 24H2+ with HVCI refuses to load it.

The practical consequence: an application that bundles WinRing0 gets flagged by
antivirus and fails on machines with default security settings. This is not a
theoretical risk; it is the fastest route to an application nobody can install.

If a user chooses the LHM bridge, they run LHM themselves and accept its risks.
We do not bundle, distribute or load that driver.

## 4. Trait and lifecycle

```rust
pub trait SensorProvider: Send {
    fn id(&self) -> ProviderId;
    /// Checked once at startup and every 30 seconds.
    fn probe(&self) -> Availability;
    /// Must not block for more than 100 ms.
    fn read(&mut self, out: &mut Vec<SensorReading>);
    /// Called when probe changes from Unavailable → Available.
    fn init(&mut self) -> Result<(), SensorError> { Ok(()) }
}

pub enum Availability {
    Available,
    NeedsElevation,
    NotAvailable(String),   // concrete reason, surfaced to the UI
    Failed(String),
}
```

`NotAvailable` carries an owned `String` rather than `&'static str` because the
helper forwards provider reasons across the pipe, and those are built at runtime
(for example, the exact Win32 error when opening a physical drive).

Implementation rules:

- `probe()` must not perform expensive I/O; its result is cached.
- `read()` must not `panic`. If one sensor fails, return the others.
- A provider that is `NotAvailable` is never called for `read()` (zero cost).
- Each provider has a time budget; a full sensor cycle stays under 100 ms, and a
  provider that overruns is skipped on the next cycle and logged.

The elevated helper implements the same trait and is merged on the client side:

```
local providers ─┐
                 ├─> merge_into(SensorReport)
helper (pipe) ───┘
```

Merge rules, enforced by unit tests:

1. A local reading wins when the `id` collides — local data is closer to the
   source.
2. The `smart` status comes from the helper when the helper answers, because it
   is the process that actually performs the IOCTL.
3. A `helper` status is always added, carrying `Available` (elevated),
   `NeedsElevation` (running but not elevated) or a failure reason.
4. Values outside the physical range are dropped, from either source.

Before any helper data is merged, the client verifies the pipe server. Normally
it compares the server process image path with the expected helper path. When
the helper runs as a Windows Service (`LocalSystem`), a non-admin client cannot
open the process; the client then reads the service PID and configured binary
path from SCM and accepts the pipe only if **both** match `core-pulse-helper`.
Any mismatch is `HelperError::Untrusted` and the data is discarded.

## 5. Status shown in the UI

No fabricated numbers, no `0 °C`, no "approximate" temperatures.

| Condition | Display |
|---|---|
| A sensor exists, `Direct` | `62 °C` + source tooltip ("NVMe SMART") |
| A sensor exists, `Derived` | `48 °C` + small marker + tooltip ("ACPI zone, usually below die temperature") |
| A sensor exists, `ThirdParty` | `71 °C` + tooltip ("LibreHardwareMonitor, third party") |
| `NeedsElevation` | `—` + "Helper not running — disk SMART needs elevation" + "Start helper (UAC)" |
| `NotAvailable` | `—` + the concrete reason returned by the provider |
| Provider failed | `—` + "Helper error — <error>" |

When the helper is not running, the Sensors tab shows the reason together with
two explicit buttons: **Start helper (UAC)** and **Install service (UAC)**. No
UAC prompt ever appears without a click. `core-pulse-cli doctor` reports the same
state from the terminal, so the diagnosis is reproducible in a bug report.

## 6. Alert thresholds

The engine lives in `core-pulse-core::alerts` and runs on the sampler thread.
Thresholds are deliberately conservative: an alert should mean "look at this",
not "something twitched".

| Metric | Warning | Critical | Duration before notification |
|---|---|---|---|
| CPU total | 90% | 97% | 5 seconds sustained |
| Memory in use | 90% | 97% | 5 seconds sustained |
| Disk temperature (SMART) | 55 °C | 60 °C | 10 seconds sustained |
| Other temperatures (CPU/GPU/ACPI) | 80 °C | 85 °C | 10 seconds sustained |
| Free disk space | < 5% | < 2% | 60 seconds sustained (volumes ≥ 1 GB) |

Deduplication and a **5-minute cooldown** per key prevent spam. An active alert
is never re-notified; clearing emits exactly one `Cleared` event. Missing sensor
data can never raise an alert, because a metric with no reading produces no
candidate at all.

## 7. What still needs verification on real hardware

Honest about this: several things cannot be tested on the development machine.

| Item | Status | How to verify |
|---|---|---|
| SMART NVMe | **parser verified with fixtures**, reading needs admin | fixture 318 K → 44.85 °C |
| SMART ATA | **parser verified with fixtures**, reading needs admin | fixture attribute 0xC2 → 41 °C |
| NVML | **not tested** (no NVIDIA GPU) | a machine with a GTX/RTX card |
| IGCL Intel | **not tested**, DLL absent here | a machine with Intel Arc |
| ADLX AMD | **not tested** | a machine with a Radeon card |
| LHM bridge | **not tested** (LHM not running) | install LHM, enable its web server, port 8085 |
| ACPI thermal zone | **proven empty here** (0 instances) | another machine (many laptops have one) |

This status is also reported at runtime by `core-pulse-cli sensors`, so the user
does not have to trust the documentation alone.

Until the items above are verified, user-facing documentation must call them
"best effort", not "supported".
