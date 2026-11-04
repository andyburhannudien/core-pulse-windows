//! GPU: utilisasi per adapter dan per proses dari PDH `GPU Engine`.
//!
//! Dua agregasi berbeda, keduanya mengikuti cara Task Manager:
//!
//! - **Per adapter**: utilisasi = **maksimum** di antara engine (3D, Copy,
//!   VideoDecode, ...). Menjumlahkan semua engine akan menghasilkan angka yang
//!   tidak mungkin (mis. 300% pada adapter tunggal).
//! - **Per proses**: utilisasi = **jumlah** engine untuk PID tersebut, dibatasi
//!   100. Ini yang membuat satu proses bisa terlihat memakai 60% GPU saat
//!   me-render sambil men-decode video.
//!
//! Instance PDH yang duplikat untuk engine yang sama diambil nilai maksimumnya,
//! supaya tidak terhitung dua kali.

use crate::model::{Degradation, DegradeReason, GpuData, GpuVendor};
use crate::sys::pdh::{Counter, Query, parse_gpu_adapter_luid, parse_gpu_engine_instance};
use std::collections::HashMap;

pub struct GpuCollector {
    query: Option<Query>,
    engine: Option<Counter>,
    dedicated: Option<Counter>,
    shared: Option<Counter>,
    names: Vec<(String, String)>,
    names_probed: bool,
}

impl Default for GpuCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl GpuCollector {
    pub fn new() -> Self {
        let mut query = Query::new();
        let engine = query
            .as_mut()
            .and_then(|q| q.add(r"\GPU Engine(*)\Utilization Percentage"));
        let dedicated = query
            .as_mut()
            .and_then(|q| q.add(r"\GPU Adapter Memory(*)\Dedicated Usage"));
        let shared = query
            .as_mut()
            .and_then(|q| q.add(r"\GPU Adapter Memory(*)\Shared Usage"));

        Self {
            query,
            engine,
            dedicated,
            shared,
            names: Vec::new(),
            names_probed: false,
        }
    }

    /// Mengembalikan (per-adapter, per-PID, degradasi).
    pub fn collect(&mut self) -> (Vec<GpuData>, HashMap<u32, f32>, Vec<Degradation>) {
        let mut degraded = Vec::new();

        if self.query.is_none() {
            degraded.push(Degradation {
                area: crate::model::Area::Gpu,
                reason: DegradeReason::ApiFailed,
                detail: "PDH unavailable; GPU utilization is not measured".into(),
            });
            return (Vec::new(), HashMap::new(), degraded);
        }

        let collected = self.query.as_mut().map(|q| q.collect()).unwrap_or(false);

        let engines = self.engine.as_ref().map(|c| c.array()).unwrap_or_default();

        if engines.is_empty() {
            degraded.push(Degradation {
                area: crate::model::Area::Gpu,
                reason: if collected {
                    DegradeReason::NotAvailable
                } else {
                    DegradeReason::ApiFailed
                },
                detail: "the GPU Engine counter returned no instance".into(),
            });
        }

        if !self.names_probed && !engines.is_empty() {
            self.names_probed = true;
            self.names = read_adapter_names();
        }

        // (luid, engtype) -> pct maksimum; instance duplikat tidak dijumlahkan.
        let mut per_engine: HashMap<((u32, u32), String), f32> = HashMap::new();
        // pid -> (engtype -> pct maksimum)
        let mut per_pid: HashMap<u32, HashMap<String, f32>> = HashMap::new();

        for (instance, value) in &engines {
            let Some(parsed) = parse_gpu_engine_instance(instance) else {
                continue;
            };
            // luid tidak ada di sebagian instance; pakai (0,0) sebagai keranjang.
            let luid = parse_gpu_adapter_luid(instance).unwrap_or((0, 0));
            let pct = (*value).clamp(0.0, 100.0) as f32;

            let key = (luid, parsed.engine.clone());
            let slot = per_engine.entry(key).or_insert(0.0);
            *slot = slot.max(pct);

            let engines_of_pid = per_pid.entry(parsed.pid).or_default();
            let e = engines_of_pid.entry(parsed.engine).or_insert(0.0);
            *e = e.max(pct);
        }

        let mut per_pid_out: HashMap<u32, f32> = per_pid
            .into_iter()
            .map(|(pid, engines)| {
                let total: f32 = engines.values().sum();
                (pid, total.clamp(0.0, 100.0))
            })
            .collect();

        // Adapter: maksimum di antara engine milik adapter tersebut.
        let mut adapter_util: HashMap<(u32, u32), f32> = HashMap::new();
        for ((luid, _engine), pct) in &per_engine {
            let slot = adapter_util.entry(*luid).or_insert(0.0);
            *slot = slot.max(*pct);
        }

        let dedicated = self
            .dedicated
            .as_ref()
            .map(|c| c.array())
            .unwrap_or_default();
        let shared = self.shared.as_ref().map(|c| c.array()).unwrap_or_default();

        let mut dedicated_by_luid: HashMap<(u32, u32), u64> = HashMap::new();
        for (instance, value) in dedicated {
            if let Some(luid) = parse_gpu_adapter_luid(&instance) {
                *dedicated_by_luid.entry(luid).or_insert(0) += value.max(0.0) as u64;
            }
        }
        let mut shared_by_luid: HashMap<(u32, u32), u64> = HashMap::new();
        for (instance, value) in shared {
            if let Some(luid) = parse_gpu_adapter_luid(&instance) {
                *shared_by_luid.entry(luid).or_insert(0) += value.max(0.0) as u64;
            }
        }

        let mut luids: Vec<(u32, u32)> = adapter_util.keys().copied().collect();
        luids.sort_unstable();
        luids.dedup();

        // Windows selalu mengekspos satu adapter software ("Microsoft Basic
        // Render Driver") yang tidak punya nama di registry kelas display.
        // Tanpa filter, daftar GPU berisi entri "GPU 2" yang membingungkan.
        if !self.names.is_empty() && luids.len() > self.names.len() {
            luids.truncate(self.names.len());
        }

        let gpus = luids
            .iter()
            .enumerate()
            .map(|(order, luid)| {
                let (name, driver) = self
                    .names
                    .get(order)
                    .cloned()
                    .unwrap_or_else(|| (format!("GPU {}", order + 1), String::new()));
                GpuData {
                    vendor: vendor_of(&name),
                    name,
                    driver_version: driver,
                    // Kapasitas butuh DXGI; PDH hanya memberi pemakaian.
                    dedicated_mem_bytes: None,
                    shared_mem_bytes: None,
                    utilization_percent: adapter_util.get(luid).copied(),
                    dedicated_used_bytes: dedicated_by_luid.get(luid).copied(),
                    shared_used_bytes: shared_by_luid.get(luid).copied(),
                    // Suhu GPU baru tersedia di P3 (NVML/IGCL/ADLX).
                    temperature_c: None,
                }
            })
            .collect::<Vec<_>>();

        if gpus.is_empty() && !engines.is_empty() {
            degraded.push(Degradation {
                area: crate::model::Area::Gpu,
                reason: DegradeReason::ApiFailed,
                detail: format!(
                    "{} instance GPU Engine ada tapi tidak ada yang bisa diparse",
                    engines.len()
                ),
            });
        }

        // PID yang tidak muncul di daftar proses (sudah mati) dibuang pemanggil.
        per_pid_out.retain(|_, v| *v >= 0.0);

        (gpus, per_pid_out, degraded)
    }
}

fn vendor_of(name: &str) -> GpuVendor {
    let lower = name.to_lowercase();
    if lower.contains("nvidia") || lower.contains("geforce") || lower.contains("quadro") {
        GpuVendor::Nvidia
    } else if lower.contains("intel") || lower.contains("arc") {
        GpuVendor::Intel
    } else if lower.contains("amd") || lower.contains("radeon") || lower.contains("ati ") {
        GpuVendor::Amd
    } else if lower.contains("microsoft") || lower.contains("basic render") {
        GpuVendor::Microsoft
    } else {
        GpuVendor::Other
    }
}

/// Nama adapter dari registry kelas display. Urutan registry biasanya sama
/// dengan urutan adapter DXGI; kalau tidak, nama bisa tertukar (best effort).
fn read_adapter_names() -> Vec<(String, String)> {
    const CLASS: &str =
        r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";
    let mut out = Vec::new();
    for i in 0..8 {
        let key = format!(r"{CLASS}\{i:04}");
        let Some(name) = crate::sys::host::read_registry_string_pub(&key, "DriverDesc") else {
            continue;
        };
        if name.trim().is_empty() {
            continue;
        }
        let driver =
            crate::sys::host::read_registry_string_pub(&key, "DriverVersion").unwrap_or_default();
        out.push((name.trim().to_string(), driver));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendor_detection() {
        assert_eq!(vendor_of("NVIDIA GeForce RTX 4070"), GpuVendor::Nvidia);
        assert_eq!(vendor_of("Intel(R) UHD Graphics 630"), GpuVendor::Intel);
        assert_eq!(vendor_of("AMD Radeon RX 7800 XT"), GpuVendor::Amd);
        assert_eq!(
            vendor_of("Microsoft Basic Render Driver"),
            GpuVendor::Microsoft
        );
        assert_eq!(vendor_of("Some Accelerator"), GpuVendor::Other);
    }

    #[test]
    fn adapter_names_are_read_from_registry() {
        let names = read_adapter_names();
        // Mesin ini punya satu iGPU; minimal satu nama harus terbaca.
        assert!(
            !names.is_empty(),
            "nama adapter tidak terbaca dari registry"
        );
    }
}
