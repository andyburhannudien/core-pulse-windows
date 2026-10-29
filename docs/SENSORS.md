# Sensor suhu

Ini bagian tersulit dari proyek, dan bagian yang paling mudah dibuat tidak jujur.
Dokumen ini menjelaskan dari mana setiap angka berasal, apa yang terjadi kalau
sumbernya tidak ada, dan kenapa beberapa pendekatan populer sengaja tidak dipakai.

## 1. Masalah inti

Windows tidak menyediakan API user-mode untuk suhu CPU. Yang tersedia hanya
jalur ACPI, dan jalur itu bergantung pada firmware. Banyak mesin — termasuk mesin
pengembangan proyek ini — tidak mengeksposnya sama sekali.

Fakta dari audit mesin pengembangan (HP 85A2, i5-9500T, Windows 11 build 26100):

| Jalur | Kelas/counter | Hasil |
|---|---|---|
| `root\WMI` → `MSAcpi_ThermalZoneTemperature` | kelas ada | **0 instance** |
| PDH `\Thermal Zone Information(*)\Temperature` | counter set ada | **0 instance** |
| `Win32_PerfFormattedData_Counters_ThermalZoneInformation` | — | kosong |
| `root\HP` | namespace ada | tidak ada kelas thermal |
| SMART disk SATA | `IOCTL_STORAGE_QUERY_PROPERTY` | tersedia (2 HDD) |
| NVML | tidak ada GPU NVIDIA | tidak berlaku |
| iGPU Intel UHD 630 | IGCL belum diuji | kemungkinan tersedia |

Konsekuensi desain: **aplikasi harus dirancang untuk hidup tanpa suhu CPU.**
Bukan sebagai edge case, tapi sebagai keadaan normal di sebagian mesin.

## 2. Tier sumber

```
Tier 0  ACPI / PDH thermal zone        user-mode, gratis, sering kosong
Tier 1  SMART disk (NVMe/ATA)          user-mode, butuh admin untuk sebagian disk
Tier 2  GPU vendor SDK                 user-mode, butuh driver vendor (sudah terpasang)
Tier 3  Bridge ke monitor eksternal    user-mode, opt-in, bergantung pihak ketiga
Tier 4  Driver kernel sendiri          TIDAK DIPAKAI di v1
```

### Tier 0 — ACPI / PDH

Dua jalur, dicoba berurutan:

1. PDH counter `\Thermal Zone Information(*)\Temperature` dan
   `\High Precision Temperature` (Kelvin × 10 / Kelvin). Lebih murah dari WMI
   dan tidak butuh COM.
2. `MSAcpi_ThermalZoneTemperature` via WMI, sebagai fallback.

Kalau keduanya kosong: `Degradation { area: Sensors, reason: NotAvailable }`.
Tidak ada percobaan membaca registry BIOS atau trik lain — itu rapuh dan tidak
portabel antar vendor.

Catatan: pada banyak laptop, thermal zone ACPI mengukur **zona chassis**, bukan
die CPU. Nilainya bisa 15–25 °C di bawah suhu die. Karena itu `confidence` untuk
jalur ini adalah `Derived`, dan UI memberi label "Zona ACPI" bukan "CPU".

### Tier 1 — SMART disk

Port langsung dari `disk-pulse`, yang sudah punya parser terverifikasi unit test:

- NVMe: `IOCTL_STORAGE_QUERY_PROPERTY` dengan `STORAGE_PROTOCOL_SPECIFIC_DATA`,
  protokol NVMe, log page `0x02` (SMART/Health Information), byte 1–2 =
  `Composite Temperature` dalam Kelvin.
- ATA: `SMART_RCV_DRIVE_DATA`, atribut `0xC2` (Temperature) dan `0xBE`
  (Airflow Temperature).
- Fallback: `IOCTL_STORAGE_PREDICT_FAILURE`, lalu storage reliability counters.

Butuh `GENERIC_READ` pada `\\.\PhysicalDriveN`, yang biasanya butuh admin.
Tanpa helper: `NeedsElevation`, dan UI menampilkan tombol "Aktifkan lewat helper".

### Tier 2 — GPU

| Vendor | Library | Cara | Catatan |
|---|---|---|---|
| NVIDIA | `nvml.dll` (sudah ada di driver) | `nvml-wrapper`, `nvmlDeviceGetTemperature` | paling andal, tanpa install tambahan |
| Intel | `igcl.dll` (Intel Graphics Driver) | `libloading` + enumerasi temperature domain | belum diverifikasi di mesin ini (hanya iGPU) |
| AMD | `amdadlx64.dll` (Adrenalin) | `libloading` + `IADLXTemperature` | belum diverifikasi |

Semua diakses lewat `libloading` dengan `Option`, bukan link statis. Kalau DLL
tidak ada, backend melaporkan `NotAvailable` dan tidak ada biaya startup.

### Tier 3 — Bridge ke monitor eksternal (opt-in)

Untuk pengguna yang butuh suhu CPU dan sudah menjalankan monitor lain, kita
menyediakan bridge **read-only**:

- **LibreHardwareMonitor**: HTTP `GET http://localhost:8085/data.json` (plugin
  Remote Web Server). Kita parse JSON-nya, petakan nama sensor ke `SensorReading`,
  `confidence: ThirdParty`.
- **HWiNFO**: Shared Memory Support (`Global\HWiNFO_SENS_SM2`), membaca struct
  yang didokumentasikan HWiNFO untuk pembaca pihak ketiga.

Aturan bridge:

1. **Nonaktif secara default.** Pengguna harus menyalakannya di Settings dan
   menyetujui peringatan bahwa data berasal dari pihak ketiga.
2. **Hanya loopback.** Alamat di-hardcode ke `127.0.0.1`, tidak bisa dikonfigurasi
   ke host lain.
3. **Timeout 500 ms, tanpa retry agresif.** Kalau monitor eksternal tidak jalan,
   bridge diam dan melaporkan `NotAvailable`.
4. **Tidak pernah menulis** ke monitor tersebut.

### Tier 4 — Driver kernel sendiri: ditolak untuk v1

Lihat [ADR-0002](ADR/0002-sensor-tiering.md). Ringkasnya: untuk membaca MSR
(`IA32_PACKAGE_THERM_STATUS` 0x1B1, `IA32_THERM_STATUS` 0x19C) dibutuhkan kode
ring0. Itu berarti driver yang harus di-sign, diuji di berbagai build Windows,
dan diperbarui setiap kali Microsoft mengubah aturan. Itu proyek tersendiri,
bukan fitur dari task manager.

## 3. Kenapa bukan WinRing0

Banyak alat open source (OpenHardwareMonitor, dan versi lama LibreHardwareMonitor)
membaca MSR lewat `WinRing0.sys`. Pendekatan itu **tidak boleh** dipakai:

- `WinRing0.sys` punya CVE-2020-14979 (IOCTL yang membocorkan primitif baca/tulis
  MSR dan port I/O ke proses mana pun).
- Microsoft memasukkannya ke daftar vulnerable driver blocklist, sehingga Defender
  memblokirnya.
- Windows 11 24H2+ dengan HVCI menolak memuatnya.

Konsekuensinya praktis: aplikasi yang mengemas WinRing0 akan di-flag antivirus
dan gagal di mesin dengan keamanan default. Ini bukan risiko teoretis, ini
jalur tercepat menuju aplikasi yang tidak bisa dipasang.

Kalau pengguna memilih bridge LHM, mereka menjalankan LHM sendiri dan menerima
risikonya sendiri. Kita tidak mengemas, tidak mendistribusikan, dan tidak
memuat driver tersebut.

## 4. Trait dan siklus hidup

```rust
pub trait SensorProvider: Send {
    fn id(&self) -> ProviderId;
    /// Dicek sekali saat startup dan setiap 30 detik.
    fn probe(&self) -> Availability;
    /// Tidak boleh blocking lebih dari 100 ms.
    fn read(&mut self, out: &mut Vec<SensorReading>);
    /// Dipanggil saat probe berubah dari Unavailable → Available.
    fn init(&mut self) -> Result<(), SensorError> { Ok(()) }
}

pub enum Availability {
    Available,
    NeedsElevation,
    NotAvailable(&'static str),   // alasan untuk log
    Failed(SensorError),
}
```

Aturan implementasi:

- `probe()` tidak boleh melakukan I/O mahal; hasilnya di-cache.
- `read()` tidak boleh `panic`. Kalau satu sensor gagal, kembalikan yang lain.
- Provider yang `NotAvailable` tidak dipanggil `read()` sama sekali (nol biaya).
- Setiap provider punya budget waktu; total satu siklus sensor < 100 ms, kalau
  lewat, provider tersebut di-skip pada siklus berikutnya dan dicatat.

## 5. Status yang ditampilkan UI

Tidak ada angka palsu, tidak ada `0 °C`, tidak ada suhu "kira-kira".

| Kondisi | Tampilan |
|---|---|
| Ada sensor, `Direct` | `62 °C` + tooltip sumber ("NVMe SMART") |
| Ada sensor, `Derived` | `48 °C` + ikon kecil + tooltip ("Zona ACPI, biasanya di bawah suhu die") |
| Ada sensor, `ThirdParty` | `71 °C` + tooltip ("LibreHardwareMonitor, pihak ketiga") |
| `NeedsElevation` | `—` + "Butuh helper" + tombol "Cara mengaktifkan" |
| `NotAvailable` | `—` + "Tidak tersedia di perangkat ini" + tombol "Kenapa?" |
| Provider gagal | `—` + "Gagal membaca" + link ke log |

Panel "Kenapa?" membuka dialog yang menjelaskan kondisi mesin spesifik — inilah
tempat `core-pulse-cli doctor` berguna, karena hasilnya bisa ditempel ke bug report.

## 6. Ambang alert

| Metrik | Warning | Critical | Durasi sebelum notifikasi |
|---|---|---|---|
| Suhu CPU | 85 °C | 95 °C | 15 detik berturut |
| Suhu GPU | 83 °C | 90 °C | 15 detik |
| Suhu disk (SMART) | 55 °C | 65 °C | 30 detik |
| SMART health | atribut kritis berubah | prediksi gagal | segera |
| Tekanan memori | 75 | 90 | 60 detik |

Dedup + cooldown 10 menit per (metrik, subjek), supaya tidak jadi spam. Desain
engine-nya diambil dari `disk-pulse` yang sudah teruji.

## 7. Yang perlu diverifikasi di mesin nyata

Jujur soal ini: beberapa hal belum bisa diuji di mesin pengembangan.

| Item | Status | Cara verifikasi |
|---|---|---|
| SMART NVMe | **belum teruji** (tidak ada NVMe) | fixture dari dump `nvme smart-log` atau mesin lain |
| NVML | **belum teruji** (tidak ada NVIDIA) | mesin dengan GTX/RTX |
| IGCL Intel | **belum teruji** | iGPU UHD 630 di mesin ini, perlu eksperimen |
| ADLX AMD | **belum teruji** | mesin dengan Radeon |
| LHM bridge | belum teruji | install LHM, aktifkan web server, port 8085 |
| ACPI thermal zone | **terbukti kosong di sini** | perlu mesin lain (banyak laptop punya) |

Sampai item di atas terverifikasi, dokumentasi pengguna harus menyebutnya sebagai
"best effort", bukan "didukung".
