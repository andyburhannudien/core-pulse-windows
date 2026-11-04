//! Collector: menyatukan semua sumber menjadi satu `Snapshot`.

use crate::model::{Degradation, DegradeReason, HostInfo, Snapshot};
use crate::sys;
use std::time::Instant;

pub struct Collector {
    host: HostInfo,
    host_degraded: Vec<Degradation>,
    cpu: sys::cpu::CpuCollector,
    procs: sys::proc::ProcCollector,
    disks: sys::disk::DiskCollector,
    net: sys::net::NetCollector,
    gpu: sys::gpu::GpuCollector,
    last: Option<Instant>,
    seq: u64,
}

impl Default for Collector {
    fn default() -> Self {
        Self::new()
    }
}

impl Collector {
    pub fn new() -> Self {
        let (host, mut host_degraded) = sys::host::collect();

        // Per-proses disk dan network butuh ETW (kernel provider), yang hanya
        // bisa dibuka dengan hak administrator. Selama helper belum ada, kolom
        // itu dibiarkan kosong alih-alih menampilkan angka tebakan.
        host_degraded.push(Degradation {
            area: crate::model::Area::Processes,
            reason: DegradeReason::NeedsElevation,
            detail: "disk/jaringan per proses butuh ETW (helper, P4)".into(),
        });

        Self {
            host,
            host_degraded,
            cpu: sys::cpu::CpuCollector::new(),
            procs: sys::proc::ProcCollector::new(),
            disks: sys::disk::DiskCollector::new(),
            net: sys::net::NetCollector::new(),
            gpu: sys::gpu::GpuCollector::new(),
            last: None,
            seq: 0,
        }
    }

    /// Bangun satu snapshot. Panggilan pertama menghasilkan CPU% nol karena
    /// delta belum tersedia — itu disengaja, bukan bug.
    pub fn sample(&mut self) -> Snapshot {
        let now = Instant::now();
        let elapsed = self.last.map(|l| now.duration_since(l)).unwrap_or_default();
        self.last = Some(now);
        self.seq += 1;
        let elapsed_secs = elapsed.as_secs_f64();

        let mut degraded = self.host_degraded.clone();

        let (cpu, cpu_deg) = self.cpu.collect(elapsed_secs);
        degraded.extend(cpu_deg);

        let (memory, mem_deg) = sys::mem::collect();
        degraded.extend(mem_deg);

        let (mut processes, proc_deg) = self.procs.collect(elapsed_secs);
        degraded.extend(proc_deg);

        let (disks, volumes, disk_deg) = self.disks.collect();
        degraded.extend(disk_deg);

        let (net, net_deg) = self.net.collect(elapsed_secs);
        degraded.extend(net_deg);

        let (gpus, gpu_by_pid, gpu_deg) = self.gpu.collect();
        degraded.extend(gpu_deg);

        // Gabungkan utilisasi GPU ke baris proses yang cocok PID-nya.
        // Kalau pengukuran GPU berhasil, proses tanpa instance GPU berarti
        // benar-benar 0% (terukur), bukan "tidak diketahui".
        if !gpus.is_empty() {
            for row in &mut processes {
                row.gpu_percent = Some(gpu_by_pid.get(&row.pid).copied().unwrap_or(0.0));
            }
        }

        let mut host = self.host.clone();
        host.uptime_secs = sys::host::uptime_secs();

        Snapshot {
            schema: crate::SCHEMA_VERSION,
            seq: self.seq,
            ts_unix_ms: crate::sampler::now_unix_ms(),
            interval_ms: elapsed.as_millis() as u32,
            host,
            cpu,
            memory,
            disks,
            volumes,
            net,
            gpus,
            processes,
            degraded,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_sample_has_zero_cpu_but_real_memory() {
        let mut c = Collector::new();
        let s = c.sample();
        assert_eq!(s.seq, 1);
        assert_eq!(s.cpu.total_percent, 0.0);
        assert!(s.memory.total_bytes > 0);
        assert!(!s.processes.is_empty());
    }

    #[test]
    fn second_sample_has_cpu_percent() {
        let mut c = Collector::new();
        let _ = c.sample();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let s = c.sample();
        assert_eq!(s.seq, 2);
        assert!(s.cpu.total_percent >= 0.0);
        assert_eq!(s.cpu.per_core_percent.len(), s.host.logical_cores as usize);
    }

    #[test]
    fn process_identity_is_unique() {
        let mut c = Collector::new();
        let s = c.sample();
        let mut keys: Vec<_> = s.processes.iter().map(|p| p.key()).collect();
        let total = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), total, "ada proses dengan identitas duplikat");
    }

    #[test]
    fn disks_and_volumes_are_collected() {
        let mut c = Collector::new();
        // Sampel kedua supaya rate PDH sudah punya delta.
        let _ = c.sample();
        std::thread::sleep(std::time::Duration::from_millis(600));
        let s = c.sample();
        assert!(!s.disks.is_empty(), "disk fisik tidak terdeteksi");
        assert!(!s.volumes.is_empty(), "volume tidak terdeteksi");
        for d in &s.disks {
            assert!(d.size_bytes > 0, "ukuran disk {} nol", d.index);
            assert!(!d.model.is_empty(), "model disk {} kosong", d.index);
            assert!(d.busy_percent >= 0.0 && d.busy_percent <= 100.0);
        }
    }

    #[test]
    fn network_adapters_are_collected() {
        let mut c = Collector::new();
        let _ = c.sample();
        std::thread::sleep(std::time::Duration::from_millis(600));
        let s = c.sample();
        assert!(!s.net.is_empty(), "adapter jaringan tidak terdeteksi");
        assert!(s.net.iter().any(|a| a.is_up), "tidak ada adapter yang up");
    }

    #[test]
    fn gpu_is_collected_or_explicitly_degraded() {
        let mut c = Collector::new();
        let _ = c.sample();
        std::thread::sleep(std::time::Duration::from_millis(600));
        let s = c.sample();
        if s.gpus.is_empty() {
            // Kalau tidak ada GPU, harus ada penjelasan, bukan diam.
            assert!(
                s.degraded.iter().any(|d| d.area == crate::model::Area::Gpu),
                "GPU kosong tanpa entri degradasi"
            );
        } else {
            for g in &s.gpus {
                assert!(!g.name.is_empty());
                if let Some(u) = g.utilization_percent {
                    assert!((0.0..=100.0).contains(&u));
                }
            }
        }
    }
}
