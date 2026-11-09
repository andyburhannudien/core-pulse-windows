//! Disk fisik dan volume.
//!
//! Identitas disk (model, bus, media) dibaca langsung dari device lewat
//! `IOCTL_STORAGE_QUERY_PROPERTY`, bukan WMI. Throughput dan antrean dari PDH
//! `PhysicalDisk`.
//!
//! Catatan akses (terverifikasi di mesin uji, non-admin):
//!
//! | IOCTL | Akses 0 | Butuh baca |
//! |---|---|---|
//! | `IOCTL_STORAGE_QUERY_PROPERTY` | **ya** | — |
//! | `IOCTL_DISK_GET_DRIVE_GEOMETRY_EX` | **ya** | — |
//! | `IOCTL_DISK_GET_LENGTH_INFO` | tidak | ya |
//! | `IOCTL_STORAGE_READ_CAPACITY` | tidak | ya |
//!
//! Ukuran disk dibaca lewat `IOCTL_DISK_GET_DRIVE_GEOMETRY_EX`, yang berjalan
//! tanpa hak baca dan juga bekerja untuk disk tanpa volume ter-mount. Kalau
//! IOCTL itu gagal, ukuran diturunkan dari jumlah kapasitas volume pada disk
//! tersebut (`IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` pada `\\.\C:` berjalan
//! tanpa hak baca) dan ditandai sebagai estimasi.

use crate::model::{BusType, Degradation, DegradeReason, DiskData, MediaType, VolumeData};
use crate::sys::pdh::{Counter, Query, parse_physical_disk_index};
use std::collections::HashMap;

pub struct DiskCollector {
    query: Option<Query>,
    read_bps: Option<Counter>,
    write_bps: Option<Counter>,
    idle_percent: Option<Counter>,
    queue: Option<Counter>,
    response_sec: Option<Counter>,
    identity: HashMap<u32, DiskIdentity>,
    identity_probed: bool,
}

#[derive(Debug, Clone, Default)]
struct DiskIdentity {
    model: String,
    bus: BusType,
    media: MediaType,
    size_bytes: u64,
    size_is_estimate: bool,
}

impl Default for DiskCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl DiskCollector {
    pub fn new() -> Self {
        let mut query = Query::new();
        let read_bps = query
            .as_mut()
            .and_then(|q| q.add(r"\PhysicalDisk(*)\Disk Read Bytes/sec"));
        let write_bps = query
            .as_mut()
            .and_then(|q| q.add(r"\PhysicalDisk(*)\Disk Write Bytes/sec"));
        let idle_percent = query
            .as_mut()
            .and_then(|q| q.add(r"\PhysicalDisk(*)\% Idle Time"));
        let queue = query
            .as_mut()
            .and_then(|q| q.add(r"\PhysicalDisk(*)\Avg. Disk Queue Length"));
        let response_sec = query
            .as_mut()
            .and_then(|q| q.add(r"\PhysicalDisk(*)\Avg. Disk sec/Transfer"));

        Self {
            query,
            read_bps,
            write_bps,
            idle_percent,
            queue,
            response_sec,
            identity: HashMap::new(),
            identity_probed: false,
        }
    }

    pub fn collect(&mut self) -> (Vec<DiskData>, Vec<VolumeData>, Vec<Degradation>) {
        let mut degraded = Vec::new();

        if self.query.is_none() {
            degraded.push(Degradation {
                area: crate::model::Area::Disk,
                reason: DegradeReason::ApiFailed,
                detail: "PdhOpenQueryW failed; disk throughput unavailable".into(),
            });
        }

        let collected = self.query.as_mut().map(|q| q.collect()).unwrap_or(false);

        let reads = rates(&self.read_bps);
        let writes = rates(&self.write_bps);
        let idle = rates(&self.idle_percent);
        let queues = rates(&self.queue);
        let responses = rates(&self.response_sec);

        let mut indices: Vec<u32> = reads
            .keys()
            .chain(writes.keys())
            .chain(idle.keys())
            .copied()
            .collect();
        indices.sort_unstable();
        indices.dedup();

        let volumes = collect_volumes();

        if indices.is_empty() && self.query.is_some() {
            degraded.push(Degradation {
                area: crate::model::Area::Disk,
                reason: if collected {
                    DegradeReason::NotAvailable
                } else {
                    DegradeReason::ApiFailed
                },
                detail: "PDH returned no PhysicalDisk instance".into(),
            });
        }

        // Identitas disk diprobe sekali: model/bus/media tidak berubah, dan
        // membuka device relatif mahal.
        if !self.identity_probed && !indices.is_empty() {
            self.identity_probed = true;
            // Ukuran disk diturunkan dari total volume per disk.
            let mut size_hint: HashMap<u32, u64> = HashMap::new();
            for v in &volumes {
                if let Some(idx) = v.disk_index {
                    *size_hint.entry(idx).or_insert(0) += v.total_bytes;
                }
            }
            for &index in &indices {
                let hint = size_hint.get(&index).copied().unwrap_or(0);
                self.identity.insert(index, probe_identity(index, hint));
            }
        }

        let disks = indices
            .into_iter()
            .map(|index| {
                let id = self.identity.get(&index).cloned().unwrap_or_default();
                let idle_pct = idle.get(&index).copied().unwrap_or(0.0);
                DiskData {
                    index,
                    model: id.model,
                    bus: id.bus,
                    media: id.media,
                    size_bytes: id.size_bytes,
                    size_is_estimate: id.size_is_estimate,
                    read_bps: reads.get(&index).copied().unwrap_or(0.0).max(0.0) as u64,
                    write_bps: writes.get(&index).copied().unwrap_or(0.0).max(0.0) as u64,
                    // Busy = 100 - idle. Lebih jujur daripada % Disk Time yang
                    // bisa melebihi 100% pada beberapa driver.
                    busy_percent: (100.0 - idle_pct).clamp(0.0, 100.0) as f32,
                    avg_response_ms: (responses.get(&index).copied().unwrap_or(0.0) * 1000.0)
                        .max(0.0) as f32,
                    queue_depth: queues.get(&index).copied().unwrap_or(0.0).max(0.0) as f32,
                }
            })
            .collect();

        if volumes.is_empty() {
            degraded.push(Degradation {
                area: crate::model::Area::Disk,
                reason: DegradeReason::AccessDenied,
                detail: "no readable volume".into(),
            });
        }

        (disks, volumes, degraded)
    }
}

fn rates(counter: &Option<Counter>) -> HashMap<u32, f64> {
    counter
        .as_ref()
        .map(|c| c.array())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(name, v)| Some((parse_physical_disk_index(&name)?, v)))
        .collect()
}

/// Enumerasi drive letter, statistik ruang, dan pemetaan ke disk fisik.
fn collect_volumes() -> Vec<VolumeData> {
    use windows::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDriveStringsW, GetVolumeInformationW,
    };
    use windows::core::PCWSTR;

    // Nilai dari winbase.h.
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    const DRIVE_REMOTE: u32 = 4;
    const DRIVE_RAMDISK: u32 = 6;

    // SAFETY: buffer None hanya menanyakan panjang yang dibutuhkan.
    let needed = unsafe { GetLogicalDriveStringsW(None) };
    if needed == 0 {
        return Vec::new();
    }
    let mut buf = vec![0u16; needed as usize + 1];
    // SAFETY: buffer cukup besar sesuai hasil panggilan pertama.
    let written = unsafe { GetLogicalDriveStringsW(Some(&mut buf)) };
    if written == 0 {
        return Vec::new();
    }

    let mut out = Vec::new();
    for root in buf[..written as usize].split(|&c| c == 0) {
        if root.is_empty() {
            continue;
        }
        let mount = String::from_utf16_lossy(root);
        let mount_w: Vec<u16> = mount.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: path NUL-terminated.
        let drive_type = unsafe { GetDriveTypeW(PCWSTR(mount_w.as_ptr())) };
        if !matches!(
            drive_type,
            DRIVE_FIXED | DRIVE_REMOVABLE | DRIVE_REMOTE | DRIVE_RAMDISK
        ) {
            continue;
        }

        let mut total = 0u64;
        let mut free = 0u64;
        // SAFETY: path valid; out-param valid. `free_for_caller` None karena
        // yang ditampilkan adalah ruang bebas volume, bukan kuota pengguna.
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                PCWSTR(mount_w.as_ptr()),
                None,
                Some(&mut total),
                Some(&mut free),
            )
        }
        .is_ok();
        if !ok {
            continue;
        }

        let mut label_buf = [0u16; 64];
        let mut fs_buf = [0u16; 32];
        // SAFETY: kedua buffer berukuran tetap sesuai tipe parameter.
        let info_ok = unsafe {
            GetVolumeInformationW(
                PCWSTR(mount_w.as_ptr()),
                Some(&mut label_buf),
                None,
                None,
                None,
                Some(&mut fs_buf),
            )
        }
        .is_ok();

        out.push(VolumeData {
            disk_index: volume_disk_index(&mount),
            mount,
            label: if info_ok {
                from_wide_fixed(&label_buf)
            } else {
                String::new()
            },
            fs: if info_ok {
                from_wide_fixed(&fs_buf)
            } else {
                String::new()
            },
            total_bytes: total,
            free_bytes: free,
        });
    }
    out
}

/// `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` pada `\\.\C:` — berjalan tanpa hak baca.
fn volume_disk_index(mount: &str) -> Option<u32> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::IO::DeviceIoControl;
    use windows::Win32::System::Ioctl::VOLUME_DISK_EXTENTS;
    use windows::core::PCWSTR;

    /// CTL_CODE(IOCTL_VOLUME_BASE=86, 0, METHOD_BUFFERED=0, FILE_ANY_ACCESS=0)
    /// = 0x00560000. Konstanta ini belum ada di crate `windows`.
    const IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: u32 = 0x0056_0000;

    let device = format!(r"\\.\{}", mount.trim_end_matches('\\'));
    let device_w: Vec<u16> = device.encode_utf16().chain(std::iter::once(0)).collect();

    // SAFETY: path NUL-terminated; akses 0 cukup untuk IOCTL ini.
    let handle: HANDLE = unsafe {
        CreateFileW(
            PCWSTR(device_w.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }
    .ok()?;

    // Ruang untuk hingga 4 extent; volume dinamis bisa punya lebih dari satu.
    let mut buf = vec![0u64; 64];
    let mut returned = 0u32;
    // SAFETY: buffer cukup besar dan selaras; ukuran byte diberikan.
    let ok = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
            None,
            0,
            Some(buf.as_mut_ptr() as *mut _),
            (buf.len() * 8) as u32,
            Some(&mut returned),
            None,
        )
    }
    .is_ok();
    // SAFETY: handle dibuka di atas dan tidak dipakai lagi.
    unsafe {
        let _ = CloseHandle(handle);
    }
    if !ok {
        return None;
    }

    // SAFETY: buffer berisi VOLUME_DISK_EXTENTS yang ditulis kernel.
    let extents = unsafe { &*(buf.as_ptr() as *const VOLUME_DISK_EXTENTS) };
    if extents.NumberOfDiskExtents == 0 {
        return None;
    }
    // SAFETY: Extents adalah array fleksibel; entri pertama ada karena
    // NumberOfDiskExtents > 0.
    Some(unsafe { extents.Extents.as_ptr().read().DiskNumber })
}

fn from_wide_fixed(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

#[repr(C)]
#[derive(Default)]
struct SeekPenaltyDescriptor {
    version: u32,
    size: u32,
    incurs_seek_penalty: u8,
    _padding: [u8; 3],
}

/// Baca model, bus, dan jenis media satu disk fisik.
///
/// `size_hint` dipakai kalau ukuran tidak bisa dibaca langsung (lihat catatan
/// akses di atas).
fn probe_identity(index: u32, size_hint: u64) -> DiskIdentity {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::IO::DeviceIoControl;
    use windows::Win32::System::Ioctl::{
        IOCTL_STORAGE_QUERY_PROPERTY, STORAGE_DEVICE_DESCRIPTOR, STORAGE_PROPERTY_QUERY,
        StorageDeviceProperty,
    };
    use windows::core::PCWSTR;

    let mut identity = DiskIdentity {
        size_bytes: size_hint,
        size_is_estimate: size_hint > 0,
        ..Default::default()
    };

    let path = format!(r"\\.\PhysicalDrive{index}");
    let path_w: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();

    // SAFETY: path NUL-terminated; akses 0 cukup untuk query properti.
    let handle: HANDLE = match unsafe {
        CreateFileW(
            PCWSTR(path_w.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    } {
        Ok(h) => h,
        Err(e) => {
            tracing::debug!(index, error = %e, "gagal membuka PhysicalDrive");
            return identity;
        }
    };

    let mut returned = 0u32;

    // --- Ukuran akurat (akses 0; bekerja juga tanpa volume ter-mount) ---
    if let Some(exact) = read_geometry_size(index) {
        identity.size_bytes = exact;
        identity.size_is_estimate = false;
    } else if let Some(exact) = read_exact_size(index) {
        identity.size_bytes = exact;
        identity.size_is_estimate = false;
    }

    // --- Model + bus type ---
    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceProperty,
        QueryType: windows::Win32::System::Ioctl::PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    let mut out = vec![0u8; 1024];
    // SAFETY: query valid, output buffer 1024 byte.
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

    if ok && returned as usize >= std::mem::size_of::<STORAGE_DEVICE_DESCRIPTOR>() {
        // SAFETY: buffer >= ukuran struct dan berisi struct yang ditulis kernel.
        let desc =
            unsafe { std::ptr::read_unaligned(out.as_ptr() as *const STORAGE_DEVICE_DESCRIPTOR) };
        identity.model = read_descriptor_string(&out, desc.VendorIdOffset, desc.ProductIdOffset);
        identity.bus = map_bus_type(desc.BusType.0);
    }

    // --- SSD vs HDD: seek penalty (property 7) ---
    let seek_query = STORAGE_PROPERTY_QUERY {
        PropertyId: windows::Win32::System::Ioctl::STORAGE_PROPERTY_ID(7),
        QueryType: windows::Win32::System::Ioctl::PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    let mut seek = SeekPenaltyDescriptor::default();
    // SAFETY: query dan output buffer sesuai tipe.
    let ok = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some(&seek_query as *const _ as *const _),
            std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            Some(&mut seek as *mut _ as *mut _),
            std::mem::size_of::<SeekPenaltyDescriptor>() as u32,
            Some(&mut returned),
            None,
        )
    }
    .is_ok();
    if ok {
        identity.media = if seek.incurs_seek_penalty != 0 {
            MediaType::Hdd
        } else {
            MediaType::Ssd
        };
    }

    // SAFETY: handle dibuka di atas dan tidak dipakai lagi.
    unsafe {
        let _ = CloseHandle(handle);
    }
    identity
}

/// Ukuran disk dari geometry. Berjalan dengan akses 0, jadi tidak butuh admin
/// dan tetap bekerja untuk disk yang tidak punya volume ter-mount.
fn read_geometry_size(index: u32) -> Option<u64> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::IO::DeviceIoControl;
    use windows::Win32::System::Ioctl::{DISK_GEOMETRY_EX, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX};
    use windows::core::PCWSTR;

    let path = format!(r"\\.\PhysicalDrive{index}");
    let path_w: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();

    // SAFETY: path NUL-terminated; akses 0 cukup untuk query geometry.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(path_w.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }
    .ok()?;

    let mut out = vec![0u8; std::mem::size_of::<DISK_GEOMETRY_EX>()];
    let mut returned = 0u32;
    // SAFETY: output buffer sesuai ukuran struct dan ditulis kernel.
    let ok = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_DISK_GET_DRIVE_GEOMETRY_EX,
            None,
            0,
            Some(out.as_mut_ptr() as *mut _),
            out.len() as u32,
            Some(&mut returned),
            None,
        )
    }
    .is_ok();
    // SAFETY: handle dibuka di atas dan tidak dipakai lagi.
    unsafe {
        let _ = CloseHandle(handle);
    }
    if !ok {
        return None;
    }

    // SAFETY: buffer berisi DISK_GEOMETRY_EX yang ditulis kernel.
    let geometry = unsafe { std::ptr::read_unaligned(out.as_ptr() as *const DISK_GEOMETRY_EX) };
    (geometry.DiskSize > 0).then_some(geometry.DiskSize as u64)
}

/// Ukuran persis disk lewat jalur yang membutuhkan hak baca (fallback).
fn read_exact_size(index: u32) -> Option<u64> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::IO::DeviceIoControl;
    use windows::Win32::System::Ioctl::{GET_LENGTH_INFORMATION, IOCTL_DISK_GET_LENGTH_INFO};
    use windows::core::PCWSTR;

    /// GENERIC_READ dari winnt.h (0x8000_0000).
    const GENERIC_READ: u32 = 0x8000_0000;

    let path = format!(r"\\.\PhysicalDrive{index}");
    let path_w: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();

    // SAFETY: path NUL-terminated.
    let handle: HANDLE = unsafe {
        CreateFileW(
            PCWSTR(path_w.as_ptr()),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }
    .ok()?;

    let mut length = GET_LENGTH_INFORMATION::default();
    let mut returned = 0u32;
    // SAFETY: output buffer sesuai tipe dan ukurannya diberikan.
    let ok = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_DISK_GET_LENGTH_INFO,
            None,
            0,
            Some(&mut length as *mut _ as *mut _),
            std::mem::size_of::<GET_LENGTH_INFORMATION>() as u32,
            Some(&mut returned),
            None,
        )
    }
    .is_ok();
    // SAFETY: handle dibuka di atas dan tidak dipakai lagi.
    unsafe {
        let _ = CloseHandle(handle);
    }
    if ok && length.Length > 0 {
        Some(length.Length as u64)
    } else {
        None
    }
}

fn read_descriptor_string(buf: &[u8], vendor_off: u32, product_off: u32) -> String {
    let read_at = |offset: u32| -> String {
        let start = offset as usize;
        if start == 0 || start >= buf.len() {
            return String::new();
        }
        let end = buf[start..]
            .iter()
            .position(|&b| b == 0)
            .map(|p| start + p)
            .unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[start..end]).trim().to_string()
    };
    let vendor = read_at(vendor_off);
    let product = read_at(product_off);
    match (vendor.is_empty(), product.is_empty()) {
        (true, true) => String::new(),
        (true, false) => product,
        (false, true) => vendor,
        (false, false) => format!("{vendor} {product}"),
    }
}

/// Nilai `STORAGE_BUS_TYPE` (ntddstor.h).
fn map_bus_type(raw: i32) -> BusType {
    match raw {
        17 => BusType::Nvme,
        11 => BusType::Sata,
        7 => BusType::Usb,
        0 => BusType::Unknown,
        _ => BusType::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bus_type_mapping_is_explicit() {
        assert_eq!(map_bus_type(17), BusType::Nvme);
        assert_eq!(map_bus_type(11), BusType::Sata);
        assert_eq!(map_bus_type(7), BusType::Usb);
        assert_eq!(map_bus_type(0), BusType::Unknown);
        assert_eq!(map_bus_type(3), BusType::Other);
    }

    #[test]
    fn descriptor_string_is_trimmed() {
        let mut buf = vec![0u8; 64];
        buf[8..16].copy_from_slice(b"ST1000\0\0");
        buf[16..24].copy_from_slice(b"LM049\0\0\0");
        assert_eq!(read_descriptor_string(&buf, 8, 16), "ST1000 LM049");
    }

    #[test]
    fn descriptor_string_handles_missing_product() {
        let mut buf = vec![0u8; 64];
        buf[8..13].copy_from_slice(b"ACME\0");
        assert_eq!(read_descriptor_string(&buf, 8, 20), "ACME");
    }

    #[test]
    fn descriptor_string_handles_out_of_range_offset() {
        let buf = vec![0u8; 16];
        assert_eq!(read_descriptor_string(&buf, 900, 901), "");
    }

    #[test]
    fn volumes_on_this_machine_are_readable() {
        let v = collect_volumes();
        assert!(!v.is_empty(), "minimal satu volume harus terbaca");
        assert!(v.iter().any(|x| x.total_bytes > 0));
        assert!(v.iter().all(|x| x.free_bytes <= x.total_bytes));
    }

    #[test]
    fn volumes_map_to_a_physical_disk() {
        let v = collect_volumes();
        assert!(
            v.iter().any(|x| x.disk_index.is_some()),
            "tidak ada volume yang terpetakan ke disk fisik"
        );
    }
}
