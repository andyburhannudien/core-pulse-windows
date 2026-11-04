//! Memori sistem: total/available dari `GlobalMemoryStatusEx`, komposisi dari
//! `SystemMemoryListInformation`, commit/pool dari `GetPerformanceInfo`.

use crate::model::{Degradation, MemoryData};

pub fn total_physical_bytes() -> u64 {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: dwLength sudah diisi, struct valid.
    if unsafe { GlobalMemoryStatusEx(&mut status) }.is_err() {
        return 0;
    }
    status.ullTotalPhys
}

pub fn collect() -> (MemoryData, Vec<Degradation>) {
    use windows::Win32::System::ProcessStatus::{K32GetPerformanceInfo, PERFORMANCE_INFORMATION};
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut degraded = Vec::new();

    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: dwLength diisi, struct valid.
    if unsafe { GlobalMemoryStatusEx(&mut status) }.is_err() {
        degraded.push(Degradation {
            area: crate::model::Area::Memory,
            reason: crate::model::DegradeReason::ApiFailed,
            detail: "GlobalMemoryStatusEx failed".into(),
        });
        return (MemoryData::default(), degraded);
    }

    let mut perf = PERFORMANCE_INFORMATION {
        cb: std::mem::size_of::<PERFORMANCE_INFORMATION>() as u32,
        ..Default::default()
    };
    // SAFETY: cb diisi, struct valid.
    let perf_ok = unsafe { K32GetPerformanceInfo(&mut perf, perf.cb) }.as_bool();

    let page = if perf_ok && perf.PageSize > 0 {
        perf.PageSize as u64
    } else {
        4096
    };

    let mut data = MemoryData {
        total_bytes: status.ullTotalPhys,
        available_bytes: status.ullAvailPhys,
        commit_total_bytes: if perf_ok {
            perf.CommitTotal as u64 * page
        } else {
            0
        },
        commit_limit_bytes: if perf_ok {
            perf.CommitLimit as u64 * page
        } else {
            0
        },
        paged_pool_bytes: if perf_ok {
            perf.KernelPaged as u64 * page
        } else {
            0
        },
        nonpaged_pool_bytes: if perf_ok {
            perf.KernelNonpaged as u64 * page
        } else {
            0
        },
        ..Default::default()
    };

    match read_memory_list() {
        Some(list) => {
            data.modified_bytes = list.modified.saturating_mul(page);
            data.standby_bytes = list.standby.saturating_mul(page);
            data.free_bytes = list.free.saturating_mul(page);
            data.cached_bytes = data.standby_bytes.saturating_add(data.modified_bytes);
            // Task Manager: in use = total - (free + standby).
            data.in_use_bytes = data
                .total_bytes
                .saturating_sub(data.free_bytes)
                .saturating_sub(data.standby_bytes);
        }
        None => {
            degraded.push(Degradation {
                area: crate::model::Area::Memory,
                reason: crate::model::DegradeReason::NotAvailable,
                detail: "memory composition (standby/modified) unavailable; \
                         only total and available are filled in"
                    .into(),
            });
            data.in_use_bytes = data.total_bytes.saturating_sub(data.available_bytes);
        }
    }

    data.pressure = pressure_score(&data);
    (data, degraded)
}

struct MemoryList {
    modified: u64,
    standby: u64,
    free: u64,
}

/// `SYSTEM_MEMORY_LIST_INFORMATION` (class 80). Struktur internal tapi stabil
/// sejak Vista: 6 counter tunggal, lalu 8 prioritas standby, 8 repurposed, 1.
fn read_memory_list() -> Option<MemoryList> {
    const SLOTS: usize = 23;
    let buf = crate::sys::nt::query_variable(
        windows::Wdk::System::SystemInformation::SYSTEM_INFORMATION_CLASS(80),
    )
    .ok()?;
    if buf.len() < SLOTS * std::mem::size_of::<usize>() {
        return None;
    }

    let mut vals = [0usize; SLOTS];
    for (i, slot) in vals.iter_mut().enumerate() {
        // SAFETY: offset sudah diverifikasi berada dalam buffer; `usize` POD.
        *slot = unsafe {
            std::ptr::read_unaligned(
                buf.as_ptr()
                    .add(i * std::mem::size_of::<usize>())
                    .cast::<usize>(),
            )
        };
    }

    Some(MemoryList {
        free: (vals[0] + vals[1]) as u64, // zero + free
        modified: vals[2] as u64,
        standby: vals[5..13].iter().map(|&v| v as u64).sum(),
    })
}

/// Heuristik tekanan memori 0–100. Bukan angka Windows; lihat DATA-MODEL.md.
fn pressure_score(m: &MemoryData) -> u8 {
    if m.total_bytes == 0 {
        return 0;
    }
    let used = m.in_use_bytes as f64 / m.total_bytes as f64;
    let commit = if m.commit_limit_bytes > 0 {
        m.commit_total_bytes as f64 / m.commit_limit_bytes as f64
    } else {
        0.0
    };
    let score = used * 70.0 + commit * 30.0;
    score.clamp(0.0, 100.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_is_zero_for_empty() {
        let m = MemoryData::default();
        assert_eq!(pressure_score(&m), 0);
    }

    #[test]
    fn pressure_is_bounded() {
        let m = MemoryData {
            total_bytes: 100,
            in_use_bytes: 200,
            commit_total_bytes: 500,
            commit_limit_bytes: 100,
            ..Default::default()
        };
        assert_eq!(pressure_score(&m), 100);
    }

    #[test]
    fn pressure_half_used() {
        let m = MemoryData {
            total_bytes: 1000,
            in_use_bytes: 500,
            commit_total_bytes: 0,
            commit_limit_bytes: 0,
            ..Default::default()
        };
        // 0.5 * 70 = 35
        assert_eq!(pressure_score(&m), 35);
    }
}
