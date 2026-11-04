# Hasil P3 — framework sensor, SMART, suhu GPU

Diambil 2026-11-04 pada mesin pengembangan yang sama
([ENVIRONMENT-AUDIT.md](ENVIRONMENT-AUDIT.md)): i5-9500T, 6 core, 8 GB,
Windows 11 build 26100, **non-admin**.

## 1. Gate G3 — hasil jujur

| Kriteria | Target | Hasil | Status |
|---|---|---|---|
| Suhu 2 HDD SATA muncul | ya | **tidak** — SMART butuh admin, helper belum ada (P4) | **BELUM** |
| Status suhu CPU jelas | ya | "tidak tersedia: firmware tidak mengekspos ACPI thermal zone (0 instance)" | **LULUS** |
| Tidak ada angka palsu | ya | 0 bacaan, 0 `0 °C`; tab Sensor menjelaskan alasannya | **LULUS** |
| Parser NVMe teruji | ya | fixture 512 byte → 44,85 °C, wear 3% | **LULUS** |
| Parser ATA teruji | ya | fixture atribut 0xC2 → 41 °C | **LULUS** |
| Nilai di luar rentang fisik dibuang | ya | test: 1234 °C dan 5000 °C dibuang | **LULUS** |
| Provider tidak tersedia tidak dipanggil `read()` | ya | test: provider `panic!` di `read()` tidak pernah terpanggil | **LULUS** |
| Snapshot p95 | < 100 ms | **40,0 ms** (sensor di thread sendiri) | **LULUS** |
| CPU aplikasi idle | < 1% | **0,38%** sistem | **LULUS** |
| Private bytes UI | < 100 MB | **38,8 MB** | **LULUS** |
| Ukuran biner UI | < 20 MB | **7,71 MB** | **LULUS** |
| Test | semua lulus | **126 test** (67 core, 4 ipc, 34 sensors, 3 fixture, 18 ui) | **LULUS** |

**G3 belum lulus penuh, dan itu dicatat sebagai temuan, bukan disembunyikan.**
Gate aslinya menuntut suhu 2 HDD muncul. Yang menghalangi bukan kode tapi hak
akses: SMART memerlukan `GENERIC_READ` pada `\\.\PhysicalDriveN`, dan itu butuh
administrator. Solusinya adalah helper elevated (P4) — sudah dirancang sejak
ADR-0004, dan P3 menyiapkan seluruh jalur pembacaannya sehingga helper tinggal
memanggilnya.

## 2. Bukti: SMART benar-benar butuh admin di mesin ini

Bukan asumsi. `cargo run --example smart-access-probe` menguji tiga tingkat hak
akses pada dua disk:

```
PhysicalDrive0:
  akses 0          IOCTL gagal: 0x0001     (ERROR_INVALID_FUNCTION)
  GENERIC_READ     open gagal: 0x0005      (ERROR_ACCESS_DENIED)
  R|W              IOCTL gagal: 0x0001
PhysicalDrive1:
  akses 0          IOCTL gagal: 0x0001
  GENERIC_READ     open gagal: 0x0005
  R|W              IOCTL gagal: 0x0001
```

Tiga kesimpulan:

1. Akses 0 **tidak cukup** untuk `IOCTL_STORAGE_QUERY_PROPERTY` dengan
   `StorageDeviceProtocolSpecificProperty` (returnnya `INVALID_FUNCTION`, bukan
   `ACCESS_DENIED` — mudah disalahartikan sebagai "disk tidak mendukung").
2. `GENERIC_READ` ditolak tanpa elevasi (`0x0005`).
3. Jadi satu-satunya jalur adalah helper elevated. Tidak ada trik user-mode.

Probe ini disimpan sebagai example supaya bisa dijalankan ulang di mesin lain
untuk membandingkan.

## 3. Yang dibangun

| Modul | Isi |
|---|---|
| `sensors/lib.rs` | trait `SensorProvider`, `Sensors` registry, `SensorStore` (ArcSwap), `spawn_sensor_poller`, filter nilai tidak masuk akal |
| `sensors/acpi.rs` | Tier 0: PDH `Thermal Zone Information` + normalisasi Kelvin/Kelvin×10/Celsius |
| `sensors/smart.rs` | Tier 1: NVMe health log (IOCTL protocol-specific), ATA SMART READ DATA (ATA pass-through + SAT fallback), parser suhu/wear |
| `sensors/gpu_vendor.rs` | Tier 2: NVML (libloading, 6 fungsi minimal), IGCL (dimuat, suhu belum diimplementasi), pencarian DLL path aman |
| `sensors/bridge.rs` | Tier 3: LHM bridge opt-in, loopback-only, 2 MB cap, tanpa redirect, parser pohon JSON |
| `model` | `Snapshot.sensors`, `Snapshot.sensor_status`, `SensorAvailability`, `SensorReading.disk_index` |
| `ui` | tab Sensor, status bar, sparkline suhu ke-5 |
| `cli` | `sensors --samples N [--bridge]` — alat verifikasi Gate G3 |
| `tests/fixtures/smart/` | payload NVMe 318 K dan ATA 41 °C |

## 4. Keputusan teknis penting

### 4.1 `probe()` harus memeriksa instance, bukan hanya counter

Versi pertama melaporkan `acpi-pdh` sebagai "tersedia" karena counter-nya
terdaftar, lalu menghasilkan 0 bacaan. Itu **melanggar ADR-0008**: UI
menampilkan "tersedia" padahal tidak ada data.

Perbaikan: `probe()` mengembalikan `NotAvailable` kalau `init()` sudah menghitung
0 instance, dan registry memanggil `probe()` **ulang setelah `init()`** supaya
jawaban yang berubah ikut terbaca. Sekarang hasilnya:

```
acpi-pdh    tidak tersedia: firmware tidak mengekspos ACPI thermal zone (0 instance)
smart       butuh administrator (helper P4)
gpu-vendor  tidak tersedia: tidak ada nvml.dll maupun igcl.dll
```

Tiga kalimat berbeda untuk tiga tindakan berbeda. Itu tujuannya.

### 4.2 Filter nilai tidak masuk akal ada di registry, bukan per provider

Sumber yang salah lebih berbahaya daripada tidak ada sumber, karena terlihat
meyakinkan. `is_plausible` dijalankan sekali untuk semua bacaan:

| Jenis | Rentang |
|---|---|
| Suhu | −40…150 °C |
| Fan | 0…20 000 RPM |
| Tegangan | 0…30 V |
| Daya | 0…2 000 W |
| Clock | 0…20 000 MHz |
| Load | 0…100 % |

Suhu 1234 °C dan 5000 °C diuji dan dibuang. Ini juga yang membuat bug driver
tidak pernah sampai ke UI sebagai angka.

### 4.3 Atribut ATA 0xBE: jangan menebak

Sebagian disk sangat lama menyimpan **(100 − suhu)** di atribut 0xBE. Tidak ada
cara memastikan dari payload saja. Versi pertama saya menebak dengan heuristik
(`raw > 60`), dan test menangkap bahwa tebakan itu salah untuk nilai 42.

Keputusan akhir: baca **apa adanya**, prioritaskan 0xC2, dan dokumentasikan
kemungkinan nilai terbalik. Menebak berarti menampilkan angka salah dengan
keyakinan tinggi — persis yang dilarang ADR-0008.

### 4.4 SMART hanya dijalankan kalau benar-benar bisa

`probe()` membuka device dengan `GENERIC_READ`; kalau `ACCESS_DENIED`, hasilnya
`NeedsElevation` dan `read()` tidak pernah dipanggil. Ini menghindari 2 disk ×
40 ms percobaan gagal setiap 2 detik.

Model disk tetap dibaca (akses 0 cukup untuk `StorageDeviceDescriptor`), jadi
label sensor berguna begitu helper tersedia.

### 4.5 Sensor di thread sendiri (2 detik), bukan di sampler

SMART bisa 5–40 ms per disk, bridge HTTP sampai 500 ms. Kalau dijalankan di
sampler 1 Hz, Gate G0 yang sudah lulus akan pecah. Sekarang:

```
sampler   (1 Hz) : CPU/mem/disk/net/GPU/proses  → p95 40 ms
sensors   (0,5 Hz): SMART/ACPI/GPU vendor       → terpisah
slow      (1/15 Hz): services/startup           → terpisah
```

Sampler hanya menyalin `SensorReport` terakhir lewat `ArcSwap` — nol syscall.

### 4.6 NVML: path absolut, 6 fungsi, bukan seluruh API

`libloading` dengan path absolut dari `System32` (THREAT-MODEL T9). Hanya enam
fungsi yang ditautkan (`Init`, `Shutdown`, `GetCount`, `GetHandleByIndex`,
`GetTemperature`, `GetName`), bukan seluruh NVML. `Drop` memanggil
`nvmlShutdown`; urutan field memastikan library hidup lebih lama daripada
pointer fungsinya.

### 4.7 IGCL: dimuat tapi suhu belum diimplementasi

`ctlInit` dipanggil dan berhasil dimuat kalau DLL ada, tapi pembacaan suhu
membutuhkan `ctlEnumerateDevices` + `ctlTemperatureGetProperties` dengan struct
yang layout-nya berbeda antar versi driver Intel, tanpa dokumentasi resmi, dan
tanpa hardware uji (mesin ini hanya iGPU UHD 630 yang tidak mengekspos IGCL —
`igcl.dll` tidak ada sama sekali). Menebak layout berisiko menampilkan angka
salah. Status dilaporkan jujur; diverifikasi ulang di mesin Intel Arc.

### 4.8 Bridge LHM: konstanta, bukan konfigurasi

URL di-hardcode `http://127.0.0.1:8085/data.json`, redirect tidak diikuti, body
dibatasi 2 MB, hanya `application/json` yang diterima, timeout 500 ms. Default
mati. Data ditandai `ThirdParty` dan UI memberi penanda `◇`.

### 4.9 Fixture replay: cara menguji di mesin yang tidak punya hardware-nya

Mesin ini tidak punya NVMe, tidak bisa membaca SMART, dan tidak punya GPU
diskrit. Tanpa fixture, parser NVMe/ATA tidak akan pernah teruji. Sekarang:

```
cargo test -p core-pulse-sensors --test fixture_replay
test result: ok. 3 passed
```

Payload 512 byte direkam sebagai hex, diputar ulang, dan diverifikasi
menghasilkan 44,85 °C / 41 °C. Parser yang sama yang akan dipakai helper.

## 5. Temuan yang mengubah rencana

| Temuan | Dampak |
|---|---|
| SMART tidak bisa dibaca tanpa admin (0x0001/0x0005) | G3 belum penuh; helper P4 wajib, bukan opsional, untuk fitur suhu disk |
| Counter thermal zone terdaftar tapi 0 instance | `probe` harus memeriksa instance; "tersedia" tanpa data melanggar ADR-0008 |
| Tidak ada `nvml.dll`/`igcl.dll` di mesin ini | Suhu GPU tidak bisa diverifikasi; kode siap, klaim ditandai "belum teruji" |
| Tebakan 0xBE salah | Dibaca apa adanya + didokumentasikan, bukan ditebak |
| Sensor poller terpisah | p95 tetap 40 ms; tanpa itu akan >100 ms |

## 6. Yang masih belum ada

- **Suhu disk** — butuh helper elevated (P4). Seluruh pembacaan sudah siap.
- **Suhu GPU Intel/AMD** — IGCL/ADLX belum diimplementasi; NVML siap tapi belum
  teruji di mesin tanpa NVIDIA.
- **Suhu CPU** — tidak akan pernah ada di mesin ini; jalur ACPI sudah
  diimplementasi dan akan berfungsi di mesin yang mengeksposnya.
- Alarm suhu dan riwayat sensor (P4).
- Fan speed, tegangan, daya — trait sudah mendukung, provider belum ada.

## 7. Cara mereproduksi

```powershell
cargo build --release -p core-pulse-cli

# Status semua provider + bacaan (Gate G3)
.\target\release\core-pulse-cli.exe sensors --samples 2

# Dengan bridge LHM (kalau LibreHardwareMonitor berjalan)
.\target\release\core-pulse-cli.exe sensors --bridge

# Bukti bahwa SMART butuh admin di mesin ini
cargo run -p core-pulse-sensors --example smart-access-probe

# Parser diuji tanpa hardware
cargo test -p core-pulse-sensors --test fixture_replay

cargo build -p core-pulse-ui
.\target\debug\core-pulse.exe sensors
```
