# Hasil P0 — fondasi dan Gate G0

Diambil 2026-10-29 pada mesin pengembangan yang sama dengan
[ENVIRONMENT-AUDIT.md](ENVIRONMENT-AUDIT.md) (i5-9500T, 6 core, 8 GB, Windows 11
build 26100, non-admin).

## 1. Toolchain

```
rustup 1.29.1
rustc  1.99.1 (58cf8f9c3 2026-10-13)
cargo  1.99.1 (5f94df478 2026-08-27)
target x86_64-pc-windows-msvc
```

Dipasang lewat `winget install Rustlang.Rustup`. MSVC Build Tools 18 dan Windows
SDK 10.0.26100 yang sudah ada langsung terdeteksi rustc — **tidak perlu**
`vcvars64.bat` manual, asumsi di ROADMAP §P0 poin 1 terbukti benar.

## 2. Gate G0 — hasil

| Kriteria | Ambang | Hasil | Status |
|---|---|---|---|
| Selisih CPU% vs jalur independen | ≤ 2% | rata-rata 0,49%, maksimum **0,89%** | LULUS |
| Jumlah proses vs Task Manager | sama | 246–259 proses, PID 0–4 ada | LULUS |
| Waktu enumerasi (collector baru) | < 50 ms | p95 **21,1 ms** | LULUS |
| Waktu build snapshot | < 100 ms | p95 **7,3 ms** | LULUS |
| CPU aplikasi (idle, UI terbuka) | < 1% | **0,27%** dari sistem (1,64% satu core) | LULUS |
| Private bytes UI | < 100 MB | **35,8 MB** (WS 81 MB) | LULUS |
| Ukuran biner | < 20 MB | UI **5,78 MB**, CLI 0,64 MB (release) | LULUS |
| Test | semua lulus | **38 test** (19 core, 4 ipc, 15 ui) | LULUS |

### Bukti: perbandingan CPU

`core-pulse-cli compare` membandingkan `NtQuerySystemInformation` +
`SystemProcessorPerformanceInformation` dengan `GetSystemTimes` — dua jalur yang
sepenuhnya independen. 12 sampel:

```
t            ours GetSystemTimes     diff
1           80.94      80.05     0.89
2           87.24      87.37    -0.13
...
12          39.84      40.62    -0.77

selisih rata-rata: 0.49%  maksimum: 0.89%
G0 CPU: LULUS (<= 2%)
```

Selisih sub-1% ini penting: artinya angka CPU% yang kita tampilkan bukan hasil
tebakan dari `sysinfo`, tapi benar-benar cocok dengan kernel.

### Bukti: budget performa

`core-pulse-cli bench --seconds 15` (release build):

```
samples: 15  (interval 1 s)
cpu_self: 0.83% dari satu core  |  0.14% dari sistem (6 core)
private_bytes: 1.9 MB   working_set: 6.7 MB
snapshot_build_ms: avg 3.92  p95 7.25  max 7.25
cold_collector_ms: avg 19.15  p95 21.11  max 21.11
BUDGET: LULUS
```

Angka UI (release build): private 35,8 MB, working set 81 MB, CPU idle
0,27% dari sistem. Dev build sedikit lebih berat (private 36 MB, WS 92 MB) tapi
masih di bawah batas.

Catatan penting soal CPU idle: awalnya UI meminta repaint setiap 100 ms
(10 fps) dan menghabiskan 3,65% satu core saat benar-benar diam. Repaint sekarang
dipicu oleh thread sampler tepat setelah snapshot baru tersedia (1 Hz), jadi saat
tidak ada interaksi aplikasi benar-benar tidur. Ini contoh regresi yang akan
tertangkap lebih awal kalau budget diukur, bukan dirasakan.

## 3. Yang dibangun

| Crate | Isi |
|---|---|
| `core-pulse-core` | model data, `Collector`, `SnapshotStore` (ArcSwap), `Ticker`, akses `Nt*` |
| `core-pulse-ipc` | protokol pipe berversi + validasi frame (belum dipakai) |
| `core-pulse-sensors` | `SensorProvider` trait + registry (belum ada backend) |
| `core-pulse-ui` | tabel proses virtualized, tab Kinerja, status bar jujur |
| `core-pulse-cli` | `top`, `doctor`, `bench`, `compare`, `record` |
| `core-pulse-helper` | stub (P4) |

Collector yang berfungsi: host (OS, CPU, RAM, uptime, elevated), CPU per-core,
memori lengkap (in use/standby/modified/free/commit/pool + skor tekanan), dan
enumerasi proses (nama, PID, PPID, start time, CPU%, working set, private bytes,
I/O, thread, handle, elevated, path executable).

## 4. Keputusan teknis yang diambil saat implementasi

1. **`SYSTEM_PROCESS_INFORMATION` didefinisikan manual.** Proyeksi di crate
   `windows 0.62` menyembunyikan `CreateTime`/`UserTime`/`KernelTime` di balik
   field `Reserved`, sehingga CPU per proses tidak mungkin dihitung. Layout
   didefinisikan sendiri dan diverifikasi dengan test `offset_of!`
   (`create_time`=32, `kernel_time`=48, `unique_process_id`=80, `handle_count`=96,
   ukuran 256 byte). Ini pelajaran nyata: test layout menyelamatkan dari
   salah-baca field yang akan tampak seperti "CPU% selalu 0".
2. **`NtQuerySystemInformation` kelas fixed-size butuh buffer persis.**
   Menggunakan helper buffer-variabel untuk `SystemProcessorPerformanceInformation`
   menghasilkan 0 core — kelas ini tidak mengembalikan
   `STATUS_INFO_LENGTH_MISMATCH`, jadi buffer 256 KiB dianggap sukses dan
   strukturnya terbaca sebagai nol. Diperbaiki dengan `query_fixed()`.
3. **egui 0.37 memakai API baru.** `eframe::App` sekarang punya `fn ui(&mut self, ui, frame)`
   (bukan `update(ctx, frame)`), dan `TopBottomPanel` diganti `egui::Panel::top/bottom`.
   Dokumentasi di UI-SPEC masih menyebut nama lama; perlu diperbarui saat P1.
4. **Default feature `eframe` memakai `wgpu`.** Di mesin ini (iGPU UHD 630,
   driver 31.0.101.2135) build `wgpu` menghasilkan window yang tidak
   ter-render. Beralih ke `glow` memperbaikinya. Dicatat di
   [COMPATIBILITY.md](COMPATIBILITY.md) sebagai syarat runtime.
5. **PID 0 (idle) disembunyikan.** Menampilkannya di puncak daftar CPU hanya
   noise; Task Manager juga tidak menampilkannya di tab Processes.
6. **Repaint dipicu sampler, bukan timer.** Versi pertama memakai
   `request_repaint_after(100ms)` dan menghabiskan 3,65% satu core saat idle.
   Sekarang thread sampler memanggil `ctx.request_repaint()` setelah menyimpan
   snapshot (1 Hz), sehingga CPU idle turun ke 1,64% satu core (0,27% sistem).
   Pola ini masuk ke [PERF.md §3](PERF.md#3-aturan-desain-yang-menjaga-budget)
   sebagai aturan, bukan sekadar catatan.

## 5. Temuan yang mengubah rencana

| Temuan | Dampak |
|---|---|
| Nama OS dari registry masih "Windows 10" di build 26100 | Perlu koreksi berbasis build number; sudah diperbaiki |
| Memori: `in use` = total − free − standby (bukan `total − available`) | Sudah sesuai cara Task Manager; `available` tetap dilaporkan terpisah |
| Enumerasi proses cold ~19 ms untuk 250 proses | Masih di bawah budget, tapi cache statis (path/elevated) memang diperlukan |
| `egui_plot 0.38` tersedia tapi belum dipakai | Sparkline manual 60 baris sudah cukup; grafik penuh menyusul di P1 |

## 6. Yang belum ada (dan disengaja)

- Disk, network, GPU, services, startup, users → P1/P2.
- Aksi proses (end task, suspend, prioritas) → P2.
- Semua sensor suhu → P3.
- Helper dan riwayat → P4.

Tidak ada klaim "sudah jadi task manager" di README; status di sana menyebut
P0 selesai dan P1 berikutnya.

## 7. Cara mereproduksi

```powershell
cargo build --release -p core-pulse-cli
.\target\release\core-pulse-cli.exe compare --seconds 12   # Gate G0 CPU
.\target\release\core-pulse-cli.exe bench --seconds 15     # budget performa
.\target\release\core-pulse-cli.exe doctor                 # ketersediaan sumber

cargo build -p core-pulse-ui
.\target\debug\core-pulse.exe                              # tab Proses
.\target\debug\core-pulse.exe performance                  # tab Kinerja
```
