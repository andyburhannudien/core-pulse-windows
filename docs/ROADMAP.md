# Roadmap

Setiap fase punya **gate**: kriteria yang harus lulus sebelum fase berikutnya
dimulai. Kalau gate gagal, kita berhenti dan mendesain ulang, bukan menumpuk kode
di atas fondasi yang belum terbukti.

Estimasi berdasarkan satu pengembang, kerja fokus, mesin pengembangan yang sudah
diaudit (lihat [ENVIRONMENT-AUDIT.md](ENVIRONMENT-AUDIT.md)).

## Ringkasan

| Fase | Isi | Estimasi | Gate |
|---|---|---|---|
| ~~P0~~ | ~~Toolchain + skeleton + spike angka~~ | ~~1–2 hari~~ | **G0 LULUS** — [P0-RESULTS.md](P0-RESULTS.md) |
| ~~P1~~ | ~~Paritas Task Manager (proses + performance)~~ | ~~1–1,5 minggu~~ | **G1 LULUS** — [P1-RESULTS.md](P1-RESULTS.md) |
| ~~P2~~ | ~~Aksi proses + tab Services/Startup/Users/Details~~ | ~~1 minggu~~ | **G2 LULUS** — [P2-RESULTS.md](P2-RESULTS.md) |
| P3 | Sensor framework + SMART + GPU + dashboard | 1–1,5 minggu | **G3 sebagian** — [P3-RESULTS.md](P3-RESULTS.md) |
| ~~P4~~ | ~~Helper + alert + UI~~ | ~~1 minggu~~ | **G4 LULUS (non-admin)** — [P4-RESULTS.md](P4-RESULTS.md) |
| P4b | Riwayat SQLite + tray + single instance | 3–4 hari | belum ada gate |
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

## P1 — Paritas Task Manager (1–1,5 minggu) — **SELESAI**

Hasil lengkap, bukti verifikasi silang, dan temuan ada di
[P1-RESULTS.md](P1-RESULTS.md). Ringkas: Gate G1 lulus dengan selisih **0,00%**
terhadap query PDH independen untuk disk/GPU/jaringan, plus verifikasi I/O nyata
yang menunjukkan rasio **1,00×** (134 MB ditulis, 135 MB terukur).

Urutan kerja yang dijalankan:

| # | Modul | Status |
|---|---|---|
| 1 | `cpu.rs`: per-core %, queue, context switch | **Selesai** (frekuensi per core menyusul) |
| 2 | `mem.rs`: in use / standby / modified / commit / pool | **Selesai** |
| 3 | `disk.rs`: per-volume + PDH `PhysicalDisk` | **Selesai** |
| 4 | `net.rs`: `GetIfTable2` | **Selesai** |
| 5 | `gpu.rs`: PDH `GPU Engine` agregat per adapter & per PID | **Selesai** |
| 6 | Tabel proses: kolom GPU, sort, search, virtualized | **Selesai** (tree & kolom Disk/Net menyusul) |
| 7 | Tab Kinerja: kartu Disk/GPU/Jaringan + sparkline 4 seri | **Selesai** |
| 8 | Detail statis: cmdline, user, integrity, signature, ikon | Belum (P2) |

**Gate G1 — hasil terukur:**

| Kriteria | Ambang | Hasil | Status |
|---|---|---|---|
| Disk throughput vs PDH independen | ≤ 5% | 0,00% | LULUS |
| GPU utilisation vs PDH independen | ≤ 5% | 0,00% | LULUS |
| Network rx vs PDH independen | ≤ 10% | 0,00% | LULUS |
| I/O nyata (rasio terukur/tertulis) | 0,5–2,0 | 1,00× | LULUS |
| Build snapshot p95 | < 100 ms | 47,6 ms | LULUS |
| Test | semua lulus | 58 test | LULUS |

## P2 — Aksi dan tab lain (1 minggu) — **SELESAI**

Hasil lengkap, bukti, dan 9 keputusan teknis ada di
[P2-RESULTS.md](P2-RESULTS.md). Ringkas: Gate G2 lulus dengan guard PID-reuse
yang diuji memakai proses nyata (bukan mock), dan snapshot tetap 45 ms setelah
service 714 entri dipindah ke thread terpisah.

Yang dibangun:

- End task (single + tree), suspend/resume (`NtSuspendProcess`/`NtResumeProcess`).
- Priority class, Efficiency Mode (`SetProcessInformation` +
  `PROCESS_POWER_THROTTLING_EXECUTION_SPEED`), buka lokasi file, salin
  nama/PID/path/command line.
- Tab Layanan (714 entri), Mulai (registry Run + folder Startup, dengan
  pemeriksaan keberadaan file), Pengguna (sesi + agregat), panel detail proses.
- `details_complete` membedakan "belum dibaca" (`…`) dari "tidak tersedia" (`—`).

**Gate G2 — hasil terukur:**

| Kriteria | Hasil | Status |
|---|---|---|
| Aksi identitas benar | terminate berhasil | LULUS |
| Aksi identitas basi (PID reuse) | ditolak, proses tetap hidup | LULUS |
| PID mati / proses kritikal | ditolak | LULUS |
| Snapshot p95 | 45,0 ms | LULUS |
| Test | 86 lulus | LULUS |

Belum ada (disengaja): start/stop layanan (butuh admin, P4), hapus entri startup,
affinity CPU.

## P3 — Sensor (1–1,5 minggu) — **SELESAI (G3 sebagian)**

Hasil lengkap, bukti, dan 9 keputusan teknis ada di
[P3-RESULTS.md](P3-RESULTS.md). Ringkas: framework sensor, provider ACPI/PDH,
SMART (NVMe + ATA), NVML/IGCL, dan bridge LHM sudah berfungsi; parser teruji
lewat fixture 512 byte; nilai di luar rentang fisik dibuang; sensor berjalan di
thread sendiri sehingga snapshot tetap 40 ms.

**Satu kriteria Gate G3 belum bisa dipenuhi di mesin ini**: suhu 2 HDD muncul.
SMART memerlukan `GENERIC_READ` pada `\\.\PhysicalDriveN`, dan itu ditolak tanpa
elevasi (dibuktikan dengan `example smart-access-probe`: `0x0001` untuk akses 0,
`0x0005` untuk `GENERIC_READ`). Solusinya helper elevated di P4 — bukan trik
user-mode.

Yang dibangun:

1. Trait `SensorProvider` + registry + `probe()` berkala 30 detik + `SensorStore`.
2. Provider ACPI/PDH (Tier 0) dengan normalisasi Kelvin/Kelvin×10/Celsius.
3. Provider SMART (Tier 1): NVMe health log + ATA SMART READ DATA + SAT fallback.
4. Provider GPU: NVML (path absolut, 6 fungsi), IGCL (dimuat, suhu belum), ADLX (belum).
5. Bridge LHM (Tier 3), opt-in, loopback-only, 2 MB cap, tanpa redirect.
6. Tab Sensor + status bar + sparkline suhu; panel "Kenapa suhu CPU tidak muncul?".
7. `core-pulse-cli sensors` melaporkan status tiap provider.

**Gate G3 — hasil:**

| Kriteria | Hasil |
|---|---|
| Suhu 2 HDD | **belum** — butuh helper (P4), terbukti dengan probe |
| Status suhu CPU jelas | LULUS — "tidak mengekspos ACPI thermal zone (0 instance)" |
| Tidak ada angka palsu | LULUS — 0 bacaan, 0 `0 °C` |
| Parser NVMe/ATA teruji | LULUS — fixture → 44,85 °C / 41 °C |
| Satu siklus provider | LULUS — p95 40 ms (di thread terpisah) |

## P4 — Helper, alerts, history (1 week) — **PARTIAL**

Done:

- Helper as a Windows Service, install/uninstall via `--install-service` with
  elevation, versioned read-only named-pipe protocol.
- Alert engine (sustained thresholds, dedup, cooldown) plus an in-UI banner and
  a dedicated tab.
- UI rework: navigation rail, command bar, status strip, English throughout.

Still open:

- SQLite history: 1-minute aggregation, 30-day retention, "what was heavy
  yesterday at 3" query (ADR-0006 is written, implementation is not).
- Tray icon, start minimised, single instance, global hotkey.
- Elevated helper verification (needs a manual UAC approval).

Results and evidence: [P4-RESULTS.md](P4-RESULTS.md).

**Gate G4:**

- Helper killed mid-run → UI does not hang, status becomes "needs helper" in
  < 2 s, reconnects automatically when the helper returns. **PASS** (death
  detected in 0 ms).
- Pipe rejects messages > 1 MB and invalid JSON without crashing. **PASS**.
- Application runs normally (without elevated features) on a non-admin account.
  **PASS**.

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
