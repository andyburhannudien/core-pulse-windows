//! core-pulse-helper: proses elevated yang menyediakan data yang butuh admin.
//!
//! Arsitektur (ADR-0004):
//!
//! ```text
//! core-pulse.exe (non-elevated)  --named pipe-->  core-pulse-helper.exe (SYSTEM)
//! ```
//!
//! Protokol **baca-saja**: tidak ada pesan yang menyebabkan helper menjalankan
//! perintah, menulis file, atau membuka path dari klien.
//!
//! Mode:
//!
//! ```text
//! core-pulse-helper.exe --serve          # jalan di foreground (dipakai SCM)
//! core-pulse-helper.exe --console        # sama, tapi log ke stderr
//! core-pulse-helper.exe --probe          # cek hak akses dan provider, lalu keluar
//! core-pulse-helper.exe --install-service   # butuh admin
//! core-pulse-helper.exe --uninstall-service # butuh admin
//! ```
//!
//! `--serve` sengaja tidak membutuhkan argumen tambahan dan tidak membaca
//! konfigurasi dari file: permukaan serangan sekecil mungkin.

#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

pub mod security;

use core_pulse_ipc::{PIPE_NAME, Request, Response, codes};
use core_pulse_sensors::{Availability, Sensors};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Batas request per koneksi. Klien normal mengirim dua (Hello + GetSensors)
/// lalu menutup. Batas ini mencegah satu koneksi menahan helper selamanya.
const MAX_REQUESTS_PER_CONNECTION: u32 = 32;

fn main() -> std::process::ExitCode {
    init_logging();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(|s| s.as_str()).unwrap_or("--serve");

    let result = match mode {
        "--probe" => probe(),
        "--console" => serve(true),
        "--serve" => serve(false),
        "--service" => service::run(),
        "--install-service" => service::install(),
        "--uninstall-service" => service::uninstall(),
        "--start-service" => service::start(),
        "--stop-service" => service::stop(),
        "--version" | "-V" => {
            println!("core-pulse-helper {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => {
            eprintln!("mode tidak dikenal: {other}");
            eprintln!(
                "gunakan --serve, --console, --probe, --service, \
                 --install-service, --uninstall-service, --start-service, --stop-service"
            );
            Err("mode tidak dikenal".to_string())
        }
    };

    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "helper stopped karena error");
            eprintln!("error: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn init_logging() {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter =
        EnvFilter::try_from_env("CORE_PULSE_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    // Log ke stderr: saat berjalan sebagai service, ini masuk ke Event Log
    // lewat host service; saat --console, terlihat di terminal.
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

/// Laporkan apakah helper benar-benar elevated dan provider mana yang tersedia.
fn probe() -> Result<(), String> {
    println!("core-pulse-helper {}", env!("CARGO_PKG_VERSION"));

    let elevated = is_elevated();
    println!("elevated: {elevated}");
    if !elevated {
        println!("  PERINGATAN: tanpa elevasi, helper tidak berguna untuk SMART.");
    }

    match security::own_user_sid() {
        Some(sid) => println!("user SID: {} byte", sid.len()),
        None => println!("user SID: tidak terbaca"),
    }

    let mut sensors = Sensors::new();
    sensors.probe_all(true);
    println!("\nprovider:");
    for (id, availability) in sensors.statuses() {
        let text = match availability {
            Availability::Available => "tersedia".to_string(),
            Availability::NeedsElevation => "butuh administrator".to_string(),
            Availability::NotAvailable(why) => format!("tidak tersedia: {why}"),
            Availability::Failed(e) => format!("gagal: {e}"),
        };
        println!("  {id:<14} {text}");
    }

    let report = sensors.poll();
    println!("\nbacaan: {} sensor", report.readings.len());
    for r in &report.readings {
        println!("  {:<34} {:>7.1} {:?}", r.label, r.value, r.unit);
    }

    // Pipe bisa dibuat? Ini uji nyata, bukan asumsi. Kalau sudah ada helper
    // lain, pembuatan instance pertama memang harus gagal (pipe squatting
    // guard), jadi laporkan sebagai "sudah berjalan", bukan error.
    match core_pulse_ipc::pipe::PipeClient::connect_timeout(PIPE_NAME, 100) {
        Ok(client) => {
            let pid = client.server_pid().unwrap_or(0);
            println!("\npipe: sudah dipegang helper lain (PID {pid})");
        }
        Err(_) => match core_pulse_ipc::pipe::PipeServer::create(PIPE_NAME) {
            Ok(pipe) => {
                drop(pipe);
                println!("\npipe: bisa dibuat ({PIPE_NAME})");
            }
            Err(e) => println!("\npipe: GAGAL dibuat — {e}"),
        },
    }

    Ok(())
}

/// Loop server foreground (dijalankan dari terminal).
fn serve(console: bool) -> Result<(), String> {
    let running = Arc::new(AtomicBool::new(true));

    // handle Ctrl+C supaya tidak meninggalkan pipe yang menggantung.
    {
        let running = Arc::clone(&running);
        let _ = ctrlc_handler(move || running.store(false, Ordering::Relaxed));
    }

    serve_loop(running, console)
}

/// Loop server: satu instance pipe per klien, berurutan.
///
/// Kesederhanaan ini disengaja: klien hanya UI lokal, dan request bersifat
/// kilat. Tidak ada thread pool yang perlu menangani klien lambat — timeout
/// baca 2 detik dan batas request per koneksi sudah membatasi.
fn serve_loop(running: Arc<AtomicBool>, console: bool) -> Result<(), String> {
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        console,
        "helper started"
    );

    let elevated = is_elevated();
    if !elevated {
        tracing::warn!("helper is running WITHOUT elevation; SMART will not be available");
    }

    // Sanity check: validasi klien harus bisa membuka token kita sendiri.
    if let Err(e) = security::validate_client(std::process::id()) {
        tracing::warn!(reason = %e, "self-validation failed; the pipe will refuse clients");
    }

    let mut sensors = Sensors::new();
    sensors.probe_all(true);
    for (id, a) in sensors.statuses() {
        tracing::info!(provider = id, status = ?a, "sensor provider");
    }

    // Instance pertama memakai FILE_FLAG_FIRST_PIPE_INSTANCE: kalau nama sudah
    // dipakai proses lain, gagal keras (pipe squatting, THREAT-MODEL T1).
    let mut pipe = core_pulse_ipc::pipe::PipeServer::create(PIPE_NAME).map_err(|e| {
        tracing::error!(error = %e, "failed to create the first pipe instance");
        format!("CreateNamedPipeW: {e}")
    })?;

    while running.load(Ordering::Relaxed) {
        if let Err(e) = pipe.accept() {
            tracing::warn!(error = %e, "accept failed");
        } else {
            let pid = client_pid(&pipe);
            let handled = match pid {
                Some(pid) => {
                    if let Err(reason) = security::validate_client(pid) {
                        tracing::warn!(client_pid = pid, reason = %reason, "client refused");
                        // Tutup tanpa menjawab: tidak ada informasi yang bocor.
                        None
                    } else {
                        Some(handle_client(&pipe, &mut sensors, pid))
                    }
                }
                None => {
                    tracing::warn!("could not read the client PID; connection refused");
                    None
                }
            };

            if let Some(Err(e)) = handled {
                tracing::warn!(error = %e, "client handling failed");
            }
        }

        if !running.load(Ordering::Relaxed) {
            pipe.disconnect();
            break;
        }

        // Siapkan instance berikutnya **sebelum** menutup yang sekarang.
        // Tanpa ini ada jendela singkat tanpa pipe, dan klien yang connect
        // tepat pada saat itu melihat ERROR_FILE_NOT_FOUND seolah helper mati.
        let next = match core_pulse_ipc::pipe::PipeServer::create_next(PIPE_NAME) {
            Ok(next) => next,
            Err(e) => {
                tracing::error!(error = %e, "failed to create the next pipe instance");
                pipe.disconnect();
                return Err(format!("CreateNamedPipeW: {e}"));
            }
        };
        pipe.disconnect();
        drop(pipe);
        pipe = next;
    }

    tracing::info!("helper stopped");
    Ok(())
}

fn client_pid(pipe: &core_pulse_ipc::pipe::PipeServer) -> Option<u32> {
    let mut pid = 0u32;
    // SAFETY: handle pipe valid; out-param valid.
    unsafe {
        windows::Win32::System::Pipes::GetNamedPipeClientProcessId(pipe.handle(), &mut pid).ok()?;
    }
    Some(pid)
}

fn handle_client(
    pipe: &core_pulse_ipc::pipe::PipeServer,
    sensors: &mut Sensors,
    client_pid: u32,
) -> Result<(), String> {
    // Pesan pertama wajib Hello. Ini memaksa klien menyatakan versi sebelum
    // helper melakukan pekerjaan apa pun.
    let first: Request = pipe
        .read_request()
        .map_err(|e| format!("baca request: {e}"))?;

    let negotiated = match first {
        Request::Hello { proto_version } => {
            if proto_version != core_pulse_ipc::PROTO_VERSION {
                let _ = pipe.write_response(&Response::Error {
                    code: codes::BAD_VERSION,
                    message: format!(
                        "versi protokol klien {proto_version}, helper {}",
                        core_pulse_ipc::PROTO_VERSION
                    ),
                });
                return Ok(());
            }
            true
        }
        _ => {
            let _ = pipe.write_response(&Response::Error {
                code: codes::MALFORMED,
                message: "pesan pertama harus Hello".into(),
            });
            false
        }
    };
    if !negotiated {
        return Ok(());
    }

    let _ = pipe.write_response(&Response::Hello {
        proto_version: core_pulse_ipc::PROTO_VERSION,
        helper_version: env!("CARGO_PKG_VERSION").to_string(),
        elevated: is_elevated(),
    });

    tracing::debug!(client_pid, "klien terhubung");

    // Setelah Hello, klien boleh mengirim request lain sampai memutus koneksi
    // atau timeout. Setiap request dijawab persis satu respons. Batas jumlah
    // request mencegah satu koneksi menahan helper tanpa akhir.
    for _ in 0..MAX_REQUESTS_PER_CONNECTION {
        let req: Request = match pipe.read_request() {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(error = %e, "klien selesai");
                return Ok(());
            }
        };

        let response = match req {
            Request::Hello { proto_version } => Response::Hello {
                proto_version,
                helper_version: env!("CARGO_PKG_VERSION").to_string(),
                elevated: is_elevated(),
            },
            Request::GetSensors => {
                sensors.probe_all(false);
                let report = sensors.poll();
                Response::Sensors {
                    readings: report.readings.clone(),
                    degraded: report.degraded.clone(),
                    statuses: report
                        .statuses
                        .iter()
                        .map(|(provider, a)| core_pulse_core::model::SensorStatus {
                            provider: provider.clone(),
                            availability: a.to_status(),
                        })
                        .collect(),
                }
            }
            Request::GetSmart { disk_index } => {
                // Hanya indeks disk yang diterima; path tidak pernah datang dari
                // klien (mencegah path traversal ke device lain).
                if disk_index > 63 {
                    Response::Error {
                        code: codes::UNSUPPORTED,
                        message: format!("disk_index {disk_index} di luar rentang 0–63"),
                    }
                } else {
                    sensors.probe_all(false);
                    let report = sensors.poll();
                    Response::Smart {
                        disks: report
                            .readings
                            .iter()
                            .filter(|r| r.disk_index == Some(disk_index))
                            .cloned()
                            .collect(),
                    }
                }
            }
        };

        if let Err(e) = pipe.write_response(&response) {
            tracing::debug!(error = %e, "failed to write response");
            return Ok(());
        }
    }

    tracing::debug!(
        client_pid,
        limit = MAX_REQUESTS_PER_CONNECTION,
        "per-connection request limit reached"
    );
    Ok(())
}

/// Apakah proses ini berjalan dengan token elevated.
fn is_elevated() -> bool {
    core_pulse_core::sys::host::is_elevated()
}

/// Handler Ctrl+C sederhana. Tanpa ini, helper yang dijalankan dari terminal
/// meninggalkan pipe dalam keadaan terhubung.
fn ctrlc_handler<F: Fn() + Send + Sync + 'static>(f: F) -> Result<(), String> {
    use windows::Win32::System::Console::SetConsoleCtrlHandler;
    use windows::core::BOOL;

    // Handler harus `extern "system"` tanpa closure; kita pakai global kecil
    // karena hanya ada satu handler per proses.
    static CALLBACK: std::sync::OnceLock<Box<dyn Fn() + Send + Sync>> = std::sync::OnceLock::new();

    extern "system" fn handler(_ctrl_type: u32) -> BOOL {
        if let Some(cb) = CALLBACK.get() {
            cb();
        }
        BOOL::from(true)
    }

    let boxed: Box<dyn Fn() + Send + Sync> = Box::new(f);
    let _ = CALLBACK.set(boxed);

    // SAFETY: fungsi handler punya signature yang benar dan hidup selama proses.
    unsafe { SetConsoleCtrlHandler(Some(handler), true) }
        .map_err(|e| format!("SetConsoleCtrlHandler: {e}"))
}

/// Windows Service: dispatcher, instalasi, dan kontrol.
mod service {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
    use windows::Win32::System::Services::*;
    use windows::core::{PCWSTR, PWSTR};

    const SERVICE_NAME: &str = "core-pulse-helper";
    const DISPLAY_NAME: &str = "core-pulse sensor helper";

    static STATUS_HANDLE: AtomicIsize = AtomicIsize::new(0);
    static RUNNING_FLAG: std::sync::OnceLock<Arc<AtomicBool>> = std::sync::OnceLock::new();

    /// Titik masuk saat SCM menjalankan `core-pulse-helper.exe --service`.
    pub fn run() -> Result<(), String> {
        let mut name_w = wide(SERVICE_NAME);
        let mut table = [
            SERVICE_TABLE_ENTRYW {
                lpServiceName: PWSTR(name_w.as_mut_ptr()),
                lpServiceProc: Some(service_main),
            },
            SERVICE_TABLE_ENTRYW {
                lpServiceName: PWSTR::null(),
                lpServiceProc: None,
            },
        ];

        // SAFETY: tabel diakhiri entri null dan hidup selama panggilan.
        unsafe { StartServiceCtrlDispatcherW(table.as_mut_ptr()) }
            .map_err(|e| format!("StartServiceCtrlDispatcherW: {e}"))
    }

    unsafe extern "system" fn service_main(_argc: u32, _argv: *mut PWSTR) {
        let name_w = wide(SERVICE_NAME);
        // SAFETY: nama valid selama panggilan.
        let handle = match unsafe {
            RegisterServiceCtrlHandlerW(PCWSTR(name_w.as_ptr()), Some(control_handler))
        } {
            Ok(h) => h,
            Err(e) => {
                tracing::error!(error = %e, "RegisterServiceCtrlHandlerW failed");
                return;
            }
        };
        STATUS_HANDLE.store(handle.0 as isize, Ordering::SeqCst);
        report(SERVICE_START_PENDING, 1, 3_000);

        let running = Arc::new(AtomicBool::new(true));
        let _ = RUNNING_FLAG.set(Arc::clone(&running));
        report(SERVICE_RUNNING, 0, 0);

        if let Err(e) = crate::serve_loop(running, false) {
            tracing::error!(error = %e, "service stopped due to an error");
        }
        report(SERVICE_STOPPED, 0, 0);
    }

    unsafe extern "system" fn control_handler(control: u32) {
        match control {
            SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
                report(SERVICE_STOP_PENDING, 1, 3_000);
                if let Some(running) = RUNNING_FLAG.get() {
                    running.store(false, Ordering::SeqCst);
                }
                // Bangunkan `ConnectNamedPipe` yang sedang menunggu. Koneksi
                // dummy ini langsung ditutup; loop akan melihat flag stop.
                let _ = core_pulse_ipc::pipe::PipeClient::connect_timeout(
                    core_pulse_ipc::PIPE_NAME,
                    50,
                );
                tracing::info!("stop request received");
            }
            SERVICE_CONTROL_INTERROGATE => report(SERVICE_RUNNING, 0, 0),
            _ => {}
        }
    }

    fn report(state: SERVICE_STATUS_CURRENT_STATE, checkpoint: u32, wait_hint: u32) {
        let raw = STATUS_HANDLE.load(Ordering::SeqCst);
        if raw == 0 {
            return;
        }
        let accepted = if state.0 == SERVICE_RUNNING.0 {
            SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN
        } else {
            0
        };
        let status = SERVICE_STATUS {
            dwServiceType: SERVICE_WIN32_OWN_PROCESS,
            dwCurrentState: state,
            dwControlsAccepted: accepted,
            dwWin32ExitCode: 0,
            dwServiceSpecificExitCode: 0,
            dwCheckPoint: checkpoint,
            dwWaitHint: wait_hint,
        };
        // SAFETY: handle dari RegisterServiceCtrlHandlerW.
        unsafe {
            let _ = SetServiceStatus(SERVICE_STATUS_HANDLE(raw as *mut _), &status);
        }
    }

    pub fn install() -> Result<(), String> {
        require_elevation()?;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        // Path dikutip supaya spasi di "Program Files" tidak memecah argumen.
        let binary = format!("\"{}\" --service", exe.display());
        let binary_w = wide(&binary);
        let mut name_w = wide(SERVICE_NAME);
        let mut display_w = wide(DISPLAY_NAME);

        // SAFETY: handle lokal; hanya membuat service.
        let scm = unsafe {
            OpenSCManagerW(
                PCWSTR::null(),
                PCWSTR::null(),
                SC_MANAGER_CREATE_SERVICE | SC_MANAGER_CONNECT,
            )
        }
        .map_err(|e| format!("OpenSCManagerW: {e} (butuh administrator)"))?;

        // SAFETY: semua string NUL-terminated dan hidup selama panggilan.
        let created = unsafe {
            CreateServiceW(
                scm,
                PCWSTR(name_w.as_mut_ptr()),
                PCWSTR(display_w.as_mut_ptr()),
                SERVICE_ALL_ACCESS,
                SERVICE_WIN32_OWN_PROCESS,
                SERVICE_AUTO_START,
                SERVICE_ERROR_NORMAL,
                PCWSTR(binary_w.as_ptr()),
                None,
                None,
                None,
                None,
                None,
            )
        };

        let service = match created {
            Ok(h) => h,
            Err(e) if win32_code(&e) == 1073 => {
                // Service sudah ada: perbarui path (build baru) lalu pakai.
                let mut service_name = wide(SERVICE_NAME);
                // SAFETY: nama valid.
                let existing = unsafe {
                    OpenServiceW(scm, PCWSTR(service_name.as_mut_ptr()), SERVICE_ALL_ACCESS)
                }
                .map_err(|e| format!("OpenServiceW: {e}"))?;
                // SAFETY: handle valid; parameter yang tidak berubah memakai
                // SERVICE_NO_CHANGE.
                unsafe {
                    ChangeServiceConfigW(
                        existing,
                        ENUM_SERVICE_TYPE(SERVICE_NO_CHANGE),
                        SERVICE_START_TYPE(SERVICE_NO_CHANGE),
                        SERVICE_ERROR(SERVICE_NO_CHANGE),
                        PCWSTR(binary_w.as_ptr()),
                        PCWSTR::null(),
                        None,
                        PCWSTR::null(),
                        PCWSTR::null(),
                        PCWSTR::null(),
                        PCWSTR::null(),
                    )
                }
                .map_err(|e| format!("ChangeServiceConfigW: {e}"))?;
                println!("service '{SERVICE_NAME}' sudah ada; path diperbarui");
                existing
            }
            Err(e) => {
                // SAFETY: scm dibuka di atas.
                unsafe {
                    let _ = CloseServiceHandle(scm);
                }
                return Err(format!("CreateServiceW: {e}"));
            }
        };

        // SAFETY: handle service valid.
        let start_result = unsafe { StartServiceW(service, None) };
        // SAFETY: handle service dan SCM valid.
        unsafe {
            let _ = CloseServiceHandle(service);
            let _ = CloseServiceHandle(scm);
        }

        match start_result {
            Ok(()) => println!("service '{SERVICE_NAME}' terpasang dan berjalan"),
            Err(e) if win32_code(&e) == 1056 => {
                println!("service '{SERVICE_NAME}' terpasang dan sudah berjalan")
            }
            Err(e) => println!(
                "service '{SERVICE_NAME}' terpasang, tetapi gagal start: {e}\n\
                 coba: sc.exe start {SERVICE_NAME}"
            ),
        }
        Ok(())
    }

    pub fn uninstall() -> Result<(), String> {
        require_elevation()?;
        let mut name_w = wide(SERVICE_NAME);
        // SAFETY: handle lokal.
        let scm = unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) }
            .map_err(|e| format!("OpenSCManagerW: {e} (butuh administrator)"))?;

        // SAFETY: nama valid.
        let service = unsafe { OpenServiceW(scm, PCWSTR(name_w.as_mut_ptr()), SERVICE_ALL_ACCESS) };
        let service = match service {
            Ok(h) => h,
            Err(e) => {
                // SAFETY: scm dibuka di atas.
                unsafe {
                    let _ = CloseServiceHandle(scm);
                }
                return if win32_code(&e) == 1060 {
                    println!("service '{SERVICE_NAME}' memang belum terpasang");
                    Ok(())
                } else {
                    Err(format!("OpenServiceW: {e}"))
                };
            }
        };

        let mut status = SERVICE_STATUS::default();
        // SAFETY: handle valid; status out-param valid. Gagal stop (mis. sudah
        // berhenti) bukan alasan untuk tidak menghapus.
        unsafe {
            let _ = ControlService(service, SERVICE_CONTROL_STOP, &mut status);
            let _ = DeleteService(service);
            let _ = CloseServiceHandle(service);
            let _ = CloseServiceHandle(scm);
        }
        println!("service '{SERVICE_NAME}' dihapus");
        Ok(())
    }

    pub fn start() -> Result<(), String> {
        require_elevation()?;
        let mut name_w = wide(SERVICE_NAME);
        // SAFETY: handle lokal.
        let scm = unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) }
            .map_err(|e| format!("OpenSCManagerW: {e} (butuh administrator)"))?;
        // SAFETY: nama valid.
        let service = unsafe { OpenServiceW(scm, PCWSTR(name_w.as_mut_ptr()), SERVICE_START) }
            .map_err(|e| format!("OpenServiceW: {e}"))?;
        // SAFETY: handle valid.
        let result = unsafe { StartServiceW(service, None) };
        // SAFETY: handle valid.
        unsafe {
            let _ = CloseServiceHandle(service);
            let _ = CloseServiceHandle(scm);
        }
        match result {
            Ok(()) => {
                println!("service '{SERVICE_NAME}' dimulai");
                Ok(())
            }
            Err(e) if win32_code(&e) == 1056 => {
                println!("service '{SERVICE_NAME}' sudah berjalan");
                Ok(())
            }
            Err(e) => Err(format!("StartServiceW: {e}")),
        }
    }

    pub fn stop() -> Result<(), String> {
        require_elevation()?;
        let mut name_w = wide(SERVICE_NAME);
        // SAFETY: handle lokal.
        let scm = unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) }
            .map_err(|e| format!("OpenSCManagerW: {e} (butuh administrator)"))?;
        // SAFETY: nama valid.
        let service = unsafe { OpenServiceW(scm, PCWSTR(name_w.as_mut_ptr()), SERVICE_STOP) }
            .map_err(|e| format!("OpenServiceW: {e}"))?;
        let mut status = SERVICE_STATUS::default();
        // SAFETY: handle valid; status out-param valid.
        let result = unsafe { ControlService(service, SERVICE_CONTROL_STOP, &mut status) };
        // SAFETY: handle valid.
        unsafe {
            let _ = CloseServiceHandle(service);
            let _ = CloseServiceHandle(scm);
        }
        match result {
            Ok(()) => {
                println!("service '{SERVICE_NAME}' dihentikan");
                Ok(())
            }
            Err(e) => Err(format!("ControlService: {e}")),
        }
    }

    fn require_elevation() -> Result<(), String> {
        if crate::is_elevated() {
            Ok(())
        } else {
            Err("operasi service membutuhkan administrator".into())
        }
    }

    fn win32_code(e: &windows::core::Error) -> u32 {
        (e.code().0 as u32) & 0xFFFF
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_name_matches_protocol_constant() {
        assert_eq!(PIPE_NAME, core_pulse_ipc::PIPE_PATH);
    }

    #[test]
    fn request_limit_is_small_and_positive() {
        // A large limit is not a feature: it is an abuse bound.
        const { assert!(MAX_REQUESTS_PER_CONNECTION >= 2 && MAX_REQUESTS_PER_CONNECTION <= 128) };
    }

    #[test]
    fn service_binary_path_is_quoted() {
        // Path dengan spasi harus dikutip; kalau tidak, SCM memecah argumen.
        let exe = std::path::Path::new(r"C:\Program Files\core-pulse\core-pulse-helper.exe");
        let binary = format!("\"{}\" --service", exe.display());
        assert!(binary.starts_with('"'));
        assert!(binary.ends_with("\" --service"));
    }

    #[test]
    fn elevation_probe_does_not_panic() {
        // Di CI non-admin hasilnya false; di mesin admin true. Keduanya valid.
        let _ = is_elevated();
    }
}
