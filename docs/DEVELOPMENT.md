# Development guide

## 1. One-time setup

```powershell
winget install --id Rustlang.Rustup -e
# close and reopen the terminal so PATH is refreshed
rustup default stable-x86_64-pc-windows-msvc
rustup component add rustfmt clippy
rustup target add x86_64-pc-windows-msvc
```

Verify the toolchain:

```powershell
rustc --version      # stable
cargo --version
cargo clippy --version
```

On this checkout the toolchain is portable: Rust lives in
`<repo>\..\..\Tools\rust` and the scripts wire `RUSTUP_HOME`, `CARGO_HOME` and
`PATH` automatically. Run `.\scripts\check-deps.ps1` to verify Rust, MSVC,
Windows SDK, the crate cache and NSIS in one table. Full setup instructions are
in [BUILD-DEPENDENCIES.md](BUILD-DEPENDENCIES.md).

What you do **not** need: full Visual Studio, .NET, Node, WebView2, CMake.
MSVC Build Tools and Windows SDK 10.0.26100 are already on the development
machine.

Optional but recommended tooling:

```powershell
cargo install cargo-nextest cargo-deny cargo-llvm-cov
cargo install cargo-fuzz --version latest   # nightly is only needed for fuzz
```

## 2. Workspace layout

```
core-pulse/
├─ Cargo.toml               # [workspace], resolver = "3"
├─ rust-toolchain.toml
├─ clippy.toml
├─ deny.toml
├─ build.ps1                # build, test, portable ZIP, NSIS installer
├─ crates/
│  ├─ core/     core-pulse-core
│  ├─ sensors/  core-pulse-sensors
│  ├─ ipc/      core-pulse-ipc
│  ├─ ui/       core-pulse-ui      (bin: core-pulse.exe)
│  ├─ helper/   core-pulse-helper  (bin: core-pulse-helper.exe)
│  └─ cli/      core-pulse-cli     (bin: core-pulse-cli.exe)
├─ installer/   core-pulse.nsi, core-pulse.ico
├─ scripts/     _common.ps1, check-deps.ps1, package-portable.ps1,
│               make-installer.ps1, make-icon.ps1, screenshot.ps1,
│               capture-docs.ps1
├─ tests/fixtures/
├─ docs/
└─ .github/workflows/ci.yml
```

## 3. Everyday commands

```powershell
cargo build                      # debug
cargo build --release            # release, LTO
cargo run -p core-pulse-ui       # run the UI (maximised)
cargo run -p core-pulse-ui -- sensors      # open a specific tab
cargo run -p core-pulse-ui -- --windowed   # keep the default window size
cargo run -p core-pulse-cli -- top --watch 1s
cargo run -p core-pulse-cli -- doctor

cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace    # or: cargo test --workspace
cargo deny check

cargo run -p core-pulse-cli -- bench --seconds 60   # performance budget
cargo run -p core-pulse-cli -- record --out tests/fixtures/snapshot/x.json
```

Packaging uses the same toolchain resolver, so it works from a normal
PowerShell window without activating anything:

```powershell
.\scripts\check-deps.ps1          # Rust, MSVC, SDK, crate cache, NSIS
.\build.ps1 -Target Portable      # artifacts\core-pulse-<version>-windows-x64-portable.zip
.\build.ps1 -Target Installer     # artifacts\core-pulse-<version>-windows-x64-setup.exe
.\build.ps1                       # build + test + portable + installer
```

The installer is per-user (`RequestExecutionLevel user`), installs under
`%LOCALAPPDATA%\Programs\core-pulse`, and never asks for UAC. The portable ZIP
contains the UI, CLI, helper, README and docs. Artifacts are written to
`artifacts/`, together with `SHA256SUMS.txt`.

Live checks (they touch the real machine) are CLI subcommands, not ignored
tests, because they print evidence:

```powershell
cargo run -p core-pulse-cli -- action-selftest   # PID-reuse guard
cargo run -p core-pulse-cli -- helper-selftest   # Gate G4 IPC checks
cargo run -p core-pulse-cli -- sensors --helper  # sensor reality check
```

Screenshots for the docs are captured from a running UI, one tab at a time:

```powershell
cargo build -p core-pulse-ui
powershell -ExecutionPolicy Bypass -File scripts\capture-docs.ps1
```

## 4. Code conventions

| Rule | Detail |
|---|---|
| Format | default `rustfmt`, no custom `rustfmt.toml` initially |
| Lint | `clippy -D warnings`, `unsafe_op_in_unsafe_fn` denied |
| Language | identifiers, comments and user-facing strings in **English** |
| Errors | `thiserror` in libraries, `anyhow` in binaries |
| Panic | forbidden in libraries on OS-provided input; `expect()` only for proven invariants |
| Logging | `tracing` with structured fields (`pid`, `provider`), not format strings |
| Units | put the unit in the field name: `_bytes`, `_ms`, `_percent`, `_c` |
| Optional | `Option` means unknown; never use `0` as a sentinel |

Commit message: `feat:`, `fix:`, `docs:`, `perf:`, `refactor:`, `test:`,
`chore:`. One commit is one change explainable in one sentence.

## 5. Debugging

| Problem | Tool |
|---|---|
| Suspicious numbers | `core-pulse-cli top --json`, compare with Task Manager |
| Sensor missing | `core-pulse-cli doctor` (shows each provider's state) |
| UI slow | `--bench`, then WPR/WPA for a CPU sample |
| Memory leak | `dhat` feature in a debug build |
| Helper not connecting | `core-pulse-cli helper`; helper log at `%LOCALAPPDATA%\core-pulse\logs\helper.log` |
| Panic | log + dialog; if neither, run from a terminal to see stderr |
| Renderer issues | run with `--windowed`; `glow` is the supported renderer |

Log level is set with `CORE_PULSE_LOG=debug` (default `info`).

## 6. Setup troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `error: linker 'link.exe' not found` | MSVC Build Tools not installed or not found | install the C++ workload into `<tools>\msvc` (see BUILD-DEPENDENCIES.md), then run `.\scripts\check-deps.ps1` |
| `rustc` not recognised after install | PATH not refreshed | open a new terminal, or let `scripts\_common.ps1` set the path |
| `error: could not compile windows-sys` | wrong target | `rustup target add x86_64-pc-windows-msvc` |
| First build very slow | large `windows` crate + LTO | normal; subsequent incremental builds are fast |
| `makensis` not found | NSIS portable not extracted | extract `nsis-3.13.zip` into `<tools>\nsis`, then re-run `check-deps.ps1` |
| `cargo deny` fails on a licence | new dependency | add it to `deny.toml` only after checking the licence |
| UI appears empty | font or first-snapshot failure | check the log; the UI must show a placeholder, not a blank window — a blank window is a bug |

## 7. Definition of done for a PR

1. `cargo fmt --check` clean.
2. `cargo clippy -- -D warnings` clean.
3. New tests for new logic; regression tests for fixed bugs.
4. No benchmark regression > 20%.
5. Documentation updated when user-visible behaviour changes (`docs/`, and
   `DATA-MODEL.md` when the schema changes).
6. No `TODO` without an issue or a phase name in the comment.
