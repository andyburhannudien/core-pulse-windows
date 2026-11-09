//! Tier 1: SMART disk (suhu dan kesehatan).
//!
//! Port dari `disk-pulse` (C#), yang parser byte-offset-nya sudah terverifikasi
//! unit test. Bagian yang di-port: NVMe health log (log page 0x02) dan ATA
//! Smartmontools READ DATA (0xD0).
//!
//! Akses perangkat: `\\.\PhysicalDriveN` dengan `GENERIC_READ | GENERIC_WRITE`
//! butuh administrator. Write access bukan untuk menulis disk: `IOCTL_ATA_PASS_THROUGH`
//! menolak handle read-only dengan `ERROR_ACCESS_DENIED`, bahkan untuk perintah
//! baca seperti SMART READ DATA. Tanpa hak itu `probe()` melaporkan
//! `NeedsElevation`, dan UI menampilkan "butuh helper" — bukan `0 °C`.
//!
//! Catatan penting soal ATA: register SMART memakai tanda tangan
//! `cylinder low = 0x4F`, `cylinder high = 0xC2`. Salah satu salah dan drive
//! menjawab ABRT, yang **terlihat persis seperti "disk ini tidak punya SMART"**.

use crate::{Availability, Confidence, SensorProvider, reading};
use core_pulse_core::model::{SensorKind, SensorReading, SensorSource, Unit};
use std::ffi::c_void;

/// Satu disk yang bisa dibaca SMART-nya.
#[derive(Debug, Clone)]
struct Target {
    index: u32,
    model: String,
    /// Protokol yang berhasil saat init: NVMe atau ATA.
    protocol: Protocol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Protocol {
    Nvme,
    Ata,
}

pub struct SmartProvider {
    targets: Vec<Target>,
    /// Alasan kegagalan terakhir, untuk dilaporkan ke UI.
    last_error: Option<String>,
    elevation_denied: bool,
}

impl Default for SmartProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SmartProvider {
    pub fn new() -> Self {
        Self {
            targets: Vec::new(),
            last_error: None,
            elevation_denied: false,
        }
    }
}

impl SensorProvider for SmartProvider {
    fn id(&self) -> &'static str {
        "smart"
    }

    fn source(&self) -> SensorSource {
        SensorSource::Smart
    }

    fn probe(&self) -> Availability {
        // Probe murah: cek apakah kita bisa membuka disk 0 dengan hak baca.
        // Ini satu-satunya cara jujur membedakan "tidak ada disk" dari
        // "butuh admin" sebelum init.
        match try_open_device(0) {
            Ok(handle) => {
                // SAFETY: handle dibuka tepat di atas, hanya untuk probe.
                unsafe {
                    let _ = windows::Win32::Foundation::CloseHandle(handle);
                }
                Availability::Available
            }
            Err(OpenError::AccessDenied) => Availability::NeedsElevation,
            Err(OpenError::NotFound) => {
                Availability::NotAvailable("no \\\\.\\PhysicalDrive0 present".into())
            }
            Err(OpenError::Other(msg)) => Availability::Failed(msg),
        }
    }

    fn init(&mut self) -> Result<(), String> {
        self.targets.clear();
        self.elevation_denied = false;

        for index in 0..16u32 {
            let handle = match try_open_device(index) {
                Ok(h) => h,
                Err(OpenError::NotFound) => continue,
                Err(OpenError::AccessDenied) => {
                    self.elevation_denied = true;
                    return Err("administrator rights are required to read SMART".into());
                }
                Err(OpenError::Other(msg)) => {
                    self.last_error = Some(msg);
                    continue;
                }
            };

            // Model: murah, dan memberi label yang bisa dibaca manusia.
            let model = crate::smart::device_model(&handle, index);

            // Coba NVMe dulu: kalau berhasil, tidak perlu ATA.
            if nvme_log(&handle).is_some() {
                self.targets.push(Target {
                    index,
                    model,
                    protocol: Protocol::Nvme,
                });
            } else if ata_smart_data(&handle).is_some() {
                self.targets.push(Target {
                    index,
                    model,
                    protocol: Protocol::Ata,
                });
            } else {
                tracing::debug!(index, "disk tidak menjawab SMART (NVMe maupun ATA)");
            }

            // SAFETY: handle dibuka di atas dan tidak dipakai lagi.
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(handle);
            }
        }

        if self.targets.is_empty() {
            return Err("no disk answered SMART".into());
        }
        Ok(())
    }

    fn read(&mut self, out: &mut Vec<SensorReading>) {
        for target in &self.targets {
            let handle = match try_open_device(target.index) {
                Ok(h) => h,
                Err(_) => continue,
            };

            let payload = match target.protocol {
                Protocol::Nvme => nvme_log(&handle),
                Protocol::Ata => ata_smart_data(&handle),
            };

            if let Some(bytes) = payload {
                match target.protocol {
                    Protocol::Nvme => {
                        if let Some(t) = nvme_temperature_c(&bytes) {
                            out.push(crate::with_disk(
                                reading(
                                    format!("disk.{}.temp", target.index),
                                    SensorKind::Temperature,
                                    format!("{} (NVMe)", target.model),
                                    t,
                                    Unit::Celsius,
                                    SensorSource::Smart,
                                    Confidence::Direct,
                                ),
                                target.index,
                            ));
                        }
                        if let Some(used) = nvme_percentage_used(&bytes) {
                            out.push(crate::with_disk(
                                reading(
                                    format!("disk.{}.wear", target.index),
                                    SensorKind::Load,
                                    format!("{} (wear)", target.model),
                                    used as f32,
                                    Unit::Percent,
                                    SensorSource::Smart,
                                    Confidence::Direct,
                                ),
                                target.index,
                            ));
                        }
                    }
                    Protocol::Ata => {
                        if let Some(t) = ata_temperature_c(&bytes) {
                            out.push(crate::with_disk(
                                reading(
                                    format!("disk.{}.temp", target.index),
                                    SensorKind::Temperature,
                                    format!("{} (SATA)", target.model),
                                    t,
                                    Unit::Celsius,
                                    SensorSource::Smart,
                                    Confidence::Direct,
                                ),
                                target.index,
                            ));
                        }
                    }
                }
            }

            // SAFETY: handle dibuka di atas dan tidak dipakai lagi.
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(handle);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Akses perangkat
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum OpenError {
    AccessDenied,
    NotFound,
    Other(String),
}

fn open_flags() -> u32 {
    // GENERIC_READ | GENERIC_WRITE dari winnt.h. `IOCTL_ATA_PASS_THROUGH`
    // mensyaratkan handle read/write, meski perintahnya hanya membaca SMART.
    // Akses 0 atau read-only menghasilkan ERROR_ACCESS_DENIED (5).
    0x8000_0000 | 0x4000_0000
}

fn try_open_device(index: u32) -> Result<windows::Win32::Foundation::HANDLE, OpenError> {
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::core::PCWSTR;

    let path = format!(r"\\.\PhysicalDrive{index}");
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();

    // SAFETY: path NUL-terminated; handle dikembalikan ke pemanggil.
    match unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            open_flags(),
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    } {
        Ok(handle) => Ok(handle),
        Err(e) => {
            let code = (e.code().0 as u32) & 0xFFFF;
            // 2 = FILE_NOT_FOUND, 3 = PATH_NOT_FOUND, 5 = ACCESS_DENIED.
            match code {
                2 | 3 => Err(OpenError::NotFound),
                5 => Err(OpenError::AccessDenied),
                _ => Err(OpenError::Other(format!("CreateFileW 0x{code:04X}"))),
            }
        }
    }
}

/// Jalankan `DeviceIoControl` dan kembalikan byte yang valid sebagai `Vec<u8>`.
fn ioctl(
    handle: windows::Win32::Foundation::HANDLE,
    code: u32,
    input: Option<&[u8]>,
    out_len: usize,
) -> Option<Vec<u8>> {
    use windows::Win32::System::IO::DeviceIoControl;

    let mut buf = vec![0u64; out_len.div_ceil(8)];
    let mut returned = 0u32;
    // SAFETY: buffer sebesar `out_len` byte dan cukup selaras; input opsional
    // menunjuk ke slice yang hidup selama panggilan.
    let ok = unsafe {
        DeviceIoControl(
            handle,
            code,
            input.map(|i| i.as_ptr() as *const c_void),
            input.map(|i| i.len() as u32).unwrap_or(0),
            Some(buf.as_mut_ptr() as *mut c_void),
            out_len as u32,
            Some(&mut returned),
            None,
        )
    }
    .is_ok();
    if !ok || returned == 0 {
        return None;
    }
    // SAFETY: `returned` byte pertama ditulis kernel.
    let mut bytes = vec![0u8; returned as usize];
    // SAFETY: buffer sumber >= returned byte.
    unsafe {
        std::ptr::copy_nonoverlapping(
            buf.as_ptr() as *const u8,
            bytes.as_mut_ptr(),
            returned as usize,
        );
    }
    Some(bytes)
}

// ---------------------------------------------------------------------------
// NVMe
// ---------------------------------------------------------------------------

/// Baca log page 0x02 (SMART/Health Information) lewat
/// `IOCTL_STORAGE_QUERY_PROPERTY` + `STORAGE_PROTOCOL_SPECIFIC_DATA`.
///
/// 512 byte data; offset 1–2 = Composite Temperature (Kelvin, little endian).
fn nvme_log(handle: &windows::Win32::Foundation::HANDLE) -> Option<Vec<u8>> {
    use windows::Win32::System::Ioctl::{
        IOCTL_STORAGE_QUERY_PROPERTY, STORAGE_PROPERTY_QUERY, STORAGE_PROTOCOL_SPECIFIC_DATA,
        StorageDeviceProtocolSpecificProperty,
    };

    const NVME_HEALTH_LOG_PAGE: u32 = 0x02;
    const DATA_LEN: usize = 512;

    // Header: STORAGE_PROPERTY_QUERY + STORAGE_PROTOCOL_SPECIFIC_DATA,
    // lalu 512 byte data. Layout-nya adalah struct gabungan yang dikirim
    // sebagai satu buffer.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct QueryWithProtocol {
        query: STORAGE_PROPERTY_QUERY,
        protocol: STORAGE_PROTOCOL_SPECIFIC_DATA,
    }

    let input = QueryWithProtocol {
        query: STORAGE_PROPERTY_QUERY {
            PropertyId: StorageDeviceProtocolSpecificProperty,
            QueryType: windows::Win32::System::Ioctl::PropertyStandardQuery,
            AdditionalParameters: [0],
        },
        protocol: STORAGE_PROTOCOL_SPECIFIC_DATA {
            ProtocolType: windows::Win32::System::Ioctl::ProtocolTypeNvme,
            DataType: 0,
            ProtocolDataRequestValue: NVME_HEALTH_LOG_PAGE,
            // NVMe: seluruh log page sekaligus.
            ProtocolDataRequestSubValue: 0,
            ProtocolDataOffset: std::mem::size_of::<STORAGE_PROTOCOL_SPECIFIC_DATA>() as u32,
            ProtocolDataLength: DATA_LEN as u32,
            FixedProtocolReturnData: 0,
            ProtocolDataRequestSubValue2: 0,
            ProtocolDataRequestSubValue3: 0,
            ProtocolDataRequestSubValue4: 0,
        },
    };

    // Input hanya header; data dikembalikan di area yang sama.
    // SAFETY: `input` adalah POD; ukurannya diberikan ke API.
    let input_bytes = unsafe {
        std::slice::from_raw_parts(
            &input as *const _ as *const u8,
            std::mem::size_of::<QueryWithProtocol>(),
        )
    };

    let out_len = std::mem::size_of::<QueryWithProtocol>() + DATA_LEN;
    let bytes = ioctl(
        *handle,
        IOCTL_STORAGE_QUERY_PROPERTY,
        Some(input_bytes),
        out_len,
    )?;

    // Data payload ada setelah header.
    let offset = std::mem::size_of::<QueryWithProtocol>();
    if bytes.len() < offset + DATA_LEN {
        return None;
    }
    Some(bytes[offset..offset + DATA_LEN].to_vec())
}

/// Composite Temperature: byte 1–2, Kelvin.
pub fn nvme_temperature_c(log: &[u8]) -> Option<f32> {
    if log.len() < 3 {
        return None;
    }
    let kelvin = u16::from_le_bytes([log[1], log[2]]);
    if kelvin == 0 {
        // 0 K berarti sensor tidak dilaporkan, bukan -273 °C.
        return None;
    }
    Some(kelvin as f32 - 273.15)
}

/// Percentage Used: byte 5, satuan persen (bisa > 100 pada disk tua).
pub fn nvme_percentage_used(log: &[u8]) -> Option<u8> {
    log.get(5).copied()
}

// ---------------------------------------------------------------------------
// ATA
// ---------------------------------------------------------------------------

/// Baca 512 byte atribut SMART lewat `IOCTL_ATA_PASS_THROUGH` (SMART READ DATA
/// 0xD0), fallback ke SAT `IOCTL_SCSI_PASS_THROUGH_DIRECT`.
fn ata_smart_data(handle: &windows::Win32::Foundation::HANDLE) -> Option<Vec<u8>> {
    // SMART signature: cylinder low = 0x4F, cylinder high = 0xC2.
    // Salah satu salah dan drive menjawab ABRT.
    const SMART_CMD: u8 = 0xB0;
    const SMART_READ_DATA: u8 = 0xD0;
    const SMART_CYL_LOW: u8 = 0x4F;
    const SMART_CYL_HIGH: u8 = 0xC2;
    const DEVICE_HEAD: u8 = 0xA0;
    const DATA_LEN: usize = 512;

    let mut task_file = [0u8; 8];
    task_file[0] = SMART_READ_DATA; // features
    task_file[1] = 0x01; // sector count
    task_file[2] = 0x00; // sector number (LBA low)
    task_file[3] = SMART_CYL_LOW;
    task_file[4] = SMART_CYL_HIGH;
    task_file[5] = DEVICE_HEAD;
    task_file[6] = SMART_CMD;

    try_ata_pass_through(*handle, &task_file, DATA_LEN)
        .or_else(|| try_scsi_pass_through(*handle, &task_file, DATA_LEN))
}

fn try_ata_pass_through(
    handle: windows::Win32::Foundation::HANDLE,
    task_file: &[u8; 8],
    data_len: usize,
) -> Option<Vec<u8>> {
    use windows::Win32::Storage::IscsiDisc::{ATA_PASS_THROUGH_EX, IOCTL_ATA_PASS_THROUGH};
    use windows::Win32::System::IO::DeviceIoControl;

    const ATA_FLAGS_DATA_IN: u16 = 2;

    let header_size = std::mem::size_of::<ATA_PASS_THROUGH_EX>();
    let total = header_size + data_len;

    // Buffer dialokasikan sebagai u64 supaya selaras untuk struct berisi pointer.
    let mut buf = vec![0u64; total.div_ceil(8)];

    // SAFETY: buffer cukup besar untuk header + data.
    unsafe {
        let ptr = buf.as_mut_ptr() as *mut u8;
        let cmd = ATA_PASS_THROUGH_EX {
            Length: header_size as u16,
            AtaFlags: ATA_FLAGS_DATA_IN,
            PathId: 0,
            TargetId: 0,
            Lun: 0,
            ReservedAsUchar: 0,
            DataTransferLength: data_len as u32,
            TimeOutValue: 10,
            ReservedAsUlong: 0,
            DataBufferOffset: header_size,
            PreviousTaskFile: [0u8; 8],
            CurrentTaskFile: *task_file,
        };
        std::ptr::write_unaligned(ptr as *mut ATA_PASS_THROUGH_EX, cmd);
    }

    let mut returned = 0u32;
    // SAFETY: header di awal buffer, ukuran total diberikan ke API.
    let ok = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_ATA_PASS_THROUGH,
            Some(buf.as_ptr() as *const c_void),
            total as u32,
            Some(buf.as_mut_ptr() as *mut c_void),
            total as u32,
            Some(&mut returned),
            None,
        )
    }
    .is_ok();
    if !ok {
        return None;
    }

    // SAFETY: kernel menulis data pada offset header selama >= data_len byte.
    unsafe {
        let base = buf.as_ptr() as *const u8;
        Some(std::slice::from_raw_parts(base.add(header_size), data_len).to_vec())
    }
}

fn try_scsi_pass_through(
    handle: windows::Win32::Foundation::HANDLE,
    task_file: &[u8; 8],
    data_len: usize,
) -> Option<Vec<u8>> {
    use windows::Win32::Storage::IscsiDisc::{
        IOCTL_SCSI_PASS_THROUGH_DIRECT, SCSI_IOCTL_DATA_IN, SCSI_PASS_THROUGH_DIRECT,
    };
    use windows::Win32::System::IO::DeviceIoControl;

    // ATA PASS-THROUGH (16): opcode 0x85, protocol 3 (PIO data-in).
    const ATA16_OPCODE: u8 = 0x85;
    const ATA_PROTOCOL_PIO_DATA_IN: u8 = 3 << 1;

    let mut cdb = [0u8; 16];
    cdb[0] = ATA16_OPCODE;
    cdb[1] = ATA_PROTOCOL_PIO_DATA_IN;
    cdb[2] = 0x2E; // extend + T_DIR (data-in) + T_LENGTH (sector count)
    cdb[3] = task_file[0]; // features
    cdb[4] = task_file[1]; // sector count
    cdb[5] = task_file[2]; // LBA low
    cdb[6] = task_file[3]; // LBA mid (cyl low)
    cdb[7] = task_file[4]; // LBA high (cyl high)
    cdb[8] = task_file[5]; // device
    cdb[9] = task_file[6]; // command
    cdb[13] = 0x01; // sector count (extend)
    cdb[14] = task_file[1];

    let mut data = vec![0u64; data_len.div_ceil(8)];
    let mut cmd = SCSI_PASS_THROUGH_DIRECT {
        Length: std::mem::size_of::<SCSI_PASS_THROUGH_DIRECT>() as u16,
        ScsiStatus: 0,
        PathId: 0,
        TargetId: 0,
        Lun: 0,
        CdbLength: 16,
        SenseInfoLength: 0,
        DataIn: SCSI_IOCTL_DATA_IN as u8,
        DataTransferLength: data_len as u32,
        TimeOutValue: 10,
        DataBuffer: data.as_mut_ptr() as *mut c_void,
        SenseInfoOffset: 0,
        Cdb: cdb,
    };

    let mut returned = 0u32;
    // SAFETY: struct valid, buffer data cukup besar dan cukup selaras.
    let ok = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_SCSI_PASS_THROUGH_DIRECT,
            Some(&cmd as *const _ as *const c_void),
            std::mem::size_of::<SCSI_PASS_THROUGH_DIRECT>() as u32,
            Some(&mut cmd as *mut _ as *mut c_void),
            std::mem::size_of::<SCSI_PASS_THROUGH_DIRECT>() as u32,
            Some(&mut returned),
            None,
        )
    }
    .is_ok();
    // Status SCSI 0 = GOOD. Status lain berarti drive menolak SMART.
    if !ok || cmd.ScsiStatus != 0 {
        return None;
    }

    // SAFETY: buffer data berisi `data_len` byte hasil command.
    unsafe { Some(std::slice::from_raw_parts(data.as_ptr() as *const u8, data_len).to_vec()) }
}

/// Suhu dari 512 byte atribut SMART ATA.
///
/// Prioritas:
/// 1. Atribut 0xC2 (Temperature) — paling standar sejak 2005.
/// 2. Atribut 0xBE (Airflow Temperature) — dipakai disk lama.
///
/// Catatan penting soal 0xBE: sebagian disk sangat lama menyimpan
/// **(100 − suhu)** di byte mentahnya, bukan suhu langsung. Tidak ada cara
/// memastikan dari payload saja, jadi nilai dibaca **apa adanya** dan
/// `confidence` tetap `Direct`. Kalau pengguna melihat angka yang tampak
/// terbalik (mis. 60 °C pada disk yang jelas dingin), itu penyebabnya — lebih
/// baik begitu daripada menebak dan menampilkan angka salah (ADR-0008).
pub fn ata_temperature_c(data: &[u8]) -> Option<f32> {
    // 512 byte pertama = 30 atribut × 12 byte, mulai offset 2.
    if data.len() < 362 {
        return None;
    }
    const VENDOR_OFFSET: usize = 2;

    let mut airflow: Option<f32> = None;
    for i in 0..30usize {
        let offset = VENDOR_OFFSET + i * 12;
        if offset + 12 > data.len() {
            break;
        }
        let id = data[offset];
        if id == 0 {
            continue;
        }
        let raw = data[offset + 5];
        // Raw 0 berarti atribut tidak dilaporkan, bukan 0 °C.
        if raw == 0 {
            continue;
        }
        if id == 0xC2 {
            return Some(raw as f32);
        }
        if id == 0xBE && airflow.is_none() {
            airflow = Some(raw as f32);
        }
    }
    airflow
}

/// Model disk dari `STORAGE_DEVICE_DESCRIPTOR`.
///
/// Dibaca dengan akses 0 (cukup untuk query properti) supaya label sensor tetap
/// bisa dibaca manusia bahkan saat SMART sendiri butuh admin.
pub fn device_model(handle: &windows::Win32::Foundation::HANDLE, index: u32) -> String {
    let _ = handle;
    match model_from_device(index) {
        Some(m) if !m.trim().is_empty() => m,
        _ => format!("Disk {index}"),
    }
}

fn model_from_device(index: u32) -> Option<String> {
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::IO::DeviceIoControl;
    use windows::Win32::System::Ioctl::{
        IOCTL_STORAGE_QUERY_PROPERTY, STORAGE_DEVICE_DESCRIPTOR, STORAGE_PROPERTY_QUERY,
        StorageDeviceProperty,
    };
    use windows::core::PCWSTR;

    let path = format!(r"\\.\PhysicalDrive{index}");
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();

    // SAFETY: path NUL-terminated; akses 0 cukup untuk query properti.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }
    .ok()?;

    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceProperty,
        QueryType: windows::Win32::System::Ioctl::PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    let mut out = vec![0u8; 1024];
    let mut returned = 0u32;
    // SAFETY: query valid, buffer 1024 byte.
    let ok = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some(&query as *const _ as *const _),
            std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            Some(out.as_mut_ptr() as *mut _),
            out.len() as u32,
            Some(&mut returned),
            None,
        )
    }
    .is_ok();
    // SAFETY: handle dibuka di atas dan tidak dipakai lagi.
    unsafe {
        let _ = windows::Win32::Foundation::CloseHandle(handle);
    }
    if !ok || (returned as usize) < std::mem::size_of::<STORAGE_DEVICE_DESCRIPTOR>() {
        return None;
    }

    // SAFETY: buffer berisi struct yang ditulis kernel.
    let desc =
        unsafe { std::ptr::read_unaligned(out.as_ptr() as *const STORAGE_DEVICE_DESCRIPTOR) };
    let read_at = |offset: u32| -> String {
        let start = offset as usize;
        if start == 0 || start >= out.len() {
            return String::new();
        }
        let end = out[start..]
            .iter()
            .position(|&b| b == 0)
            .map(|p| start + p)
            .unwrap_or(out.len());
        String::from_utf8_lossy(&out[start..end]).trim().to_string()
    };
    let vendor = read_at(desc.VendorIdOffset);
    let product = read_at(desc.ProductIdOffset);
    Some(format!("{vendor} {product}").trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nvme_payload(temp_kelvin: u16, used: u8) -> Vec<u8> {
        let mut log = vec![0u8; 512];
        log[0] = 0; // critical warning
        log[1..3].copy_from_slice(&temp_kelvin.to_le_bytes());
        log[3] = 100; // available spare
        log[4] = 0;
        log[5] = used;
        log
    }

    #[test]
    fn nvme_temperature_is_kelvin() {
        // 318 K = 44.85 °C
        let log = nvme_payload(318, 3);
        let t = nvme_temperature_c(&log).unwrap();
        assert!((t - 44.85).abs() < 0.01, "dapat {t}");
    }

    #[test]
    fn nvme_zero_kelvin_means_unknown() {
        let log = nvme_payload(0, 0);
        assert_eq!(nvme_temperature_c(&log), None);
    }

    #[test]
    fn nvme_short_payload_is_rejected() {
        assert_eq!(nvme_temperature_c(&[0u8; 2]), None);
        assert_eq!(nvme_temperature_c(&[]), None);
    }

    #[test]
    fn nvme_percentage_used_is_byte_five() {
        let log = nvme_payload(300, 42);
        assert_eq!(nvme_percentage_used(&log), Some(42));
    }

    fn ata_attributes(entries: &[(u8, u8)]) -> Vec<u8> {
        let mut data = vec![0u8; 512];
        for (i, (id, raw)) in entries.iter().enumerate() {
            let offset = 2 + i * 12;
            data[offset] = *id;
            data[offset + 5] = *raw;
        }
        data
    }

    #[test]
    fn ata_temperature_attribute_c2() {
        let data = ata_attributes(&[(0x09, 90), (0xC2, 38)]);
        assert_eq!(ata_temperature_c(&data), Some(38.0));
    }

    #[test]
    fn ata_temperature_prefers_c2_over_be() {
        let data = ata_attributes(&[(0xBE, 55), (0xC2, 41)]);
        assert_eq!(ata_temperature_c(&data), Some(41.0));
    }

    #[test]
    fn ata_airflow_attribute_be() {
        let data = ata_attributes(&[(0xBE, 42)]);
        assert_eq!(ata_temperature_c(&data), Some(42.0));
    }

    #[test]
    fn ata_prefers_c2_even_when_be_comes_first() {
        // 0xBE muncul lebih dulu di payload; 0xC2 tetap yang dipakai.
        let data = ata_attributes(&[(0xBE, 55), (0xC2, 40)]);
        assert_eq!(ata_temperature_c(&data), Some(40.0));
    }

    #[test]
    fn ata_without_temperature_attribute_is_none() {
        let data = ata_attributes(&[(0x09, 90), (0x0C, 12)]);
        assert_eq!(ata_temperature_c(&data), None);
    }

    #[test]
    fn ata_short_payload_is_rejected() {
        assert_eq!(ata_temperature_c(&[0u8; 10]), None);
    }

    #[test]
    fn ata_zero_raw_is_unknown_not_zero_degrees() {
        let data = ata_attributes(&[(0xC2, 0)]);
        assert_eq!(
            ata_temperature_c(&data),
            None,
            "raw 0 berarti sensor tidak ada, bukan 0 °C"
        );
    }

    #[test]
    fn provider_reports_elevation_on_this_machine() {
        let p = SmartProvider::new();
        let a = p.probe();
        // Mesin uji tidak elevated: harus NeedsElevation, bukan Failed/Available.
        // Mesin lain bisa Available; keduanya valid.
        assert!(
            matches!(a, Availability::NeedsElevation | Availability::Available),
            "status tidak terduga: {a:?}"
        );
    }

    #[test]
    fn model_is_readable_without_elevation() {
        // Akses 0 cukup untuk query properti, jadi model harus terbaca bahkan
        // saat SMART butuh admin. Ini yang membuat label sensor berguna.
        let model = model_from_device(0);
        assert!(model.is_some(), "model disk 0 harus terbaca tanpa admin");
        assert!(!model.unwrap().trim().is_empty());
    }
}
