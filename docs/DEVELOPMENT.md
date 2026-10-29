# Panduan pengembangan

## 1. Setup sekali jalan

Mesin ini **belum punya Rust**. Langkah pertama:

```powershell
winget install --id Rustlang.Rustup -e
# tutup dan buka ulang terminal supaya PATH ter-update
rustup default stable-x86_64-pc-windows-msvc
rustup component add rustfmt clippy
rustup target add x86_64-pc-windows-msvc
```

Verifikasi toolchain:

```powershell
rustc --version      # stable
cargo --version
cargo clippy --version
```

Yang **tidak** perlu dipasang: Visual Studio penuh, .NET, Node, WebView2, CMake.
MSVC Build Tools dan Windows SDK 10.0.26100 sudah ada di mesin ini.

Alat pendukung (opsional tapi disarankan):

```powershell
cargo install cargo-nextest cargo-deny cargo-llvm-cov
cargo install cargo-fuzz --version latest   # butuh nightly untuk fuzz saja
```

## 2. Struktur workspace

```
core-pulse/
├─ Cargo.toml               # [workspace], resolver = "3"
├─ rust-toolchain.toml
├─ clippy.toml
├─ deny.toml
├─ crates/
│  ├─ core/     core-pulse-core
│  ├─ sensors/  core-pulse-sensors
│  ├─ ipc/      core-pulse-ipc
│  ├─ ui/       core-pulse-ui      (bin: core-pulse.exe)
│  ├─ helper/   core-pulse-helper  (bin: core-pulse-helper.exe)
│  └─ cli/      core-pulse-cli     (bin: core-pulse-cli.exe)
├─ tests/fixtures/
├─ docs/
└─ .github/workflows/ci.yml
```

## 3. Perintah sehari-hari

```powershell
cargo build                      # debug
cargo build --release            # release, LTO
cargo run -p core-pulse-ui       # jalankan UI
cargo run -p core-pulse-cli -- top --watch 1s
cargo run -p core-pulse-cli -- doctor

cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace    # atau: cargo test --workspace
cargo deny check

cargo run -p core-pulse-cli -- bench --seconds 60   # budget performa
cargo run -p core-pulse-cli -- record --out tests/fixtures/snapshot/x.json
```

Test live (menyentuh mesin sungguhan) hanya jalan kalau diminta:

```powershell
$env:CORE_PULSE_LIVE = "1"
cargo nextest run --workspace --run-ignored all
```

## 4. Konvensi kode

| Aturan | Detail |
|---|---|
| Format | `rustfmt` default, tanpa `rustfmt.toml` kustom di awal |
| Lint | `clippy -D warnings`, `unsafe_op_in_unsafe_fn` deny |
| Nama | tipe & fungsi English; komentar boleh Indonesia untuk alasan desain |
| Error | `thiserror` di library, `anyhow` di binary |
| Panic | dilarang di library pada input dari OS; `expect()` hanya untuk invariant yang terbukti |
| Log | `tracing`, dengan field terstruktur (`pid`, `provider`), bukan format string |
| Unit | satuan di nama field: `_bytes`, `_ms`, `_percent`, `_c` |
| Optional | `Option` = tidak diketahui; jangan pakai `0` sebagai sentinel |

Commit message: `feat:`, `fix:`, `docs:`, `perf:`, `refactor:`, `test:`, `chore:`.
Satu commit = satu perubahan yang bisa dijelaskan dalam satu kalimat.

## 5. Debugging

| Masalah | Alat |
|---|---|
| Angka aneh | `core-pulse-cli top --json` lalu bandingkan dengan Task Manager |
| Sensor tidak muncul | `core-pulse-cli doctor` (menampilkan status tiap provider) |
| UI lambat | `--bench`, lalu WPR/WPA untuk sampel CPU |
| Kebocoran memori | `dhat` feature di debug build |
| Helper tidak terhubung | log helper di `%LOCALAPPDATA%\core-pulse\logs\helper.log` |
| Panic | log + dialog; kalau tidak ada, jalankan dari terminal untuk melihat stderr |

Level log diatur lewat `CORE_PULSE_LOG=debug` (default `info`).

## 6. Troubleshooting setup

| Gejala | Penyebab | Solusi |
|---|---|---|
| `error: linker 'link.exe' not found` | MSVC env tidak terdeteksi | jalankan dari *x64 Native Tools Command Prompt*, atau `cmd /c "vcvars64.bat && cargo build"` |
| `rustc` tidak dikenali setelah install | PATH belum refresh | buka terminal baru |
| `error: could not compile windows-sys` | target salah | `rustup target add x86_64-pc-windows-msvc` |
| Build sangat lambat pertama kali | `windows` crate besar + LTO | normal; incremental build berikutnya cepat |
| `cargo deny` gagal karena lisensi | dependensi baru | tambahkan ke `deny.toml` hanya setelah lisensi diperiksa |
| UI muncul kosong | gagal load font/snapshot pertama | cek log; UI harus menampilkan placeholder, bukan kosong — kalau kosong, itu bug |

## 7. Definisi "selesai" untuk sebuah PR

1. `cargo fmt --check` bersih.
2. `cargo clippy -- -D warnings` bersih.
3. Test baru untuk logika baru; test regresi untuk bug yang diperbaiki.
4. Tidak ada penurunan benchmark > 20%.
5. Dokumentasi diperbarui kalau perilaku yang terlihat pengguna berubah
   (`docs/`, dan `DATA-MODEL.md` kalau skema berubah).
6. Tidak ada `TODO` tanpa issue atau tanpa nama fase di komentar.
