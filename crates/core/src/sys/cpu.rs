//! CPU: topologi, utilisasi per core.
//!
//! Frekuensi per core (MHz) ditunda ke P1; `CallNtPowerInformation` butuh
//! `Win32_System_Power` dan tidak dibutuhkan untuk Gate G0.

use crate::model::{CpuData, Degradation};

/// Collector stateful: persentase CPU butuh delta antar sampel.
pub struct CpuCollector {
    prev: Vec<PerfTimes>,
}

#[derive(Debug, Clone, Copy, Default)]
struct PerfTimes {
    idle: i64,
    kernel: i64,
    user: i64,
}

impl Default for CpuCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl CpuCollector {
    pub fn new() -> Self {
        Self { prev: Vec::new() }
    }

    pub fn collect(&mut self, _elapsed_secs: f64) -> (CpuData, Vec<Degradation>) {
        let mut degraded = Vec::new();
        let now = match read_perf_times() {
            Ok(v) => v,
            Err(e) => {
                degraded.push(Degradation {
                    area: crate::model::Area::Cpu,
                    reason: crate::model::DegradeReason::ApiFailed,
                    detail: format!("NtQuerySystemInformation(ProcessorPerformance): {e}"),
                });
                return (CpuData::default(), degraded);
            }
        };

        let mut per_core = vec![0.0f32; now.len()];
        let mut total_busy = 0f64;
        let mut total_all = 0f64;

        if self.prev.len() == now.len() {
            for (i, (cur, prev)) in now.iter().zip(self.prev.iter()).enumerate() {
                let kernel = (cur.kernel - prev.kernel).max(0) as f64;
                let user = (cur.user - prev.user).max(0) as f64;
                let idle = (cur.idle - prev.idle).max(0) as f64;
                let all = kernel + user;
                let busy = (all - idle).max(0.0);
                per_core[i] = if all > 0.0 {
                    (busy / all * 100.0).clamp(0.0, 100.0) as f32
                } else {
                    0.0
                };
                total_busy += busy;
                total_all += all;
            }
        }

        let total_percent = if total_all > 0.0 {
            ((total_busy / total_all) * 100.0).clamp(0.0, 100.0) as f32
        } else {
            0.0
        };

        self.prev = now;

        (
            CpuData {
                total_percent,
                per_core_percent: per_core,
                current_mhz: Vec::new(),
                ..Default::default()
            },
            degraded,
        )
    }
}

/// (brand, physical, logical, base_mhz)
pub fn brand_and_topology() -> (String, u32, u32, u32) {
    let logical = logical_cpu_count();
    let physical = physical_core_count().max(1);
    let brand = crate::sys::host::read_registry_string_pub(
        r"HARDWARE\DESCRIPTION\System\CentralProcessor\0",
        "ProcessorNameString",
    )
    .map(|s| s.trim().to_string())
    .unwrap_or_default();
    let base_mhz = crate::sys::host::read_registry_dword(
        r"HARDWARE\DESCRIPTION\System\CentralProcessor\0",
        "~MHz",
    )
    .unwrap_or(0);
    (brand, physical, logical, base_mhz)
}

pub fn logical_cpu_count() -> u32 {
    use windows::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
    let mut info = SYSTEM_INFO::default();
    // SAFETY: `info` out-param valid dan ditulis penuh.
    unsafe { GetSystemInfo(&mut info) };
    info.dwNumberOfProcessors.max(1)
}

fn physical_core_count() -> u32 {
    use windows::Win32::System::SystemInformation::{
        GetLogicalProcessorInformation, RelationProcessorCore, SYSTEM_LOGICAL_PROCESSOR_INFORMATION,
    };

    let mut needed = 0u32;
    // SAFETY: panggilan pertama dengan buffer None hanya menanyakan ukuran.
    unsafe {
        let _ = GetLogicalProcessorInformation(None, &mut needed);
    }
    if needed == 0 {
        return 0;
    }

    let count = needed as usize / std::mem::size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION>();
    let mut buf = vec![SYSTEM_LOGICAL_PROCESSOR_INFORMATION::default(); count];
    // SAFETY: buffer berukuran `needed` byte, sesuai yang diminta API.
    if unsafe { GetLogicalProcessorInformation(Some(buf.as_mut_ptr()), &mut needed) }.is_err() {
        return 0;
    }

    buf.iter()
        .filter(|info| info.Relationship == RelationProcessorCore)
        .count() as u32
}

fn read_perf_times() -> Result<Vec<PerfTimes>, crate::sys::nt::NtError> {
    use windows::Wdk::System::SystemInformation::SystemProcessorPerformanceInformation;
    use windows::Win32::System::WindowsProgramming::SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION;

    let n = logical_cpu_count().max(1) as usize;
    let stride = std::mem::size_of::<SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION>();
    let buf = crate::sys::nt::query_fixed(SystemProcessorPerformanceInformation, n * stride)?;
    if buf.len() < n * stride {
        return Err(crate::sys::nt::NtError(0));
    }

    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        // SAFETY: offset dihitung dari buffer yang panjangnya sudah diverifikasi,
        // dan struct adalah POD tanpa padding tak terinisialisasi yang dibaca.
        let info = unsafe {
            std::ptr::read_unaligned(
                buf.as_ptr()
                    .add(i * stride)
                    .cast::<SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION>(),
            )
        };
        out.push(PerfTimes {
            idle: info.IdleTime,
            kernel: info.KernelTime,
            user: info.UserTime,
        });
    }
    Ok(out)
}
