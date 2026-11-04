# Kompatibilitas

Dokumen ini menyatakan apa yang **didukung**, apa yang **best effort**, dan apa
yang **tidak akan pernah** berjalan. Tujuannya supaya tidak ada klaim berlebihan
di README.

## 1. Sistem operasi

| Versi | Status | Catatan |
|---|---|---|
| Windows 11 24H2+ (build 26100+) | **Didukung penuh** | target pengembangan |
| Windows 11 21H2–23H2 | Didukung | `NtQuerySystemInformation` sama |
| Windows 10 1809–22H2 | Didukung | beberapa counter PDH GPU berbeda nama |
| Windows Server 2019/2022 | Best effort | tidak diuji, tapi API-nya sama |
| Windows 8.1 / 7 | **Tidak didukung** | butuh API yang tidak ada; jangan dipaksa |
| Windows on ARM | **Tidak didukung** di v1 | butuh build `aarch64-pc-windows-msvc` dan uji terpisah |

Arsitektur: `x86_64` saja di v1. Build 32-bit tidak masuk akal untuk aplikasi ini
(keterbatasan handle dan alamat), dan ARM64 ditunda sampai ada hardware untuk
menguji.

## 2. Fitur per jenis mesin

Kolom "mesin ini" merujuk pada mesin pengembangan yang diaudit di
[ENVIRONMENT-AUDIT.md](ENVIRONMENT-AUDIT.md).

| Fitur | Desktop/laptop modern | Mesin ini | Syarat |
|---|---|---|---|
| Daftar proses + CPU/RAM | ya | **ya** | — |
| Per-core CPU + frekuensi | ya | **ya** | `CallNtPowerInformation` |
| Komposisi memori (standby/modified) | ya | **ya** | `NtQuerySystemInformation` |
| Disk I/O per volume | ya | **ya** | PDH `PhysicalDisk` |
| Network I/O per adapter | ya | **ya** | `GetIfTable2` |
| GPU utilization per proses | ya | **ya (iGPU)** | PDH `GPU Engine` |
| GPU utilization per adapter | ya | **ya** | PDH `GPU Adapter Memory` |
| Signature & publisher | ya | **ya** | `WinVerifyTrust` |
| Suhu disk SATA | ya, butuh admin | **ya, butuh helper** | `SMART_RCV_DRIVE_DATA` |
| Suhu disk NVMe | ya, butuh admin | **tidak berlaku** | NVMe health log |
| Ukuran disk fisik | ya | **estimasi** (butuh admin untuk angka persis) | `IOCTL_DISK_GET_LENGTH_INFO` |
| Kapasitas memori GPU | ya | **belum** (DXGI, P3) | `IDXGIAdapter3::QueryVideoMemoryInfo` |
| Nama adapter GPU | ya | **ya, best effort** | registry kelas display |
| Suhu CPU via ACPI | tergantung firmware | **tidak** (0 thermal zone) | firmware mengekspos TZ |
| Suhu CPU via MSR | butuh driver | **tidak** | driver ring0 (ditolak v1) |
| Suhu GPU NVIDIA | ya | tidak berlaku | `nvml.dll` |
| Suhu GPU Intel | best effort | **belum teruji** | `igcl.dll` |
| Suhu GPU AMD | best effort | tidak berlaku | `amdadlx64.dll` |
| Per-proses network (ETW) | butuh admin | **tidak** (non-admin) | ETW kernel-network |
| Throttle reasons | butuh admin | **tidak** | `Kernel-Processor-Power` ETW |

## 3. Persyaratan runtime

| Komponen | Wajib? | Catatan |
|---|---|---|
| Windows 10 1809+ | ya | — |
| WebView2 | **tidak** | kita pakai egui, bukan WebView |
| .NET Runtime | **tidak** | murni Rust |
| Visual C++ Redistributable | **tidak** | MSVC runtime di-link statis (`+crt-static`) |
| OpenGL 3.3 (driver GPU) | **ya** | renderer `glow`; `wgpu` gagal render di iGPU UHD 630 mesin uji |
| Hak administrator | tidak untuk UI | hanya untuk helper |
| Ruang disk | ~40 MB | biner + riwayat |

Catatan `+crt-static`: ini menghindari kebutuhan redistributable, dengan
konsekuensi ukuran biner sedikit lebih besar. Sudah diperhitungkan di
[PERF.md](PERF.md).

## 4. Interaksi dengan software lain

| Software | Interaksi |
|---|---|
| Antivirus | tidak ada driver ring0 → tidak memicu deteksi berbasis vulnerable driver. Proses helper yang di-sign mengurangi false positive. |
| LibreHardwareMonitor | opsional, dibaca lewat HTTP loopback kalau pengguna mengaktifkan bridge. Kita **tidak** memuat DLL-nya. |
| HWiNFO | opsional, shared memory, read-only. |
| HWiNFO/CPU-Z/AIDA | tidak konflik; mereka pakai driver sendiri. |
| Windows Defender | tidak ada pengecualian folder yang diminta. |
| RivaTuner/Afterburner | tidak konflik; sama-sama membaca NVML. |
| Proses anti-cheat (Vanguard, EAC) | kita tidak membaca memori proses lain dan tidak memasang driver → tidak konflik. Ini alasan tambahan menolak driver kernel. |
| RivaTuner/Afterburner | tidak konflik; sama-sama membaca NVML. |

## 5. Catatan build dan renderer

- Renderer memakai `eframe` fitur `glow` (OpenGL). Build `wgpu` menghasilkan
  window tanpa isi pada iGPU UHD 630 dengan driver 31.0.101.2135; `glow` berjalan
  normal. Kalau suatu saat pindah ke `wgpu`, uji ulang di mesin ini.
- `egui 0.37` memakai API `App::ui(&mut self, ui, frame)` dan
  `egui::Panel::top/bottom` (bukan `TopBottomPanel`). Contoh di dokumentasi
  egui versi lama tidak bisa dipakai apa adanya.

## 6. Batasan yang diketahui
1. **Suhu CPU tidak dijamin ada.** Ini batasan platform, bukan bug. Lihat
   [SENSORS.md](SENSORS.md).
2. **Per-proses network butuh ETW + admin.** Tanpa itu kolom disembunyikan.
3. **Command line proses sistem** sering tidak terbaca meski elevated (proses
   terproteksi). UI menampilkan alasan, bukan string kosong.
4. **Ikon proses** bisa lambat pada executable di network share. Ada timeout
   250 ms dan ikon generik.
5. **Proses 32-bit dari aplikasi 64-bit** terbaca normal untuk statistik, tapi
   PEB read untuk command line membutuhkan penanganan `WOW64` tersendiri.
6. **Counter PDH GPU** bisa kosong di VM atau mesin tanpa WDDM driver yang benar.
7. **Jumlah proses > 2000** (kasus ekstrem, banyak proses kecil) akan membuat
   enumerasi melewati budget 50 ms. UI tetap jalan, hanya cadence turun ke 2 detik
   secara otomatis dan menampilkan indikator.

## 7. Matriks uji hardware

Untuk rilis, minimal harus diuji di:

| Konfigurasi | Tujuan |
|---|---|
| Intel + iGPU + HDD SATA (mesin ini) | baseline, non-admin |
| AMD Ryzen + dGPU NVIDIA + NVMe | NVML, NVMe SMART |
| Laptop Intel + thermal zone ACPI | Tier 0 berfungsi |
| Laptop AMD + Radeon iGPU | ADLX best effort |
| VM (Hyper-V) | memastikan degradasi mulus, bukan crash |
| Mesin dengan 2 monitor DPI berbeda | layout |

Sampai matriks ini terpenuhi, rilis diberi label **beta** dan README menyebutkan
konfigurasi yang sudah diverifikasi.
