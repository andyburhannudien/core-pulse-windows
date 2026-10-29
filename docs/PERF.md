# Budget performa

Aplikasi monitoring yang boros sumber daya adalah lelucon yang menyedihkan.
Budget di dokumen ini adalah **syarat rilis**, diukur di CI, bukan dirasakan
saat coding.

## 1. Target

| Metrik | Target | Batas keras | Cara ukur |
|---|---|---|---|
| CPU rata-rata (idle, UI terbuka) | < 0,5% | 1,0% | `GetProcessTimes` delta 60 detik |
| CPU saat scroll tabel | < 3% | 5% | sama, selama interaksi |
| Private bytes UI | < 70 MB | 100 MB | `GetProcessMemoryInfo` |
| Working set UI | < 120 MB | 180 MB | sama |
| Waktu enumerasi 400 proses | < 30 ms | 50 ms | `criterion` |
| Waktu satu siklus semua collector | < 60 ms | 100 ms | `criterion` |
| Serialisasi snapshot → JSON | < 5 ms | 10 ms | `criterion` |
| Alokasi steady-state per snapshot | 0 setelah warmup | 4 alokasi | `dhat` / counter alokasi |
| Ukuran `core-pulse.exe` (release) | < 12 MB | 20 MB | ukur file |
| Waktu startup sampai frame pertama | < 400 ms | 800 ms | log timestamp |
| Query riwayat 24 jam | < 50 ms | 150 ms | `criterion` dengan DB berisi |

Angka ini ditetapkan untuk mesin kelas i5-9500T dengan 8 GB RAM — kelas terendah
yang didukung. Mesin yang lebih cepat akan lebih baik; budget tidak boleh
dinaikkan hanya karena mesin pengembang lebih kencang.

## 2. Cara mengukur

### Di aplikasi

`core-pulse-cli bench --seconds 60` menjalankan sampling loop headless selama
60 detik, lalu mencetak:

```
samples: 60
cpu_self_avg: 0.31%   cpu_self_max: 0.87%
private_bytes: 58.2 MB  working_set: 96.4 MB
enumerate_ms: avg 2.4  p95 3.1  max 4.8
snapshot_build_ms: avg 11.2  p95 14.0
allocations_per_sample: 0
```

Alat ini yang dipakai di CI dan saat investigasi regresi. Tidak perlu membuka
Task Manager untuk menilai aplikasi ini.

### Di CI

- `cargo bench` untuk fungsi murni (enumerasi, serialisasi, query).
- `core-pulse-cli bench` untuk metrik proses (hanya di runner Windows, ambang
  dilonggarkan 2× karena VM lebih lambat, dan tren yang dilihat, bukan angka
  absolut).
- Regresi > 20% dibanding baseline `main` menggagalkan PR.

### Profiling manual

| Alat | Untuk apa |
|---|---|
| Windows Performance Recorder/Analyzer | CPU sampling, alokasi, I/O saat startup |
| `cargo flamegraph` (butuh dtrace/perf, terbatas di Windows) | profil cepat fungsi murni |
| `dhat` | kebocoran dan alokasi per snapshot |
| `GetProcessIoCounters` di `bench` | memastikan kita tidak menyentuh disk di loop |

## 3. Aturan desain yang menjaga budget

1. **Tidak ada polling lebih cepat dari kebutuhan.** Angka 1 Hz; grafik
   menginterpolasi, bukan menambah sampel.
2. **Repaint dipicu oleh data, bukan timer.** Thread sampler memanggil
   `request_repaint()` setelah menyimpan snapshot. Saat tidak ada interaksi,
   aplikasi benar-benar tidur. Versi pertama memakai timer 100 ms dan memakan
   3,65% satu core saat idle — terukur, bukan dugaan.
3. **Tidak ada alokasi di jalur render.** Tabel memakai `TableBuilder` dengan
   `body.rows()` dan `&str` yang sudah ada di snapshot. String diformat ke
   `SmallString`/buffer yang dipakai ulang.
4. **Ikon dan signature di-cache per path executable**, bukan per PID. 40 proses
   `chrome.exe` memakai satu ikon.
5. **PDH query dibuat sekali**, bukan per sampel. `PdhCollectQueryData` lalu baca
   counter yang sudah terdaftar.
6. **Snapshot lama dibuang.** `ArcSwap` menahan satu snapshot; tidak ada riwayat
   di `core`. Riwayat tampilan hanya ring buffer 60 titik di UI.
7. **`Vec` dipakai ulang** antar snapshot di sampler (`clear()` + `extend`),
   bukan `Vec::new()` tiap detik. Snapshot yang dibagikan tetap immutable karena
   kita membangun nilai baru dari buffer yang dipakai ulang.
8. **Jangan panggil WMI di loop.** WMI ~10–100× lebih mahal dari PDH/`Nt*`.
   WMI hanya untuk data statis (nama OS, model disk) dan fallback sensor.

## 4. Biaya yang sudah diperkirakan

| Operasi | Perkiraan biaya | Frekuensi |
|---|---|---|
| `NtQuerySystemInformation(SystemProcessInformation)` | 1–3 ms | 1 Hz |
| `GetProcessMemoryInfo` × 400 | 0,5–2 ms | 1 Hz |
| `GetProcessTimes` × 400 | < 1 ms | 1 Hz |
| `PdhCollectQueryData` (satu query, 12 counter) | 0,2–1 ms | 1 Hz |
| Enumerasi volume | 0,5–2 ms | 1 Hz |
| `GetIfTable2` | < 0,5 ms | 1 Hz |
| SMART read (2 disk) | 5–40 ms | 0,2 Hz (5 detik) |
| `WinVerifyTrust` per exe baru | 5–20 ms | sekali per exe |
| Decode ikon per exe baru | 2–10 ms | sekali per exe |
| Tulis SQLite batch 60 sampel | 2–5 ms | 1 menit |

Total steady-state: **< 10 ms per detik** untuk semua collector. Sisanya adalah
render egui, yang didominasi GPU/CPU compositing dan bergantung pada luas window.

## 5. Regresi yang sudah diketahui akan terjadi

Dicatat supaya tidak mengejutkan:

- Membuka tab Details dengan kolom signature untuk pertama kali akan memicu
  `WinVerifyTrust` untuk ~100 executable sekaligus. Mitigasi: lazy per baris yang
  terlihat + worker pool 2 thread, dan angka sementara ditampilkan sebagai `…`.
- Membuka tab Performance dengan 6 grafik dan 60 titik masing-masing menambah
  ~1–2% CPU karena redraw 10 fps. Mitigasi: saat window tidak fokus, turunkan ke
  1 fps (angka tetap 1 Hz).
- Riwayat 30 hari dengan 400 proses bisa membuat `history.db` ~150 MB. Mitigasi:
  `top_process_1h` hanya menyimpan 20 proses teratas per jam.
