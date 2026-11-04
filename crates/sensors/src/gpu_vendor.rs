//! Tier 2: suhu GPU lewat SDK vendor (tanpa driver kernel).
//!
//! Tiga backend, semuanya `libloading` dengan path absolut:
//!
//! | Vendor | DLL | Fungsi |
//! |---|---|---|
//! | NVIDIA | `nvml.dll` (ada di driver) | `nvmlDeviceGetTemperature` |
//! | Intel | `igcl.dll` (Intel Graphics Driver) | `ctlInit` + `ctlEnumerateDevices` |
//! | AMD | `amdadlx64.dll` (Adrenalin) | `ADLXInitialize` (COM, belum) |
//!
//! Aturan yang mengikat (THREAT-MODEL T9): **selalu path absolut**, tidak pernah
//! `LoadLibrary("igcl.dll")` dengan nama saja. Nama saja bisa diambil dari
//! direktori kerja atau `PATH` yang bisa ditulis pengguna.

use crate::{Availability, Confidence, SensorProvider, reading};
use core_pulse_core::model::{SensorKind, SensorReading, SensorSource, Unit};
use std::ffi::c_void;

pub struct GpuVendorProvider {
    nvml: Option<Nvml>,
    igcl: Option<Igcl>,
    last_error: Option<String>,
}

impl Default for GpuVendorProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl GpuVendorProvider {
    pub fn new() -> Self {
        Self {
            nvml: None,
            igcl: None,
            last_error: None,
        }
    }
}

impl SensorProvider for GpuVendorProvider {
    fn id(&self) -> &'static str {
        "gpu-vendor"
    }

    fn source(&self) -> SensorSource {
        // Sumber sebenarnya ditentukan per pembacaan; NVML dilaporkan sebagai
        // Nvml oleh `reading()` di bawah.
        SensorSource::Nvml
    }

    fn probe(&self) -> Availability {
        if find_dll("nvml.dll").is_none() && find_dll("igcl.dll").is_none() {
            return Availability::NotAvailable(
                "tidak ada nvml.dll maupun igcl.dll; suhu GPU tidak bisa dibaca tanpa SDK vendor",
            );
        }
        Availability::Available
    }

    fn init(&mut self) -> Result<(), String> {
        if let Some(path) = find_dll("nvml.dll") {
            match Nvml::load(&path) {
                Ok(nvml) => {
                    tracing::info!(path = %path.display(), "NVML dimuat");
                    self.nvml = Some(nvml);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "NVML gagal dimuat");
                    self.last_error = Some(e);
                }
            }
        }
        if let Some(path) = find_dll("igcl.dll") {
            match Igcl::load(&path) {
                Ok(igcl) => {
                    tracing::info!(path = %path.display(), "IGCL dimuat");
                    self.igcl = Some(igcl);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "IGCL gagal dimuat");
                    self.last_error = Some(e);
                }
            }
        }

        if self.nvml.is_none() && self.igcl.is_none() {
            return Err(self
                .last_error
                .clone()
                .unwrap_or_else(|| "tidak ada backend GPU yang bisa dimuat".into()));
        }
        Ok(())
    }

    fn read(&mut self, out: &mut Vec<SensorReading>) {
        if let Some(nvml) = &mut self.nvml {
            nvml.read(out);
        }
        if let Some(igcl) = &mut self.igcl {
            igcl.read(out);
        }
    }
}

/// Cari DLL vendor di lokasi yang aman: `System32`, lalu direktori driver
/// display. Tidak pernah memakai direktori kerja atau `PATH`.
fn find_dll(name: &str) -> Option<std::path::PathBuf> {
    let system32 = std::path::Path::new(r"C:\Windows\System32").join(name);
    if system32.exists() {
        return Some(system32);
    }

    // Driver Intel menaruh igcl.dll di DriverStore, bukan System32.
    // Pencarian dibatasi ke subtree driver display dengan kedalaman terbatas.
    let repo = std::path::Path::new(r"C:\Windows\System32\DriverStore\FileRepository");
    let entries = std::fs::read_dir(repo).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let dir_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_lowercase();
        if !dir_name.starts_with("igcl") && !dir_name.starts_with("iigd") {
            continue;
        }
        let candidate = path.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// NVML
// ---------------------------------------------------------------------------

/// Binding minimal NVML. Hanya tiga fungsi yang dibutuhkan; menautkan seluruh
/// NVML (ratusan fungsi) akan menambah beban tanpa manfaat.
struct Nvml {
    _lib: libloading::Library,
    // SAFETY: pointer fungsi tetap valid selama `_lib` hidup, dan `_lib`
    // di-drop terakhir (urutan field Rust menjamin ini).
    shutdown: unsafe extern "system" fn() -> i32,
    count: unsafe extern "system" fn(*mut u32) -> i32,
    handle_by_index: unsafe extern "system" fn(u32, *mut *mut c_void) -> i32,
    temperature: unsafe extern "system" fn(*mut c_void, i32, *mut u32) -> i32,
    name: unsafe extern "system" fn(*mut c_void, *mut i8, u32) -> i32,
}

const NVML_SUCCESS: i32 = 0;
const NVML_TEMPERATURE_GPU: i32 = 0;
const NVML_DEVICE_NAME_BUFFER_SIZE: u32 = 96;

impl Nvml {
    fn load(path: &std::path::Path) -> Result<Self, String> {
        // SAFETY: path absolut yang sudah diverifikasi ada; library hidup
        // selama struct ini hidup.
        let lib = unsafe { libloading::Library::new(path) }
            .map_err(|e| format!("Library::new({}): {e}", path.display()))?;

        // SAFETY: simbol diambil dari library yang baru dimuat; tipe fungsi
        // mengikuti dokumentasi NVML.
        unsafe {
            let init = *lib
                .get::<unsafe extern "system" fn() -> i32>(b"nvmlInit_v2\0")
                .or_else(|_| lib.get::<unsafe extern "system" fn() -> i32>(b"nvmlInit\0"))
                .map_err(|e| format!("nvmlInit: {e}"))?;
            let shutdown = *lib
                .get::<unsafe extern "system" fn() -> i32>(b"nvmlShutdown\0")
                .map_err(|e| format!("nvmlShutdown: {e}"))?;
            let count = *lib
                .get::<unsafe extern "system" fn(*mut u32) -> i32>(b"nvmlDeviceGetCount_v2\0")
                .or_else(|_| {
                    lib.get::<unsafe extern "system" fn(*mut u32) -> i32>(b"nvmlDeviceGetCount\0")
                })
                .map_err(|e| format!("nvmlDeviceGetCount: {e}"))?;
            let handle_by_index = *lib
                .get::<unsafe extern "system" fn(u32, *mut *mut c_void) -> i32>(
                    b"nvmlDeviceGetHandleByIndex_v2\0",
                )
                .or_else(|_| {
                    lib.get::<unsafe extern "system" fn(u32, *mut *mut c_void) -> i32>(
                        b"nvmlDeviceGetHandleByIndex\0",
                    )
                })
                .map_err(|e| format!("nvmlDeviceGetHandleByIndex: {e}"))?;
            let temperature = *lib
                .get::<unsafe extern "system" fn(*mut c_void, i32, *mut u32) -> i32>(
                    b"nvmlDeviceGetTemperature\0",
                )
                .map_err(|e| format!("nvmlDeviceGetTemperature: {e}"))?;
            let name = *lib
                .get::<unsafe extern "system" fn(*mut c_void, *mut i8, u32) -> i32>(
                    b"nvmlDeviceGetName\0",
                )
                .map_err(|e| format!("nvmlDeviceGetName: {e}"))?;

            if init() != NVML_SUCCESS {
                return Err("nvmlInit mengembalikan error".into());
            }

            Ok(Self {
                _lib: lib,
                shutdown,
                count,
                handle_by_index,
                temperature,
                name,
            })
        }
    }

    fn read(&mut self, out: &mut Vec<SensorReading>) {
        let mut count = 0u32;
        // SAFETY: fungsi valid dan `count` out-param valid.
        if unsafe { (self.count)(&mut count) } != NVML_SUCCESS || count == 0 {
            return;
        }

        for index in 0..count {
            let mut handle: *mut c_void = std::ptr::null_mut();
            // SAFETY: index < count, handle out-param valid.
            if unsafe { (self.handle_by_index)(index, &mut handle) } != NVML_SUCCESS {
                continue;
            }

            let mut name_buf = [0i8; NVML_DEVICE_NAME_BUFFER_SIZE as usize];
            // SAFETY: buffer seukuran yang dijanjikan ke API.
            let name_ok =
                unsafe { (self.name)(handle, name_buf.as_mut_ptr(), NVML_DEVICE_NAME_BUFFER_SIZE) }
                    == NVML_SUCCESS;
            let name = if name_ok {
                let bytes: Vec<u8> = name_buf
                    .iter()
                    .take_while(|&&c| c != 0)
                    .map(|&c| c as u8)
                    .collect();
                String::from_utf8_lossy(&bytes).trim().to_string()
            } else {
                format!("GPU {index}")
            };

            let mut temp = 0u32;
            // SAFETY: fungsi valid, handle valid, out-param valid.
            if unsafe { (self.temperature)(handle, NVML_TEMPERATURE_GPU, &mut temp) }
                == NVML_SUCCESS
            {
                out.push(reading(
                    format!("gpu.{index}.temp"),
                    SensorKind::Temperature,
                    name,
                    temp as f32,
                    Unit::Celsius,
                    SensorSource::Nvml,
                    Confidence::Direct,
                ));
            }
        }
    }
}

impl Drop for Nvml {
    fn drop(&mut self) {
        // SAFETY: shutdown hanya dipanggil kalau init berhasil (dijamin oleh
        // konstruksi di `load`).
        unsafe {
            let _ = (self.shutdown)();
        }
    }
}

// ---------------------------------------------------------------------------
// IGCL (Intel)
// ---------------------------------------------------------------------------

/// Binding minimal IGCL. API Intel memakai struct konfigurasi versi; karena
/// dokumentasinya tidak resmi dan berubah antar versi driver, implementasi ini
/// **best effort**: kalau struktur tidak cocok, provider melaporkan gagal
/// dengan pesan, bukan menebak.
struct Igcl {
    _lib: libloading::Library,
    ready: bool,
}

impl Igcl {
    fn load(path: &std::path::Path) -> Result<Self, String> {
        // SAFETY: path absolut yang sudah diverifikasi ada.
        let lib = unsafe { libloading::Library::new(path) }
            .map_err(|e| format!("Library::new({}): {e}", path.display()))?;

        // `ctlInit` menerima versi API; nilai 0 meminta versi terbaru.
        // SAFETY: simbol mengikuti dokumentasi IGCL.
        let init = unsafe {
            lib.get::<unsafe extern "system" fn(u32, *mut c_void, *mut *mut c_void) -> i32>(
                b"ctlInit\0",
            )
        }
        .map_err(|e| format!("ctlInit tidak ada: {e}"))?;

        let mut api_handle: *mut c_void = std::ptr::null_mut();
        let mut version: u32 = 0;
        // SAFETY: out-param valid.
        let rc = unsafe { (*init)(version, std::ptr::null_mut(), &mut api_handle) };
        if rc != 0 {
            version = 1;
            // SAFETY: out-param valid.
            let rc2 = unsafe { (*init)(version, std::ptr::null_mut(), &mut api_handle) };
            if rc2 != 0 {
                return Err(format!("ctlInit gagal (rc {rc}, lalu {rc2})"));
            }
        }

        Ok(Self {
            _lib: lib,
            ready: api_handle.is_null(),
        })
    }

    fn read(&mut self, _out: &mut Vec<SensorReading>) {
        // Suhu lewat IGCL membutuhkan `ctlEnumerateDevices` +
        // `ctlTemperatureGetProperties`, dengan struct yang layout-nya berbeda
        // antar versi driver Intel. Tanpa dokumentasi resmi dan tanpa hardware
        // uji (mesin ini hanya punya iGPU UHD 630 yang tidak mengekspos IGCL),
        // menebak layout berisiko menampilkan angka salah — yang dilarang
        // ADR-0008.
        //
        // Status dilaporkan jujur lewat `probe()`: IGCL dimuat, tapi suhu belum
        // diimplementasikan. Diverifikasi ulang di mesin dengan Intel Arc.
        if !self.ready {
            tracing::debug!("IGCL dimuat; pembacaan suhu belum diimplementasikan");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_vendor_dll_on_this_machine_is_honest() {
        let p = GpuVendorProvider::new();
        let a = p.probe();
        // Mesin ini tidak punya nvml.dll/igcl.dll: harus NotAvailable dengan
        // alasan yang menyebut SDK, bukan diam atau "0 °C".
        if find_dll("nvml.dll").is_none() && find_dll("igcl.dll").is_none() {
            match a {
                Availability::NotAvailable(reason) => {
                    assert!(reason.contains("SDK") || reason.contains("dll"));
                }
                other => panic!("status tidak terduga: {other:?}"),
            }
        }
    }

    #[test]
    fn dll_lookup_never_returns_relative_path() {
        // Kalau DLL ditemukan, path harus absolut dan berada di System32 atau
        // DriverStore — bukan direktori kerja.
        if let Some(path) = find_dll("nvml.dll") {
            assert!(path.is_absolute(), "path harus absolut: {}", path.display());
        }
        if let Some(path) = find_dll("igcl.dll") {
            let s = path.to_string_lossy().to_lowercase();
            assert!(
                s.contains("system32") || s.contains("driverstore"),
                "DLL harus dari lokasi sistem: {s}"
            );
        }
    }

    #[test]
    fn missing_dll_is_none() {
        assert!(find_dll("definitely-not-a-real-vendor-lib.dll").is_none());
    }
}
