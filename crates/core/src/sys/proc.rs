//! Enumerasi proses via `NtQuerySystemInformation(SystemProcessInformation)`.
//!
//! Struktur `SYSTEM_PROCESS_INFORMATION` didefinisikan sendiri karena proyeksi
//! di crate `windows` menyembunyikan field waktu (CreateTime/UserTime/KernelTime)
//! di balik `Reserved`. Layout mengikuti dokumentasi PHNT/ReactOS dan diverifikasi
//! lewat test ukuran + field pada mesin nyata.

use crate::model::{Degradation, ProcessKey, ProcessRow, ProcessStatus};
use std::collections::HashMap;
use std::path::PathBuf;

#[repr(C)]
#[derive(Clone, Copy)]
struct SystemProcessInformation {
    next_entry_offset: u32,
    number_of_threads: u32,
    working_set_private_size: usize,
    hard_fault_count: u32,
    number_of_threads_high_watermark: u32,
    cycle_time: u64,
    create_time: i64,
    user_time: i64,
    kernel_time: i64,
    image_name: windows::Win32::Foundation::UNICODE_STRING,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
    handle_count: u32,
    session_id: u32,
    unique_process_key: usize,
    peak_virtual_size: usize,
    virtual_size: usize,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
    private_page_count: usize,
    read_operation_count: u64,
    write_operation_count: u64,
    other_operation_count: u64,
    read_transfer_count: u64,
    write_transfer_count: u64,
    other_transfer_count: u64,
}

#[derive(Debug, Clone, Default)]
struct StaticInfo {
    exe_path: Option<PathBuf>,
    elevated: Option<bool>,
}

/// Detail statis per proses. Dibaca sekali lalu dipakai ulang; PID reuse
/// terdeteksi karena kuncinya `ProcessKey` (PID + waktu pembuatan).
#[derive(Debug, Clone, Default)]
struct DetailInfo {
    cmdline: Option<String>,
    user: Option<String>,
    integrity: Option<crate::model::Integrity>,
    eco_qos: bool,
}

/// Collector stateful: CPU% per proses butuh delta waktu antar sampel.
pub struct ProcCollector {
    prev_cpu: HashMap<ProcessKey, u64>,
    cache: HashMap<ProcessKey, StaticInfo>,
    details: HashMap<ProcessKey, DetailInfo>,
    signatures: crate::sys::details::DetailsCache,
    signature_budget: usize,
    ncpu: f64,
}

impl Default for ProcCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcCollector {
    pub fn new() -> Self {
        Self {
            prev_cpu: HashMap::new(),
            cache: HashMap::new(),
            details: HashMap::new(),
            signatures: crate::sys::details::DetailsCache::new(),
            signature_budget: 0,
            ncpu: crate::sys::cpu::logical_cpu_count().max(1) as f64,
        }
    }

    pub fn collect(&mut self, elapsed_secs: f64) -> (Vec<ProcessRow>, Vec<Degradation>) {
        let mut degraded = Vec::new();
        let buf = match crate::sys::nt::query_variable(
            windows::Wdk::System::SystemInformation::SystemProcessInformation,
        ) {
            Ok(buf) => buf,
            Err(e) => {
                degraded.push(Degradation {
                    area: crate::model::Area::Processes,
                    reason: crate::model::DegradeReason::ApiFailed,
                    detail: format!("process enumeration failed: {e}"),
                });
                return (Vec::new(), degraded);
            }
        };

        let stride = std::mem::size_of::<SystemProcessInformation>();
        let mut rows = Vec::with_capacity(256);
        let mut next_cpu = HashMap::with_capacity(256);
        let mut next_cache = HashMap::with_capacity(256);
        let mut next_details = HashMap::with_capacity(256);
        let mut offset = 0usize;
        let elapsed = if elapsed_secs <= 0.0 {
            1.0
        } else {
            elapsed_secs
        };
        // Jumlah proses baru yang detailnya boleh dibaca pada sampel ini.
        // Tanpa batas, sampel pertama akan memakan ~75 ms untuk 250 proses.
        let mut detail_budget = DETAILS_PER_SAMPLE;
        let mut sig_budget = self.signature_budget;

        while offset + stride <= buf.len() {
            // SAFETY: offset + stride <= len sudah diperiksa; struct adalah POD
            // dan kita memakai read_unaligned karena buffer tidak dijamin selaras.
            let raw = unsafe {
                std::ptr::read_unaligned(
                    buf.as_ptr().add(offset).cast::<SystemProcessInformation>(),
                )
            };

            let pid = raw.unique_process_id as u32;
            let create_100ns = raw.create_time.max(0) as u64;
            let start_time_unix_ms = filetime_100ns_to_unix_ms(create_100ns);
            let cpu_100ns = (raw.kernel_time.max(0) + raw.user_time.max(0)) as u64;
            let key = ProcessKey {
                pid,
                start_time_unix_ms,
            };

            let cpu_percent = match self.prev_cpu.get(&key) {
                Some(prev) if cpu_100ns >= *prev => {
                    let delta = (cpu_100ns - prev) as f64;
                    // 10_000_000 unit = 1 detik; bagi jumlah core untuk persen sistem.
                    ((delta / 10_000_000.0) / (elapsed * self.ncpu) * 100.0).clamp(0.0, 100.0)
                        as f32
                }
                _ => 0.0,
            };
            next_cpu.insert(key, cpu_100ns);

            let static_info = self
                .cache
                .get(&key)
                .cloned()
                .unwrap_or_else(|| query_static(pid));

            // Semua entri yang sudah ada harus tetap ada di cache berikutnya;
            // hanya entri yang benar-benar dibaca yang ditambahkan.
            let (detail, detail_complete) = match self.details.get(&key) {
                Some(cached) => {
                    next_details.insert(key, cached.clone());
                    (cached.clone(), true)
                }
                None if detail_budget > 0 && pid != 0 => {
                    detail_budget -= 1;
                    let fresh = query_details(pid);
                    next_details.insert(key, fresh.clone());
                    (fresh, true)
                }
                None => (DetailInfo::default(), false),
            };

            let name = read_image_name(&raw.image_name);
            let name = match (pid, name.is_empty()) {
                (0, true) => "System Idle Process".to_string(),
                (4, true) => "System".to_string(),
                (_, true) => format!("<pid {pid}>"),
                _ => name,
            };

            // Signature hanya diperiksa kalau pemanggil memberi budget, karena
            // satu verifikasi bisa 5–20 ms. UI menyalakannya saat kolom tanda
            // tangan terlihat.
            let signature = if sig_budget > 0 {
                match &static_info.exe_path {
                    Some(path)
                        if self.signatures.signature_for(path).status
                            != crate::model::SigStatus::Unknown =>
                    {
                        sig_budget -= 1;
                        Some(self.signatures.signature_for(path))
                    }
                    _ => None,
                }
            } else {
                None
            };

            next_cache.insert(key, static_info.clone());
            let is_svc_host = is_service_host(&name);
            rows.push(ProcessRow {
                pid,
                parent_pid: raw.inherited_from_unique_process_id as u32,
                session_id: raw.session_id,
                name,
                exe_path: static_info.exe_path,
                cmdline: detail.cmdline,
                user: detail.user,
                start_time_unix_ms,
                cpu_percent,
                cpu_time_ms: cpu_100ns / 10_000,
                mem_private_bytes: raw.private_page_count as u64,
                mem_working_set_bytes: raw.working_set_size as u64,
                mem_shared_bytes: (raw.working_set_size as u64)
                    .saturating_sub(raw.working_set_private_size as u64),
                io_read_bytes: raw.read_transfer_count,
                io_write_bytes: raw.write_transfer_count,
                disk_bps: None,
                net_bps: None,
                gpu_percent: None,
                // Detail yang belum kubaca ditandai `false` supaya UI tidak
                // menampilkan "—" (tidak tersedia) padahal hanya "belum dicoba".
                details_complete: detail_complete && (pid != 0),
                threads: raw.number_of_threads,
                handles: raw.handle_count,
                status: ProcessStatus::Running,
                elevated: static_info.elevated,
                integrity: detail.integrity,
                signature,
                eco_qos: detail.eco_qos,
                service_host: is_svc_host,
            });

            if raw.next_entry_offset == 0 {
                break;
            }
            offset = match offset.checked_add(raw.next_entry_offset as usize) {
                Some(next) if next > offset => next,
                _ => break,
            };
        }

        self.prev_cpu = next_cpu;
        self.cache = next_cache;

        // Pass kedua: isi detail untuk proses yang belum punya cache.
        //
        // Urutannya sengaja dari proses **termuda** ke tertua, bukan urutan
        // enumerasi. Proses yang baru muncul (mis. yang baru dijalankan
        // pengguna) biasanya ada di akhir daftar; tanpa pengurutan ini, budget
        // habis untuk proses lama dan proses baru menunggu beberapa detik.
        let mut pending: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| !self.details.contains_key(&r.key()) && r.pid != 0)
            .map(|(i, _)| i)
            .collect();
        pending.sort_by_key(|&i| std::cmp::Reverse(rows[i].start_time_unix_ms));

        for i in pending.into_iter().take(DETAILS_PER_SAMPLE) {
            let key = rows[i].key();
            let fresh = query_details(rows[i].pid);
            next_details.insert(key, fresh.clone());
            rows[i].cmdline = fresh.cmdline;
            rows[i].user = fresh.user;
            rows[i].integrity = fresh.integrity;
            rows[i].eco_qos = fresh.eco_qos;
            rows[i].details_complete = true;
        }

        self.details = next_details;
        (rows, degraded)
    }

    /// Izinkan verifikasi tanda tangan untuk paling banyak `n` file baru per
    /// sampel. Default 0 (tidak ada) karena satu verifikasi 5–20 ms.
    pub fn set_signature_budget(&mut self, n: usize) {
        self.signature_budget = n;
    }
}

/// Berapa proses baru yang detailnya boleh dibaca per sampel. 40 × ~0,3 ms
/// ≈ 12 ms, jadi detail terisi dalam ~6 detik untuk 250 proses tanpa
/// melanggar budget.
const DETAILS_PER_SAMPLE: usize = 40;

fn is_service_host(name: &str) -> bool {
    matches!(
        name.to_lowercase().as_str(),
        "svchost.exe" | "services.exe" | "lsass.exe" | "dllhost.exe" | "taskhostw.exe"
    )
}

/// Command line, user, integrity, dan mode efisiensi untuk satu proses.
fn query_details(pid: u32) -> DetailInfo {
    let (user, integrity) = crate::sys::details::user_and_integrity(pid);
    DetailInfo {
        cmdline: crate::sys::details::command_line(pid),
        user,
        integrity,
        eco_qos: read_efficiency_mode(pid),
    }
}

/// Apakah Efficiency Mode (EcoQoS) aktif untuk proses ini.
fn read_efficiency_mode(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        GetProcessInformation, OpenProcess, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        PROCESS_POWER_THROTTLING_STATE, PROCESS_QUERY_LIMITED_INFORMATION, ProcessPowerThrottling,
    };

    // SAFETY: pid dari enumerasi; hanya hak baca terbatas.
    let Ok(handle) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
        return false;
    };

    let mut state = PROCESS_POWER_THROTTLING_STATE::default();
    // SAFETY: struct sesuai tipe yang diminta dan ukurannya benar.
    let ok = unsafe {
        GetProcessInformation(
            handle,
            ProcessPowerThrottling,
            &mut state as *mut _ as *mut _,
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        )
    }
    .is_ok();
    // SAFETY: handle dibuka di atas dan tidak dipakai lagi.
    unsafe {
        let _ = CloseHandle(handle);
    }
    ok && state.StateMask & PROCESS_POWER_THROTTLING_EXECUTION_SPEED != 0
}

fn read_image_name(name: &windows::Win32::Foundation::UNICODE_STRING) -> String {
    if name.Buffer.is_null() || name.Length == 0 {
        return String::new();
    }
    let len = (name.Length as usize / 2).min(260);
    // SAFETY: Buffer menunjuk ke string UTF-16 sepanjang Length byte di dalam
    // buffer NtQuerySystemInformation yang masih hidup selama fungsi ini.
    let slice = unsafe { std::slice::from_raw_parts(name.Buffer.0, len) };
    String::from_utf16_lossy(slice)
}

fn query_static(pid: u32) -> StaticInfo {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{TOKEN_ELEVATION, TOKEN_QUERY};
    use windows::Win32::System::Threading::{
        OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    use windows::core::PWSTR;

    let mut info = StaticInfo::default();
    // SAFETY: pid dari enumerasi; hanya meminta hak baca terbatas.
    let Ok(handle) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
        return info;
    };

    let mut buf = vec![0u16; 32768];
    let mut size = buf.len() as u32;
    // SAFETY: buffer UTF-16 valid dan ukurannya diberikan.
    if unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut size,
        )
    }
    .is_ok()
        && size > 0
    {
        info.exe_path = Some(PathBuf::from(String::from_utf16_lossy(
            &buf[..size as usize],
        )));
    }

    let mut token = HANDLE::default();
    // SAFETY: handle proses valid; token out-param valid.
    if unsafe { OpenProcessToken(handle, TOKEN_QUERY, &mut token) }.is_ok() {
        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned = 0u32;
        // SAFETY: ukuran buffer sesuai tipe TokenElevation.
        let ok = unsafe {
            windows::Win32::Security::GetTokenInformation(
                token,
                windows::Win32::Security::TokenElevation,
                Some(&mut elevation as *mut _ as *mut _),
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut returned,
            )
        }
        .is_ok();
        // SAFETY: token dibuka di atas, ditutup di sini.
        unsafe {
            let _ = CloseHandle(token);
        }
        if ok {
            info.elevated = Some(elevation.TokenIsElevated != 0);
        }
    }

    // SAFETY: handle proses dibuka di atas, ditutup di sini.
    unsafe {
        let _ = CloseHandle(handle);
    }
    info
}

/// FILETIME (100 ns sejak 1601-01-01) → unix milidetik.
fn filetime_100ns_to_unix_ms(t: u64) -> u64 {
    const EPOCH_DIFF_MS: u64 = 11_644_473_600_000;
    (t / 10_000).saturating_sub(EPOCH_DIFF_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_layout_matches_windows_abi() {
        // x64: header 4+4, lalu usize, 4+4, 5 × i64/u64 ...
        // Yang paling penting: UniqueProcessId dan offset field waktu.
        assert_eq!(std::mem::size_of::<SystemProcessInformation>(), 256);
        assert_eq!(
            std::mem::offset_of!(SystemProcessInformation, create_time),
            32
        );
        assert_eq!(
            std::mem::offset_of!(SystemProcessInformation, kernel_time),
            48
        );
        assert_eq!(
            std::mem::offset_of!(SystemProcessInformation, unique_process_id),
            80
        );
        assert_eq!(
            std::mem::offset_of!(SystemProcessInformation, handle_count),
            96
        );
    }

    #[test]
    fn filetime_epoch_is_zero() {
        // 1601-01-01 00:00:00 UTC
        assert_eq!(filetime_100ns_to_unix_ms(0), 0);
    }

    #[test]
    fn filetime_roundtrip() {
        // 2026-10-29T00:00:00Z = 1793318400000 ms unix
        let unix_ms = 1_793_318_400_000u64;
        let ft = (unix_ms + 11_644_473_600_000) * 10_000;
        assert_eq!(filetime_100ns_to_unix_ms(ft), unix_ms);
    }

    #[test]
    fn enumeration_finds_current_process() {
        let mut c = ProcCollector::new();
        let _ = c.collect(1.0);
        let (rows, _) = c.collect(1.0);
        let me = std::process::id();
        assert!(
            rows.iter().any(|r| r.pid == me),
            "proses sendiri tidak ditemukan"
        );
        assert!(rows.len() > 10, "terlalu sedikit proses: {}", rows.len());
    }
}
