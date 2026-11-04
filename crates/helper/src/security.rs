//! Validasi klien pipe.
//!
//! THREAT-MODEL T1/T2: ACL pipe mengizinkan Authenticated Users menulis, karena
//! UI non-admin harus bisa mengirim request. Itu berarti **siapa pun** di mesin
//! bisa mencoba terhubung. Karena itu helper memeriksa token klien sebelum
//! menjawab:
//!
//! 1. Token harus bisa dibuka dan punya user SID yang valid.
//! 2. Integrity level minimal Medium — proses Low (AppContainer, browser
//!    sandbox) ditolak.
//! 3. User harus **sama** dengan user sesi console yang aktif. Ini yang
//!    mencegah user lain di mesin yang sama membaca data sesi orang lain.
//!
//! Kalau validasi gagal, helper menutup koneksi tanpa menjawab. Tidak ada
//! informasi yang bocor lewat pesan error.

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, LookupAccountNameW, PSID,
    SID_NAME_USE, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TOKEN_USER, TokenIntegrityLevel, TokenUser,
};
use windows::Win32::System::RemoteDesktop::{
    WTS_CURRENT_SERVER_HANDLE, WTS_INFO_CLASS, WTSDomainName, WTSFreeMemory,
    WTSGetActiveConsoleSessionId, WTSQuerySessionInformationW, WTSUserName,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::core::{PCWSTR, PWSTR};

/// Alasan klien ditolak, untuk log helper (tidak dikirim ke klien).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenialReason {
    CannotOpenProcess,
    CannotOpenToken,
    NoUserSid,
    LowIntegrity,
    DifferentUser,
    NoConsoleUser,
}

impl std::fmt::Display for DenialReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::CannotOpenProcess => "tidak bisa membuka proses klien",
            Self::CannotOpenToken => "tidak bisa membuka token klien",
            Self::NoUserSid => "token klien tidak punya user SID",
            Self::LowIntegrity => "integrity level klien terlalu rendah",
            Self::DifferentUser => "user klien berbeda dengan user sesi aktif",
            Self::NoConsoleUser => "tidak ada user sesi console untuk dibandingkan",
        };
        write!(f, "{text}")
    }
}

/// Validasi klien berdasarkan PID-nya.
pub fn validate_client(client_pid: u32) -> Result<(), DenialReason> {
    let client_sid = user_sid_of_process(client_pid)?;

    // 1. Integrity: tolak Low (AppContainer/browser sandbox).
    let integrity = integrity_of_process(client_pid)?;
    if integrity < 0x2000 {
        return Err(DenialReason::LowIntegrity);
    }

    // 2. User harus sama dengan user sesi console aktif.
    let console_sid = console_user_sid().ok_or(DenialReason::NoConsoleUser)?;
    if !sid_eq(&client_sid, &console_sid) {
        return Err(DenialReason::DifferentUser);
    }

    Ok(())
}

/// SID user dari sebuah proses, disalin ke `Vec<u8>` supaya bisa dibandingkan
/// setelah handle ditutup.
fn user_sid_of_process(pid: u32) -> Result<Vec<u8>, DenialReason> {
    // SAFETY: pid dari API; hanya hak baca terbatas.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map_err(|_| DenialReason::CannotOpenProcess)?;

    let mut token = HANDLE::default();
    // SAFETY: handle proses valid; token out-param valid.
    let ok = unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.is_ok();
    // SAFETY: handle proses tidak dipakai lagi.
    unsafe {
        let _ = CloseHandle(process);
    }
    if !ok {
        return Err(DenialReason::CannotOpenToken);
    }

    let result = token_user_sid(token);
    // SAFETY: token dibuka di atas.
    unsafe {
        let _ = CloseHandle(token);
    }
    result
}

fn token_user_sid(token: HANDLE) -> Result<Vec<u8>, DenialReason> {
    let mut needed = 0u32;
    // SAFETY: panggilan pertama hanya menanyakan ukuran.
    let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut needed) };
    if needed == 0 {
        return Err(DenialReason::CannotOpenToken);
    }

    let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
    // SAFETY: buffer >= needed byte dan cukup selaras.
    if unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr() as *mut _),
            needed,
            &mut needed,
        )
    }
    .is_err()
    {
        return Err(DenialReason::CannotOpenToken);
    }

    // SAFETY: buffer berisi TOKEN_USER yang ditulis API.
    let token_user = unsafe { &*(buf.as_ptr() as *const TOKEN_USER) };
    if token_user.User.Sid.is_invalid() {
        return Err(DenialReason::NoUserSid);
    }
    copy_sid(token_user.User.Sid).ok_or(DenialReason::NoUserSid)
}

fn integrity_of_process(pid: u32) -> Result<u32, DenialReason> {
    // SAFETY: pid dari API; hanya hak baca terbatas.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map_err(|_| DenialReason::CannotOpenProcess)?;
    let mut token = HANDLE::default();
    // SAFETY: handle proses valid; token out-param valid.
    let ok = unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.is_ok();
    // SAFETY: handle proses tidak dipakai lagi.
    unsafe {
        let _ = CloseHandle(process);
    }
    if !ok {
        return Err(DenialReason::CannotOpenToken);
    }

    let result = token_integrity_rid(token);
    // SAFETY: token dibuka di atas.
    unsafe {
        let _ = CloseHandle(token);
    }
    result
}

fn token_integrity_rid(token: HANDLE) -> Result<u32, DenialReason> {
    let mut needed = 0u32;
    // SAFETY: panggilan pertama hanya menanyakan ukuran.
    let _ = unsafe { GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut needed) };
    if needed == 0 {
        return Err(DenialReason::CannotOpenToken);
    }

    let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
    // SAFETY: buffer >= needed byte dan cukup selaras.
    if unsafe {
        GetTokenInformation(
            token,
            TokenIntegrityLevel,
            Some(buf.as_mut_ptr() as *mut _),
            needed,
            &mut needed,
        )
    }
    .is_err()
    {
        return Err(DenialReason::CannotOpenToken);
    }

    // SAFETY: buffer berisi TOKEN_MANDATORY_LABEL yang ditulis API.
    let label = unsafe { &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL) };
    let sid = label.Label.Sid;
    if sid.is_invalid() {
        return Err(DenialReason::NoUserSid);
    }
    rid_of_sid(sid).ok_or(DenialReason::NoUserSid)
}

fn rid_of_sid(sid: PSID) -> Option<u32> {
    // SAFETY: SID dari token valid.
    let count_ptr = unsafe { GetSidSubAuthorityCount(sid) };
    if count_ptr.is_null() {
        return None;
    }
    // SAFETY: pointer dari API menunjuk satu byte valid.
    let count = unsafe { *count_ptr };
    if count == 0 {
        return None;
    }
    // SAFETY: indeks terakhir SID valid.
    let rid_ptr = unsafe { GetSidSubAuthority(sid, (count - 1) as u32) };
    if rid_ptr.is_null() {
        return None;
    }
    // SAFETY: pointer dari API menunjuk satu u32 valid.
    Some(unsafe { *rid_ptr })
}

/// Salin SID ke `Vec<u8>` supaya bisa dibandingkan setelah handle ditutup.
fn copy_sid(sid: PSID) -> Option<Vec<u8>> {
    if sid.is_invalid() {
        return None;
    }
    // SAFETY: SID valid; `GetLengthSid` adalah cara resmi menghitung ukurannya.
    let len = unsafe { windows::Win32::Security::GetLengthSid(sid) };
    if len == 0 {
        return None;
    }
    // SAFETY: SID valid sepanjang `len` byte yang dilaporkan API.
    Some(unsafe { std::slice::from_raw_parts(sid.0 as *const u8, len as usize) }.to_vec())
}

fn sid_eq(a: &[u8], b: &[u8]) -> bool {
    a == b
}

/// SID user yang sedang login di sesi console aktif.
///
/// Dibaca lewat Terminal Services (`WTSQuerySessionInformationW`) lalu
/// `LookupAccountNameW`. Cara ini tetap benar saat layar terkunci, dan tidak
/// salah mengambil `winlogon.exe`/`dwm.exe` yang berjalan sebagai SYSTEM.
fn console_user_sid() -> Option<Vec<u8>> {
    // 0xFFFFFFFF berarti tidak ada sesi console (mis. server tanpa login).
    let session = unsafe { WTSGetActiveConsoleSessionId() };
    if session == u32::MAX {
        return None;
    }

    let user = wts_string(session, WTSUserName)?;
    if user.is_empty() {
        return None;
    }
    let domain = wts_string(session, WTSDomainName).unwrap_or_default();
    let account = if domain.is_empty() {
        user
    } else {
        format!("{domain}\\{user}")
    };
    sid_from_account_name(&account)
}

/// Baca satu string dari WTS. Buffer dimiliki WTS dan harus dibebaskan dengan
/// `WTSFreeMemory`.
fn wts_string(session: u32, class: WTS_INFO_CLASS) -> Option<String> {
    let mut buffer = PWSTR::null();
    let mut bytes = 0u32;
    // SAFETY: handle server saat ini; out-param valid.
    unsafe {
        WTSQuerySessionInformationW(
            Some(WTS_CURRENT_SERVER_HANDLE),
            session,
            class,
            &mut buffer,
            &mut bytes,
        )
    }
    .ok()?;

    if buffer.is_null() {
        return None;
    }
    // SAFETY: WTS mengembalikan string UTF-16 NUL-terminated sepanjang `bytes`.
    let text = unsafe { buffer.to_string() }.ok();
    // SAFETY: buffer dialokasikan oleh WTS.
    unsafe {
        WTSFreeMemory(buffer.0 as *mut core::ffi::c_void);
    }
    text
}

/// SID dari nama akun (`DOMAIN\user` atau `user`).
fn sid_from_account_name(account: &str) -> Option<Vec<u8>> {
    let account_w: Vec<u16> = account.encode_utf16().chain(std::iter::once(0)).collect();
    let mut sid_size = 0u32;
    let mut domain_size = 0u32;
    let mut sid_type = SID_NAME_USE(0);

    // Panggilan pertama hanya menanyakan ukuran. Kegagalan karena buffer kosong
    // adalah hal normal.
    let _ = unsafe {
        LookupAccountNameW(
            PCWSTR::null(),
            PCWSTR(account_w.as_ptr()),
            None,
            &mut sid_size,
            None,
            &mut domain_size,
            &mut sid_type,
        )
    };
    if sid_size == 0 {
        return None;
    }

    // SID berisi sub-authority u32, jadi buffer harus selaras 4 byte.
    let mut sid_buf = vec![0u32; (sid_size as usize).div_ceil(4)];
    let mut domain_buf = vec![0u16; domain_size as usize];
    let mut sid_len = sid_size;
    let mut domain_len = domain_size;

    // SAFETY: buffer berukuran sesuai hasil panggilan pertama.
    unsafe {
        LookupAccountNameW(
            PCWSTR::null(),
            PCWSTR(account_w.as_ptr()),
            Some(PSID(sid_buf.as_mut_ptr() as *mut core::ffi::c_void)),
            &mut sid_len,
            Some(PWSTR(domain_buf.as_mut_ptr())),
            &mut domain_len,
            &mut sid_type,
        )
    }
    .ok()?;

    copy_sid(PSID(sid_buf.as_ptr() as *mut core::ffi::c_void))
}

/// Bantu: SID user proses ini (dipakai saat helper memvalidasi dirinya sendiri).
pub fn own_user_sid() -> Option<Vec<u8>> {
    let mut token = HANDLE::default();
    // SAFETY: pseudo-handle selalu valid.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
        return None;
    }
    let sid = token_user_sid(token).ok();
    // SAFETY: token dibuka di atas.
    unsafe {
        let _ = CloseHandle(token);
    }
    sid
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_process_passes_validation() {
        // Helper memvalidasi dirinya sendiri sebagai sanity check saat start.
        let pid = std::process::id();
        let result = validate_client(pid);
        // Di mesin uji ini, sesi console ada (user sedang login), jadi harus lolos.
        // Kalau dijalankan di sesi tanpa console (CI headless), NoConsoleUser
        // adalah jawaban yang benar — bukan panic.
        match result {
            Ok(()) => {}
            Err(DenialReason::NoConsoleUser) => {}
            Err(other) => panic!("validasi proses sendiri gagal: {other}"),
        }
    }

    #[test]
    fn nonexistent_pid_is_rejected() {
        let result = validate_client(0xFFFF_FF00);
        assert_eq!(result, Err(DenialReason::CannotOpenProcess));
    }

    #[test]
    fn own_sid_is_readable() {
        let sid = own_user_sid().expect("SID sendiri harus terbaca");
        assert!(sid.len() >= 8, "SID minimal 8 byte, dapat {}", sid.len());
    }

    #[test]
    fn same_sid_compares_equal() {
        let a = own_user_sid().unwrap();
        let b = own_user_sid().unwrap();
        assert!(sid_eq(&a, &b));
    }

    #[test]
    fn console_user_matches_own_sid_when_logged_in() {
        // Di sesi interaktif, user console harus user yang menjalankan test.
        // CI headless boleh tidak punya sesi console (None); itu bukan panic.
        if let Some(console) = console_user_sid() {
            let own = own_user_sid().expect("SID sendiri harus terbaca");
            assert!(
                sid_eq(&console, &own),
                "SID console harus sama dengan SID user yang menjalankan test"
            );
        }
    }
}
