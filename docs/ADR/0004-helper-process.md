# ADR-0004 — Helper terpisah dengan protokol baca-saja

**Status:** Diterima
**Tanggal:** 2026-10-29

## Konteks

Sebagian data yang kita butuhkan hanya tersedia untuk proses berhak tinggi:

- SMART disk (`\\.\PhysicalDriveN` butuh `GENERIC_READ`),
- ETW per-proses jaringan,
- throttle reasons dari kernel.

Ada tiga cara menyediakannya ke UI:

1. Jalankan seluruh aplikasi sebagai administrator (satu proses, UAC saat start).
2. Helper elevated yang dijalankan sesuai permintaan, komunikasi lewat IPC.
3. Helper sebagai Windows Service yang selalu berjalan.

## Keputusan

Pakai **helper terpisah** (`core-pulse-helper.exe`) yang bisa dipasang sebagai
Windows Service, dengan **protokol baca-saja** lewat named pipe
`\\.\pipe\core-pulse-v1`. UI tidak pernah elevated.

Protokol (lengkap di [THREAT-MODEL §3 T2](../THREAT-MODEL.md#t2--pipe-sebagai-jalur-eskalasi-hak)):

```
UI  → helper : Hello { proto_version }
UI  → helper : GetSensors
UI  → helper : GetSmart { disk_index }
helper → UI  : SensorUpdate { readings }
helper → UI  : SmartUpdate { disks }
helper → UI  : Error { code, message }
```

Tidak ada pesan yang menyebabkan helper mengeksekusi perintah, menulis file, atau
membuka path yang ditentukan klien.

## Alasan

1. **Least privilege.** Prinsip dasar: UI yang menggambar tabel tidak butuh hak
   SYSTEM. Kalau UI crash atau ada bug parsing, kerusakannya terbatas pada proses
   non-admin.
2. **UAC hanya sekali**, saat instalasi helper — bukan setiap kali aplikasi
   dibuka. Task manager yang meminta UAC setiap start akan ditinggalkan pengguna.
3. **UI tetap berfungsi tanpa helper.** Ini yang membuat aplikasi bisa dipakai
   di lingkungan terkunci (kantor, kios): fitur dasar jalan, fitur elevated
   dijelaskan sebagai tidak tersedia.
4. **Service memberi konsistensi.** Data SMART tetap terkumpul saat UI tertutup,
   sehingga riwayat suhu tidak berlubang.
5. **Protokol baca-saja membatasi dampak kompromi.** Kalau pipe disalahgunakan,
   penyerang hanya mendapat data sensor — bukan eksekusi kode sebagai SYSTEM.

## Alternatif yang ditolak

- **Seluruh aplikasi elevated**: melanggar least privilege, UAC setiap start,
  dan membuat bug UI berpotensi fatal. Ditolak.
- **Helper yang menerima perintah generik** (mis. "jalankan ini", "baca file"):
  satu pesan seperti itu mengubah helper menjadi backdoor SYSTEM. Ditolak tanpa
  kompromi; kalau suatu hari dibutuhkan, harus lewat ADR baru.
- **Elevasi sesuai permintaan (runas) per operasi**: UX buruk (UAC berulang),
  dan tetap butuh IPC.
- **Tanpa helper sama sekali**: kehilangan SMART dan suhu disk sepenuhnya.

## Konsekuensi

Positif:

- Permukaan serangan kecil dan bisa dianalisis (lihat THREAT-MODEL).
- UI bisa diuji tanpa hak admin.
- Instalasi helper bersifat opsional dan bisa dicabut.

Negatif:

- Dua biner, dua proses, satu protokol berversi untuk dipelihara.
- Perlu penanganan koneksi putus, versi tidak cocok, dan helper versi lama.
- Pengguna harus melakukan langkah instalasi tambahan untuk suhu disk.

## Detail yang mengikat

- Pipe dibuat dengan `FILE_FLAG_FIRST_PIPE_INSTANCE`.
- ACL: `SYSTEM` full, `Administrators` full, `Authenticated Users` read/write
  (write dibutuhkan untuk mengirim request), tapi helper memvalidasi token klien
  (user sama dengan sesi, integrity ≥ Medium).
- Maksimum pesan 1 MiB, kedalaman JSON dibatasi, timeout baca 2 detik.
- Versi protokol dinegosiasikan; helper menolak versi mayor yang tidak dikenal
  dan UI menampilkan "helper perlu diperbarui".
