# Threat model

Aplikasi ini membaca informasi sistem dan — dalam mode helper — berjalan dengan
hak `LocalSystem`. Itu kombinasi yang layak dianalisis, bukan diasumsikan aman.

Metodologi: STRIDE ringan per trust boundary, difokuskan pada apa yang benar-benar
ada di desain (bukan daftar ancaman generik).

## 1. Aset

| Aset | Nilai bagi penyerang |
|---|---|
| Proses `core-pulse-helper.exe` (LocalSystem) | eksekusi kode sebagai SYSTEM → eskalasi penuh |
| Named pipe `\\.\pipe\core-pulse-v1` | titik masuk ke proses berhak tinggi |
| Database riwayat | kebiasaan pengguna, nama proses, waktu aktif (privasi) |
| Log | path, nama pengguna, kadang command line |
| Integritas angka yang ditampilkan | keputusan pengguna (mis. mematikan proses yang salah) |

## 2. Trust boundary

```
[ pengguna non-admin ] --pipe--> [ helper LocalSystem ] --IOCTL--> [ kernel/driver ]
        │
        └── [ core-pulse.exe ] --baca--> [ API Windows ]

[ pihak ketiga: LHM/HWiNFO ] --HTTP loopback--> [ core-pulse.exe ]
```

Empat boundary: pipe (rendah → tinggi), syscall (user → kernel), loopback HTTP
(pihak ketiga → kita), dan supply chain (crates.io → build).

## 3. Ancaman dan mitigasi

### T1 — Proses lain menyamar sebagai helper / klien

**Skenario:** malware membuat named pipe dengan nama sama lebih dulu, lalu UI
terhubung ke sana dan menerima data palsu (mis. semua suhu normal) — atau
sebaliknya, malware membuat pipe server dan helper kita terhubung sebagai klien.

**Mitigasi:**

- Helper membuat pipe dengan `FILE_FLAG_FIRST_PIPE_INSTANCE`, sehingga tidak bisa
  didahului.
- UI memverifikasi PID server pipe (`GetNamedPipeServerProcessId`) dan mencocokkan
  path executable-nya dengan helper yang terpasang, plus memverifikasi tanda
  tangan biner helper sebelum menerima data.
- Helper memverifikasi klien lewat impersonation + `GetTokenInformation`, hanya
  menerima token dengan integrity `Medium`+ dan user yang sama dengan sesi aktif.
- Kalau verifikasi gagal: UI menampilkan "helper tidak tepercaya" dan **tidak**
  memakai datanya.

**Residual risk:** rendah, tapi bergantung pada kualitas verifikasi signature.
Kalau helper tidak di-sign (build internal), verifikasi diganti dengan
perbandingan hash biner yang dipasang.

### T2 — Pipe sebagai jalur eskalasi hak

**Skenario:** penyerang mengirim perintah ke helper untuk melakukan sesuatu yang
tidak boleh dilakukan user non-admin (membaca disk, mengubah service, mematikan
proses sistem).

**Mitigasi desain — protokol sengaja sempit:**

| Pesan | Arah | Efek |
|---|---|---|
| `Hello { proto_version }` | UI → helper | negosiasi versi |
| `GetSensors` | UI → helper | baca sensor |
| `GetSmart { disk_index }` | UI → helper | baca SMART disk tertentu |
| `SensorUpdate { readings }` | helper → UI | data |
| `SmartUpdate { disks }` | helper → UI | data |
| `Error { code, message }` | helper → UI | — |

Tidak ada pesan "jalankan perintah", "baca file", "tulis registry", atau
"terminate proses". **Helper tidak pernah mengeksekusi apa pun atas permintaan
klien.** Kalau suatu saat fitur butuh itu, harus lewat ADR baru dengan analisis
ulang, bukan ditambahkan diam-diam.

`disk_index` divalidasi sebagai bilangan bulat dalam rentang disk yang benar-benar
ada; helper membangun path `\\.\PhysicalDriveN` sendiri, tidak menerima path dari
klien (mencegah path traversal ke device lain).

### T3 — PID reuse (aksi mengenai proses yang salah)

**Skenario:** pengguna menekan "End task" pada PID 4820. Antara render dan aksi,
proses itu mati dan PID 4820 dipakai proses lain — yang kemudian dimatikan.

**Mitigasi:**

- `ProcessRow` selalu membawa `start_time_unix_ms`.
- Sebelum `TerminateProcess`, buka handle dengan `OpenProcess`, lalu
  `GetProcessTimes` dan bandingkan creation time. Kalau beda > 1 detik: batalkan,
  tampilkan "proses sudah berubah, muat ulang daftar".
- Handle yang dipakai untuk aksi adalah handle yang diverifikasi — bukan PID
  yang dibuka ulang.
- Test otomatis mensimulasikan ini (spawn proses, catat PID, biarkan mati, spawn
  lagi sampai PID sama, pastikan aksi ditolak).

Ini bukan mitigasi teoretis: PID reuse adalah penyebab bug nyata di banyak task
manager.

### T4 — Payload SMART berbahaya

**Skenario:** disk (atau driver/USB adapter) mengembalikan buffer yang panjangnya
tidak sesuai, offset aneh, atau string tanpa NUL.

**Mitigasi:**

- Semua parser menerima `&[u8]` dan memvalidasi panjang sebelum indexing; tidak
  ada `unwrap()` pada slice.
- Offset NVMe health log dan ATA attribute sudah diverifikasi unit test di
  `disk-pulse`; port-nya mempertahankan test tersebut.
- Fuzz target untuk setiap parser (`cargo fuzz run nvme_log`, `ata_attrs`).
- String dari device di-truncate ke 64 byte dan dibersihkan dari karakter kontrol
  sebelum ditampilkan atau di-log.

### T5 — Pesan pipe yang dibuat jahat

**Skenario:** klien mengirim JSON 500 MB, JSON dalam (deeply nested), atau pesan
tanpa akhir untuk membuat helper kehabisan memori.

**Mitigasi:**

- Header pesan wajib: `u32 length` + `u16 version`; `length > 1 MiB` ditolak
  sebelum alokasi.
- `serde_json` dengan batas kedalaman (`disable_recursion_limit` **tidak**
  dipakai).
- Timeout baca 2 detik per pesan; koneksi ditutup kalau terlampaui.
- Maksimum 8 klien bersamaan; klien ke-9 ditolak.

### T6 — Log injection / kebocoran data lewat log

**Skenario:** nama proses berisi `\n` dan teks palsu, atau command line memuat
password, lalu masuk log dan terkirim ke bug report.

**Mitigasi:**

- Semua nilai dari OS di-escape (`{:?}` untuk string, bukan `{}`) saat log.
- Command line **tidak** pernah ditulis ke log; hanya panjangnya.
- Log default `info`, tanpa path lengkap pengguna kecuali level `debug`.
- `core-pulse-cli doctor` mengumpulkan info sistem tanpa nama pengguna dan tanpa
  daftar proses.

### T7 — Bridge pihak ketiga (LHM/HWiNFO)

**Skenario:** sesuatu di `localhost:8085` menjawab dengan JSON besar atau berisi
HTML/redirect ke host lain.

**Mitigasi:**

- Alamat di-hardcode `127.0.0.1`, port tetap, tanpa mengikuti redirect.
- Respons dibatasi 2 MB dan harus `Content-Type: application/json`.
- Hanya membaca field yang dikenal; nilai di luar rentang fisik (mis. suhu
  -273 °C atau 5000 °C) dibuang.
- Nonaktif default; pengguna menyalakan dengan peringatan eksplisit.
- Data ditandai `confidence: ThirdParty` dan tidak pernah dipakai untuk alert
  `Critical` tanpa konfirmasi pengguna.

### T8 — Supply chain

**Mitigasi:**

- `cargo deny check` untuk advisory (RUSTSEC), lisensi, dan duplikat versi.
- `Cargo.lock` di-commit (ini binary, bukan library).
- Dependensi minimal: `windows`, `sysinfo`, `serde`, `parking_lot`, `tracing`,
  `eframe`, `egui_extras`, `egui_plot`, `arc-swap`, `rusqlite`, `tray-icon`,
  `muda`, `libloading`, `nvml-wrapper` (opsional). Tidak ada dependensi yang
  menarik puluhan crate transitif tanpa alasan.
- `cargo audit` dijalankan mingguan di CI, bukan hanya saat rilis.

### T9 — DLL hijacking

**Skenario:** `igcl.dll` atau `nvml.dll` diambil dari direktori kerja atau
`PATH` yang bisa ditulis pengguna.

**Mitigasi:**

- `libloading` selalu memakai path absolut dari `System32` atau direktori driver
  yang diketahui, dan `LoadLibraryExW` dengan `LOAD_LIBRARY_SEARCH_SYSTEM32`.
- Tidak ada `LoadLibrary("nama.dll")` dengan nama saja.
- Verifikasi tanda tangan DLL vendor sebelum dipakai (best effort; kalau gagal,
  backend dilaporkan `NotAvailable`).

### T10 — Update channel (kalau ada)

Kalau auto-update ditambahkan nanti: manifest ditandatangani, HTTPS wajib,
signature diperiksa **sebelum** eksekusi, dan update helper butuh elevasi
terpisah dengan konfirmasi. Untuk v1, tidak ada auto-update.

## 4. Yang sengaja tidak dilakukan (dan kenapa)

| Tidak dilakukan | Alasan |
|---|---|
| Menjalankan perintah arbitrer dari pipe | satu-satunya alasan helper ada adalah membaca; menambah eksekusi mengubah risiko secara kualitatif |
| Menyimpan token/kredensial | tidak ada fitur yang butuh |
| Membaca memori proses lain | butuh `PROCESS_VM_READ`, sering diblokir, dan tidak diperlukan untuk fitur v1 |
| Driver kernel | lihat ADR-0002 |
| Telemetri | tidak ada data keluar; bug report bersifat manual |
| Menjalankan sebagai SYSTEM untuk UI | UI non-elevated adalah prinsip, bukan preferensi |

## 5. Ringkasan residual risk

| Risiko | Tingkat setelah mitigasi |
|---|---|
| Eskalasi lewat pipe | rendah (protokol tanpa perintah, ACL ketat, verifikasi klien) |
| Aksi salah proses (PID reuse) | sangat rendah (guard creation time + test) |
| Crash karena payload device aneh | rendah (validasi + fuzz) |
| Kebocoran privasi lewat log | rendah (tanpa cmdline, escaping) |
| Data sensor palsu dari pihak ketiga | sedang (opt-in, ditandai, tidak untuk alert kritis) |
| Driver pihak ketiga yang diblokir | nol untuk kita (tidak mengemas driver) |
