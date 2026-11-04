//! Model data yang dibagikan ke UI, CLI, dan helper.
//!
//! Aturan utama: `Option::None` berarti **tidak diketahui**. `0` berarti nilai
//! nol yang benar. Lihat docs/ADR/0008-honest-degradation.md.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Nilai yang naik satu setiap snapshot. UI memakai ini untuk mendeteksi data segar.
pub type Seq = u64;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema: u16,
    pub seq: Seq,
    pub ts_unix_ms: u64,
    pub interval_ms: u32,
    pub host: HostInfo,
    pub cpu: CpuData,
    pub memory: MemoryData,
    pub disks: Vec<DiskData>,
    pub volumes: Vec<VolumeData>,
    pub net: Vec<NetData>,
    pub gpus: Vec<GpuData>,
    pub processes: Vec<ProcessRow>,
    /// Service, startup, dan sesi diperbarui jauh lebih jarang daripada angka
    /// CPU (lihat `Collector::sample`), jadi bisa kosong di snapshot awal.
    #[serde(default)]
    pub services: Vec<ServiceRow>,
    #[serde(default)]
    pub startup: Vec<StartupEntry>,
    #[serde(default)]
    pub sessions: Vec<SessionRow>,
    pub degraded: Vec<Degradation>,
}

impl Snapshot {
    /// Snapshot kosong untuk keadaan awal sebelum sampler berjalan.
    pub fn empty() -> Self {
        Self {
            schema: crate::SCHEMA_VERSION,
            seq: 0,
            ts_unix_ms: 0,
            interval_ms: 0,
            host: HostInfo::default(),
            cpu: CpuData::default(),
            memory: MemoryData::default(),
            disks: Vec::new(),
            volumes: Vec::new(),
            net: Vec::new(),
            gpus: Vec::new(),
            processes: Vec::new(),
            services: Vec::new(),
            startup: Vec::new(),
            sessions: Vec::new(),
            degraded: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HostInfo {
    pub computer_name: String,
    pub os_name: String,
    pub os_build: u32,
    pub cpu_brand: String,
    pub physical_cores: u32,
    pub logical_cores: u32,
    pub base_clock_mhz: u32,
    pub total_ram_bytes: u64,
    pub uptime_secs: u64,
    pub is_elevated: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CpuData {
    pub total_percent: f32,
    pub per_core_percent: Vec<f32>,
    pub current_mhz: Vec<u32>,
    pub queue_length: u32,
    pub context_switches_per_sec: u64,
    pub interrupts_per_sec: u64,
    pub throttle_reasons: Option<ThrottleReasons>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThrottleReasons {
    pub thermal: bool,
    pub power_limit: bool,
    pub current_limit: bool,
    pub other: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryData {
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub in_use_bytes: u64,
    pub modified_bytes: u64,
    pub standby_bytes: u64,
    pub free_bytes: u64,
    pub cached_bytes: u64,
    pub commit_total_bytes: u64,
    pub commit_limit_bytes: u64,
    pub paged_pool_bytes: u64,
    pub nonpaged_pool_bytes: u64,
    pub pressure: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BusType {
    Nvme,
    Sata,
    Usb,
    Other,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaType {
    Ssd,
    Hdd,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiskData {
    pub index: u32,
    pub model: String,
    pub bus: BusType,
    pub media: MediaType,
    pub size_bytes: u64,
    /// `true` kalau `size_bytes` dihitung dari jumlah volume ter-mount, bukan
    /// dari kapasitas fisik (terjadi saat aplikasi tidak elevated).
    #[serde(default)]
    pub size_is_estimate: bool,
    pub read_bps: u64,
    pub write_bps: u64,
    pub busy_percent: f32,
    pub avg_response_ms: f32,
    pub queue_depth: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VolumeData {
    pub mount: String,
    pub label: String,
    pub fs: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub disk_index: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetData {
    pub name: String,
    pub description: String,
    pub mac: String,
    pub link_speed_bps: u64,
    pub rx_bps: u64,
    pub tx_bps: u64,
    pub is_up: bool,
    pub ipv4: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GpuVendor {
    Intel,
    Nvidia,
    Amd,
    Microsoft,
    #[default]
    Other,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GpuData {
    pub name: String,
    pub vendor: GpuVendor,
    pub driver_version: String,
    /// Kapasitas memori khusus. `None` sampai DXGI dipakai (P3); PDH hanya
    /// melaporkan pemakaian, bukan kapasitas.
    pub dedicated_mem_bytes: Option<u64>,
    /// Kapasitas memori bersama. `None` dengan alasan yang sama.
    pub shared_mem_bytes: Option<u64>,
    pub utilization_percent: Option<f32>,
    /// Pemakaian memori khusus saat ini (PDH `GPU Adapter Memory`).
    pub dedicated_used_bytes: Option<u64>,
    /// Pemakaian memori bersama saat ini. Ini yang terisi pada iGPU.
    #[serde(default)]
    pub shared_used_bytes: Option<u64>,
    pub temperature_c: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProcessStatus {
    #[default]
    Running,
    Suspended,
    NotResponding,
    Zombie,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Integrity {
    Untrusted,
    Low,
    Medium,
    High,
    System,
    Protected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SigStatus {
    Valid,
    Unsigned,
    Invalid,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signature {
    pub status: SigStatus,
    pub publisher: Option<String>,
}

/// Identitas proses yang stabil: PID saja bisa dipakai ulang.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProcessKey {
    pub pid: u32,
    pub start_time_unix_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProcessRow {
    pub pid: u32,
    pub parent_pid: u32,
    /// ID sesi Terminal Services. 0 = sesi layanan.
    pub session_id: u32,
    pub name: String,
    pub exe_path: Option<PathBuf>,
    pub cmdline: Option<String>,
    pub user: Option<String>,
    pub start_time_unix_ms: u64,

    pub cpu_percent: f32,
    pub cpu_time_ms: u64,
    pub mem_private_bytes: u64,
    pub mem_working_set_bytes: u64,
    pub mem_shared_bytes: u64,

    pub io_read_bytes: u64,
    pub io_write_bytes: u64,
    pub disk_bps: Option<u64>,
    pub net_bps: Option<u64>,
    pub gpu_percent: Option<f32>,

    /// `true` = detail statis (cmdline, user, integrity) sudah dibaca untuk
    /// proses ini. `false` = belum dibaca; UI harus menampilkan "…", bukan "—".
    /// Membedakan keduanya penting: `None` berarti "tidak tersedia", sedangkan
    /// `false` berarti "belum dicoba" (ADR-0008).
    #[serde(default)]
    pub details_complete: bool,
    pub threads: u32,
    pub handles: u32,
    pub status: ProcessStatus,
    pub elevated: Option<bool>,
    pub integrity: Option<Integrity>,
    pub signature: Option<Signature>,
    pub eco_qos: bool,
    pub service_host: bool,
}

impl ProcessRow {
    pub fn key(&self) -> ProcessKey {
        ProcessKey {
            pid: self.pid,
            start_time_unix_ms: self.start_time_unix_ms,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Area {
    Cpu,
    Memory,
    Disk,
    Net,
    Gpu,
    Sensors,
    Processes,
    Host,
    Services,
    Startup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DegradeReason {
    NotAvailable,
    NeedsElevation,
    AccessDenied,
    ApiFailed,
    Timeout,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Degradation {
    pub area: Area,
    pub reason: DegradeReason,
    pub detail: String,
}

// ---------------------------------------------------------------------------
// Services, startup, sessions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ServiceRunState {
    Stopped,
    StartPending,
    StopPending,
    Running,
    ContinuePending,
    PausePending,
    Paused,
    #[default]
    Unknown,
}

impl ServiceRunState {
    /// Nilai `SERVICE_STATUS.dwCurrentState`.
    pub fn from_raw(raw: u32) -> Self {
        match raw {
            1 => Self::Stopped,
            2 => Self::StartPending,
            3 => Self::StopPending,
            4 => Self::Running,
            5 => Self::ContinuePending,
            6 => Self::PausePending,
            7 => Self::Paused,
            _ => Self::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Stopped => "Berhenti",
            Self::StartPending => "Mulai…",
            Self::StopPending => "Berhenti…",
            Self::Running => "Berjalan",
            Self::ContinuePending => "Lanjut…",
            Self::PausePending => "Jeda…",
            Self::Paused => "Dijeda",
            Self::Unknown => "—",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ServiceStartType {
    Boot,
    System,
    Auto,
    /// Start otomatis tapi tertunda — dampak boot lebih kecil.
    AutoDelayed,
    Manual,
    Disabled,
    #[default]
    Unknown,
}

impl ServiceStartType {
    /// Nilai `SERVICE_STATUS.dwStartType`. Nilai 2 (auto) tidak membedakan
    /// delayed auto start; perbedaan itu hanya ada di konfigurasi, bukan status.
    pub fn from_raw(raw: u32) -> Self {
        match raw {
            0 => Self::Boot,
            1 => Self::System,
            2 => Self::Auto,
            3 => Self::Manual,
            4 => Self::Disabled,
            _ => Self::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Boot => "Boot",
            Self::System => "System",
            Self::Auto => "Otomatis",
            Self::AutoDelayed => "Otomatis (tertunda)",
            Self::Manual => "Manual",
            Self::Disabled => "Nonaktif",
            Self::Unknown => "—",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServiceRow {
    pub name: String,
    pub display_name: String,
    pub state: ServiceRunState,
    pub start_type: ServiceStartType,
    /// PID proses host, 0 kalau service tidak berjalan.
    pub pid: u32,
    pub accepts_stop: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StartupSource {
    /// `HKLM\...\Run`
    #[default]
    RegistryMachine,
    /// `HKCU\...\Run`
    RegistryUser,
    /// Folder Startup di ProgramData atau per pengguna.
    StartupFolder,
    /// Scheduled task dengan trigger saat logon/boot.
    ScheduledTask,
}

impl StartupSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::RegistryMachine => "HKLM\\Run",
            Self::RegistryUser => "HKCU\\Run",
            Self::StartupFolder => "Folder Startup",
            Self::ScheduledTask => "Task Scheduler",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StartupEntry {
    pub name: String,
    pub command: String,
    pub source: StartupSource,
    /// `true` kalau entri ada di subtree Wow6432Node (aplikasi 32-bit).
    pub is_32bit: bool,
    /// File yang dirujuk ada di disk. `None` kalau path tidak bisa diurai.
    pub target_exists: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionRow {
    pub session_id: u32,
    pub user: Option<String>,
    pub state: String,
    pub process_count: u32,
    pub cpu_percent: f32,
    pub mem_private_bytes: u64,
}

// ---------------------------------------------------------------------------
// Sensor
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SensorKind {
    Temperature,
    Fan,
    Voltage,
    Power,
    Clock,
    Load,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    Celsius,
    Rpm,
    Volt,
    Watt,
    Mhz,
    Percent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SensorSource {
    Acpi,
    Pdh,
    Smart,
    Nvml,
    Igcl,
    Adlx,
    LhmBridge,
    Hwinfo,
    Helper,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    /// Dibaca langsung dari sensor hardware.
    Direct,
    /// Diturunkan dari nilai lain.
    Derived,
    /// Sumber pihak ketiga.
    ThirdParty,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorReading {
    pub id: String,
    pub kind: SensorKind,
    pub label: String,
    pub value: f32,
    pub unit: Unit,
    pub min: f32,
    pub max: f32,
    pub avg: f32,
    pub source: SensorSource,
    pub confidence: Confidence,
    pub ts_unix_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_snapshot_is_valid() {
        let s = Snapshot::empty();
        assert_eq!(s.schema, crate::SCHEMA_VERSION);
        assert_eq!(s.seq, 0);
        assert!(s.processes.is_empty());
    }

    #[test]
    fn process_key_is_stable_identity() {
        let row = ProcessRow {
            pid: 42,
            start_time_unix_ms: 1000,
            ..Default::default()
        };
        assert_eq!(
            row.key(),
            ProcessKey {
                pid: 42,
                start_time_unix_ms: 1000
            }
        );
        // PID sama, waktu mulai beda -> identitas beda.
        let other = ProcessRow {
            pid: 42,
            start_time_unix_ms: 2000,
            ..Default::default()
        };
        assert_ne!(row.key(), other.key());
    }

    #[test]
    fn unknown_is_null_in_json() {
        let row = ProcessRow {
            pid: 1,
            net_bps: None,
            gpu_percent: None,
            ..Default::default()
        };
        let json = serde_json::to_string(&row).unwrap();
        assert!(json.contains("\"net_bps\":null"));
        assert!(json.contains("\"gpu_percent\":null"));
    }

    #[test]
    fn zero_is_not_null() {
        let row = ProcessRow {
            pid: 1,
            net_bps: Some(0),
            ..Default::default()
        };
        let json = serde_json::to_string(&row).unwrap();
        assert!(json.contains("\"net_bps\":0"));
    }
}
