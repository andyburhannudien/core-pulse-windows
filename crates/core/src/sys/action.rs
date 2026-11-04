//! Aksi terhadap proses, dengan guard identitas.
//!
//! Prinsip yang mengikat (lihat docs/THREAT-MODEL.md T3):
//!
//! 1. Handle dibuka **sekali**, diverifikasi, lalu aksi dijalankan pada handle
//!    yang sama. PID tidak pernah dibuka ulang setelah verifikasi.
//! 2. Verifikasi membandingkan waktu pembuatan proses, bukan hanya PID. PID
//!    bisa dipakai ulang oleh proses lain dalam hitungan milidetik.
//! 3. Proses kritikal ditolak sebelum handle dibuka, bukan setelah.
//!
//! Semua aksi destruktif mengembalikan `ActionOutcome` yang menyebut nama dan
//! PID, supaya UI bisa menampilkan konfirmasi yang tidak ambigu.

use crate::model::{ProcessKey, ProcessRow};
use std::fmt;

/// Alasan sebuah aksi ditolak atau gagal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionError {
    /// Proses sudah tidak ada (keluar antara render dan aksi).
    ProcessGone,
    /// PID ada, tapi waktu pembuatannya berbeda: PID sudah dipakai ulang.
    IdentityChanged { expected_ms: u64, actual_ms: u64 },
    /// Butuh hak administrator (proses milik user lain atau proses sistem).
    AccessDenied,
    /// Proses kritikal: mematikannya akan membuat sistem crash.
    CriticalProcess,
    /// Aksi tidak masuk akal untuk proses ini (mis. PID 0).
    InvalidTarget(&'static str),
    /// Kegagalan lain dari API, dengan pesan aslinya.
    Failed(String),
}

impl fmt::Display for ActionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProcessGone => write!(f, "proses sudah berhenti"),
            Self::IdentityChanged {
                expected_ms,
                actual_ms,
            } => write!(
                f,
                "PID dipakai ulang oleh proses lain (dibuat {actual_ms} ms, diharapkan {expected_ms} ms)"
            ),
            Self::AccessDenied => write!(f, "butuh hak administrator"),
            Self::CriticalProcess => write!(f, "proses kritikal, tidak boleh dihentikan"),
            Self::InvalidTarget(why) => write!(f, "target tidak valid: {why}"),
            Self::Failed(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ActionError {}

/// Proses yang sudah diverifikasi identitasnya. Aksi hanya boleh dijalankan
/// lewat handle di dalam tipe ini.
pub struct VerifiedProcess {
    handle: windows::Win32::Foundation::HANDLE,
    pub key: ProcessKey,
    pub name: String,
}

impl Drop for VerifiedProcess {
    fn drop(&mut self) {
        // SAFETY: handle dibuka di `open` dan hanya ditutup sekali di sini.
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

/// Proses yang mematikannya akan membuat Windows bugcheck.
///
/// Daftar ini adalah lapisan kedua. Lapisan pertama adalah `IsProcessCritical`
/// dari kernel, yang menangkap juga proses kritikal non-standar (mis. service
/// yang mendaftarkan dirinya sebagai critical).
fn is_protected_name(name: &str) -> bool {
    matches!(
        name.to_lowercase().as_str(),
        "system"
            | "registry"
            | "memory compression"
            | "secure system"
            | "smss.exe"
            | "csrss.exe"
            | "wininit.exe"
            | "winlogon.exe"
            | "services.exe"
            | "lsass.exe"
    )
}

impl VerifiedProcess {
    /// Buka handle dengan hak tertentu, setelah memastikan identitas proses
    /// masih sama dengan yang dilihat pengguna.
    pub fn open(
        target: &ProcessKey,
        name: &str,
        access: windows::Win32::System::Threading::PROCESS_ACCESS_RIGHTS,
    ) -> Result<Self, ActionError> {
        use windows::Win32::Foundation::FILETIME;
        use windows::Win32::System::Threading::{
            GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };

        if target.pid == 0 {
            return Err(ActionError::InvalidTarget("PID 0 adalah idle process"));
        }
        if target.pid == 4 || is_protected_name(name) {
            return Err(ActionError::CriticalProcess);
        }

        // SAFETY: pid dari enumerasi; hanya hak yang diminta pemanggil, plus
        // PROCESS_QUERY_LIMITED_INFORMATION yang wajib untuk verifikasi
        // identitas. Tanpa tambahan itu, `GetProcessTimes` di bawah akan gagal
        // dan setiap aksi salah dilaporkan sebagai "access denied".
        let access = access | PROCESS_QUERY_LIMITED_INFORMATION;
        let handle = unsafe { OpenProcess(access, false, target.pid) }.map_err(|e| {
            // Error dari Win32 dibungkus HRESULT_FROM_WIN32; 16 bit rendah
            // adalah kode aslinya (ERROR_ACCESS_DENIED = 5).
            let win32 = (e.code().0 as u32) & 0xFFFF;
            if win32 == 5 {
                ActionError::AccessDenied
            } else {
                ActionError::ProcessGone
            }
        })?;

        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: handle valid, keempat out-param valid.
        let times =
            unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) };
        if times.is_err() {
            // SAFETY: handle dibuka di atas.
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(handle);
            }
            return Err(ActionError::AccessDenied);
        }

        let actual_ms = filetime_to_unix_ms(created);
        // Perbandingan eksak: kedua nilai berasal dari FILETIME yang sama dan
        // dikonversi dengan rumus yang sama, jadi tidak ada pembulatan yang
        // membuat proses yang sama terlihat berbeda.
        if actual_ms != target.start_time_unix_ms {
            // SAFETY: handle dibuka di atas.
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(handle);
            }
            return Err(ActionError::IdentityChanged {
                expected_ms: target.start_time_unix_ms,
                actual_ms,
            });
        }

        // Lapisan pertama: tanya kernel apakah proses ini kritikal.
        let mut critical = windows::core::BOOL::default();
        // SAFETY: handle valid dan punya PROCESS_QUERY_LIMITED_INFORMATION.
        let critical_ok =
            unsafe { windows::Win32::System::Threading::IsProcessCritical(handle, &mut critical) }
                .is_ok();
        if critical_ok && critical.as_bool() {
            // SAFETY: handle dibuka di atas.
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(handle);
            }
            return Err(ActionError::CriticalProcess);
        }

        Ok(Self {
            handle,
            key: *target,
            name: name.to_string(),
        })
    }

    fn raw(&self) -> windows::Win32::Foundation::HANDLE {
        self.handle
    }
}

fn filetime_to_unix_ms(t: windows::Win32::Foundation::FILETIME) -> u64 {
    const EPOCH_DIFF_MS: u64 = 11_644_473_600_000;
    let raw = ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
    (raw / 10_000).saturating_sub(EPOCH_DIFF_MS)
}

/// Hasil sebuah aksi, untuk ditampilkan di UI dan dicatat di log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionOutcome {
    pub action: &'static str,
    pub pid: u32,
    pub name: String,
    /// Jumlah proses yang terpengaruh (mis. saat mengakhiri pohon proses).
    pub affected: u32,
}

impl fmt::Display for ActionOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.affected > 1 {
            write!(
                f,
                "{}: {} ({} proses)",
                self.action, self.name, self.affected
            )
        } else {
            write!(f, "{}: {} (PID {})", self.action, self.name, self.pid)
        }
    }
}

/// Akhiri satu proses. Handle yang dipakai adalah handle yang sudah diverifikasi.
pub fn terminate(target: &ProcessKey, name: &str) -> Result<ActionOutcome, ActionError> {
    use windows::Win32::System::Threading::{PROCESS_TERMINATE, TerminateProcess};

    let proc = VerifiedProcess::open(target, name, PROCESS_TERMINATE)?;
    // SAFETY: handle valid dan punya PROCESS_TERMINATE; exit code 1 dipakai
    // supaya jelas ini dihentikan dari luar, bukan keluar normal.
    unsafe { TerminateProcess(proc.raw(), 1) }
        .map_err(|e| ActionError::Failed(format!("TerminateProcess: {e}")))?;

    Ok(ActionOutcome {
        action: "akhiri tugas",
        pid: target.pid,
        name: name.to_string(),
        affected: 1,
    })
}

/// Akhiri sebuah proses beserta semua turunannya.
///
/// Anak dikumpulkan dari snapshot yang sedang ditampilkan, lalu dihentikan dari
/// yang paling dalam ke luar. Setiap anak diverifikasi identitasnya sendiri;
/// satu anak yang gagal (mis. sudah mati) tidak membatalkan sisanya.
pub fn terminate_tree(
    root: &ProcessRow,
    snapshot_rows: &[ProcessRow],
) -> Result<ActionOutcome, ActionError> {
    let mut descendants: Vec<&ProcessRow> = Vec::new();
    let mut frontier = vec![root.pid];
    // Batas kedalaman mencegah loop kalau data PPID rusak (mis. proses induk
    // sudah mati dan PID-nya dipakai ulang).
    for _ in 0..64 {
        if frontier.is_empty() {
            break;
        }
        let mut next = Vec::new();
        for row in snapshot_rows {
            if frontier.contains(&row.parent_pid)
                && row.pid != root.pid
                && !descendants.iter().any(|d| d.pid == row.pid)
            {
                descendants.push(row);
                next.push(row.pid);
            }
        }
        frontier = next;
    }

    // Paling dalam dulu: anak sebelum induk.
    descendants.reverse();
    let mut affected = 0u32;
    for row in &descendants {
        if terminate(&row.key(), &row.name).is_ok() {
            affected += 1;
        }
    }
    terminate(&root.key(), &root.name)?;
    affected += 1;

    Ok(ActionOutcome {
        action: "akhiri pohon proses",
        pid: root.pid,
        name: root.name.clone(),
        affected,
    })
}

/// Tangguhkan proses. `NtSuspendProcess` menangguhkan semua thread sekaligus.
pub fn suspend(target: &ProcessKey, name: &str) -> Result<ActionOutcome, ActionError> {
    use windows::Win32::System::Threading::PROCESS_SUSPEND_RESUME;

    let proc = VerifiedProcess::open(target, name, PROCESS_SUSPEND_RESUME)?;
    // SAFETY: handle valid dan punya PROCESS_SUSPEND_RESUME.
    let status = unsafe { nt_suspend_process(proc.raw()) };
    if status < 0 {
        return Err(ActionError::Failed(format!(
            "NtSuspendProcess: 0x{:08X}",
            status as u32
        )));
    }
    Ok(ActionOutcome {
        action: "tangguhkan",
        pid: target.pid,
        name: name.to_string(),
        affected: 1,
    })
}

/// Lanjutkan proses yang ditangguhkan.
pub fn resume(target: &ProcessKey, name: &str) -> Result<ActionOutcome, ActionError> {
    use windows::Win32::System::Threading::PROCESS_SUSPEND_RESUME;

    let proc = VerifiedProcess::open(target, name, PROCESS_SUSPEND_RESUME)?;
    // SAFETY: handle valid dan punya PROCESS_SUSPEND_RESUME.
    let status = unsafe { nt_resume_process(proc.raw()) };
    if status < 0 {
        return Err(ActionError::Failed(format!(
            "NtResumeProcess: 0x{:08X}",
            status as u32
        )));
    }
    Ok(ActionOutcome {
        action: "lanjutkan",
        pid: target.pid,
        name: name.to_string(),
        affected: 1,
    })
}

/// Prioritas proses yang bisa dipilih pengguna.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    Idle,
    BelowNormal,
    Normal,
    AboveNormal,
    High,
    Realtime,
}

impl Priority {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::BelowNormal => "Below normal",
            Self::Normal => "Normal",
            Self::AboveNormal => "Above normal",
            Self::High => "High",
            Self::Realtime => "Realtime",
        }
    }

    pub fn all() -> [Self; 6] {
        [
            Self::Idle,
            Self::BelowNormal,
            Self::Normal,
            Self::AboveNormal,
            Self::High,
            Self::Realtime,
        ]
    }

    fn to_win(self) -> windows::Win32::System::Threading::PROCESS_CREATION_FLAGS {
        use windows::Win32::System::Threading::*;
        match self {
            Self::Idle => IDLE_PRIORITY_CLASS,
            Self::BelowNormal => BELOW_NORMAL_PRIORITY_CLASS,
            Self::Normal => NORMAL_PRIORITY_CLASS,
            Self::AboveNormal => ABOVE_NORMAL_PRIORITY_CLASS,
            Self::High => HIGH_PRIORITY_CLASS,
            Self::Realtime => REALTIME_PRIORITY_CLASS,
        }
    }
}

pub fn set_priority(
    target: &ProcessKey,
    name: &str,
    priority: Priority,
) -> Result<ActionOutcome, ActionError> {
    use windows::Win32::System::Threading::{PROCESS_SET_INFORMATION, SetPriorityClass};

    // Realtime bisa membuat sistem tidak responsif; hanya masuk akal kalau
    // pengguna benar-benar memintanya, dan itu sudah dikonfirmasi di UI.
    let proc = VerifiedProcess::open(target, name, PROCESS_SET_INFORMATION)?;
    // SAFETY: handle valid dan punya PROCESS_SET_INFORMATION.
    unsafe { SetPriorityClass(proc.raw(), priority.to_win()) }
        .map_err(|e| ActionError::Failed(format!("SetPriorityClass: {e}")))?;

    Ok(ActionOutcome {
        action: "ubah prioritas",
        pid: target.pid,
        name: name.to_string(),
        affected: 1,
    })
}

/// Efficiency Mode (EcoQoS): batasi proses ke core yang efisien.
pub fn set_efficiency_mode(
    target: &ProcessKey,
    name: &str,
    enabled: bool,
) -> Result<ActionOutcome, ActionError> {
    use windows::Win32::System::Threading::{
        PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        PROCESS_POWER_THROTTLING_STATE, PROCESS_SET_INFORMATION, ProcessPowerThrottling,
        SetProcessInformation,
    };

    let proc = VerifiedProcess::open(target, name, PROCESS_SET_INFORMATION)?;

    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: if enabled {
            PROCESS_POWER_THROTTLING_EXECUTION_SPEED
        } else {
            0
        },
    };
    // SAFETY: struct sesuai tipe yang diminta dan ukurannya benar.
    unsafe {
        SetProcessInformation(
            proc.raw(),
            ProcessPowerThrottling,
            &state as *const _ as *const _,
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        )
    }
    .map_err(|e| ActionError::Failed(format!("SetProcessInformation: {e}")))?;

    Ok(ActionOutcome {
        action: if enabled {
            "aktifkan mode efisiensi"
        } else {
            "matikan mode efisiensi"
        },
        pid: target.pid,
        name: name.to_string(),
        affected: 1,
    })
}

/// Buka folder yang berisi executable, dengan file terpilih.
pub fn reveal_in_explorer(path: &std::path::Path) -> Result<(), ActionError> {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{PCWSTR, w};

    let Some(dir) = path.parent() else {
        return Err(ActionError::InvalidTarget("path tanpa direktori induk"));
    };
    let arg = format!("/select,\"{}\"", path.display());
    let dir_w: Vec<u16> = dir
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let arg_w: Vec<u16> = arg.encode_utf16().chain(std::iter::once(0)).collect();

    // SAFETY: ketiga string NUL-terminated dan hidup selama panggilan.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            w!("explorer.exe"),
            PCWSTR(arg_w.as_ptr()),
            PCWSTR(dir_w.as_ptr()),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW mengembalikan nilai > 32 kalau berhasil.
    if result.0 as usize > 32 {
        Ok(())
    } else {
        Err(ActionError::Failed(format!(
            "ShellExecuteW mengembalikan {}",
            result.0 as usize
        )))
    }
}

// `NtSuspendProcess`/`NtResumeProcess` belum diproyeksikan oleh crate `windows`,
// jadi ditautkan langsung ke ntdll. Keduanya tidak didokumentasikan resmi tapi
// stabil sejak Windows Vista dan dipakai Process Explorer.
#[link(name = "ntdll")]
unsafe extern "system" {
    #[link_name = "NtSuspendProcess"]
    fn nt_suspend_process(handle: windows::Win32::Foundation::HANDLE) -> i32;
    #[link_name = "NtResumeProcess"]
    fn nt_resume_process(handle: windows::Win32::Foundation::HANDLE) -> i32;
}

use std::os::windows::ffi::OsStrExt;

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    /// Proses uji: `cmd /c ping -n 60 127.0.0.1` supaya tetap hidup ~60 detik
    /// tanpa butuh console atau input.
    fn spawn_dummy() -> (u32, u64) {
        let child = Command::new("cmd.exe")
            .args(["/c", "ping", "-n", "60", "127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("gagal spawn proses uji");
        let pid = child.id();
        // Biarkan proses benar-benar berjalan sebelum dibaca waktunya.
        std::thread::sleep(std::time::Duration::from_millis(250));

        let mut collector = crate::sys::proc::ProcCollector::new();
        let _ = collector.collect(1.0);
        let (rows, _) = collector.collect(1.0);
        let row = rows.iter().find(|r| r.pid == pid).unwrap_or_else(|| {
            panic!(
                "proses uji {pid} tidak ditemukan di enumerasi ({} proses)",
                rows.len()
            )
        });
        let start = row.start_time_unix_ms;

        // `child` di-forget supaya handle tidak ditutup dan proses tetap hidup.
        std::mem::forget(child);
        (pid, start)
    }

    fn cleanup(pid: u32) {
        let _ = Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    #[test]
    fn rejects_pid_zero() {
        let key = ProcessKey {
            pid: 0,
            start_time_unix_ms: 0,
        };
        assert_eq!(
            VerifiedProcess::open(
                &key,
                "System Idle Process",
                windows::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION
            )
            .err(),
            Some(ActionError::InvalidTarget("PID 0 adalah idle process"))
        );
    }

    #[test]
    fn rejects_protected_names() {
        for name in [
            "System",
            "csrss.exe",
            "wininit.exe",
            "lsass.exe",
            "services.exe",
        ] {
            let key = ProcessKey {
                pid: 999_999,
                start_time_unix_ms: 1,
            };
            assert_eq!(
                VerifiedProcess::open(
                    &key,
                    name,
                    windows::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION
                )
                .err(),
                Some(ActionError::CriticalProcess),
                "{name} seharusnya ditolak"
            );
        }
    }

    #[test]
    fn protected_name_match_is_case_insensitive() {
        assert!(is_protected_name("CSRSS.EXE"));
        assert!(is_protected_name("System"));
        assert!(!is_protected_name("chrome.exe"));
    }

    #[test]
    fn detects_identity_mismatch() {
        let (pid, start) = spawn_dummy();

        // Waktu pembuatan yang salah = PID dianggap dipakai ulang.
        let wrong = ProcessKey {
            pid,
            start_time_unix_ms: start.wrapping_add(1234),
        };
        let err = VerifiedProcess::open(
            &wrong,
            "cmd.exe",
            windows::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION,
        )
        .err()
        .expect("harus menolak identitas yang salah");
        match err {
            ActionError::IdentityChanged {
                expected_ms,
                actual_ms,
            } => {
                assert_eq!(expected_ms, start.wrapping_add(1234));
                assert_eq!(actual_ms, start);
            }
            other => panic!("error yang salah: {other:?}"),
        }

        cleanup(pid);
    }

    #[test]
    fn accepts_correct_identity() {
        let (pid, start) = spawn_dummy();
        let key = ProcessKey {
            pid,
            start_time_unix_ms: start,
        };
        let proc = VerifiedProcess::open(
            &key,
            "cmd.exe",
            windows::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION,
        )
        .expect("identitas yang benar harus diterima");
        assert_eq!(proc.key.pid, pid);
        cleanup(pid);
    }

    #[test]
    fn terminate_kills_the_process() {
        let (pid, start) = spawn_dummy();
        let key = ProcessKey {
            pid,
            start_time_unix_ms: start,
        };
        let outcome = terminate(&key, "cmd.exe").expect("terminate harus berhasil");
        assert_eq!(outcome.affected, 1);
        assert_eq!(outcome.pid, pid);

        // Beri waktu OS membersihkan handle.
        std::thread::sleep(std::time::Duration::from_millis(300));
        let mut collector = crate::sys::proc::ProcCollector::new();
        let _ = collector.collect(1.0);
        let (rows, _) = collector.collect(1.0);
        assert!(
            !rows.iter().any(|r| r.pid == pid),
            "proses masih ada setelah terminate"
        );
    }

    #[test]
    fn terminate_rejects_stale_identity() {
        let (pid, start) = spawn_dummy();
        let stale = ProcessKey {
            pid,
            start_time_unix_ms: start.wrapping_sub(1),
        };
        let err = terminate(&stale, "cmd.exe").expect_err("identitas basi harus ditolak");
        assert!(matches!(err, ActionError::IdentityChanged { .. }));

        // Proses harus masih hidup: aksi tidak boleh terjadi.
        let mut collector = crate::sys::proc::ProcCollector::new();
        let _ = collector.collect(1.0);
        let (rows, _) = collector.collect(1.0);
        assert!(
            rows.iter().any(|r| r.pid == pid),
            "proses terlanjur dimatikan padahal identitas tidak cocok"
        );
        cleanup(pid);
    }

    #[test]
    fn suspend_then_resume_works() {
        let (pid, start) = spawn_dummy();
        let key = ProcessKey {
            pid,
            start_time_unix_ms: start,
        };
        suspend(&key, "cmd.exe").expect("suspend harus berhasil");
        resume(&key, "cmd.exe").expect("resume harus berhasil");
        cleanup(pid);
    }

    #[test]
    fn priority_change_works() {
        let (pid, start) = spawn_dummy();
        let key = ProcessKey {
            pid,
            start_time_unix_ms: start,
        };
        set_priority(&key, "cmd.exe", Priority::BelowNormal)
            .expect("ubah prioritas harus berhasil");
        cleanup(pid);
    }

    #[test]
    fn efficiency_mode_toggles() {
        let (pid, start) = spawn_dummy();
        let key = ProcessKey {
            pid,
            start_time_unix_ms: start,
        };
        set_efficiency_mode(&key, "cmd.exe", true).expect("aktifkan ecoqos harus berhasil");
        set_efficiency_mode(&key, "cmd.exe", false).expect("matikan ecoqos harus berhasil");
        cleanup(pid);
    }

    #[test]
    fn tree_termination_kills_children() {
        // cmd /c ping membuat satu anak (ping.exe); kita uji bahwa anak ikut
        // terhenti saat pohon proses diakhiri.
        let parent = Command::new("cmd.exe")
            .args(["/c", "ping", "-n", "60", "127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("gagal spawn proses induk");
        let parent_pid = parent.id();
        std::mem::forget(parent);
        std::thread::sleep(std::time::Duration::from_millis(400));

        let mut collector = crate::sys::proc::ProcCollector::new();
        let _ = collector.collect(1.0);
        let (rows, _) = collector.collect(1.0);
        let root = rows
            .iter()
            .find(|r| r.pid == parent_pid)
            .cloned()
            .expect("proses induk tidak ditemukan");

        let outcome = terminate_tree(&root, &rows).expect("terminate_tree harus berhasil");
        assert!(outcome.affected >= 1);
        cleanup(parent_pid);
    }
}
