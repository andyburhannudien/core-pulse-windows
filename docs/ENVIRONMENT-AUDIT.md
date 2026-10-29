# Audit lingkungan pengembangan

Diambil 2026-10-29 dari mesin kerja, dengan perintah yang bisa diulang. Dokumen
ini bukan opini — setiap baris punya output yang mendasarinya. Tujuannya supaya
keputusan desain bisa ditelusuri ke fakta, dan supaya keterbatasan mesin tidak
ditemukan saat sudah menulis banyak kode.

## 1. Ringkasan

| Kategori | Temuan | Dampak |
|---|---|---|
| Toolchain Rust | **tidak terpasang** | harus install rustup |
| Compiler C/linker | MSVC Build Tools + Windows SDK ada | target msvc siap setelah rustup |
| Hak akses | **non-admin** | helper, SMART, ETW tidak bisa diuji langsung |
| Thermal zone ACPI | **0 instance** | suhu CPU tidak terbaca sama sekali |
| GPU | Intel UHD 630 (iGPU saja) | NVML tidak bisa diuji |
| Storage | 2× HDD SATA, **tanpa NVMe** | parser NVMe hanya lewat fixture |
| RAM | 8 GB | budget memori ketat, menguntungkan egui |

## 2. Detail

### Sistem operasi dan hardware

```
Microsoft Windows 11 IoT Enterprise LTSC
Version 10.0.26100  Build 26100

Baseboard : HP / 85A2
CPU       : Intel(R) Core(TM) i5-9500T CPU @ 2.20GHz
            Cores 6 / Logical 6 / MaxClock 2208 MHz   (tanpa Hyper-Threading)
GPU       : Intel(R) UHD Graphics 630, driver 31.0.101.2135
RAM       : 2 × 4294967296 B (4 GiB + 4 GiB), 2400 MT/s dan 2667 MT/s (campur)
Disk      : ST1000LM049-2GH172  HDD 1000 GB  SATA
            ST500LT012-9WS142   HDD  500 GB  SATA
```

Implikasi: 6 logical core berarti `per_core_percent` punya 6 entri, dan tidak ada
kasus SMT untuk diuji. RAM campuran kecepatan berarti tidak ada profil XMP untuk
diuji — tidak relevan untuk aplikasi ini, tapi dicatat supaya tidak salah
menyimpulkan "memory speed tidak terbaca" sebagai bug.

### Toolchain

```
rustc / cargo / rustup        -> CommandNotFoundException
%USERPROFILE%\.cargo          -> False
%USERPROFILE%\.rustup         -> False

Visual Studio 18 BuildTools   -> ada
  VC\Tools\MSVC               -> ada
  vswhere -requires VC.Tools  -> C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools
Windows SDK                   -> 10.0.26100.0
git                           -> 2.56.0.windows.3
node                          -> v26.12.1
winget                        -> v1.29.380
dotnet                        -> tidak terpasang
```

Implikasi: cukup `winget install Rustlang.Rustup` + `rustup default
stable-x86_64-pc-windows-msvc`. Tidak perlu memasang Visual Studio penuh. Perlu
diverifikasi saat P0 bahwa `link.exe` ditemukan oleh rustc — kalau tidak, build
harus dijalankan dari *x64 Native Tools Command Prompt* atau `vcvars64.bat`
di-source lebih dulu.

`dotnet` tidak ada, dan itu **bukan** masalah: proyek ini tidak memakai .NET.
WebView2 (156.0.4314.50) juga terpasang tapi tidak akan dipakai karena UI memakai
egui — dicatat hanya untuk menutup opsi Tauri secara sadar, bukan karena
keterbatasan.

### Sensor

```
Get-CimInstance root/wmi MSAcpi_ThermalZoneTemperature
  -> kelas ada, 0 instance

Get-Counter -ListSet "Thermal Zone Information"
  -> counter set ada: Temperature, High Precision Temperature,
     Throttle Reasons, % Passive Limit
  -> 0 instance saat di-query

Win32_PerfFormattedData_Counters_ThermalZoneInformation
  -> kosong

root\HP (namespace)
  -> ada, tidak ada kelas thermal yang bisa dibaca tanpa elevasi

Get-PhysicalDisk
  -> 2 HDD SATA (SMART SATA tersedia, butuh akses ke \\.\PhysicalDriveN)
```

**Ini temuan terpenting dari audit.** Suhu CPU tidak dapat dibaca dari user-mode
di mesin ini, bukan karena kode belum ditulis, tapi karena firmware HP 85A2 tidak
mengekspos ACPI thermal zone. Konsekuensinya tertulis di
[SENSORS.md](SENSORS.md) dan [ADR-0002](ADR/0002-sensor-tiering.md): arsitektur
sensor harus mengasumsikan "tidak tersedia" sebagai keadaan normal.

### Akses dan keamanan

```
IsInRole(Administrator)  -> False
Confirm-SecureBootUEFI   -> Access denied (butuh elevasi)
Service *HardwareMonitor* -> tidak ada
C:\Windows\System32\drivers\*Ring0*  -> tidak ada
```

Implikasi: tidak ada driver monitoring pihak ketiga yang terpasang, jadi kita
tidak bisa "menumpang" apa pun. Pengujian helper elevated harus dilakukan dengan
membuka terminal elevated secara manual — tidak bisa dari sesi ini.

### Jaringan

```
https://static.crates.io  -> 200
https://github.com        -> 200
```

`cargo fetch` akan bekerja tanpa konfigurasi proxy. Ini penting untuk memastikan
P0 tidak terhambat.

## 3. Konsekuensi untuk rencana

| Keputusan | Alasan dari audit |
|---|---|
| egui, bukan Tauri | 8 GB RAM; tidak butuh WebView2 yang sudah ada |
| Sensor berlapis + helper | thermal zone kosong, SMART butuh admin |
| Fixture-first testing | tidak ada NVMe, tidak ada NVIDIA, tidak ada thermal zone |
| Tanpa driver kernel | butuh signing + uji di banyak mesin; mesin ini non-admin |
| Budget RAM < 100 MB | 8 GB total, aplikasi monitoring tidak boleh jadi beban |
| Verifikasi di mesin lain sebelum rilis | matriks hardware tidak bisa dipenuhi di sini |

## 4. Yang belum bisa diverifikasi dari audit ini

- Apakah `link.exe` benar-benar ditemukan rustc (perlu build percobaan).
- Apakah IGCL bisa membaca suhu iGPU UHD 630 (perlu eksperimen).
- Apakah Secure Boot aktif (butuh elevasi) — relevan hanya kalau kelak ada driver.
- Perilaku setelah sleep/resume di mesin ini.

Semua item di atas menjadi tugas P0/P3, bukan asumsi.
