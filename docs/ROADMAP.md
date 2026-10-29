# Roadmap

Setiap fase punya **gate**: kriteria yang harus lulus sebelum fase berikutnya
dimulai. Kalau gate gagal, kita berhenti dan mendesain ulang, bukan menumpuk kode
di atas fondasi yang belum terbukti.

Estimasi berdasarkan satu pengembang, kerja fokus, mesin pengembangan yang sudah
diaudit (lihat [ENVIRONMENT-AUDIT.md](ENVIRONMENT-AUDIT.md)).

## Ringkasan

| Fase | Isi | Estimasi | Gate |
|---|---|---|---|
| ~~P0~~ | ~~Toolchain + skeleton + spike angka~~ | ~~1–2 hari~~ | **G0 LULUS** — lihat [P0-RESULTS.md](P0-RESULTS.md) |
| P1 | Paritas Task Manager (proses + performance) | 1–1,5 minggu | G1: selisih < 5% vs Task Manager selama 60 detik |
| P2 | Aksi proses + tab Services/Startup/Users/Details | 1 minggu | G2: tidak ada aksi yang mengenai PID salah |
| P3 | Sensor framework + SMART + GPU + dashboard | 1–1,5 minggu | G3: suhu 2 HDD muncul, status CPU jelas |
| P4 | Helper elevated + alert + riwayat + tray | 1 minggu | G4: helper mati → UI tetap benar |
| P5 | Installer, docs pengguna, CI lengkap | 3–5 hari | G5: install di mesin bersih tanpa admin |

Total: **5–6 minggu** sampai v1.0 yang bisa dipakai harian.

## P0 — Fondasi (1–2 hari) — **SELESAI**

Hasil lengkap, angka terukur, dan temuan ada di [P0-RESULTS.md](P0-RESULTS.md).
Ringkas: G0 lulus dengan selisih CPU 0,49%, enumerasi p95 21 ms, CPU idle 0,14%,
private bytes 36 MB. Dua bug desain ditemukan dan diperbaiki selama P0 (layout
`SYSTEM_PROCESS_INFORMATION` dan `query_fixed` untuk kelas fixed-size) — keduanya
akan menghasilkan "CPU% selalu 0" yang sulit dilacak.

Pekerjaan yang dulu direncanakan:

1. `winget install Rustlang.Rustup`, lalu `rustup default stable-x86_64-pc-windows-msvc`.
   VS 18 BuildTools dan Windows SDK 10.0.26100 sudah terpasang, jadi tidak perlu
   Visual Studio penuh.
2. Skeleton workspace: 6 crate, `rust-toolchain.toml`, `clippy.toml`, `.gitignore`,
   `deny.toml`, `ci.yml`.
3. `core/src/sys/ntapi.rs`: enumerasi proses via `NtQuerySystemInformation`.
4. `core-pulse-cli top --watch 1s`: CPU%, RAM per proses, jumlah proses.
5. Tabel virtualized `egui_extras::TableBuilder`, 300+ baris, refresh 1 Hz.

**Gate G0 — hasil terukur:**

| Kriteria | Ambang | Hasil | Status |
|---|---|---|---|
| Total CPU% vs jalur independen | selisih ≤ 2% | maksimum 0,89% | LULUS |
| Jumlah proses vs Task Manager | sama persis | 246–259 | LULUS |
| Waktu enumerasi | < 50 ms | p95 21,1 ms | LULUS |
| CPU idle aplikasi | < 1% | 0,14% | LULUS |
| RAM UI | < 100 MB | 36 MB private | LULUS |

Kalau `TotalTime` dari `SystemProcessInformation` tidak akurat di build 26100,
fallback berurutan: `GetProcessTimes` per proses → `QueryProcessCycleTime` /
`QueryProcessCycleTime` per core. Kalau ketiganya gagal, **berhenti** dan
laporkan sebelum menulis UI lebih jauh.

## P1 — Paritas Task Manager (1–1,5 minggu)

Urutan kerja (tiap langkah bisa diverifikasi lewat CLI):

| # | Modul | Verifikasi |
|---|---|---|
| 1 | `cpu.rs`: per-core %, frekuensi, queue, context switch | bandingkan tab Performance |
| 2 | `mem.rs`: in use / standby / modified / commit / pool | bandingkan komposisi memori |
| 3 | `disk.rs`: per-volume + PDH `PhysicalDisk` | copy file besar, lihat angka |
| 4 | `net.rs`: `GetIfTable2` + PDH `Network Interface` | unduh file, lihat angka |
| 5 | `gpu.rs`: PDH `GPU Engine` agregat per PID | bandingkan kolom GPU Task Manager |
| 6 | `ui/views/processes.rs`: tabel penuh, tree, search, sort | checklist manual |
| 7 | `ui/views/performance.rs`: grafik CPU/mem/disk/net/GPU | — |
| 8 | Detail statis: cmdline, user, integrity, elevated, signature, ikon | — |

**Gate G1:**

- Angka di samping Task Manager dalam ±5% selama 60 detik berturut, untuk CPU,
  memori, disk, dan jaringan.
- Sort dan search tetap responsif dengan 400+ proses (input → render < 16 ms).
- `core-pulse-cli top --json` mengeluarkan skema yang cocok dengan
  [DATA-MODEL.md](DATA-MODEL.md) dan lolos validasi skema.

## P2 — Aksi dan tab lain (1 minggu)

- End task (single + tree), dengan konfirmasi untuk proses sistem.
- Suspend / resume (`NtSuspendProcess` / `NtResumeProcess`).
- Priority class, affinity mask, Efficiency Mode (`SetProcessInformation` +
  `PROCESS_POWER_THROTTLING_EXECUTION_SPEED`).
- Buka lokasi file, copy command line, copy PID.
- Tab Services (SCM: status, start type, start/stop), Startup (Registry Run +
  Task Scheduler), Users (sesi + proses per sesi), Details (kolom teknis).

**Gate G2:**

- Setiap aksi memverifikasi `(pid, start_time)` sebelum bertindak; test otomatis
  mensimulasikan PID reuse dan memastikan aksi ditolak.
- Aksi pada proses yang butuh elevasi menghasilkan pesan jelas, bukan gagal senyap.
- Tidak ada aksi yang bisa dijalankan tanpa konfirmasi pada proses kritikal
  (`System`, `csrss.exe`, `wininit.exe`, `services.exe`, PID 0/4).

## P3 — Sensor (1–1,5 minggu)

1. Trait `SensorProvider` + registry provider + `probe()` berkala 30 detik.
2. Provider ACPI/PDH (Tier 0).
3. Provider SMART (Tier 1) — port dari `disk-pulse`, termasuk parser dan test.
4. Provider GPU: NVML, IGCL, ADLX (Tier 2).
5. Bridge LHM + HWiNFO (Tier 3), opt-in, default mati.
6. UI: dashboard sensor, grafik suhu vs beban, panel "Kenapa?" per sensor.
7. `core-pulse-cli doctor` melaporkan ketersediaan setiap provider.

**Gate G3:**

- Di mesin pengembangan: suhu 2 HDD (SATA SMART) muncul dan masuk akal
  (25–45 °C saat idle).
- Suhu CPU menampilkan status `NeedsElevation` atau `NotAvailable` dengan
  penjelasan yang benar — **bukan** angka.
- Di mesin dengan NVMe/NVIDIA: suhu terbaca (perlu verifikasi di hardware lain,
  lihat [SENSORS.md §7](SENSORS.md#7-yang-perlu-diverifikasi-di-mesin-nyata)).
- Total waktu satu siklus semua provider < 100 ms.

## P4 — Helper, alert, riwayat (1 minggu)

- Helper sebagai Windows Service, install/uninstall lewat `--install-service`
  dengan elevasi, protokol named pipe read-only berversi.
- Alert engine (dedup + cooldown), notifikasi toast + baris di UI.
- Riwayat SQLite: agregasi 1 menit, retensi 30 hari, query "apa yang berat
  kemarin jam 3".
- Tray icon, start minimized, single instance, global hotkey.

**Gate G4:**

- Helper dimatikan di tengah jalan → UI tidak hang, kolom berubah ke
  "Butuh helper" dalam < 2 detik, reconnect otomatis saat helper hidup lagi.
- Pipe menolak pesan > 1 MB dan JSON yang tidak valid tanpa crash.
- Aplikasi berjalan normal (tanpa fitur elevated) di akun non-admin.

## P5 — Polish (3–5 hari)

- Installer: NSIS (per-user, tanpa admin) + portable ZIP. MSI hanya kalau ada
  kebutuhan enterprise.
- Code signing kalau didistribusikan publik (lihat
  [OPEN-QUESTIONS.md](OPEN-QUESTIONS.md)).
- Dokumentasi pengguna: README, FAQ sensor, halaman troubleshooting.
- CI lengkap: fmt, clippy, nextest, deny, coverage, benchmark gate.
- Fuzz target: parser SMART, parser JSON IPC.

**Gate G5:** install di mesin bersih (tanpa Rust, tanpa VS) berjalan tanpa admin
dan tanpa meminta UAC saat startup.

## Ketergantungan antar fase

```
P0 ──► P1 ──► P2
       │
       └────► P3 ──► P4 ──► P5
```

P3 tidak bergantung pada P2 secara teknis; keduanya bisa ditukar kalau prioritas
berubah (misal ingin demo sensor lebih awal). P4 bergantung pada P3 karena helper
dibangun untuk melayani provider Tier 1/3.

## Risiko jadwal

| Risiko | Dampak | Mitigasi |
|---|---|---|
| Akurasi CPU% tidak cocok dengan Task Manager | P1 mundur, fondasi diragukan | itu isi G0; diuji di hari pertama |
| Sensor CPU tidak tersedia di mesin target | fitur andalan tidak bisa didemokan | dokumentasi jujur + bridge opt-in + uji di mesin lain |
| IGCL/ADLX tidak terdokumentasi baik | Tier 2 Intel/AMD tidak selesai | jadikan best-effort, jangan blokir rilis |
| Waktu habis di UI polish | rilis tertunda | P5 sengaja pendek; fungsionalitas dulu |
