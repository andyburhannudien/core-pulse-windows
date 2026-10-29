# ADR-0006 — Riwayat SQLite dengan `rusqlite` bundled

**Status:** Diterima
**Tanggal:** 2026-10-29

## Konteks

Fitur "apa yang makan CPU kemarin jam 3" butuh penyimpanan historis. Volume data
rendah (satu baris per menit ≈ 43.200 baris/bulan), tapi harus tahan crash,
mudah di-query rentang waktu, dan tidak menambah dependensi berat.

Kandidat: SQLite (`rusqlite`), `sled`, `redb`, file JSON/JSONL, DuckDB.

## Keputusan

Pakai **SQLite lewat `rusqlite 0.41` dengan fitur `bundled`**, di
`%LOCALAPPDATA%\core-pulse\history.db`, WAL mode, agregasi 1 menit, retensi
30 hari.

## Alasan

1. **Query waktu adalah kekuatan SQLite.** "Rata-rata CPU per jam selama 7 hari"
   adalah satu `GROUP BY`, bukan kode agregasi manual.
2. **`bundled` menghilangkan masalah versi.** SQLite dikompilasi ke dalam biner;
   tidak bergantung pada DLL sistem atau runtime yang mungkin tidak ada.
3. **Tahan crash.** WAL + transaksi berarti file tidak korup kalau proses mati.
4. **Bisa diperiksa dengan alat lain.** Pengguna bisa membuka DB dengan DB Browser
   for SQLite. Itu transparansi yang tidak dimiliki format biner kustom.
5. **Berkas tunggal.** Mudah di-backup, mudah dihapus (uninstall bersih).
6. **`redb`/`sled`** cepat tapi memaksa menulis sendiri semua logika agregasi dan
   query rentang. Tidak sepadan untuk volume data ini.

## Konsekuensi

Positif:

- Skema bisa berevolusi dengan migrasi bertingkat (`PRAGMA user_version`).
- Query kompleks (top process per jam, tren 30 hari) menjadi sederhana.
- Waktu build bertambah sedikit karena SQLite dikompilasi (sekitar 10–20 detik
  pada build bersih).

Negatif:

- Menambah ~1,5 MB ke biner.
- Perlu disiplin menulis dalam batch; commit per sampel akan boros I/O.
- Perlu `VACUUM` berkala setelah penghapusan data lama.

## Aturan implementasi

- Satu koneksi, dipakai dari satu thread writer saja (SQLite tidak bagus untuk
  penulisan paralel).
- Tulis dalam transaksi berisi 60 sampel (sekali per menit).
- `PRAGMA journal_mode=WAL`, `synchronous=NORMAL`, `temp_store=MEMORY`.
- Batas ukuran file 200 MB; kalau terlampaui, hapus partisi terlama lebih awal
  dari jadwal retensi.
- Migrasi skema wajib punya test naik-versi dari setiap versi sebelumnya.
- Riwayat bisa dimatikan sepenuhnya di Settings (privasi); aplikasi tetap berfungsi.
