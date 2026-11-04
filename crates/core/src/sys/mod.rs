//! Akses langsung ke API Windows. Satu-satunya tempat `unsafe` di crate ini.

pub mod action;
pub mod cpu;
pub mod details;
pub mod disk;
pub mod gpu;
pub mod host;
pub mod mem;
pub mod net;
pub mod pdh;
pub mod proc;
pub mod services;
pub mod startup;

/// Query `NtQuerySystemInformation` untuk kelas informasi yang mengembalikan
/// buffer variabel. Buffer tumbuh sampai muat.
#[cfg(windows)]
pub(crate) mod nt {
    use std::ffi::c_void;
    use windows::Wdk::System::SystemInformation::{
        NtQuerySystemInformation, SYSTEM_INFORMATION_CLASS,
    };

    /// Error dari NTSTATUS.
    #[derive(Debug, Clone, Copy, thiserror::Error)]
    #[error("NtQuerySystemInformation gagal: 0x{0:08X}")]
    pub struct NtError(pub u32);

    pub const STATUS_INFO_LENGTH_MISMATCH: u32 = 0xC000_0004;

    /// Query kelas informasi berukuran tetap (mis. per-core performance).
    /// Buffer dialokasikan persis sebesar yang diminta API.
    pub fn query_fixed(
        class: SYSTEM_INFORMATION_CLASS,
        size_bytes: usize,
    ) -> Result<Vec<u8>, NtError> {
        let mut buf = vec![0u8; size_bytes.max(1)];
        let mut needed: u32 = 0;
        // SAFETY: buffer valid untuk `size_bytes` byte; `needed` out-param valid.
        let status = unsafe {
            NtQuerySystemInformation(
                class,
                buf.as_mut_ptr() as *mut c_void,
                size_bytes as u32,
                &mut needed,
            )
        };
        if status.is_ok() {
            Ok(buf)
        } else {
            Err(NtError(status.0 as u32))
        }
    }

    /// Query buffer variabel. Mengembalikan `Vec<u8>` yang sudah diisi.
    pub fn query_variable(class: SYSTEM_INFORMATION_CLASS) -> Result<Vec<u8>, NtError> {
        // Ukuran awal 256 KiB cukup untuk sebagian besar mesin; tumbuh bila perlu.
        let mut size: usize = 256 * 1024;
        for _ in 0..8 {
            let mut buf = vec![0u8; size];
            let mut needed: u32 = 0;
            // SAFETY: buf valid untuk `size` byte, dan `needed` adalah out-param
            // yang selalu ditulis oleh API.
            let status = unsafe {
                NtQuerySystemInformation(
                    class,
                    buf.as_mut_ptr() as *mut c_void,
                    size as u32,
                    &mut needed,
                )
            };

            if status.is_ok() {
                return Ok(buf);
            }
            if status.0 as u32 == STATUS_INFO_LENGTH_MISMATCH {
                size = (needed as usize).max(size * 2);
                continue;
            }
            return Err(NtError(status.0 as u32));
        }
        Err(NtError(STATUS_INFO_LENGTH_MISMATCH))
    }
}
