//! Wrapper tipis untuk Performance Data Helper (PDH).
//!
//! Semua counter didaftarkan sekali saat startup, lalu dibaca setiap sampel.
//! `PdhCollectQueryData` dipanggil sekali per siklus untuk seluruh query —
//! mendaftarkan counter per sampel akan 10–100× lebih mahal.
//!
//! Nama counter selalu Inggris (`PdhAddEnglishCounterW`), supaya tidak rusak di
//! Windows berbahasa Indonesia atau bahasa lain.

use std::ffi::c_void;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Performance::{
    PDH_FMT_COUNTERVALUE, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY,
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhGetFormattedCounterValue, PdhOpenQueryW,
};
use windows::core::PCWSTR;

const PDH_MORE_DATA: u32 = 0x8000_07D2;

/// Satu query PDH yang menampung banyak counter.
pub struct Query {
    handle: PDH_HQUERY,
}

impl Query {
    pub fn new() -> Option<Self> {
        let mut handle = PDH_HQUERY(std::ptr::null_mut());
        // SAFETY: tidak ada data source (nil), handle out-param valid.
        let rc = unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut handle) };
        if rc == ERROR_SUCCESS.0 {
            Some(Self { handle })
        } else {
            tracing::warn!(rc, "PdhOpenQueryW gagal");
            None
        }
    }

    /// Daftarkan counter. `path` memakai nama Inggris, mis.
    /// `\PhysicalDisk(*)\Disk Read Bytes/sec`.
    pub fn add(&mut self, path: &str) -> Option<Counter> {
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut handle = PDH_HCOUNTER(std::ptr::null_mut());
        // SAFETY: path NUL-terminated dan hidup selama panggilan.
        let rc =
            unsafe { PdhAddEnglishCounterW(self.handle, PCWSTR(wide.as_ptr()), 0, &mut handle) };
        if rc == ERROR_SUCCESS.0 {
            Some(Counter { handle })
        } else {
            tracing::debug!(rc, path, "counter PDH tidak tersedia");
            None
        }
    }

    /// Ambil sampel baru untuk semua counter dalam query ini.
    pub fn collect(&mut self) -> bool {
        // SAFETY: handle dimiliki oleh self dan valid sampai Drop.
        unsafe { PdhCollectQueryData(self.handle) == ERROR_SUCCESS.0 }
    }
}

impl Drop for Query {
    fn drop(&mut self) {
        // SAFETY: handle dibuka di `new` dan hanya ditutup sekali di sini.
        unsafe {
            let _ = PdhCloseQuery(self.handle);
        }
    }
}

/// Handle counter. Valid selama `Query` pemiliknya hidup; tidak punya `Drop`.
#[derive(Clone, Copy)]
pub struct Counter {
    handle: PDH_HCOUNTER,
}

impl Counter {
    /// Nilai tunggal. `None` kalau counter belum punya data (sampel pertama)
    /// atau gagal diformat.
    pub fn value(&self) -> Option<f64> {
        let mut value = PDH_FMT_COUNTERVALUE::default();
        // SAFETY: handle valid, `value` out-param valid.
        let rc =
            unsafe { PdhGetFormattedCounterValue(self.handle, PDH_FMT_DOUBLE, None, &mut value) };
        if rc != ERROR_SUCCESS.0 || value.CStatus != ERROR_SUCCESS.0 {
            return None;
        }
        // SAFETY: dengan PDH_FMT_DOUBLE, union yang aktif adalah doubleValue.
        Some(unsafe { value.Anonymous.doubleValue })
    }

    /// Nilai per instance, mis. per disk atau per proses.
    pub fn array(&self) -> Vec<(String, f64)> {
        let mut size = 0u32;
        let mut count = 0u32;
        // Panggilan pertama hanya menanyakan ukuran buffer.
        // SAFETY: buffer None berarti "hitung saja"; out-param valid.
        let rc = unsafe {
            PdhGetFormattedCounterArrayW(self.handle, PDH_FMT_DOUBLE, &mut size, &mut count, None)
        };
        if rc != PDH_MORE_DATA || size == 0 || count == 0 {
            return Vec::new();
        }

        // Buffer dialokasikan sebagai u64 supaya selaras untuk pointer di dalam
        // PDH_FMT_COUNTERVALUE_ITEM_W (Vec<u8> hanya terjamin selaras 1).
        let words = (size as usize).div_ceil(8);
        let mut buf = vec![0u64; words];
        // SAFETY: buffer >= size byte dan cukup selaras; item count out-param valid.
        let rc = unsafe {
            PdhGetFormattedCounterArrayW(
                self.handle,
                PDH_FMT_DOUBLE,
                &mut size,
                &mut count,
                Some(buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W),
            )
        };
        if rc != ERROR_SUCCESS.0 {
            return Vec::new();
        }

        let count = (count as usize)
            .min(buf.len() * 8 / std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>());
        // SAFETY: PDH menulis tepat `count` item ke buffer; tipe dan jumlahnya
        // sesuai dengan yang dilaporkan API pada panggilan pertama.
        let items = unsafe {
            std::slice::from_raw_parts(buf.as_ptr() as *const PDH_FMT_COUNTERVALUE_ITEM_W, count)
        };

        items
            .iter()
            .filter_map(|item| {
                if item.FmtValue.CStatus != ERROR_SUCCESS.0 || item.szName.is_null() {
                    return None;
                }
                // SAFETY: szName menunjuk ke string UTF-16 NUL-terminated di
                // dalam buffer yang masih hidup selama iterasi ini.
                let name = unsafe { item.szName.to_string() }.ok()?;
                // SAFETY: dengan PDH_FMT_DOUBLE, union aktif adalah doubleValue.
                let value = unsafe { item.FmtValue.Anonymous.doubleValue };
                Some((name, value))
            })
            .collect()
    }
}

/// Nama instance PDH untuk `PhysicalDisk` berbentuk `"0 C:"`, `"1 D:"`,
/// `"_Total"`. Mengembalikan indeks disk.
pub fn parse_physical_disk_index(instance: &str) -> Option<u32> {
    let head = instance.split_whitespace().next()?;
    head.parse().ok()
}

/// Instance `GPU Engine` berbentuk
/// `pid_1234_luid_0x00000000_0x0000D0C1_phys_0_eng_0_engtype_3D`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuEngineInstance {
    pub pid: u32,
    pub engine: String,
}

pub fn parse_gpu_engine_instance(instance: &str) -> Option<GpuEngineInstance> {
    // Bentuk: `pid_1234_luid_..._engtype_3D`. Setelah dipecah pada `_`, `pid`
    // dan `engtype` masing-masing diikuti satu token berisi nilainya.
    let mut tokens = instance.split('_');
    let mut pid = None;
    let mut engine = None;
    while let Some(token) = tokens.next() {
        match token {
            "pid" => pid = tokens.next().and_then(|t| t.parse::<u32>().ok()),
            "engtype" => engine = tokens.next().map(|t| t.to_string()),
            _ => {}
        }
    }
    Some(GpuEngineInstance {
        pid: pid?,
        engine: engine?,
    })
}

/// Instance `GPU Adapter Memory` berbentuk
/// `luid_0x00000000_0x0000D0C1_phys_0_dedicated`.
pub fn parse_gpu_adapter_luid(instance: &str) -> Option<(u32, u32)> {
    let mut high = None;
    let mut low = None;
    for part in instance.split('_') {
        if let Some(rest) = part.strip_prefix("0x") {
            if let Ok(v) = u32::from_str_radix(rest, 16) {
                if high.is_none() {
                    high = Some(v);
                } else if low.is_none() {
                    low = Some(v);
                }
            }
        }
    }
    Some((high?, low?))
}

// Menjaga `c_void` tetap dipakai kalau kelak ada API mentah yang butuh.
const _: Option<*mut c_void> = None;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_physical_disk_instances() {
        assert_eq!(parse_physical_disk_index("0 C:"), Some(0));
        assert_eq!(parse_physical_disk_index("1 D:"), Some(1));
        assert_eq!(parse_physical_disk_index("12"), Some(12));
        assert_eq!(parse_physical_disk_index("_Total"), None);
    }

    #[test]
    fn parses_gpu_engine_instance() {
        let i = parse_gpu_engine_instance(
            "pid_4820_luid_0x00000000_0x0000D0C1_phys_0_eng_0_engtype_3D",
        )
        .unwrap();
        assert_eq!(i.pid, 4820);
        assert_eq!(i.engine, "3D");
    }

    #[test]
    fn parses_multiword_engine_type() {
        let i = parse_gpu_engine_instance("pid_99_luid_0x0_0x1_phys_0_eng_0_engtype_VideoDecode")
            .unwrap();
        assert_eq!(i.pid, 99);
        assert_eq!(i.engine, "VideoDecode");
    }

    #[test]
    fn rejects_gpu_instance_without_pid() {
        assert!(parse_gpu_engine_instance("luid_0x0_0x1_phys_0_eng_0_engtype_3D").is_none());
    }

    #[test]
    fn parses_adapter_luid() {
        assert_eq!(
            parse_gpu_adapter_luid("luid_0x00000000_0x0000D0C1_phys_0_dedicated"),
            Some((0, 0xD0C1))
        );
    }
}
