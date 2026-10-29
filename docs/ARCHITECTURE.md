# Arsitektur

Dokumen ini menjelaskan batas antar komponen, model thread, dan alur data.
Aturan di sini bersifat mengikat: perubahan yang melanggarnya harus lewat ADR baru.

## 1. Batas crate

```
core-pulse-core      model + collector + sampler      tanpa UI, tanpa winit, tanpa egui
core-pulse-sensors   trait SensorProvider + backend   boleh unsafe, boleh IOCTL
core-pulse-ipc       protokol pipe + tipe serde       dipakai UI dan helper
core-pulse-ui        bin core-pulse.exe               eframe, tray, view
core-pulse-helper    bin core-pulse-helper.exe        service, elevated
core-pulse-cli       bin core-pulse-cli.exe           headless, --json
```

Aturan dependensi (dijaga CI, bukan konvensi):

| Crate | Boleh bergantung pada | Dilarang |
|---|---|---|
| `core` | `windows`, `sysinfo`, `serde`, `parking_lot`, `tracing` | `egui`, `eframe`, `winit`, `sensors`, `ipc` |
| `sensors` | `core`, `windows`, `libloading`, `nvml-wrapper` | `egui`, `eframe` |
| `ipc` | `serde`, `windows` | `core`, `sensors`, `egui` |
| `ui` | `core`, `sensors`, `ipc`, `eframe`, `egui_extras`, `egui_plot`, `arc-swap`, `tray-icon`, `muda`, `rusqlite` | — |
| `helper` | `sensors`, `ipc`, `core` | `egui`, `eframe` |
| `cli` | `core`, `sensors`, `ipc`, `clap` | `egui`, `eframe` |

`core` tidak tahu bahwa ada UI. Ini yang membuat `core-pulse-cli` mungkin, dan
membuat collector bisa diuji tanpa membuka window.

## 2. Model thread

```
main thread (winit/eframe)
  ├── render 60 fps saat ada input, ~10 fps saat idle
  ├── hanya membaca ArcSwap<Snapshot>  (load(), tanpa lock)
  └── mengirim Command ke sampler lewat kanal mpsc

sampler thread (1 thread, prioritas normal)
  ├── setiap 1000 ms: collect semua collector → Snapshot
  ├── store ke ArcSwap<Snapshot>  (satu-satunya penulis)
  └── kirim event alert ke main thread lewat kanal

helper client thread (1 thread, hanya kalau helper aktif)
  ├── connect/reconnect named pipe dengan backoff
  └── menulis SensorUpdate ke ArcSwap<SensorState> terpisah

icon/signature worker pool (2 thread, opsional)
  └── decode ikon & verifikasi tanda tangan di luar render loop
```

Prinsip:

1. **Render loop tidak pernah melakukan syscall.** Kalau sebuah nilai belum ada
   di snapshot, UI menampilkan placeholder (`—`), bukan memblokir menunggu data.
2. **Satu penulis per state.** `ArcSwap` dipakai justru supaya pembaca tidak
   perlu lock; penulisnya tunggal sehingga tidak ada kontensi.
3. **Sampling tidak mengejar render.** 1 Hz untuk angka, 60 detik ring buffer
   untuk grafik. Tidak ada `sleep(16ms)` polling.
4. **Kegagalan satu collector tidak menghentikan yang lain.** Setiap collector
   mengembalikan `Result<Data, CollectError>`; error dicatat ke snapshot sebagai
   `degraded: true` pada bagian tersebut dan tetap ditampilkan.

## 3. Alur data

```
        windows API / PDH                      helper (opsional)
                │                                     │
                v                                     v
     core::collect::*  ──────────►  Snapshot  ◄──── sensors::*
                │                        │
                │                        │ ArcSwap::store
                v                        v
        cli --json                ui::render (read-only)
```

`Snapshot` adalah nilai immutable yang dibagikan. Tidak ada `Mutex<Vec<Process>>`
di jalur render. Untuk grafik, `ui` menyimpan ring buffer sendiri yang diisi dari
snapshot baru (deteksi lewat `snapshot.seq`), sehingga `core` tidak perlu tahu
tentang histori tampilan.

## 4. Kebijakan `unsafe`

- `#![deny(unsafe_op_in_unsafe_fn)]` dan `#![warn(clippy::undocumented_unsafe_blocks)]`
  di seluruh workspace.
- Setiap blok `unsafe` wajib punya komentar `// SAFETY:` yang menyebut invariant
  yang menjamin keamanannya (bukan "karena ini Windows").
- `unsafe` hanya diizinkan di `core/src/sys/`, `sensors/`, dan `ipc/`. `ui/`
  dilarang memuat `unsafe` kecuali untuk `SetProcessDpiAwarenessContext` saat
  startup, dan itu dibungkus satu fungsi.
- Handle Windows dibungkus tipe RAII (`OwnedHandle`, `PdhQuery`, `PipeHandle`)
  dengan `Drop` yang menutup. Tidak ada `CloseHandle` manual yang tersebar.

## 5. Penanganan error

Tiga kelas, dengan perlakuan berbeda:

| Kelas | Contoh | Perlakuan |
|---|---|---|
| Fatal saat startup | gagal membuat window, gagal load `egui` font | keluar dengan pesan + exit code |
| Degradasi | `MSAcpi_ThermalZoneTemperature` kosong, NVML tidak ada | tandai `unavailable`, UI tetap jalan |
| Transien | proses hilang di tengah enumerasi, pipe putus | `retry` + backoff, jangan log spam |

`anyhow` dipakai di binary (`ui`, `helper`, `cli`), `thiserror` di library
(`core`, `sensors`, `ipc`). Library tidak boleh `panic!` pada input dari OS.

## 6. Cadence dan biaya

| Bagian | Frekuensi | Alasan |
|---|---|---|
| Enumerasi proses | 1 Hz | cukup untuk mata manusia, murah (~1–3 ms) |
| Per-core CPU | 1 Hz | delta berbasis waktu, butuh interval stabil |
| Memori sistem | 1 Hz | — |
| Disk / net / GPU (PDH) | 1 Hz | PDH butuh dua sampel untuk rate |
| Sensor suhu | 1 Hz (2 Hz untuk grafik) | sensor hardware sendiri jarang update lebih cepat |
| Ikon proses | sekali per executable | cache LRU 256 entri |
| Signature | sekali per executable, lazy | mahal (~5–20 ms), hanya saat kolom dibuka |
| Riwayat SQLite | agregasi 1 menit | batasi I/O, retensi 30 hari |

## 7. Jalur degradasi

Setiap fitur punya perilaku eksplisit saat sumbernya hilang. Ini kontrak, bukan
saran:

| Fitur | Tanpa sumber | Yang ditampilkan |
|---|---|---|
| Suhu CPU | tidak ada ACPI zone / helper | "Tidak tersedia di perangkat ini" + link bantuan |
| Suhu disk | SMART butuh admin, helper mati | "Butuh helper" (bukan `0 °C`) |
| Suhu GPU | tidak ada NVML/IGCL/ADLX | kolom GPU temp disembunyikan |
| Per-proses jaringan | butuh ETW (admin) | kolom disembunyikan, tooltip menjelaskan |
| Command line proses lain | butuh `PROCESS_QUERY_LIMITED_INFORMATION` + hak | sel kosong dengan tooltip "akses ditolak" |
| Ikon proses | gagal decode | ikon generik |

Aturan emas: **`None` berarti tidak diketahui, `0` berarti benar-benar nol.**
Jangan pernah memakai `0` sebagai pengganti "tidak tahu".

## 8. Persistensi

SQLite (`rusqlite`, fitur `bundled`) di `%LOCALAPPDATA%\core-pulse\history.db`:

- `sample_1m(ts, cpu_avg, cpu_max, mem_used, gpu_avg, temp_cpu, temp_gpu, temp_disk)`
- `top_process_1h(hour, exe_name, cpu_ms, peak_mem)`
- `alert(ts, kind, subject, value, message)`

Tulis dalam batch (satu transaksi per 60 sampel), WAL mode, `synchronous=NORMAL`.
Retensi 30 hari lewat `DELETE` berkala + `VACUUM` mingguan. Skema lengkap di
[DATA-MODEL.md](DATA-MODEL.md).

## 9. Observability

- `tracing` + `tracing-appender` ke `%LOCALAPPDATA%\core-pulse\logs\`, rotasi harian,
  retensi 7 hari, default level `info`.
- Panic hook menulis lokasi panic + versi + build hash ke log, lalu menampilkan
  dialog, bukan jendela kosong.
- `core-pulse-cli doctor` melaporkan: versi Windows, hak akses, ketersediaan PDH
  counter set, ketersediaan tiap sensor backend, status helper. Ini alat dukungan
  pertama saat ada bug report.

## 10. Yang sengaja tidak dilakukan

- Tidak ada plugin system. Permukaan serangan tidak sepadan manfaatnya di v1.
- Tidak ada remote monitoring / web server. Itu aplikasi lain.
- Tidak ada `static mut` atau global mutable state selain satu `ArcSwap` per proses.
- Tidak ada retry loop tak terbatas pada enumerasi; kalau gagal 5 kali berturut,
  berhenti dan laporkan di UI.
