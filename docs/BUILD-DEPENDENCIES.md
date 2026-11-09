# Dependensi Build

Status: **berlaku** — toolchain berada di satu folder yang dicari lewat `CORE_PULSE_TOOLS`
Diperbarui: 2026-11-09

Tujuan dokumen ini: satu tempat yang menjawab "apa saja yang dibutuhkan untuk
membangun, menguji, dan memaketkan core-pulse, dan di mana letaknya".

Lokasi di bawah memakai `<tools>` untuk folder toolchain. Pada mesin ini
`<tools> = C:\Users\b-ext\Loc\Tools`.

## 1. Daftar dependensi

| Dependensi | Lokasi | Versi | Wajib untuk |
|---|---|---|---|
| Rust toolchain | `<tools>\rust\rustup` + `<tools>\rust\cargo` | stable 1.99.1 | build, test |
| Crate cache | `<tools>\rust\cargo\registry` | 434 crate (440 entri `Cargo.lock` termasuk 6 workspace) | build offline |
| MSVC Build Tools | `<tools>\msvc` | 17.14.44 / MSVC 14.44.35207 | link |
| Windows SDK | `C:\Program Files (x86)\Windows Kits\10` | 10.0.26100.0 | link |
| NSIS | `<tools>\nsis\nsis-3.13\makensis.exe` | 3.13 | installer |
| Git | sistem | 2.56.0 | clone, versi |

Rust memakai `RUSTUP_HOME` dan `CARGO_HOME` di `<tools>\rust`, jadi toolchain
dan cache crate ikut berpindah bersama folder `Tools`. Windows SDK tetap
terpasang sistem-wide karena komponen itu dipasang oleh Visual Studio Build
Tools; ini satu-satunya bagian yang masih menyentuh `C:`.

Versi Rust mengikuti `rust-toolchain.toml` (`stable`, target
`x86_64-pc-windows-msvc`, komponen `rustfmt` dan `clippy`).

## 2. Variabel lingkungan

Di-set per user, bukan per mesin, supaya tidak butuh admin:

| Variabel | Nilai | Dipakai oleh |
|---|---|---|
| `CORE_PULSE_TOOLS` | `C:\Users\b-ext\Loc\Tools` | resolver di `scripts\_common.ps1` |
| `RUSTUP_HOME` | `<tools>\rust\rustup` | rustup |
| `CARGO_HOME` | `<tools>\rust\cargo` | cargo, cache crate |
| `PATH` | `<tools>\rust\cargo\bin` di depan | `cargo`, `rustc`, `rustup` |

```powershell
[Environment]::SetEnvironmentVariable('CORE_PULSE_TOOLS','C:\Users\b-ext\Loc\Tools','User')
[Environment]::SetEnvironmentVariable('RUSTUP_HOME','C:\Users\b-ext\Loc\Tools\rust\rustup','User')
[Environment]::SetEnvironmentVariable('CARGO_HOME','C:\Users\b-ext\Loc\Tools\rust\cargo','User')
```

`CORE_PULSE_TOOLS` adalah override. Kalau tidak di-set, `Resolve-ToolsRoot`
mencoba folder `Tools` yang bersebelahan dengan checkout, lalu `C:\Tools` dan
`D:\Tools`. Dengan begitu checkout yang dipindah bersama folder `Tools`-nya
tetap bisa build tanpa mengubah variabel apa pun.

`RUSTUP_HOME` dan `CARGO_HOME` juga di-set oleh `Set-ToolchainEnvironment` pada
setiap skrip, jadi build tidak bergantung pada environment shell yang kebetulan
sudah terbuka.

## 3. Memasang dari nol

Semua perintah di bawah tidak membutuhkan admin, kecuali Visual Studio Build
Tools (installer akan meminta UAC sekali).

```powershell
$tools = 'C:\Users\b-ext\Loc\Tools'
[Environment]::SetEnvironmentVariable('CORE_PULSE_TOOLS',$tools,'User')

# 1. Rust portable
New-Item -ItemType Directory -Force -Path "$tools\rust" | Out-Null
Invoke-WebRequest `
  https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe `
  -OutFile "$tools\rust\rustup-init.exe"

$env:RUSTUP_HOME = "$tools\rust\rustup"
$env:CARGO_HOME  = "$tools\rust\cargo"
& "$tools\rust\rustup-init.exe" -y --no-modify-path `
  --default-toolchain stable-x86_64-pc-windows-msvc --profile default

& "$tools\rust\cargo\bin\rustup.exe" component add rustfmt clippy
& "$tools\rust\cargo\bin\rustup.exe" target add x86_64-pc-windows-msvc

# 2. MSVC Build Tools + Windows SDK (UAC sekali)
Invoke-WebRequest https://aka.ms/vs/17/release/vs_buildtools.exe `
  -OutFile "$tools\vs_buildtools.exe"
& "$tools\vs_buildtools.exe" --quiet --wait --norestart --nocache `
  --installPath "$tools\msvc" `
  --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended `
  --add Microsoft.VisualStudio.Component.Windows11SDK.26100

# 3. NSIS portable
Invoke-WebRequest `
  'https://downloads.sourceforge.net/project/nsis/NSIS%203/3.13/nsis-3.13.zip' `
  -OutFile "$tools\nsis\nsis-3.13.zip"
Expand-Archive "$tools\nsis\nsis-3.13.zip" -DestinationPath "$tools\nsis"

# 4. Cache crate
& "$tools\rust\cargo\bin\cargo.exe" fetch --locked
```

Catatan: unduhan SourceForge memakai halaman perantara. Kalau `Invoke-WebRequest`
mengembalikan HTML, ikuti URL di `<meta http-equiv="refresh">` atau unduh ZIP
`nsis-3.13.zip` secara manual dari halaman SourceForge NSIS.

## 4. Memverifikasi

```powershell
.\scripts\check-deps.ps1          # tabel lolos/gagal
.\scripts\check-deps.ps1 -Json    # untuk skrip lain
```

Skrip mengecek cargo/rustc/rustup, komponen `rustfmt` dan `clippy`, target
`x86_64-pc-windows-msvc`, `link.exe`, Windows SDK, cache crate, NSIS, dan file
proyek wajib. Keluar dengan kode `1` bila ada yang hilang, dan mencetak perintah
perbaikannya.

## 5. Build dan paket

```powershell
.\build.ps1 -Target Build       # cargo build --release --workspace --locked
.\build.ps1 -Target Test        # cargo test --workspace --locked
.\build.ps1 -Target Check       # cargo fmt --check + clippy -D warnings
.\build.ps1 -Target Portable    # artifacts\core-pulse-<versi>-windows-x64-portable.zip
.\build.ps1 -Target Installer   # artifacts\core-pulse-<versi>-windows-x64-setup.exe
.\build.ps1                     # build + test + portable + installer
```

Keluaran ada di `artifacts/`, bersama `SHA256SUMS.txt`. Installer memakai NSIS
dengan `RequestExecutionLevel user`: tidak ada UAC, default direktori
`%LOCALAPPDATA%\Programs\core-pulse`, dan uninstaller terdaftar di HKCU.

Paket portable berisi ketiga binary (`core-pulse.exe`, `core-pulse-cli.exe`,
`core-pulse-helper.exe`), README, dokumentasi, dan ikon. Helper tetap memerlukan
elevasi saat dijalankan; UI dan CLI tidak.

## 6. Build offline

Setelah `cargo fetch --locked` selesai, seluruh crate ada di
`<tools>\rust\cargo\registry`. Build dapat dijalankan tanpa internet:

```powershell
$env:CARGO_NET_OFFLINE = 'true'
.\build.ps1 -Target Build
```

## 7. Bila folder toolchain berubah

Tidak ada path yang perlu diubah di skrip. Cukup arahkan satu variabel:

```powershell
[Environment]::SetEnvironmentVariable('CORE_PULSE_TOOLS','<folder baru>','User')
```

Bila folder `rust` ikut pindah, set juga `RUSTUP_HOME` dan `CARGO_HOME` ke
lokasi barunya. Setelah itu jalankan `.\scripts\check-deps.ps1`.
