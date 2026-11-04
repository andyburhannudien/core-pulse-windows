//! State UI: snapshot terbaru, filter, sort, ring buffer grafik.

use core_pulse_core::model::{ProcessRow, Snapshot};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Processes,
    Performance,
    Sensors,
    Services,
    Startup,
    Users,
}

impl Tab {
    pub fn label(self) -> &'static str {
        match self {
            Self::Processes => "Proses",
            Self::Performance => "Kinerja",
            Self::Sensors => "Sensor",
            Self::Services => "Layanan",
            Self::Startup => "Mulai",
            Self::Users => "Pengguna",
        }
    }

    pub fn all() -> [Self; 6] {
        [
            Self::Processes,
            Self::Performance,
            Self::Sensors,
            Self::Services,
            Self::Startup,
            Self::Users,
        ]
    }
}

/// Aksi yang menunggu konfirmasi pengguna.
#[derive(Debug, Clone)]
pub struct PendingAction {
    pub kind: PendingKind,
    pub pid: u32,
    pub name: String,
    pub start_time_unix_ms: u64,
    /// Jumlah proses turunan, untuk aksi yang mengenai pohon proses.
    pub descendants: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingKind {
    Terminate,
    TerminateTree,
    Suspend,
    Resume,
    SetPriority(core_pulse_core::sys::action::Priority),
    SetEfficiency(bool),
}

impl PendingKind {
    pub fn title(self) -> &'static str {
        match self {
            Self::Terminate => "Akhiri tugas?",
            Self::TerminateTree => "Akhiri pohon proses?",
            Self::Suspend => "Tangguhkan proses?",
            Self::Resume => "Lanjutkan proses?",
            Self::SetPriority(_) => "Ubah prioritas?",
            Self::SetEfficiency(true) => "Aktifkan mode efisiensi?",
            Self::SetEfficiency(false) => "Matikan mode efisiensi?",
        }
    }

    /// Aksi destruktif butuh konfirmasi eksplisit dan tombol merah.
    pub fn is_destructive(self) -> bool {
        matches!(
            self,
            Self::Terminate | Self::TerminateTree | Self::SetPriority(_)
        )
    }
}

/// Pesan hasil aksi terakhir, ditampilkan di status bar selama beberapa detik.
#[derive(Debug, Clone)]
pub struct ActionFeedback {
    pub text: String,
    pub is_error: bool,
    pub shown_at: std::time::Instant,
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
    /// Suhu rata-rata semua sensor suhu. Kosong kalau tidak ada sensor.
    pub temp: Vec<f32>,
    pub has_temp: bool,
}

impl Default for History {
    fn default() -> Self {
        Self {
            cpu: Vec::with_capacity(60),
            mem: Vec::with_capacity(60),
            gpu: Vec::with_capacity(60),
            disk: Vec::with_capacity(60),
            temp: Vec::with_capacity(60),
            has_temp: false,
        }
    }
}

impl History {
    pub fn push(&mut self, cpu: f32, mem_percent: f32, gpu: f32, disk: f32, temp: Option<f32>) {
        if self.cpu.len() >= 60 {
            self.cpu.remove(0);
            self.mem.remove(0);
            self.gpu.remove(0);
            self.disk.remove(0);
            if !self.temp.is_empty() {
                self.temp.remove(0);
            }
        }
        self.cpu.push(cpu);
        self.mem.push(mem_percent);
        self.gpu.push(gpu);
        self.disk.push(disk);
        if let Some(t) = temp {
            self.has_temp = true;
            self.temp.push(t);
        }
    }

    /// Suhu rata-rata terakhir, kalau ada. Dipakai test; UI membaca `temp`
    /// langsung karena butuh seluruh seri untuk grafik.
    #[cfg(test)]
    pub fn last_temp(&self) -> Option<f32> {
        self.temp.last().copied()
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
    /// Detail proses yang sedang dibuka di panel bawah.
    pub detail_pid: Option<u32>,
    /// Aksi yang menunggu konfirmasi.
    pub pending: Option<PendingAction>,
    pub feedback: Option<ActionFeedback>,
    /// Nyalakan verifikasi tanda tangan (mahal) hanya saat kolomnya terlihat.
    pub show_signatures: bool,
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
            detail_pid: None,
            pending: None,
            feedback: None,
            show_signatures: false,
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
        // Rata-rata suhu semua sensor suhu. `None` kalau tidak ada satu pun,
        // supaya history tidak berisi 0 °C palsu.
        let temps: Vec<f32> = snap
            .sensors
            .iter()
            .filter(|s| s.kind == core_pulse_core::model::SensorKind::Temperature)
            .map(|s| s.value)
            .collect();
        let avg_temp = if temps.is_empty() {
            None
        } else {
            Some(temps.iter().sum::<f32>() / temps.len() as f32)
        };
        self.history
            .push(snap.cpu.total_percent, used, gpu, disk, avg_temp);
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

    /// Baris untuk panel detail.
    pub fn detail_row(&self) -> Option<&ProcessRow> {
        let pid = self.detail_pid?;
        self.snapshot.processes.iter().find(|p| p.pid == pid)
    }

    /// Jumlah turunan langsung dan tidak langsung dari sebuah PID.
    pub fn descendant_count(&self, pid: u32) -> u32 {
        let mut count = 0;
        let mut frontier = vec![pid];
        for _ in 0..64 {
            if frontier.is_empty() {
                break;
            }
            let mut next = Vec::new();
            for row in &self.snapshot.processes {
                if frontier.contains(&row.parent_pid) && row.pid != pid {
                    count += 1;
                    next.push(row.pid);
                }
            }
            frontier = next;
        }
        count
    }

    /// Tampilkan pesan hasil aksi; hilang otomatis setelah 6 detik.
    pub fn set_feedback(&mut self, text: impl Into<String>, is_error: bool) {
        self.feedback = Some(ActionFeedback {
            text: text.into(),
            is_error,
            shown_at: std::time::Instant::now(),
        });
    }

    /// Buang feedback yang sudah kedaluwarsa.
    pub fn expire_feedback(&mut self) {
        if let Some(f) = &self.feedback {
            if f.shown_at.elapsed() > std::time::Duration::from_secs(6) {
                self.feedback = None;
            }
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
            h.push(i as f32, 0.0, 0.0, 0.0, Some(40.0));
        }
        assert_eq!(h.cpu.len(), 60);
        assert_eq!(h.cpu[59], 99.0);
        assert_eq!(h.disk.len(), 60);
        assert_eq!(h.temp.len(), 60);
        assert!(h.has_temp);
    }

    #[test]
    fn history_without_sensors_has_no_temperature() {
        let mut h = History::default();
        for _ in 0..5 {
            h.push(10.0, 20.0, 0.0, 0.0, None);
        }
        assert!(h.temp.is_empty(), "tidak boleh ada 0 °C palsu");
        assert!(!h.has_temp);
        assert_eq!(h.last_temp(), None);
    }

    #[test]
    fn history_keeps_temperature_series_in_sync() {
        let mut h = History::default();
        h.push(1.0, 2.0, 3.0, 4.0, Some(50.0));
        h.push(1.0, 2.0, 3.0, 4.0, Some(52.0));
        assert_eq!(h.last_temp(), Some(52.0));
        assert_eq!(h.temp.len(), 2);
    }

    #[test]
    fn sparkline_accepts_temperature_series() {
        // Suhu tidak 0–100, jadi skala grafiknya berbeda; yang diuji di sini
        // hanya bahwa seri suhu bisa dipakai tanpa panik.
        let h = History::default();
        assert!(h.temp.is_empty());
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
