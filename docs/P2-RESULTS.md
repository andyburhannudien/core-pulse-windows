# Hasil P2 — aksi proses, detail, layanan, startup, pengguna

Diambil 2026-11-04 pada mesin pengembangan yang sama
([ENVIRONMENT-AUDIT.md](ENVIRONMENT-AUDIT.md)): i5-9500T, 6 core, 8 GB,
Windows 11 build 26100, **non-admin**.

## 1. Gate G2 — hasil

| Kriteria | Ambang | Hasil | Status |
|---|---|---|---|
| Aksi dengan identitas benar | berhasil | terminate berhasil, `affected=1` | LULUS |
| Aksi dengan identitas basi (PID reuse) | ditolak + proses hidup | **ditolak**, proses tetap hidup | LULUS |
| Aksi pada PID yang sudah mati | ditolak | ditolak | LULUS |
| Aksi pada proses kritikal (PID 4) | ditolak | ditolak | LULUS |
| Aksi non-destruktif dengan identitas salah | ditolak | ditolak | LULUS |
| Build snapshot p95 | < 100 ms | **45,0 ms** | LULUS |
| CPU aplikasi idle | < 1% | **0,63%** sistem | LULUS |
| Private bytes UI | < 100 MB | **38,7 MB** | LULUS |
| Ukuran biner UI | < 20 MB | **6,22 MB** | LULUS |
| Test | semua lulus | **86 test** (67 core, 4 ipc, 15 ui) | LULUS |

### Bukti: `core-pulse-cli action-selftest`

Seluruh skenario memakai **proses uji yang benar-benar berjalan**, bukan mock:

```
core-pulse action-selftest

  [LULUS] identitas benar → terminate berhasil
  [LULUS] identitas basi → ditolak — PID 7132, diharapkan 1793765637475 ms, aktual 1793765642475 ms
  [LULUS] setelah ditolak, proses tetap hidup
  [LULUS] PID tidak ada → ditolak
  [LULUS] proses kritikal (PID 4) → ditolak
  [LULUS] identitas salah pada aksi non-destruktif → ditolak

G2: LULUS (semua skenario sesuai harapan)
```

Skenario kedua adalah inti Gate G2: PID sama, waktu pembuatan berbeda 5 detik.
Aksi **ditolak sebelum `TerminateProcess` dipanggil**, dan verifikasi lanjutan
memastikan proses masih hidup. Ini bukan test teoretis — PID reuse adalah penyebab
bug nyata di banyak task manager (lihat [THREAT-MODEL T3](THREAT-MODEL.md#t3--pid-reuse-aksi-mengenai-proses-yang-salah)).

### Bukti: budget performa

```
samples: 20  (interval 1 s)
cpu_self: 1.14% dari satu core  |  0.19% dari sistem (6 core)
private_bytes: 6.6 MB   working_set: 13.9 MB
snapshot_build_ms: avg 15.82  p95 44.98  max 44.98
cold_collector_ms: avg 51.22  p95 55.05  max 55.05
BUDGET: LULUS
```

## 2. Yang dibangun

| Modul | Isi |
|---|---|
| `sys/action.rs` | `VerifiedProcess`, terminate, terminate-tree, suspend/resume, prioritas, EcoQoS, reveal-in-explorer |
| `sys/details.rs` | command line (NtQueryInformationProcess kelas 60), user + integrity (token), signature Authenticode + publisher, cache per path |
| `sys/services.rs` | 714 layanan via SCM, start type dari registry |
| `sys/startup.rs` | 4 jalur registry (HKLM/HKCU × 32/64-bit) + folder Startup + pemeriksaan keberadaan file |
| `model` | `ServiceRow`, `StartupEntry`, `SessionRow`, `session_id`, `details_complete` |
| `collector` | `SlowStore` + `spawn_slow_refresher` — service/startup di thread sendiri |
| `ui` | tab Layanan/Mulai/Pengguna, panel detail, menu kontekstual, dialog konfirmasi, feedback status bar |
| `cli` | `action-selftest`, `services`, `startup`, `users` |

## 3. Keputusan teknis penting

### 3.1 `PROCESS_QUERY_LIMITED_INFORMATION` wajib ditambahkan

Bug yang tertangkap test: `OpenProcess(PROCESS_TERMINATE)` berhasil, lalu
`GetProcessTimes` gagal karena handle tidak punya hak query. Akibatnya **setiap
aksi dilaporkan "access denied"** padahal sebenarnya identitasnya bisa
diverifikasi. Perbaikannya: hak query selalu di-`|=` dengan hak aksi, sehingga
satu handle bisa diverifikasi dan dipakai.

Ini juga memastikan prinsip "handle yang diverifikasi = handle yang dipakai"
(THREAT-MODEL T3) benar-benar ditegakkan: tidak ada `OpenProcess` kedua.

### 3.2 Kode error Win32 ada di 16 bit rendah

`windows::core::Error::code()` mengembalikan `HRESULT_FROM_WIN32`, jadi
`ERROR_ACCESS_DENIED` muncul sebagai `0x80070005`, bukan `5`. Membandingkan
dengan `5` membuat semua kegagalan jatuh ke "ProcessGone". Sekarang dipakai
`& 0xFFFF`.

### 3.3 Bug cache detail (ditemukan sendiri saat verifikasi)

Versi pertama menyimpan entri cache kosong untuk proses yang **belum** dibaca
detailnya. Sampel berikutnya melihat cache ada, menganggap detail lengkap, dan
nilai kosong itu tersimpan permanen. Gejalanya: proses yang baru dijalankan
selamanya menampilkan `cmdline: null` meski `details_complete: true`.

Perbaikan: hanya entri yang benar-benar dibaca yang masuk cache, dan pembacaan
dipindah ke pass kedua yang **memprioritaskan proses termuda**. Alasannya
praktis: proses yang baru dijalankan pengguna ada di akhir urutan enumerasi, jadi
dengan budget 40 per sampel ia bisa menunggu 6 detik. Sekarang muncul di sampel
pertama.

### 3.4 `details_complete` membedakan "belum dibaca" dari "tidak tersedia"

Tanpa field ini, `cmdline: null` ambigu: apakah proses terproteksi menolak
dibaca, atau kita belum mencoba? UI sekarang menampilkan `…` untuk yang belum
dibaca dan `—` untuk yang benar-benar tidak tersedia. Ini penerapan ADR-0008
pada tingkat field.

### 3.5 Service: 714 entri butuh 150 ms

Membaca start type lewat `OpenServiceW` + `QueryServiceConfigW` per service
memakan ~200 µs × 714 = ~145 ms. Snapshot p95 melonjak dari 45 ms ke **153 ms**
dan melanggar budget.

Dua perbaikan, keduanya diperlukan:

1. **Start type dari registry** (`HKLM\SYSTEM\CurrentControlSet\Services\<nama>\Start`)
   — ~15 µs, 13× lebih murah. Bonus: `DelayedAutostart` bisa dibaca, sehingga
   "Otomatis (tertunda)" kini dibedakan dari "Otomatis".
2. **Refresh di thread terpisah** (`spawn_slow_refresher` + `SlowStore`). Sampler
   membaca `ArcSwap` dan tidak pernah menyentuh SCM. Ini juga membuat tab Layanan
   tidak memperlambat tab Proses.

Hasil: p95 kembali ke **45 ms**.

### 3.6 Signature hanya diverifikasi saat kolomnya terlihat

Satu `WinVerifyTrust` 5–20 ms; 250 proses = 5 detik. Karena itu:

- checkbox "tanda tangan" default **mati**,
- `SnapshotStore::set_signatures_wanted()` meneruskan niat UI ke sampler,
- budget 3 file baru per sampel saat aktif,
- hasil di-cache per **path** (40 `chrome.exe` = 1 verifikasi).

Catatan temuan: `notepad.exe` di build 26100 ini adalah stub Store app yang
**tidak** bertanda tangan; `explorer.exe` valid. Dokumentasi pengguna tidak boleh
mengklaim "semua biner sistem bertanda tangan".

### 3.7 Proses kritikal: dua lapis

1. `IsProcessCritical` dari kernel — menangkap juga service yang mendaftarkan
   diri sebagai critical.
2. Daftar nama (`System`, `csrss.exe`, `wininit.exe`, `winlogon.exe`,
   `services.exe`, `lsass.exe`, `smss.exe`, `Registry`, `Memory Compression`,
   `Secure System`) — menangkap kasus saat `IsProcessCritical` gagal dipanggil.

Keduanya harus lolos sebelum handle dibuka. UI juga menonaktifkan seluruh item
menu untuk proses tersebut, dengan penjelasan "proses kritikal: aksi dinonaktifkan".

### 3.8 Panel detail harus dideklarasikan sebelum tabel

`egui::Panel::bottom` menyusutkan `ui` induk, jadi tabel yang sudah menghabiskan
tinggi akan membuat panel tidak terlihat. Urutan pemanggilan adalah kontrak, bukan
detail kosmetik. Ini juga berlaku untuk panel lain di masa depan.

### 3.9 Eksekusi di thread UI

`terminate`/`suspend` bisa memakan 1–20 ms. Saat ini dijalankan langsung di thread
UI karena hanya terjadi atas aksi pengguna (bukan per frame), dan menambah
channel + thread worker untuk aksi yang jarang lebih banyak biaya daripada
manfaatnya. Kalau kelak ada "akhiri semua proses terpilih" untuk 100+ proses,
aksi harus dipindah ke worker; dicatat sebagai batasan.

## 4. Temuan yang mengubah rencana

| Temuan | Dampak |
|---|---|
| `PROCESS_QUERY_LIMITED_INFORMATION` wajib ditambah | Tanpa itu semua aksi gagal; ditemukan oleh test, bukan produksi |
| HRESULT_FROM_WIN32 di 16 bit rendah | Klasifikasi error harus `& 0xFFFF`, bukan `== 5` |
| Cache detail kosong merusak permanen | Hanya cache hasil yang benar-benar dibaca |
| Service 714 entri = 150 ms | Start type dari registry + thread terpisah |
| `notepad.exe` tidak bertanda tangan di build 26100 | Test signature memakai `explorer.exe` |
| Sesi 0 tidak punya user yang terbaca non-admin | UI menjelaskan alasannya, bukan menampilkan "—" tanpa konteks |

## 5. Yang masih belum ada

- Menjalankan/menghentikan layanan — butuh admin + helper (P4). Tab Layanan
  menampilkan tombol nonaktif dengan penjelasan.
- Menghapus/menonaktifkan entri startup — ditunda; menghapus registry tanpa
  backup bukan fitur yang boleh dibuat cepat.
- Affinity CPU, integrity level editing, "buka properti file" lengkap.
- Kolom disk/jaringan **per proses** — butuh ETW + admin (P4).
- Semua sensor suhu — P3.

## 6. Cara mereproduksi

```powershell
cargo build --release -p core-pulse-cli
.\target\release\core-pulse-cli.exe action-selftest
.\target\release\core-pulse-cli.exe services --limit 20
.\target\release\core-pulse-cli.exe startup
.\target\release\core-pulse-cli.exe users
.\target\release\core-pulse-cli.exe bench --seconds 20

cargo build -p core-pulse-ui
.\target\debug\core-pulse.exe                  # tab Proses + menu kontekstual
.\target\debug\core-pulse.exe services         # tab Layanan
.\target\debug\core-pulse.exe startup          # tab Mulai
.\target\debug\core-pulse.exe users            # tab Pengguna
.\target\debug\core-pulse.exe detail:4         # panel detail langsung
```
