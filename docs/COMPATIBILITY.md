# Compatibility

This document states what is **supported**, what is **best effort**, and what
will **never** work. The point is to keep the README from over-claiming.

## 1. Operating systems

| Version | Status | Note |
|---|---|---|
| Windows 11 24H2+ (build 26100+) | **Fully supported** | development target |
| Windows 11 21H2–23H2 | Supported | same `NtQuerySystemInformation` |
| Windows 10 1809–22H2 | Supported | some PDH GPU counters have different names |
| Windows Server 2019/2022 | Best effort | untested, but the APIs match |
| Windows 8.1 / 7 | **Not supported** | required APIs are missing; do not force it |
| Windows on ARM | **Not supported** in v1 | needs an `aarch64-pc-windows-msvc` build and separate testing |

Architecture: `x86_64` only in v1. A 32-bit build makes no sense for this
application (handle and address-space limits), and ARM64 is deferred until there
is hardware to test on.

## 2. Features by machine type

The "this machine" column refers to the audited development machine in
[ENVIRONMENT-AUDIT.md](ENVIRONMENT-AUDIT.md).

| Feature | Modern desktop/laptop | This machine | Requirement |
|---|---|---|---|
| Process list + CPU/RAM | yes | **yes** | — |
| Per-core CPU + frequency | yes | **yes** | `CallNtPowerInformation` |
| Memory composition (standby/modified) | yes | **yes** | `NtQuerySystemInformation` |
| Disk I/O per volume | yes | **yes** | PDH `PhysicalDisk` |
| Network I/O per adapter | yes | **yes** | `GetIfTable2` |
| GPU utilization per process | yes | **yes (iGPU)** | PDH `GPU Engine` |
| GPU utilization per adapter | yes | **yes** | PDH `GPU Adapter Memory` |
| Signature & publisher | yes | **yes** | `WinVerifyTrust` |
| SATA disk temperature | yes, needs admin | **yes, needs the helper** | `SMART_RCV_DRIVE_DATA` |
| NVMe disk temperature | yes, needs admin | **not applicable** | NVMe health log |
| Physical disk size | yes | **estimated** (exact value needs admin) | `IOCTL_DISK_GET_LENGTH_INFO` |
| GPU memory capacity | yes | **not yet** (DXGI) | `IDXGIAdapter3::QueryVideoMemoryInfo` |
| GPU adapter name | yes | **yes, best effort** | display class registry |
| CPU temperature via ACPI | firmware-dependent | **no** (0 thermal zones) | firmware exposes a TZ |
| CPU temperature via MSR | needs a driver | **no** | ring-0 driver (rejected for v1) |
| NVIDIA GPU temperature | yes | not applicable | `nvml.dll` |
| Intel GPU temperature | best effort | **not verified** | `igcl.dll` |
| AMD GPU temperature | best effort | not applicable | `amdadlx64.dll` |
| Per-process network (ETW) | needs admin | **no** (non-admin) | ETW kernel-network |
| Throttle reasons | needs admin | **no** | `Kernel-Processor-Power` ETW |

## 3. Runtime requirements

| Component | Required? | Note |
|---|---|---|
| Windows 10 1809+ | yes | — |
| WebView2 | **no** | we use egui, not a WebView |
| .NET Runtime | **no** | pure Rust |
| Visual C++ Redistributable | **no** | the MSVC runtime is statically linked (`+crt-static`) |
| OpenGL 3.3 (GPU driver) | **yes** | `glow` renderer; `wgpu` produced an empty window on this machine's UHD 630 |
| Administrator rights | not for the UI | only for the helper |
| Disk space | ~40 MB | binary + history |

The `+crt-static` note: it avoids a redistributable at the cost of a slightly
larger binary. That trade-off is accounted for in [PERF.md](PERF.md).

## 4. Interaction with other software

| Software | Interaction |
|---|---|
| Antivirus | no ring-0 driver, so nothing triggers vulnerable-driver detection. A signed helper reduces false positives further. |
| LibreHardwareMonitor | optional; read over loopback HTTP only if the user enables the bridge. We never load its DLL. |
| HWiNFO | optional, shared memory, read-only. |
| HWiNFO / CPU-Z / AIDA64 | no conflict; they use their own drivers. |
| Windows Defender | no folder exclusion is requested. |
| RivaTuner / Afterburner | no conflict; both read NVML. |
| Anti-cheat processes (Vanguard, EAC) | we do not read other processes' memory and install no driver, so there is no conflict. This is an additional reason to reject a kernel driver. |

## 5. Build and renderer notes

- The renderer is `eframe` with the `glow` feature (OpenGL). A `wgpu` build
  produced an empty window on the UHD 630 with driver 31.0.101.2135; `glow`
  works. If we ever move to `wgpu`, retest on this machine.
- `egui 0.37` uses the `App::ui(&mut self, ui, frame)` API and
  `egui::Panel::top/bottom` (not `TopBottomPanel`). Examples in older egui
  documentation cannot be used verbatim.
- Fonts: Segoe UI and Consolas are loaded from `C:\Windows\Fonts` when present,
  with the bundled Ubuntu/Hack faces as fallbacks, so the UI still renders on a
  machine without those files.

## 6. Known limitations

1. **CPU temperature is not guaranteed.** That is a platform limitation, not a
   bug. See [SENSORS.md](SENSORS.md).
2. **Per-process network needs ETW + admin.** Without it the column is hidden.
3. **Command lines of protected system processes** often stay unreadable even
   when elevated. The UI shows the reason, never an empty string.
4. **Process 32-bit PEB reads** work for statistics but command-line retrieval
   needs separate `WOW64` handling.
5. **PDH GPU counters** can be empty in a VM or on a machine without a proper
   WDDM driver.
6. **More than ~2000 processes** (an extreme case, many tiny processes) pushes
   enumeration past the 50 ms budget. The UI still works; only the cadence drops
   to 2 seconds and an indicator appears.
7. **Helper required for SMART.** Without an elevated helper, disk temperature
   is reported as `needs elevation`, never as a number.
8. **No history persistence yet.** The Alerts tab keeps events in memory for the
   current session; SQLite history is designed but not implemented.
9. **Single instance is not enforced.** Two UI windows can run; both talk to the
   same helper, which serialises requests.

## 7. Hardware test matrix

For a release, at minimum test on:

| Configuration | Purpose |
|---|---|
| Intel + iGPU + SATA HDD (this machine) | baseline, non-admin |
| AMD Ryzen + NVIDIA dGPU + NVMe | NVML, NVMe SMART |
| Intel laptop + ACPI thermal zone | Tier 0 works |
| AMD laptop + Radeon iGPU | ADLX best effort |
| VM (Hyper-V) | verify graceful degradation, not crashes |
| Machine with two monitors at different DPI | layout |

Until this matrix is satisfied, releases are labelled **beta** and the README
lists the configurations that have actually been verified.
