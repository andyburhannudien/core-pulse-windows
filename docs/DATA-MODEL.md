# Model data

Semua tipe di sini berada di `core-pulse-core::model` dan dipakai bersama oleh
UI, CLI, dan helper (lewat `serde`). Skema adalah **kontrak**: `core-pulse-cli --json`
dipakai orang lain, jadi perubahan yang memutus kompatibilitas wajib menaikkan
`SCHEMA_VERSION` dan didokumentasikan di [CHANGELOG-SCHEMA](DATA-MODEL.md#changelog-skema).

## 1. Konvensi satuan

| Tipe data | Satuan | Tipe Rust | Catatan |
|---|---|---|---|
| Ukuran | byte | `u64` | jangan pakai `f64`, presisi hilang di atas 8 PiB |
| Persentase | 0–100 | `f32` | bukan 0.0–1.0; UI membulatkan sendiri |
| Durasi | milidetik | `u64` | CPU time, response time |
| Timestamp | unix ms | `u64` | UTC, bukan local |
| Suhu | °C | `f32` | bisa negatif |
| Frekuensi | MHz | `u32` | — |
| Rate | byte/detik | `u64` | — |

**`Option::None` berarti "tidak diketahui".** `0` berarti nilai nol yang benar.
Ini aturan paling penting di dokumen ini; UI dan CLI wajib membedakannya.

## 2. `Snapshot`

```rust
pub const SCHEMA_VERSION: u16 = 1;

pub struct Snapshot {
    pub schema: u16,
    /// Naik 1 setiap snapshot baru. UI memakai ini untuk mendeteksi data segar.
    pub seq: u64,
    pub ts_unix_ms: u64,
    /// Interval sampling sebenarnya, bukan yang diminta. Dipakai untuk delta.
    pub interval_ms: u32,
    pub host: HostInfo,
    pub cpu: CpuData,
    pub memory: MemoryData,
    pub disks: Vec<DiskData>,
    pub volumes: Vec<VolumeData>,
    pub net: Vec<NetData>,
    pub gpus: Vec<GpuData>,
    pub processes: Vec<ProcessRow>,
    /// Bagian yang gagal dibaca pada snapshot ini.
    pub degraded: Vec<Degradation>,
}
```

`Snapshot` bersifat immutable dan dibagikan lewat `ArcSwap`. Tidak ada `Mutex`
di jalur render.

### `HostInfo`

```rust
pub struct HostInfo {
    pub computer_name: String,
    pub os_name: String,          // "Windows 11 IoT Enterprise LTSC"
    pub os_build: u32,            // 26100
    pub cpu_brand: String,        // "Intel(R) Core(TM) i5-9500T CPU @ 2.20GHz"
    pub physical_cores: u32,      // 6
    pub logical_cores: u32,       // 6  (i5-9500T tanpa HT)
    pub base_clock_mhz: u32,      // 2208
    pub total_ram_bytes: u64,
    pub uptime_secs: u64,
    pub is_elevated: bool,        // apakah core-pulse sendiri elevated
}
```

### `CpuData`

```rust
pub struct CpuData {
    pub total_percent: f32,
    pub per_core_percent: Vec<f32>,
    pub current_mhz: Vec<u32>,        // per logical core, dari CallNtPowerInformation
    pub queue_length: u32,
    pub context_switches_per_sec: u64,
    pub interrupts_per_sec: u64,
    /// Alasan throttle menurut kernel, kalau tersedia (butuh admin).
    pub throttle_reasons: Option<ThrottleReasons>,
}

pub struct ThrottleReasons {
    pub thermal: bool,
    pub power_limit: bool,
    pub current_limit: bool,
    pub other: bool,
}
```

### `MemoryData`

```rust
pub struct MemoryData {
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub in_use_bytes: u64,        // total - free - standby
    pub modified_bytes: u64,
    pub standby_bytes: u64,
    pub free_bytes: u64,
    pub cached_bytes: u64,
    pub commit_total_bytes: u64,
    pub commit_limit_bytes: u64,  // RAM + pagefile
    pub paged_pool_bytes: u64,
    pub nonpaged_pool_bytes: u64,
    /// Skor tekanan memori 0–100 (heuristik, lihat catatan di bawah).
    pub pressure: u8,
}
```

`pressure` adalah heuristik kita sendiri, **bukan** angka Windows: kombinasi
`in_use/total`, `commit/limit`, dan laju pertumbuhan commit. Rumusnya
didokumentasikan di kode (`mem::pressure_score`) dan tidak boleh dipakai untuk
klaim "RAM Anda kritis" tanpa menyebut bahwa itu estimasi.

### `DiskData` / `VolumeData`

```rust
pub struct DiskData {
    pub index: u32,               // \\.\PhysicalDriveN
    pub model: String,
    pub bus: BusType,             // Nvme, Sata, Usb, Other, Unknown
    pub size_bytes: u64,
    pub media: MediaType,         // Ssd, Hdd, Unknown
    /// `true` = size_bytes dihitung dari volume ter-mount, bukan kapasitas fisik.
    /// Terjadi saat aplikasi tidak elevated (lihat P1-RESULTS §3.1).
    pub size_is_estimate: bool,
    pub read_bps: u64,
    pub write_bps: u64,
    pub busy_percent: f32,        // 100 - % Idle Time, bukan % Disk Time
    pub avg_response_ms: f32,
    pub queue_depth: f32,
}

pub struct VolumeData {
    pub mount: String,            // "C:\"
    pub label: String,
    pub fs: String,               // "NTFS", "ReFS", "FAT32"
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub disk_index: Option<u32>,
}
```

### `NetData`

```rust
pub struct NetData {
    pub name: String,             // "Ethernet", "Wi-Fi"
    pub description: String,      // nama driver
    pub mac: String,
    pub link_speed_bps: u64,
    pub rx_bps: u64,
    pub tx_bps: u64,
    pub is_up: bool,
    pub ipv4: Option<String>,
}
```

### `GpuData`

```rust
pub struct GpuData {
    pub name: String,             // "Intel(R) UHD Graphics 630"
    pub vendor: GpuVendor,        // Intel, Nvidia, Amd, Microsoft, Other
    pub driver_version: String,
    /// Kapasitas memori khusus. `None` sampai DXGI dipakai (P3); PDH hanya
    /// melaporkan pemakaian, bukan kapasitas.
    pub dedicated_mem_bytes: Option<u64>,
    pub shared_mem_bytes: Option<u64>,
    pub utilization_percent: Option<f32>,
    /// Pemakaian memori khusus saat ini (PDH `GPU Adapter Memory`).
    pub dedicated_used_bytes: Option<u64>,
    /// Pemakaian memori bersama saat ini. Ini yang terisi pada iGPU.
    pub shared_used_bytes: Option<u64>,
    pub temperature_c: Option<f32>,   // None kalau tidak ada NVML/IGCL/ADLX
}
```

**Agregasi GPU** (meniru Task Manager, lihat [P1-RESULTS §3.3](P1-RESULTS.md#33-gpu-maksimum-per-adapter-jumlah-per-proses)):

- `utilization_percent` per adapter = **maksimum** di antara engine (3D, Copy,
  VideoDecode, ...), bukan jumlah.
- `gpu_percent` per proses = **jumlah** engine untuk PID tersebut, dibatasi 100.
- `None` hanya berarti "tidak terukur". Kalau pengukuran GPU aktif dan sebuah
  proses tidak punya instance engine, nilainya `Some(0.0)`.

### `ProcessRow`

Ini baris terpenting di aplikasi. Field opsional menandakan "butuh hak akses
atau sumber data yang mungkin tidak ada".

```rust
pub struct ProcessRow {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,                  // "chrome.exe"
    pub exe_path: Option<PathBuf>,
    pub cmdline: Option<String>,
    pub user: Option<String>,          // "DESKTOP\user"
    pub start_time_unix_ms: u64,       // WAJIB: guard PID reuse

    pub cpu_percent: f32,
    pub cpu_time_ms: u64,
    pub mem_private_bytes: u64,        // "Memory (active private working set)"
    pub mem_working_set_bytes: u64,
    pub mem_shared_bytes: u64,

    pub io_read_bytes: u64,
    pub io_write_bytes: u64,
    pub disk_bps: Option<u64>,         // dari PDH per-process
    pub net_bps: Option<u64>,          // butuh ETW (admin)
    pub gpu_percent: Option<f32>,      // dari PDH GPU Engine

    pub threads: u32,
    pub handles: u32,
    pub status: ProcessStatus,         // Running, Suspended, NotResponding, Zombie
    pub elevated: Option<bool>,
    pub integrity: Option<Integrity>,  // Untrusted..System
    pub signature: Option<Signature>,  // lazy, hanya saat diminta
    pub eco_qos: bool,                 // Efficiency mode aktif?
    pub service_host: bool,            // svchost.exe dsb
}

pub struct Signature {
    pub status: SigStatus,             // Valid, Unsigned, Invalid, Unknown
    pub publisher: Option<String>,     // "Microsoft Windows"
}

pub enum ProcessStatus { Running, Suspended, NotResponding, Zombie }
pub enum Integrity { Untrusted, Low, Medium, High, System, Protected }
```

`start_time_unix_ms` bukan field hiasan. Kombinasi `(pid, start_time)` adalah
identitas proses yang stabil; PID saja bisa dipakai ulang oleh proses lain.
Semua aksi destruktif (end task, suspend) **wajib** memverifikasi pasangan ini
sebelum bertindak. Lihat [THREAT-MODEL.md](THREAT-MODEL.md).

### `Degradation`

```rust
pub struct Degradation {
    pub area: Area,          // Cpu, Memory, Disk, Net, Gpu, Sensors, Processes
    pub reason: DegradeReason,
    pub detail: String,      // pesan manusia, bahasa Inggris, untuk log
}

pub enum DegradeReason {
    NotAvailable,      // sumber tidak ada di mesin ini
    NeedsElevation,    // ada tapi butuh admin
    AccessDenied,
    ApiFailed,
    Timeout,
}
```

UI memetakan `NeedsElevation` → "Butuh hak administrator (pasang helper)",
`NotAvailable` → "Tidak tersedia di perangkat ini". Dua kalimat berbeda karena
tindakan pengguna berbeda.

## 3. Sensor

```rust
pub struct SensorReading {    pub id: String,               // stabil: "cpu.package", "disk.0.temp"
    pub kind: SensorKind,         // Temperature, Fan, Voltage, Power, Clock, Load
    pub label: String,            // "CPU Package", "ST1000LM049 Temperature"
    pub value: f32,
    pub unit: Unit,               // Celsius, Rpm, Volt, Watt, MHz, Percent
    pub min: f32,
    pub max: f32,
    pub avg: f32,
    pub source: SensorSource,
    pub confidence: Confidence,
    pub ts_unix_ms: u64,
}

pub enum SensorSource {
    Acpi,       // MSAcpi_ThermalZoneTemperature / PDH Thermal Zone
    Pdh,        // Thermal Zone Information counters
    Smart,      // SMART disk (NVMe health log / ATA attributes)
    Nvml,       // NVIDIA
    Igcl,       // Intel Graphics Control Library
    Adlx,       // AMD
    LhmBridge,  // LibreHardwareMonitor web server, opt-in
    Helper,     // helper kita sendiri
}

pub enum Confidence {
    /// Dibaca langsung dari sensor hardware.
    Direct,
    /// Diturunkan dari nilai lain (mis. suhu dihitung dari zona ACPI).
    Derived,
    /// Sumber pihak ketiga, bisa salah kalau versinya beda.
    ThirdParty,
}
```

`confidence` ada supaya UI bisa menampilkan penanda kecil pada nilai `Derived`
dan `ThirdParty`. Pengguna berhak tahu angka mana yang benar-benar dari sensor.

## 4. Contoh JSON (`core-pulse-cli top --json`)

```json
{
  "schema": 1,
  "seq": 4812,
  "ts_unix_ms": 1793318400123,
  "interval_ms": 1000,
  "host": {
    "computer_name": "DESKTOP-XXXX",
    "os_name": "Windows 11 IoT Enterprise LTSC",
    "os_build": 26100,
    "cpu_brand": "Intel(R) Core(TM) i5-9500T CPU @ 2.20GHz",
    "physical_cores": 6,
    "logical_cores": 6,
    "base_clock_mhz": 2208,
    "total_ram_bytes": 8589934592,
    "uptime_secs": 182340,
    "is_elevated": false
  },
  "cpu": {
    "total_percent": 12.4,
    "per_core_percent": [18.2, 9.1, 7.4, 22.0, 8.8, 9.0],
    "current_mhz": [2208, 2208, 1600, 2208, 1600, 1600],
    "queue_length": 2,
    "context_switches_per_sec": 24112,
    "interrupts_per_sec": 8134,
    "throttle_reasons": null
  },
  "memory": {
    "total_bytes": 8589934592,
    "available_bytes": 3221225472,
    "in_use_bytes": 4294967296,
    "modified_bytes": 134217728,
    "standby_bytes": 1073741824,
    "free_bytes": 671088640,
    "cached_bytes": 1207959552,
    "commit_total_bytes": 5368709120,
    "commit_limit_bytes": 17179869184,
    "paged_pool_bytes": 268435456,
    "nonpaged_pool_bytes": 134217728,
    "pressure": 41
  },
  "processes": [
    {
      "pid": 4820,
      "parent_pid": 1024,
      "name": "chrome.exe",
      "exe_path": "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
      "cmdline": "\"C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe\"",
      "user": "DESKTOP-XXXX\\user",
      "start_time_unix_ms": 1793310000000,
      "cpu_percent": 4.2,
      "cpu_time_ms": 918233,
      "mem_private_bytes": 402653184,
      "mem_working_set_bytes": 536870912,
      "mem_shared_bytes": 134217728,
      "io_read_bytes": 8123456,
      "io_write_bytes": 1203456,
      "disk_bps": 0,
      "net_bps": null,
      "gpu_percent": 1.8,
      "threads": 34,
      "handles": 1204,
      "status": "Running",
      "elevated": false,
      "integrity": "Medium",
      "signature": { "status": "Valid", "publisher": "Google LLC" },
      "eco_qos": false,
      "service_host": false
    }
  ],
  "degraded": [
    { "area": "Sensors", "reason": "NotAvailable", "detail": "no ACPI thermal zone exposed" },
    { "area": "Sensors", "reason": "NeedsElevation", "detail": "SMART requires helper" }
  ]
}
```

Field `net_bps: null` dan `degraded` adalah bagian dari kontrak: konsumen JSON
harus bisa membedakan "nol byte/detik" dari "tidak diukur".

## 5. Riwayat SQLite

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;

CREATE TABLE meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);                                   -- 'schema_version', 'installed_at'

CREATE TABLE sample_1m (
  ts            INTEGER PRIMARY KEY, -- unix detik, dibulatkan ke menit
  cpu_avg       REAL NOT NULL,
  cpu_max       REAL NOT NULL,
  mem_used      INTEGER NOT NULL,
  gpu_avg       REAL,
  temp_cpu      REAL,                -- NULL kalau tidak tersedia
  temp_gpu      REAL,
  temp_disk_max REAL
);

CREATE TABLE top_process_1h (
  hour      INTEGER NOT NULL,
  exe_name  TEXT NOT NULL,
  cpu_ms    INTEGER NOT NULL,
  peak_mem  INTEGER NOT NULL,
  PRIMARY KEY (hour, exe_name)
);

CREATE TABLE alert (
  ts      INTEGER NOT NULL,
  kind    TEXT NOT NULL,
  subject TEXT NOT NULL,
  value   REAL NOT NULL,
  message TEXT NOT NULL
);

CREATE INDEX idx_alert_ts ON alert(ts);
```

Aturan: `NULL` untuk suhu yang tidak tersedia — konsisten dengan aturan `Option`.
Retensi 30 hari untuk `sample_1m`, 90 hari untuk `top_process_1h`, 90 hari untuk
`alert`. Ukuran maksimum file 200 MB; kalau terlampaui, hapus partisi terlama
lebih awal.

## Changelog skema

| Versi | Tanggal | Perubahan |
|---|---|---|
| 1 | 2026-10-29 | Versi awal, dokumen ini |
