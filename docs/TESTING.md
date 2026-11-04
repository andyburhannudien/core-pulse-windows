# Testing strategy

Principle: **logic that can be tested without Windows should be tested without
Windows**, and logic that requires Windows should be tested with fixtures that
can be replayed.

Practical reason: the development machine has two SATA HDDs, no NVMe, no
discrete GPU, no ACPI thermal zone, and runs without admin rights. If tests
depended on that hardware, half the code would never be exercised.

## 1. The pyramid

```
        /\
       /  \      manual checklist (parity vs Task Manager)
      /────\     integration live (opt-in, needs a real machine)
     /      \    fixture replay (recorded snapshots & payloads)
    /────────\   unit test (parsers, calculations, state machines)
   /──────────\  compile-time (type system, clippy, deny)
```

Rough target: 70% unit, 20% fixture, 10% live/manual. Not a sacred number, but
the direction is: do not rely on manual tests for anything that can be tested
purely.

**Current count: 167 tests** (core 73, sensors 41 + 3 fixture, ipc 17, helper 9,
ui 24), plus `clippy --workspace --all-targets` with zero warnings.

## 2. Unit tests

Things that **must** have unit tests:

| Module | Cases |
|---|---|
| `sys::ntapi` parser | truncated buffer, `NextEntryOffset` 0, non-UTF8 process name, PID 0 |
| CPU% calculation | zero time delta (no division by zero), counter wrap, offline core, uneven intervals |
| Memory pressure | bounds 0% / 100%, commit > limit, negative values from the API |
| SMART NVMe | valid 512-byte health log, temperature 0 K, truncated bytes |
| SMART ATA | attributes 0xC2/0xBE, missing threshold, value 0 (sensor absent) |
| Signature | missing path, unsigned file, invalid signature, cache hit |
| `Snapshot` serialization | JSON round-trip, `None` stays `null`, `schema` included |
| PID-reuse guard | creation time differing by 1 ms, by 1 s, identical |
| Alert engine | dedup, cooldown, flapping, return to normal, missing data never alerts |
| IPC framing | oversized length rejected, version mismatch, invalid JSON, truncated frame |
| Helper merge | local wins on collision, implausible value dropped, missing helper is not an error |
| Number formatting | 1023 B → "1.0 KB", 1 GiB, percentage rounding, middle truncation |
| Clock formatting | local-offset application, zero padding, civil-date conversion |
| SQLite history | batch insert, range query, retention, schema migration (when implemented) |

Rule: every bug found in the field **must** produce a regression unit test before
it is fixed. Otherwise it comes back.

## 3. Fixture replay

This is the heart of the strategy. Data from the OS is recorded to a file, then
the collector is tested by replaying that recording.

```
tests/fixtures/
├── snapshot/
│   ├── win11-26100-idle.json          # ~400 processes, this machine
│   ├── win11-26100-load.json          # while a Rust build was running
│   └── win10-19045-legacy.json
├── smart/
│   ├── nvme-health-samsung-980.bin    # 512-byte log page 0x02
│   ├── nvme-health-wd-sn770.bin
│   ├── ata-attrs-st1000lm049.bin
│   └── ata-attrs-corrupt.bin          # deliberately corrupt, fuzz seed
└── ipc/
    ├── sensor-update-v1.json
    └── malformed/                      # malformed messages for rejection tests
```

Recording: `core-pulse-cli record --out tests/fixtures/snapshot/x.json`. That
command has existed since P1, so recording a fixture is never manual work that
gets postponed.

Fixture rules:

- **Anonymised**: user names become `user`, computer names `PC`, personal paths
  `C:\Users\user\...`. A test asserts that no fixture contains the real name.
- **Small**: one snapshot with ~400 processes is ≈200 KB of JSON; compress with
  `zstd` if needed, but never exceed 2 MB per file.
- **Versioned**: the fixture stores `schema` so migrations can be tested.

## 4. Live / opt-in tests

Tests that touch the real machine are gated so a plain `cargo test` stays
deterministic. In this repository the live checks are implemented as **CLI
subcommands** rather than `#[ignore]` tests, because they need to spawn helper
processes and print evidence:

```powershell
# PID-reuse guard against real processes (spawns and kills a test process)
.\target\release\core-pulse-cli.exe action-selftest

# Helper end-to-end: spawn, oversized frame, invalid JSON, helper death
.\target\release\core-pulse-cli.exe helper-selftest

# Sensor reality check on this machine
.\target\release\core-pulse-cli.exe sensors --helper --samples 2
```

What the live checks cover:

- The process enumeration returns > 0 entries and includes PID 4.
- Total CPU% stays within 0–100 × core count.
- Disk temperature, when readable, is within -10..90 °C.
- The helper starts, answers `Hello`, survives malformed input, and its death is
  detected as `NotRunning`.
- The UI opens, renders, and exits with code 0 (manual, via screenshot).

## 5. Fuzz

`cargo fuzz` targets:

| Target | Input | Goal |
|---|---|---|
| `nvme_log` | random bytes | no panic, no out-of-bounds |
| `ata_attrs` | random bytes | same |
| `ipc_message` | random bytes | rejected with an error, never a panic |
| `lhm_json` | random JSON | parser does not panic or allocate wildly |

Run for 10 minutes in nightly CI, and keep any discovered corpus under
`tests/fixtures/fuzz-corpus/` so regressions are caught.

## 6. Manual checklist (parity vs Task Manager)

Run before every P-gate and before a release. This is the only way to verify
"the numbers are right".

| # | Step | Expectation |
|---|---|---|
| 1 | Open Task Manager and core-pulse side by side, Processes tab | same process count |
| 2 | Sort by CPU descending, wait 60 s | same top five, difference < 5% |
| 3 | Open five Chrome tabs | chrome.exe rises in both, difference < 10% |
| 4 | Copy a 2 GB file | Disk column and Performance tab follow |
| 5 | Download a large file | Network column follows |
| 6 | Play a 4K video | GPU column appears and is plausible |
| 7 | Performance → Memory | in use / standby / free match |
| 8 | End task on Notepad | disappears from both |
| 9 | Start a process as admin | elevated column is correct |
| 10 | Unplug/replug a USB drive | volume appears/disappears without a crash |
| 11 | Leave idle for 30 minutes | CPU < 1%, RAM stable (no leak) |
| 12 | Kill the helper while running | UI does not hang, status changes |
| 13 | Set DPI to 150% | layout is not clipped |
| 14 | Switch light/dark theme | text contrast stays readable |
| 15 | Sleep then resume | sampling recovers, no phantom spike |
| 16 | Send an oversized IPC frame | helper rejects it and keeps serving |
| 17 | Send invalid JSON over the pipe | helper rejects it and keeps serving |

Item 15 matters: after resume the time delta can be large and produce a phantom
CPU% spike. The collector must detect an `interval_ms` jump over 5 seconds and
discard that sample.

## 7. CI

```yaml
# .github/workflows/ci.yml (outline)
jobs:
  check:      # windows-latest: fmt, clippy -D warnings, tests, deny
  bench:      # windows-latest: criterion, compare against baseline
  coverage:   # tarpaulin/llvm-cov, target 65% for core+sensors
  fuzz:       # nightly, 10 minutes per target
  build:      # release build, measure exe size, upload artifact
```

Merge gate: `check` green + no benchmark regression > 20% + coverage not down
by more than 2 points.

## 8. Flaky-test policy

A flaky test is worse than no test, because it trains people to ignore failures.

- An intermittently failing test is labelled `flaky` and **quarantined** within
  48 hours.
- If the cause is timing, replace it with explicit synchronisation, not `sleep`.
- If the cause is hardware, move it to the live (opt-in) category.
- A test quarantined for more than two weeks is deleted or fixed — never left
  hanging.
