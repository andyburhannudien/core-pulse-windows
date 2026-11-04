# Performance budget

A monitoring application that wastes resources is a sad joke. The budget in this
document is a **release requirement**, measured in CI, not felt while coding.

## 1. Targets

| Metric | Target | Hard limit | How measured |
|---|---|---|---|
| Average CPU (idle, UI open) | < 0.5% | 1.0% | `GetProcessTimes` delta over 60 s |
| CPU while scrolling the table | < 3% | 5% | same, during interaction |
| UI private bytes | < 70 MB | 100 MB | `GetProcessMemoryInfo` |
| UI working set | < 120 MB | 180 MB | same |
| Enumerating 400 processes | < 30 ms | 50 ms | `criterion` |
| One cycle of all collectors | < 60 ms | 100 ms | `criterion` |
| Snapshot → JSON serialization | < 5 ms | 10 ms | `criterion` |
| Steady-state allocations per snapshot | 0 after warmup | 4 | `dhat` / allocation counter |
| `core-pulse.exe` size (release) | < 12 MB | 20 MB | file size |
| Startup to first frame | < 400 ms | 800 ms | log timestamp |
| 24-hour history query | < 50 ms | 150 ms | `criterion` with a populated DB |

These numbers are set for an i5-9500T-class machine with 8 GB of RAM — the
lowest class supported. Faster machines will do better; the budget must not be
raised just because the development machine is fast.

**Latest measured values (release build, i5-9500T, idle, UI maximised, P4):**

| Metric | Measured | Budget |
|---|---|---|
| CPU while idle | **0.40% of system** (2.42% of one core) | < 1.0% |
| UI private bytes | **44.0 MB** | < 100 MB |
| UI binary (release) | **8.02 MB** | < 20 MB |
| Sampler snapshot build | p95 45 ms | < 100 ms |

The earlier P0 figure of 0.14% was measured before the sensor poller, alert
engine, and the six-chart Performance tab existed. 0.40% is the honest current
number; the 0.5% target still holds but the margin is now thin, and the next
optimisation pass should look at repaint frequency rather than the collectors.

## 2. How to measure

### In the application

`core-pulse-cli bench --seconds 60` runs the sampling loop headless for 60
seconds, then prints:

```
samples: 60
cpu_self_avg: 0.31%   cpu_self_max: 0.87%
private_bytes: 58.2 MB  working_set: 96.4 MB
enumerate_ms: avg 2.4  p95 3.1  max 4.8
snapshot_build_ms: avg 11.2  p95 14.0
allocations_per_sample: 0
```

This is the tool used in CI and when investigating a regression. You do not need
to open Task Manager to judge this application.

### Di CI

- `cargo bench` untuk fungsi murni (enumerasi, serialisasi, query).
- `core-pulse-cli bench` untuk metrik proses (hanya di runner Windows, ambang
  dilonggarkan 2× karena VM lebih lambat, dan tren yang dilihat, bukan angka
  absolut).
- Regresi > 20% dibanding baseline `main` menggagalkan PR.

### Manual profiling

| Tool | For what |
|---|---|
| Windows Performance Recorder/Analyzer | CPU sampling, allocations, I/O during startup |
| `cargo flamegraph` (needs dtrace/perf, limited on Windows) | quick pure-function profiles |
| `dhat` | leaks and per-snapshot allocations |
| `GetProcessIoCounters` in `bench` | verify we do not touch the disk in the loop |

## 3. Design rules that protect the budget

1. **No polling faster than necessary.** Numbers run at 1 Hz; charts interpolate
   rather than adding samples.
2. **Repaint is data-driven, not timer-driven.** The sampler thread calls
   `request_repaint()` after storing a snapshot. When there is no interaction the
   application genuinely sleeps. The first version used a 100 ms timer and burned
   3.65% of one core while idle — measured, not guessed.
3. **No allocation on the render path.** The table uses `TableBuilder` with
   `body.rows()` and `&str` borrowed from the snapshot where possible.
4. **Icons and signatures are cached per executable path**, not per PID. Forty
   `chrome.exe` processes share one icon.
5. **PDH queries are created once**, not per sample. `PdhCollectQueryData` then
   read the already-registered counters.
6. **Old snapshots are dropped.** `ArcSwap` holds one snapshot; there is no
   history in `core`. Display history is only a 60-point ring buffer in the UI.
7. **`Vec` buffers are reused** across snapshots in the sampler (`clear()` then
   `extend`). The shared snapshot stays immutable because a new value is built
   from reused buffers.
8. **Never call WMI in the loop.** WMI is ~10–100× more expensive than PDH/`Nt*`.
   WMI is only for static data (OS name, disk model) and as a sensor fallback.
9. **Slow data has its own thread.** Services and startup entries cost ~150 ms
   per scan; reading them in the sampler would violate the cycle budget, so they
   refresh every 15 s on a separate thread behind an `ArcSwap`.
10. **Sensors have their own thread at 2 Hz.** SMART costs 5–40 ms per disk and
    the LHM bridge can block for the HTTP timeout; neither may delay the 1 Hz
    process numbers.

## 4. Estimated costs

| Operation | Estimated cost | Frequency |
|---|---|---|
| `NtQuerySystemInformation(SystemProcessInformation)` | 1–3 ms | 1 Hz |
| `GetProcessMemoryInfo` × 400 | 0.5–2 ms | 1 Hz |
| `GetProcessTimes` × 400 | < 1 ms | 1 Hz |
| `PdhCollectQueryData` (one query, 12 counters) | 0.2–1 ms | 1 Hz |
| Volume enumeration | 0.5–2 ms | 1 Hz |
| `GetIfTable2` | < 0.5 ms | 1 Hz |
| SMART read (2 disks) | 5–40 ms | 0.5 Hz (2 s) |
| `WinVerifyTrust` per new exe | 5–20 ms | once per exe |
| Icon decode per new exe | 2–10 ms | once per exe |
| SQLite batch of 60 samples | 2–5 ms | 1 minute |

Steady state: **< 10 ms per second** for all collectors. The rest is egui
rendering, dominated by GPU/CPU composition and proportional to window area.

## 5. Known regressions

Recorded so they do not surprise anyone:

- Opening the Details tab with the signature column triggers `WinVerifyTrust`
  for ~100 executables at once. Mitigation: lazy per visible row, a budget of
  three per tick, and `…` while pending.
- The Performance tab repaints more often while charts animate. Mitigation: the
  sampler still produces exactly 1 sample per second, and repaints follow data.
- A 30-day history with 400 processes could grow `history.db` to ~150 MB.
  Mitigation: `top_process_1h` keeps only the top 20 processes per hour.
