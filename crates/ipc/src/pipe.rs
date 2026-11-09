//! Transport named pipe di atas Win32.
//!
//! Server memakai `FILE_FLAG_FIRST_PIPE_INSTANCE` (THREAT-MODEL T1): kalau ada
//! proses lain yang sudah membuat pipe dengan nama sama, pembuatan kita gagal
//! dan helper melaporkannya, bukan diam-diam menyerahkan data ke pipe palsu.
//!
//! ACL dibangun dari SDDL:
//!
//! ```text
//! D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;AU)
//! ```
//!
//! - `SY` SYSTEM: penuh (helper berjalan sebagai SYSTEM).
//! - `BA` Administrators: penuh.
//! - `AU` Authenticated Users: baca+tulis (wajib untuk mengirim request).
//!
//! Ini **tidak** berarti siapa pun boleh membaca data: helper memvalidasi token
//! klien sebelum menjawab (lihat `helper::security`).

use crate::{
    FrameError, FrameHeader, MAX_MESSAGE_BYTES, PROTO_VERSION, READ_TIMEOUT_MS, write_message,
};
use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Security::SECURITY_ATTRIBUTES;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, NAMED_PIPE_MODE, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    WaitNamedPipeW,
};
use windows::Win32::System::Services::{
    CloseServiceHandle, OpenSCManagerW, OpenServiceW, QUERY_SERVICE_CONFIGW, QueryServiceConfigW,
    QueryServiceStatusEx, SC_HANDLE, SC_MANAGER_CONNECT, SC_STATUS_PROCESS_INFO,
    SERVICE_QUERY_CONFIG, SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_STATUS_PROCESS,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    WaitForSingleObject,
};
use windows::core::{PCWSTR, PWSTR};

/// SDDL ACL untuk pipe. Konstanta, bukan input pengguna.
const PIPE_SDDL: &str = "D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;AU)";

/// `SDDL_REVISION_1` dari sddl.h.
const SDDL_REVISION_1: u32 = 1;

// `ConvertStringSecurityDescriptorToSecurityDescriptorW` belum diproyeksikan
// oleh crate `windows`, jadi ditautkan langsung. API ini stabil sejak XP dan
// didokumentasikan resmi.
#[link(name = "advapi32")]
unsafe extern "system" {
    fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
        string_security_descriptor: PCWSTR,
        string_sddl_revision: u32,
        security_descriptor: *mut *mut core::ffi::c_void,
        security_descriptor_size: *mut u32,
    ) -> i32;
}

/// Deskriptor keamanan yang membebaskan memorinya sendiri.
struct SecurityDescriptor {
    raw: *mut core::ffi::c_void,
}

impl SecurityDescriptor {
    fn from_sddl(sddl: &str) -> Option<Self> {
        let wide: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
        let mut raw: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut size = 0u32;
        // SAFETY: string NUL-terminated; out-param valid.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(wide.as_ptr()),
                SDDL_REVISION_1,
                &mut raw,
                &mut size,
            )
        };
        if ok == 0 || raw.is_null() {
            return None;
        }
        Some(Self { raw })
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            // SAFETY: pointer dialokasikan oleh LocalAlloc di dalam API di atas;
            // LocalFree (dari kernel32) adalah pasangannya.
            unsafe {
                local_free(self.raw);
            }
        }
    }
}

// `LocalFree` belum diproyeksikan oleh crate `windows` di modul Memory.
#[link(name = "kernel32")]
unsafe extern "system" {
    #[link_name = "LocalFree"]
    fn local_free(h: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
}

/// Satu instance pipe server. Instance dibuat per koneksi: satu klien pada satu
/// waktu, dan pipe baru dibuat setelah klien sebelumnya selesai.
pub struct PipeServer {
    handle: HANDLE,
    name: Vec<u16>,
}

impl PipeServer {
    /// Buat instance pertama. Gagal kalau nama sudah dipakai proses lain
    /// (`FILE_FLAG_FIRST_PIPE_INSTANCE`) — itu perilaku yang diinginkan.
    pub fn create(name: &str) -> std::io::Result<Self> {
        Self::create_inner(name, true)
    }

    /// Instance berikutnya setelah klien sebelumnya selesai.
    pub fn create_next(name: &str) -> std::io::Result<Self> {
        Self::create_inner(name, false)
    }

    fn create_inner(name: &str, first: bool) -> std::io::Result<Self> {
        let sd = SecurityDescriptor::from_sddl(PIPE_SDDL).ok_or_else(|| {
            std::io::Error::other("gagal membangun security descriptor dari SDDL")
        })?;

        let attrs = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.raw,
            bInheritHandle: false.into(),
        };

        let name_w: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();

        let open_mode = if first {
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE
        } else {
            PIPE_ACCESS_DUPLEX
        };
        let pipe_mode = NAMED_PIPE_MODE(
            PIPE_TYPE_BYTE.0 | PIPE_READMODE_BYTE.0 | PIPE_WAIT.0 | PIPE_REJECT_REMOTE_CLIENTS.0,
        );

        // SAFETY: nama NUL-terminated dan hidup selama panggilan; `attrs` valid
        // dan deskriptor keamanannya hidup lebih lama dari panggilan.
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(name_w.as_ptr()),
                open_mode,
                pipe_mode,
                PIPE_UNLIMITED_INSTANCES,
                64 * 1024,
                64 * 1024,
                READ_TIMEOUT_MS,
                Some(&attrs),
            )
        };

        if handle.is_invalid() {
            return Err(std::io::Error::last_os_error());
        }

        Ok(Self {
            handle,
            name: name_w,
        })
    }

    /// Tunggu klien. Blocking sampai ada yang connect atau timeout.
    pub fn accept(&self) -> std::io::Result<()> {
        // SAFETY: handle valid dan dimiliki self.
        match unsafe { ConnectNamedPipe(self.handle, None) } {
            Ok(()) => Ok(()),
            Err(e) => {
                // ERROR_PIPE_CONNECTED (535) berarti klien sudah connect sebelum
                // ConnectNamedPipe dipanggil. Itu bukan kegagalan.
                let code = (e.code().0 as u32) & 0xFFFF;
                if code == 535 {
                    Ok(())
                } else {
                    Err(std::io::Error::other(format!("ConnectNamedPipe: {e}")))
                }
            }
        }
    }

    pub fn handle(&self) -> HANDLE {
        self.handle
    }

    pub fn name(&self) -> &[u16] {
        &self.name
    }

    /// Putuskan klien saat ini supaya instance bisa dipakai lagi.
    pub fn disconnect(&self) {
        // SAFETY: handle valid dan dimiliki self.
        unsafe {
            let _ = DisconnectNamedPipe(self.handle);
        }
    }

    /// Bungkus handle sebagai `File` untuk baca/tulis dengan `Read`/`Write`.
    /// Kepemilikan handle tetap di `PipeServer`; `File` tidak menutupnya.
    fn as_file(&self) -> std::mem::ManuallyDrop<std::fs::File> {
        use std::os::windows::io::FromRawHandle;
        // SAFETY: handle valid selama `self` hidup, dan `ManuallyDrop`
        // memastikan `File` tidak menutup handle yang bukan miliknya.
        std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_handle(self.handle.0) })
    }

    /// Baca satu request dengan batas waktu.
    ///
    /// Sebelumnya method ini memakai `ReadFile` blocking pada handle sinkron,
    /// sehingga klien yang connect lalu diam akan menahan helper **selamanya**
    /// dan semua klien lain ditolak. Sekarang data ditunggu dengan
    /// `PeekNamedPipe` sampai deadline; hanya setelah byte-nya tersedia kita
    /// membaca, jadi panggilan ini tidak pernah menggantung.
    pub fn read_request_timeout(&self, timeout_ms: u32) -> Result<crate::Request, FrameError> {
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms as u64);

        let mut header_buf = [0u8; FrameHeader::SIZE];
        self.wait_for_bytes(FrameHeader::SIZE, deadline)?;
        self.read_exact_sync(&mut header_buf)?;

        let header = FrameHeader::decode(&header_buf);
        header.validate().map_err(FrameError::Invalid)?;

        let mut payload = vec![0u8; header.length as usize];
        if !payload.is_empty() {
            self.wait_for_bytes(payload.len(), deadline)?;
            self.read_exact_sync(&mut payload)?;
        }
        Ok(serde_json::from_slice(&payload)?)
    }

    /// Tunggu sampai paling sedikit `needed` byte bisa dibaca, atau deadline
    /// habis. Polling 2 ms cukup murah untuk server yang hanya melayani satu
    /// klien lokal, dan jauh lebih sederhana daripada I/O overlapped.
    fn wait_for_bytes(
        &self,
        needed: usize,
        deadline: std::time::Instant,
    ) -> Result<(), FrameError> {
        let mut available = 0u32;
        loop {
            // SAFETY: handle valid dan dimiliki self; out-param valid.
            let ok = unsafe {
                windows::Win32::System::Pipes::PeekNamedPipe(
                    self.handle,
                    None,
                    0,
                    None,
                    Some(&mut available),
                    None,
                )
            };
            if ok.is_err() {
                return Err(FrameError::Io(std::io::Error::last_os_error()));
            }
            if available as usize >= needed {
                return Ok(());
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return Err(FrameError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "timeout menunggu data dari klien",
                )));
            }
            std::thread::sleep(std::time::Duration::from_millis(2).min(deadline - now));
        }
    }

    fn read_exact_sync(&self, buf: &mut [u8]) -> Result<(), FrameError> {
        use std::io::Read;
        let mut file = self.as_file();
        file.read_exact(buf)?;
        Ok(())
    }

    /// Baca satu request tanpa batas waktu eksplisit. Dipertahankan untuk test
    /// dan pemakaian yang sudah tahu data pasti tersedia.
    pub fn read_request(&self) -> Result<crate::Request, FrameError> {
        self.read_request_timeout(SERVER_REQUEST_TIMEOUT_MS)
    }

    pub fn write_response(&self, resp: &crate::Response) -> Result<(), FrameError> {
        let mut file = self.as_file();
        write_message(&mut *file, resp)?;
        // Tunggu klien membaca semua byte sebelum instance diputus. Tanpa ini,
        // `DisconnectNamedPipe` membuang data yang belum dibaca dan klien
        // melihat "No process is on the other end of the pipe" (233).
        //
        // Batasan yang diketahui: `FlushFileBuffers` pada pipe tidak punya
        // parameter timeout. Klien lokal yang mengirim request lalu berhenti
        // membaca bisa menahan helper selama helper menunggu drain.
        // `read_request_timeout` di bawah menutup varian serangan yang sudah
        // terbukti (klien connect lalu diam total); varian "kirim lalu tidak
        // baca" masih terbuka dan dicatat di docs/THREAT-MODEL.md T5 sebagai
        // residual risk sampai protokol punya ACK eksplisit.
        // SAFETY: handle valid dan dimiliki self.
        unsafe {
            let _ = windows::Win32::Storage::FileSystem::FlushFileBuffers(self.handle);
        }
        Ok(())
    }
}

impl Drop for PipeServer {
    fn drop(&mut self) {
        // SAFETY: handle dibuat di `create_inner` dan hanya ditutup di sini.
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

/// Batas waktu bawaan untuk satu operasi baca/tulis klien. Helper yang sehat
/// menjawab dalam hitungan milidetik; angka ini hanya pengaman agar sensor
/// thread tidak pernah menggantung.
pub const CLIENT_IO_TIMEOUT_MS: u32 = 1_500;

/// Batas waktu server menunggu request berikutnya pada satu koneksi. Klien
/// normal mengirim request dalam mikrodetik; nilai ini hanya mencegah satu
/// koneksi diam menahan helper.
pub const SERVER_REQUEST_TIMEOUT_MS: u32 = READ_TIMEOUT_MS;

/// Batas waktu mencoba connect. Kalau helper tidak ada, `WaitNamedPipeW`
/// langsung gagal; kalau ada tetapi sibuk, kita menyerah cepat.
pub const CLIENT_CONNECT_TIMEOUT_MS: u32 = 300;

/// Klien pipe (dipakai UI dan CLI).
///
/// Handle dibuka dengan `FILE_FLAG_OVERLAPPED` supaya baca/tulis punya batas
/// waktu. Tanpa itu, helper yang macet bisa membuat thread sensor menunggu
/// selamanya.
pub struct PipeClient {
    handle: HANDLE,
}

impl PipeClient {
    /// Hubungkan ke helper dengan batas waktu bawaan.
    pub fn connect(name: &str) -> std::io::Result<Self> {
        Self::connect_timeout(name, CLIENT_CONNECT_TIMEOUT_MS)
    }

    /// Hubungkan dengan batas waktu eksplisit. Gagal cepat kalau helper tidak
    /// berjalan atau semua instance sedang dipakai.
    pub fn connect_timeout(name: &str, timeout_ms: u32) -> std::io::Result<Self> {
        let name_w: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms as u64);

        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "timeout menunggu instance pipe helper",
                ));
            }
            let wait_ms = remaining.as_millis().min(u32::MAX as u128) as u32;

            // SAFETY: nama NUL-terminated dan hidup selama panggilan.
            let available = unsafe { WaitNamedPipeW(PCWSTR(name_w.as_ptr()), wait_ms.max(1)) };
            if !available.as_bool() {
                let err = std::io::Error::last_os_error();
                return Err(match err.raw_os_error() {
                    Some(121) => std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "helper sibuk (timeout WaitNamedPipeW)",
                    ),
                    _ => err,
                });
            }

            // SAFETY: nama NUL-terminated; akses baca+tulis dibutuhkan untuk
            // duplex pipe. OVERLAPPED dipakai oleh read/write berbatas waktu.
            let handle = unsafe {
                CreateFileW(
                    PCWSTR(name_w.as_ptr()),
                    0x8000_0000 | 0x4000_0000, // GENERIC_READ | GENERIC_WRITE
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED,
                    None,
                )
            };

            match handle {
                Ok(h) => return Ok(Self { handle: h }),
                Err(e) => {
                    // ERROR_PIPE_BUSY: klien lain mengambil instance lebih dulu.
                    // Coba lagi selama anggaran waktu belum habis.
                    if win32_code(&e) == 231 {
                        continue;
                    }
                    return Err(std::io::Error::other(format!("CreateFileW(pipe): {e}")));
                }
            }
        }
    }

    /// Kirim satu pesan dengan batas waktu bawaan.
    pub fn request<T: Serialize>(&self, req: &T) -> Result<(), FrameError> {
        self.request_timeout(req, CLIENT_IO_TIMEOUT_MS)
    }

    pub fn request_timeout<T: Serialize>(
        &self,
        req: &T,
        timeout_ms: u32,
    ) -> Result<(), FrameError> {
        let payload = serde_json::to_vec(req)?;
        if payload.len() > MAX_MESSAGE_BYTES as usize {
            return Err(FrameError::Invalid("payload melebihi batas 1 MiB"));
        }
        let header = FrameHeader {
            length: payload.len() as u32,
            proto_version: PROTO_VERSION,
        };
        let mut frame = Vec::with_capacity(FrameHeader::SIZE + payload.len());
        frame.extend_from_slice(&header.encode());
        frame.extend_from_slice(&payload);
        self.write_all_timeout(&frame, timeout_ms)?;
        Ok(())
    }

    /// Baca satu pesan dengan batas waktu bawaan.
    pub fn response<T: for<'de> Deserialize<'de>>(&self) -> Result<T, FrameError> {
        self.response_timeout(CLIENT_IO_TIMEOUT_MS)
    }

    pub fn response_timeout<T: for<'de> Deserialize<'de>>(
        &self,
        timeout_ms: u32,
    ) -> Result<T, FrameError> {
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms as u64);

        let mut header_buf = [0u8; FrameHeader::SIZE];
        self.read_exact_until(&mut header_buf, deadline)?;
        let header = FrameHeader::decode(&header_buf);
        header.validate().map_err(FrameError::Invalid)?;

        let mut payload = vec![0u8; header.length as usize];
        self.read_exact_until(&mut payload, deadline)?;
        Ok(serde_json::from_slice(&payload)?)
    }

    /// Tulis byte mentah. Dipakai `helper-selftest` untuk menguji penolakan
    /// frame rusak; jalur normal memakai [`Self::request`].
    pub fn write_raw(&self, data: &[u8], timeout_ms: u32) -> std::io::Result<()> {
        self.write_all_timeout(data, timeout_ms)
    }

    fn write_all_timeout(&self, mut data: &[u8], timeout_ms: u32) -> std::io::Result<()> {
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms as u64);
        while !data.is_empty() {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "timeout menulis ke pipe helper",
                ));
            }
            let written = self.write_some_timeout(data, remaining.as_millis() as u32)?;
            if written == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "pipe helper menulis nol byte",
                ));
            }
            data = &data[written..];
        }
        Ok(())
    }

    fn read_exact_until(
        &self,
        mut buf: &mut [u8],
        deadline: std::time::Instant,
    ) -> std::io::Result<()> {
        while !buf.is_empty() {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "timeout membaca dari pipe helper",
                ));
            }
            let read = self.read_some_timeout(buf, remaining.as_millis() as u32)?;
            if read == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "helper menutup pipe sebelum pesan selesai",
                ));
            }
            let rest = buf;
            buf = &mut rest[read..];
        }
        Ok(())
    }

    fn read_some_timeout(&self, buf: &mut [u8], timeout_ms: u32) -> std::io::Result<usize> {
        let mut overlapped = OVERLAPPED::default();
        let mut read = 0u32;
        // SAFETY: handle valid; buffer dan OVERLAPPED hidup selama operasi.
        let result = unsafe {
            ReadFile(
                self.handle,
                Some(buf),
                Some(&mut read),
                Some(&mut overlapped),
            )
        };
        match result {
            Ok(()) => Ok(read as usize),
            Err(e) if win32_code(&e) == 997 => self.wait_overlapped(&overlapped, timeout_ms),
            Err(e) => Err(std::io::Error::other(format!("ReadFile(pipe): {e}"))),
        }
    }

    fn write_some_timeout(&self, data: &[u8], timeout_ms: u32) -> std::io::Result<usize> {
        let mut overlapped = OVERLAPPED::default();
        let mut written = 0u32;
        // SAFETY: handle valid; buffer dan OVERLAPPED hidup selama operasi.
        let result = unsafe {
            WriteFile(
                self.handle,
                Some(data),
                Some(&mut written),
                Some(&mut overlapped),
            )
        };
        match result {
            Ok(()) => Ok(written as usize),
            Err(e) if win32_code(&e) == 997 => self.wait_overlapped(&overlapped, timeout_ms),
            Err(e) => Err(std::io::Error::other(format!("WriteFile(pipe): {e}"))),
        }
    }

    fn wait_overlapped(&self, overlapped: &OVERLAPPED, timeout_ms: u32) -> std::io::Result<usize> {
        // SAFETY: handle valid; OVERLAPPED milik operasi yang masih pending.
        let wait = unsafe { WaitForSingleObject(self.handle, timeout_ms.max(1)) };
        if wait != WAIT_OBJECT_0 {
            // Batalkan operasi dan tunggu pembatalan selesai supaya buffer
            // pemanggil tidak dipakai setelah fungsi ini kembali.
            unsafe {
                let _ = CancelIoEx(self.handle, Some(overlapped));
                let _ = WaitForSingleObject(self.handle, 1_000);
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "timeout operasi pipe helper",
            ));
        }

        let mut transferred = 0u32;
        // SAFETY: operasi selesai; hasil dibaca tanpa menunggu lagi.
        unsafe { GetOverlappedResult(self.handle, overlapped, &mut transferred, false) }
            .map_err(|e| std::io::Error::other(format!("GetOverlappedResult: {e}")))?;
        Ok(transferred as usize)
    }

    /// PID proses di ujung seberang pipe. Dipakai helper untuk memvalidasi
    /// klien dan dipakai UI untuk memverifikasi server.
    pub fn server_pid(&self) -> Option<u32> {
        let mut pid = 0u32;
        // SAFETY: handle valid; out-param valid.
        unsafe {
            windows::Win32::System::Pipes::GetNamedPipeServerProcessId(self.handle, &mut pid)
                .ok()?;
        }
        Some(pid)
    }
}

/// Ambil kode Win32 dari `windows_core::Error` (HRESULT_FROM_WIN32).
fn win32_code(e: &windows::core::Error) -> u32 {
    (e.code().0 as u32) & 0xFFFF
}

/// Path executable sebuah proses. Dipakai klien untuk memastikan pipe
/// dipegang oleh helper yang memang kita pasang.
pub fn process_image_path(pid: u32) -> Option<std::path::PathBuf> {
    // SAFETY: pid dari API; hanya hak baca terbatas.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;

    let mut buf = vec![0u16; 32_768];
    let mut len = buf.len() as u32;
    // SAFETY: buffer valid sepanjang `len`; `QueryFullProcessImageNameW`
    // menulis jumlah karakter tanpa NUL.
    let result = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    };
    // SAFETY: handle dibuka di atas.
    unsafe {
        let _ = CloseHandle(process);
    }
    result.ok()?;
    Some(std::path::PathBuf::from(String::from_utf16_lossy(
        &buf[..len as usize],
    )))
}

impl Drop for PipeClient {
    fn drop(&mut self) {
        // SAFETY: handle dibuka di `connect` dan hanya ditutup di sini.
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

/// Proses yang menjalankan sebuah Windows Service, beserta executable yang
/// dikonfigurasi di SCM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceProcess {
    pub pid: u32,
    pub image_path: std::path::PathBuf,
}

/// PID dan binary path sebuah service yang sedang berjalan.
///
/// Ini jalur verifikasi cadangan untuk helper `LocalSystem`: user biasa tidak
/// boleh `OpenProcess` proses SYSTEM, tetapi SCM tetap bisa dibaca dengan
/// `SERVICE_QUERY_STATUS | SERVICE_QUERY_CONFIG`.
///
/// `None` jika service tidak ada, tidak berjalan, atau SCM menolak query.
/// `image_path` hanya executable-nya: tanda kutip dan argumen (mis.
/// `--service`) dibuang.
pub fn service_process(service_name: &str) -> Option<ServiceProcess> {
    let name: Vec<u16> = service_name
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    // SAFETY: nama service NUL-terminated; hanya membuka SCM lokal.
    let scm = unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) }.ok()?;

    // SAFETY: `scm` valid dan nama NUL-terminated.
    let service = unsafe {
        OpenServiceW(
            scm,
            PCWSTR(name.as_ptr()),
            SERVICE_QUERY_STATUS | SERVICE_QUERY_CONFIG,
        )
    };

    let result = service.ok().and_then(|service| {
        let process = query_running_service(service);
        // SAFETY: handle dibuka di atas dan tidak dipakai lagi.
        unsafe {
            let _ = CloseServiceHandle(service);
        }
        process
    });

    // SAFETY: handle dibuka di atas dan tidak dipakai lagi.
    unsafe {
        let _ = CloseServiceHandle(scm);
    }
    result
}

/// Query status proses lalu konfigurasi binary dari satu service.
fn query_running_service(service: SC_HANDLE) -> Option<ServiceProcess> {
    let mut status = SERVICE_STATUS_PROCESS::default();
    let mut needed = 0u32;
    let size = std::mem::size_of::<SERVICE_STATUS_PROCESS>();
    // SAFETY: `status` valid dan berukuran `size`; slice hanya hidup selama
    // panggilan dan tidak dipakai setelahnya.
    let bytes = unsafe {
        std::slice::from_raw_parts_mut(std::ptr::from_mut(&mut status).cast::<u8>(), size)
    };
    // SAFETY: buffer cukup besar untuk SERVICE_STATUS_PROCESS.
    unsafe { QueryServiceStatusEx(service, SC_STATUS_PROCESS_INFO, Some(bytes), &mut needed) }
        .ok()?;

    if status.dwCurrentState != SERVICE_RUNNING || status.dwProcessId == 0 {
        return None;
    }

    let image_path = query_service_image_path(service)?;
    Some(ServiceProcess {
        pid: status.dwProcessId,
        image_path,
    })
}

fn query_service_image_path(service: SC_HANDLE) -> Option<std::path::PathBuf> {
    let mut needed = 0u32;
    // Panggilan pertama sengaja gagal dengan ERROR_INSUFFICIENT_BUFFER untuk
    // mendapatkan ukuran buffer.
    // SAFETY: out-param valid; tidak ada buffer untuk ditulis.
    let _ = unsafe { QueryServiceConfigW(service, None, 0, &mut needed) };
    if needed == 0 {
        return None;
    }

    // `Vec<usize>` memberi alignment yang cukup untuk struct berisi pointer.
    let mut buffer = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
    let config = buffer.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>();
    // SAFETY: buffer minimal `needed` byte dan cukup aligned.
    unsafe { QueryServiceConfigW(service, Some(config), needed, &mut needed) }.ok()?;
    // SAFETY: SCM mengisi `lpBinaryPathName` menunjuk ke string di `buffer`,
    // yang masih hidup selama konversi ini.
    let raw = unsafe { (*config).lpBinaryPathName.to_string() }.ok()?;
    parse_service_binary_path(&raw)
}

/// Ambil executable dari `lpBinaryPathName` yang disimpan SCM. Nilainya berupa
/// command line utuh, mis. `"C:\...\core-pulse-helper.exe" --service`.
fn parse_service_binary_path(raw: &str) -> Option<std::path::PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    if let Some(rest) = raw.strip_prefix('"') {
        let end = rest.find('"')?;
        if end == 0 {
            return None;
        }
        return Some(std::path::PathBuf::from(&rest[..end]));
    }

    // Tidak dikutip: path bisa mengandung spasi. Coba prefix terpanjang yang
    // benar-benar ada sebagai file sebelum jatuh ke token pertama. Kalau ragu,
    // token pertama membuat verifikasi gagal-tertutup (deny), bukan lolos.
    if std::path::Path::new(raw).is_file() {
        return Some(std::path::PathBuf::from(raw));
    }
    for (index, _) in raw.match_indices(' ').rev() {
        let candidate = &raw[..index];
        if std::path::Path::new(candidate).is_file() {
            return Some(std::path::PathBuf::from(candidate));
        }
    }
    raw.split_whitespace().next().map(std::path::PathBuf::from)
}

/// Batas ukuran payload yang bisa diterima helper; dipakai di test.
pub const fn max_payload() -> u32 {
    MAX_MESSAGE_BYTES
}

/// Versi protokol yang dinegosiasikan; dipakai di test.
pub const fn proto_version() -> u16 {
    PROTO_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sddl_is_parsed_by_windows() {
        // Kalau SDDL salah, CreateNamedPipeW akan memakai ACL default — yang
        // bisa terlalu terbuka. Jadi ini bukan test kosmetik.
        assert!(
            SecurityDescriptor::from_sddl(PIPE_SDDL).is_some(),
            "SDDL ACL harus valid"
        );
    }

    #[test]
    fn invalid_sddl_is_rejected() {
        assert!(SecurityDescriptor::from_sddl("this is not sddl").is_none());
    }

    #[test]
    fn server_and_client_can_exchange_a_message() {
        use crate::{Request, Response};

        let name = format!(r"\\.\pipe\core-pulse-test-{}", std::process::id());

        let server = std::thread::spawn({
            let name = name.clone();
            move || -> Result<(), String> {
                let pipe = PipeServer::create(&name).map_err(|e| e.to_string())?;
                pipe.accept().map_err(|e| e.to_string())?;
                let req: Request = pipe.read_request().map_err(|e| e.to_string())?;
                let resp = match req {
                    Request::Hello { proto_version } => Response::Hello {
                        proto_version,
                        helper_version: "test".into(),
                        elevated: false,
                    },
                    _ => Response::Error {
                        code: crate::codes::UNSUPPORTED,
                        message: "hanya Hello".into(),
                    },
                };
                pipe.write_response(&resp).map_err(|e| e.to_string())?;
                pipe.disconnect();
                Ok(())
            }
        });

        // Beri server waktu membuat instance pertama.
        std::thread::sleep(std::time::Duration::from_millis(200));

        let client = PipeClient::connect(&name).expect("klien harus bisa connect");
        client
            .request(&Request::Hello {
                proto_version: PROTO_VERSION,
            })
            .expect("request harus terkirim");
        let resp: Response = client.response().expect("respons harus terbaca");
        match resp {
            Response::Hello { helper_version, .. } => assert_eq!(helper_version, "test"),
            other => panic!("respons salah: {other:?}"),
        }

        server.join().unwrap().unwrap();
    }

    #[test]
    fn missing_pipe_connect_fails_fast() {
        let name = format!(
            r"\\.\pipe\core-pulse-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let start = std::time::Instant::now();
        let result = PipeClient::connect_timeout(&name, 200);
        assert!(result.is_err(), "pipe yang tidak ada harus gagal");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(1),
            "connect tidak boleh menggantung"
        );
    }

    #[test]
    fn response_times_out_when_server_does_not_answer() {
        use crate::Response;

        let name = format!(r"\\.\pipe\core-pulse-silent-{}", std::process::id());

        let server = std::thread::spawn({
            let name = name.clone();
            move || {
                let pipe = PipeServer::create(&name).expect("server");
                pipe.accept().expect("accept");
                std::thread::sleep(std::time::Duration::from_millis(600));
                pipe.disconnect();
            }
        });

        std::thread::sleep(std::time::Duration::from_millis(150));
        let client = PipeClient::connect(&name).expect("client");
        let start = std::time::Instant::now();
        let result: Result<Response, _> = client.response_timeout(150);
        match result {
            Err(FrameError::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::TimedOut),
            other => panic!("harus timeout, dapat {other:?}"),
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
        server.join().unwrap();
    }

    #[test]
    fn process_image_path_returns_current_exe() {
        let actual = process_image_path(std::process::id()).expect("path proses sendiri");
        let expected = std::env::current_exe().expect("current_exe");
        let actual = std::fs::canonicalize(&actual).unwrap_or(actual);
        let expected = std::fs::canonicalize(&expected).unwrap_or(expected);
        assert!(
            actual
                .to_string_lossy()
                .eq_ignore_ascii_case(&expected.to_string_lossy()),
            "path proses sendiri harus cocok: {} vs {}",
            actual.display(),
            expected.display()
        );
    }

    #[test]
    fn service_binary_path_strips_quotes_and_arguments() {
        assert_eq!(
            parse_service_binary_path(
                r#""C:\Program Files\core-pulse\core-pulse-helper.exe" --service"#
            ),
            Some(std::path::PathBuf::from(
                r"C:\Program Files\core-pulse\core-pulse-helper.exe"
            ))
        );
    }

    #[test]
    fn service_binary_path_accepts_unquoted_executable() {
        assert_eq!(
            parse_service_binary_path(r"C:\core-pulse\core-pulse-helper.exe --service"),
            Some(std::path::PathBuf::from(
                r"C:\core-pulse\core-pulse-helper.exe"
            ))
        );
    }

    #[test]
    fn service_binary_path_rejects_empty_and_unterminated_quotes() {
        assert_eq!(parse_service_binary_path(""), None);
        assert_eq!(parse_service_binary_path("   "), None);
        assert_eq!(parse_service_binary_path(r#""C:\helper.exe"#), None);
        assert_eq!(parse_service_binary_path(r#""" --service"#), None);
    }

    #[test]
    fn missing_service_has_no_process() {
        let name = format!("core-pulse-missing-service-{}", std::process::id());
        assert!(
            service_process(&name).is_none(),
            "service yang tidak ada tidak boleh menghasilkan proses"
        );
    }

    #[test]
    fn second_server_with_same_name_is_rejected() {
        // Inilah yang mencegah pipe squatting (THREAT-MODEL T1).
        let name = format!(r"\\.\pipe\core-pulse-squat-{}", std::process::id());
        let first = PipeServer::create(&name).expect("instance pertama harus berhasil");
        let second = PipeServer::create(&name);
        assert!(
            second.is_err(),
            "instance kedua dengan nama sama harus ditolak"
        );
        drop(first);
    }
}
