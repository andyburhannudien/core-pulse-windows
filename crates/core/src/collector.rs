//! Collector: menyatukan semua sumber menjadi satu `Snapshot`.

use crate::model::{Degradation, HostInfo, Snapshot};
use crate::sys;
use std::time::Instant;

pub struct Collector {
    host: HostInfo,
    host_degraded: Vec<Degradation>,
    cpu: sys::cpu::CpuCollector,
    procs: sys::proc::ProcCollector,
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
        let (host, host_degraded) = sys::host::collect();
        Self {
            host,
            host_degraded,
            cpu: sys::cpu::CpuCollector::new(),
            procs: sys::proc::ProcCollector::new(),
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

        let mut degraded = self.host_degraded.clone();

        let (cpu, cpu_deg) = self.cpu.collect(elapsed.as_secs_f64());
        degraded.extend(cpu_deg);

        let (memory, mem_deg) = sys::mem::collect();
        degraded.extend(mem_deg);

        let (processes, proc_deg) = self.procs.collect(elapsed.as_secs_f64());
        degraded.extend(proc_deg);

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
            disks: Vec::new(),
            volumes: Vec::new(),
            net: Vec::new(),
            gpus: Vec::new(),
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
}
