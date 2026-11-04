# Hasil P1 — disk, jaringan, GPU, dan Gate G1

Diambil 2026-11-04 pada mesin pengembangan yang sama
([ENVIRONMENT-AUDIT.md](ENVIRONMENT-AUDIT.md)): i5-9500T, 6 core, 8 GB,
Windows 11 build 26100, **non-admin**.

## 1. Gate G1 — hasil

| Kriteria | Ambang | Hasil | Status |
|---|---|---|---|
| Disk throughput vs PDH independen | ≤ 5% | **0,00%** maksimum | LULUS |
| GPU utilisation vs PDH independen | ≤ 5% | **0,00%** maksimum | LULUS |
| Network rx vs PDH independen | ≤ 10% | **0,00%** maksimum | LULUS |
| **I/O nyata** (tulis 134 MB, akumulasi rate) | rasio 0,5–2,0 | **1,00×** | LULUS |
| Build snapshot p95 | < 100 ms | **47,6 ms** | LULUS |
| CPU aplikasi idle | < 1% | **0,64%** sistem | LULUS |
| Private bytes UI | < 100 MB | **38,4 MB** | LULUS |
| Ukuran biner UI | < 20 MB | **5,86 MB** | LULUS |
| Test | semua lulus | **58 test** (39 core, 4 ipc, 15 ui) | LULUS |

### Bukti: verifikasi silang

`core-pulse-cli verify` membuat **query PDH kedua yang terpisah** dari collector,
lalu membandingkan hasil keduanya setiap detik. Ini disengaja: kalau collector
membandingkan dirinya dengan dirinya sendiri, bug parsing atau agregasi tidak akan
pernah tertangkap.

```
disk throughput    selisih rata-rata 0.00%  maksimum 0.00%  ambang  5%  LULUS
GPU utilisation    selisih rata-rata 0.00%  maksimum 0.00%  ambang  5%  LULUS
network rx         selisih rata-rata 0.00%  maksimum 0.00%  ambang 10%  LULUS
```

### Bukti: I/O nyata

Konsistensi dengan PDH belum membuktikan bahwa angkanya benar. Karena itu ada
verifikasi kedua yang menulis file sungguhan sambil mengakumulasi `rate × interval`
dari collector:

```
I/O nyata: 134 MB ditulis dalam 1.99 s (9 sampel, puncak 204.7 MB/s)
  terakumulasi dari collector: 135 MB  (rasio 1.00× terhadap byte ditulis)
  LULUS
```

Rasio 1,00× berarti angka disk yang ditampilkan benar-benar mengukur byte yang
benar-benar ditulis. Ini satu-satunya jenis test yang bisa menangkap kesalahan
"terlihat masuk akal tapi salah" pada pengukuran I/O.

### Bukti: budget performa

```
samples: 15  (interval 1 s)
cpu_self: 0.92% dari satu core  |  0.15% dari sistem (6 core)
private_bytes: 5.6 MB   working_set: 12.5 MB
snapshot_build_ms: avg 18.59  p95 47.57  max 47.57
cold_collector_ms: avg 22.51  p95 33.07  max 33.07
BUDGET: LULUS
```

Snapshot build naik dari 7,3 ms (P0) ke 47,6 ms (P1) karena tambahan tiga sumber
PDH dan enumerasi adapter. Masih di bawah 100 ms, tapi ini **tren yang harus
diawasi**: P2/P3 akan menambah lebih banyak. Kalau melewati 70 ms, langkah
berikutnya adalah memindahkan probe identitas disk ke thread terpisah dan
memakai `PdhCollectQueryData` sekali untuk semua counter (sudah dilakukan) plus
membatasi `GetIfTable2` ke setiap 5 detik.

## 2. Yang dibangun

| Modul | Isi | Sumber data |
|---|---|---|
| `sys/pdh.rs` | wrapper PDH: query tunggal, counter tunggal & array, parser instance | `pdh.dll` |
| `sys/disk.rs` | 2 disk fisik + 4 volume | `IOCTL_STORAGE_QUERY_PROPERTY`, `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS`, PDH `PhysicalDisk` |
| `sys/net.rs` | 4 adapter (Ethernet, Wi-Fi, Wi-Fi 2, Tailscale) | `GetIfTable2` |
| `sys/gpu.rs` | 1 adapter, util per adapter & per proses | PDH `GPU Engine`, `GPU Adapter Memory` |
| `ui` | kolom GPU, kartu Disk/GPU/Jaringan, sparkline 4 seri | — |
| `cli` | `verify` (verifikasi silang + I/O nyata) | — |

## 3. Keputusan teknis penting

### 3.1 Ukuran disk tidak bisa dibaca tanpa admin

`IOCTL_DISK_GET_LENGTH_INFO` dan `IOCTL_STORAGE_READ_CAPACITY` sama-sama gagal
dengan `Access is denied` pada handle yang dibuka dengan akses 0 — padahal akses 0
cukup untuk `IOCTL_STORAGE_QUERY_PROPERTY` (model, bus) dan
`IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` (pemetaan volume→disk).

Solusinya: ukuran diturunkan dari jumlah kapasitas volume pada disk itu, dan
field baru `size_is_estimate: bool` menandai bahwa angka itu perkiraan. CLI
menampilkannya sebagai `500.8 GB (estimasi)`. Ini konsisten dengan ADR-0008:
lebih baik jujur daripada menampilkan angka yang terlihat pasti.

Konsekuensi yang dicatat: kalau ada partisi tidak ter-mount, ukuran akan lebih
kecil dari kapasitas fisik. Saat elevated, `IOCTL_DISK_GET_LENGTH_INFO` berhasil
dan `size_is_estimate` menjadi `false`.

### 3.2 Busy = 100 − idle, bukan % Disk Time

`% Disk Time` bisa melebihi 100% pada beberapa driver (nilainya adalah
"% idle time terbalik" yang dihitung per permintaan I/O, bukan per waktu).
`100 − % Idle Time` selalu berada di 0–100 dan cocok dengan Task Manager.

### 3.3 GPU: maksimum per adapter, jumlah per proses

Dua agregasi berbeda, keduanya meniru Task Manager:

- **Per adapter**: **maksimum** di antara engine. Menjumlahkan semua engine
  (3D + Copy + VideoDecode + ...) menghasilkan angka mustahil seperti 300% pada
  satu adapter.
- **Per proses**: **jumlah** engine untuk PID tersebut, dibatasi 100. Satu proses
  yang me-render sambil men-decode video memang memakai dua engine sekaligus.

Instance PDH duplikat untuk engine yang sama diambil nilai maksimumnya supaya
tidak terhitung dua kali.

### 3.4 Instance PDH punya bentuk yang tidak seragam

Tiga bentuk berbeda, semuanya diverifikasi dengan test:

| Counter | Bentuk instance | Catatan |
|---|---|---|
| `PhysicalDisk` | `0 C: F:` | indeks = token pertama; `_Total` harus dibuang |
| `GPU Engine` | `pid_4820_luid_0x..._engtype_3D` | `pid` dan nilainya **dua token terpisah** |
| `GPU Adapter Memory` | `luid_0x0_0x1_phys_0_dedicated` | LUID = dua nilai hex |

Kesalahan pertama saya: mengira `engtype_3D` satu token dan `pid4820` satu token.
Test menangkapnya sebelum masuk ke UI.

### 3.5 Nama adapter GPU dari registry, bukan DXGI

PDH memberi LUID tapi tidak memberi nama adapter. Nama dibaca dari
`HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-...}\NNNN\DriverDesc`.
Windows selalu punya satu adapter software ("Microsoft Basic Render Driver") yang
tidak punya entri registry; tanpa filter, daftar GPU berisi "GPU 2" yang
membingungkan. Adapter tanpa nama dibuang kalau ada adapter bernama.

Catatan: pencocokan nama ke LUID bersifat **best effort** (urutan registry biasanya
sama dengan urutan adapter). DXGI di P3 akan menggantinya dengan pencocokan LUID
yang pasti, sekaligus memberi kapasitas memori (PDH hanya memberi pemakaian).

### 3.6 Filter adapter jaringan

`GetIfTable2` mengembalikan **41 adapter** di mesin ini, termasuk WFP filter,
Hyper-V vSwitch, dan Wi-Fi Direct. Task Manager hanya menampilkan adapter hardware
yang aktif. Filter yang dipakai:

- buang loopback (`IF_TYPE_SOFTWARE_LOOPBACK` = 24),
- buang tunnel,
- buang yang punya flag filter,
- buang yang bukan hardware **dan** tidak up,
- buang `Local Area Connection*` (Wi-Fi Direct) dan deskripsi berisi "virtual".

Hasilnya 4 adapter yang masuk akal.

### 3.7 PDH rate butuh dua sampel

`PdhCollectQueryData` pertama tidak menghasilkan rate. Konsekuensinya:

- `doctor` mengambil dua sampel dengan jeda 700 ms,
- `verify` mem-prime query referensi sebelum loop,
- `top` tanpa `--watch` memberi jeda 500 ms sebelum mencetak,
- UI: sampel pertama menampilkan 0 B/s selama <1 detik. Diterima; alternatifnya
  adalah menampilkan `—` yang lebih membingungkan pada detik pertama.

## 4. Temuan yang mengubah rencana

| Temuan | Dampak |
|---|---|
| Ukuran disk butuh admin | Tambah `size_is_estimate` di model; dokumentasi jujur |
| GPU kapasitas memori tidak dari PDH | `dedicated_mem_bytes`/`shared_mem_bytes` = `None` sampai DXGI di P3 |
| 41 adapter dari `GetIfTable2` | Filter eksplisit; tanpa itu UI penuh noise |
| Snapshot build naik 7 → 48 ms | Perlu diawasi; ambang internal 70 ms sebelum optimasi |
| `% Idle Time` per disk lebih stabil dari `% Disk Time` | Busy = 100 − idle |
| Proses tanpa instance GPU = 0%, bukan `—` | Kalau pengukuran GPU aktif, `None` hanya berarti "tidak terukur" |

## 5. Yang masih belum ada

- Disk/jaringan **per proses** — butuh ETW kernel provider + admin (P4, helper).
  Snapshot menandainya `NeedsElevation`, UI menampilkan "1 sumber terbatas".
- Kapasitas memori GPU (DXGI) dan suhu GPU (NVML/IGCL/ADLX) — P3.
- Aksi proses, tab Services/Startup/Users — P2.
- Semua sensor suhu — P3.

## 6. Cara mereproduksi

```powershell
cargo build --release -p core-pulse-cli
.\target\release\core-pulse-cli.exe verify --seconds 10 --io-size-mb 128
.\target\release\core-pulse-cli.exe doctor
.\target\release\core-pulse-cli.exe bench --seconds 15

cargo build -p core-pulse-ui
.\target\debug\core-pulse.exe                 # tab Proses (kolom GPU)
.\target\debug\core-pulse.exe performance     # tab Kinerja
```
