//! State UI: snapshot terbaru, filter, sort, ring buffer grafik.

use core_pulse_core::model::{ProcessRow, Snapshot};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Processes,
    Performance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Cpu,
    Memory,
    Name,
    Pid,
}

impl SortKey {
    pub fn is_numeric(self) -> bool {
        matches!(self, Self::Cpu | Self::Memory | Self::Pid)
    }
}

/// Riwayat grafik: 60 titik, 1 Hz.
pub struct History {
    pub cpu: Vec<f32>,
    pub mem: Vec<f32>,
    pub gpu: Vec<f32>,
    pub disk: Vec<f32>,
}

impl Default for History {
    fn default() -> Self {
        Self {
            cpu: Vec::with_capacity(60),
            mem: Vec::with_capacity(60),
            gpu: Vec::with_capacity(60),
            disk: Vec::with_capacity(60),
        }
    }
}

impl History {
    pub fn push(&mut self, cpu: f32, mem_percent: f32, gpu: f32, disk: f32) {
        if self.cpu.len() >= 60 {
            self.cpu.remove(0);
            self.mem.remove(0);
            self.gpu.remove(0);
            self.disk.remove(0);
        }
        self.cpu.push(cpu);
        self.mem.push(mem_percent);
        self.gpu.push(gpu);
        self.disk.push(disk);
    }
}

pub struct AppState {
    pub tab: Tab,
    pub search: String,
    pub sort: SortKey,
    pub sort_asc: bool,
    pub show_system: bool,
    pub history: History,
    pub last_seq: u64,
    pub selected: Option<u32>,
    /// Snapshot yang sedang ditampilkan (immutable, dari ArcSwap).
    pub snapshot: Arc<Snapshot>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            tab: Tab::Processes,
            search: String::new(),
            sort: SortKey::Cpu,
            sort_asc: false,
            show_system: false,
            history: History::default(),
            last_seq: 0,
            selected: None,
            snapshot: Arc::new(Snapshot::empty()),
        }
    }
}

impl AppState {
    /// Ambil snapshot baru kalau ada. Dipanggil sekali per frame.
    pub fn update_snapshot(&mut self, snap: &Arc<Snapshot>) {
        if snap.seq == self.last_seq {
            return;
        }
        self.last_seq = snap.seq;
        let total = snap.memory.total_bytes.max(1) as f32;
        let used = snap.memory.in_use_bytes as f32 / total * 100.0;
        let gpu = snap
            .gpus
            .iter()
            .filter_map(|g| g.utilization_percent)
            .fold(0.0f32, f32::max);
        let disk = snap
            .disks
            .iter()
            .map(|d| d.busy_percent)
            .fold(0.0f32, f32::max);
        self.history.push(snap.cpu.total_percent, used, gpu, disk);
        self.snapshot = Arc::clone(snap);
    }

    /// Baris proses yang lolos filter dan sudah diurutkan.
    pub fn visible_rows(&self) -> Vec<&ProcessRow> {
        let needle = self.search.trim().to_lowercase();
        let mut rows: Vec<&ProcessRow> = self
            .snapshot
            .processes
            .iter()
            // PID 0 adalah idle: menampilkannya di puncak daftar hanya noise.
            .filter(|p| p.pid != 0)
            .filter(|p| self.show_system || !is_system(p))
            .filter(|p| {
                if needle.is_empty() {
                    return true;
                }
                p.name.to_lowercase().contains(&needle) || p.pid.to_string().contains(&needle)
            })
            .collect();

        rows.sort_by(|a, b| {
            let ord = match self.sort {
                SortKey::Cpu => a
                    .cpu_percent
                    .partial_cmp(&b.cpu_percent)
                    .unwrap_or(std::cmp::Ordering::Equal),
                SortKey::Memory => a.mem_private_bytes.cmp(&b.mem_private_bytes),
                SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SortKey::Pid => a.pid.cmp(&b.pid),
            };
            if self.sort_asc { ord } else { ord.reverse() }
        });
        rows
    }

    pub fn set_sort(&mut self, key: SortKey) {
        if self.sort == key {
            self.sort_asc = !self.sort_asc;
        } else {
            self.sort = key;
            // Kolom numerik mulai dari terbesar; kolom teks mulai dari A.
            self.sort_asc = !key.is_numeric();
        }
    }
}

/// Proses inti Windows yang disembunyikan kecuali pengguna memintanya.
fn is_system(p: &ProcessRow) -> bool {
    matches!(p.pid, 4)
        || matches!(
            p.name.as_str(),
            "System"
                | "Registry"
                | "Memory Compression"
                | "Secure System"
                | "smss.exe"
                | "csrss.exe"
                | "wininit.exe"
                | "winlogon.exe"
                | "services.exe"
                | "lsass.exe"
                | "fontdrvhost.exe"
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_pulse_core::model::ProcessRow;

    fn row(pid: u32, name: &str, cpu: f32, mem: u64) -> ProcessRow {
        ProcessRow {
            pid,
            name: name.into(),
            cpu_percent: cpu,
            mem_private_bytes: mem,
            ..Default::default()
        }
    }

    fn state_with(rows: Vec<ProcessRow>) -> AppState {
        let mut s = AppState::default();
        let mut snap = Snapshot::empty();
        snap.processes = rows;
        s.snapshot = Arc::new(snap);
        s
    }

    #[test]
    fn sorts_by_cpu_desc_by_default() {
        let mut s = state_with(vec![
            row(1, "a.exe", 1.0, 10),
            row(2, "b.exe", 50.0, 20),
            row(3, "c.exe", 10.0, 30),
        ]);
        s.show_system = true;
        let names: Vec<_> = s.visible_rows().iter().map(|r| r.name.clone()).collect();
        assert_eq!(names, vec!["b.exe", "c.exe", "a.exe"]);
    }

    #[test]
    fn idle_process_is_always_hidden() {
        let s = state_with(vec![
            row(0, "System Idle Process", 90.0, 1),
            row(5, "x.exe", 1.0, 1),
        ]);
        let names: Vec<_> = s.visible_rows().iter().map(|r| r.name.clone()).collect();
        assert_eq!(names, vec!["x.exe"]);
    }

    #[test]
    fn system_processes_hidden_by_default() {
        let s = state_with(vec![row(4, "System", 1.0, 1), row(900, "app.exe", 2.0, 1)]);
        assert_eq!(s.visible_rows().len(), 1);
    }

    #[test]
    fn search_matches_name_and_pid() {
        let mut s = state_with(vec![
            row(1, "chrome.exe", 1.0, 1),
            row(2, "firefox.exe", 1.0, 1),
        ]);
        s.search = "fire".into();
        assert_eq!(s.visible_rows().len(), 1);
        s.search = "2".into();
        assert_eq!(s.visible_rows().len(), 1);
        s.search = "".into();
        assert_eq!(s.visible_rows().len(), 2);
    }

    #[test]
    fn sort_toggle_flips_direction() {
        let mut s = state_with(vec![row(1, "a", 1.0, 1), row(2, "b", 2.0, 1)]);
        // Kolom numerik: klik pertama = menurun.
        s.set_sort(SortKey::Memory);
        assert!(!s.sort_asc, "kolom numerik default menurun");
        // Kolom sama: klik berikutnya membalik arah.
        s.set_sort(SortKey::Memory);
        assert!(s.sort_asc);
        // Kolom teks: default menaik.
        s.set_sort(SortKey::Name);
        assert!(s.sort_asc);
    }

    #[test]
    fn history_is_capped() {
        let mut h = History::default();
        for i in 0..100 {
            h.push(i as f32, 0.0, 0.0, 0.0);
        }
        assert_eq!(h.cpu.len(), 60);
        assert_eq!(h.cpu[59], 99.0);
        assert_eq!(h.disk.len(), 60);
    }

    #[test]
    fn update_snapshot_ignores_same_seq() {
        let mut s = AppState::default();
        let mut snap = Snapshot::empty();
        snap.seq = 5;
        let a = Arc::new(snap.clone());
        s.update_snapshot(&a);
        assert_eq!(s.last_seq, 5);
        s.update_snapshot(&a);
        assert_eq!(
            s.history.cpu.len(),
            1,
            "seq sama tidak boleh menambah histori"
        );
    }
}
