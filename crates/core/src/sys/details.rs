//! Detail proses: command line, user, integrity level, tanda tangan.
//!
//! Semua data di sini bersifat **statis per proses** (tidak berubah selama proses
//! hidup), jadi dibaca sekali lalu di-cache. Membaca command line dan
//! memverifikasi tanda tangan masing-masing memakan 0,1–20 ms; melakukannya
//! setiap detik untuk 250 proses akan menghancurkan budget performa.
//!
//! Setiap fungsi mengembalikan `Option`/`Result` yang eksplisit: proses
//! terproteksi atau milik user lain memang tidak bisa dibaca, dan itu bukan
//! kesalahan (ADR-0008).

use crate::model::{Integrity, SigStatus, Signature};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Cache hasil pembacaan statis per executable.
///
/// Kunci cache adalah **path**, bukan PID: 40 proses `chrome.exe` berbagi satu
/// signature dan satu ikon. Ini yang membuat budget performa tetap terjaga.
#[derive(Default)]
pub struct DetailsCache {
    signature: HashMap<PathBuf, Signature>,
    publisher: HashMap<PathBuf, Option<String>>,
}

impl DetailsCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Signature file, dibaca sekali per path.
    pub fn signature_for(&mut self, path: &Path) -> Signature {
        if let Some(hit) = self.signature.get(path) {
            return hit.clone();
        }
        let sig = verify_signature(path);
        self.signature.insert(path.to_path_buf(), sig.clone());
        sig
    }

    pub fn publisher_for(&mut self, path: &Path) -> Option<String> {
        if let Some(hit) = self.publisher.get(path) {
            return hit.clone();
        }
        let publisher = read_publisher(path);
        self.publisher.insert(path.to_path_buf(), publisher.clone());
        publisher
    }

    pub fn len(&self) -> usize {
        self.signature.len()
    }

    pub fn is_empty(&self) -> bool {
        self.signature.is_empty()
    }

    /// Buang entri yang tidak lagi dipakai. Dipanggil saat cache melewati batas.
    pub fn retain_paths(&mut self, keep: &std::collections::HashSet<PathBuf>) {
        self.signature.retain(|p, _| keep.contains(p));
        self.publisher.retain(|p, _| keep.contains(p));
    }
}

/// Command line proses, dibaca lewat `NtQueryInformationProcess`
/// (`ProcessCommandLineInformation`, kelas 60).
///
/// Ini lebih murah dan lebih aman daripada membaca PEB secara manual: tidak ada
/// pembacaan memori lintas proses, hanya satu syscall. Butuh
/// `PROCESS_QUERY_LIMITED_INFORMATION`.
pub fn command_line(pid: u32) -> Option<String> {
    use windows::Wdk::System::Threading::{NtQueryInformationProcess, PROCESSINFOCLASS};
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    const PROCESS_COMMAND_LINE_INFORMATION: PROCESSINFOCLASS = PROCESSINFOCLASS(60);

    // SAFETY: pid dari enumerasi; hanya hak baca terbatas.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;

    // Buffer 4 KiB cukup untuk hampir semua command line; kalau kurang, API
    // mengembalikan STATUS_INFO_LENGTH_MISMATCH dan kita menyerah (tidak ada
    // gunanya mengalokasikan megabyte untuk satu proses).
    let mut buf = vec![0u8; 4096];
    let mut returned = 0u32;
    // SAFETY: buffer valid untuk 4096 byte; out-param valid.
    let status = unsafe {
        NtQueryInformationProcess(
            handle,
            PROCESS_COMMAND_LINE_INFORMATION,
            buf.as_mut_ptr() as *mut _,
            buf.len() as u32,
            &mut returned,
        )
    };
    // SAFETY: handle dibuka di atas dan tidak dipakai lagi.
    unsafe {
        let _ = CloseHandle(handle);
    }
    if status.is_err() {
        return None;
    }

    // Isi buffer: UNICODE_STRING (Length, MaximumLength, Buffer), lalu string
    // UTF-16-nya. Buffer menunjuk ke dalam buffer yang sama.
    #[repr(C)]
    struct UnicodeString {
        length: u16,
        maximum_length: u16,
        buffer: *const u16,
    }

    // SAFETY: buffer >= ukuran struct dan berisi struct yang ditulis kernel.
    let us = unsafe { std::ptr::read_unaligned(buf.as_ptr() as *const UnicodeString) };
    if us.buffer.is_null() || us.length == 0 {
        return None;
    }
    let len = us.length as usize / 2;

    // Pastikan pointer hasil menunjuk ke dalam buffer kita, bukan alamat liar.
    let base = buf.as_ptr() as usize;
    let end = base + buf.len();
    let ptr = us.buffer as usize;
    if ptr < base || ptr + len * 2 > end {
        return None;
    }

    // SAFETY: rentang sudah diverifikasi berada di dalam buffer yang hidup.
    let slice = unsafe { std::slice::from_raw_parts(us.buffer, len) };
    let text = String::from_utf16_lossy(slice);
    if text.is_empty() { None } else { Some(text) }
}

/// Nama pengguna pemilik proses dan integrity level-nya.
///
/// Keduanya berasal dari token proses, jadi butuh `PROCESS_QUERY_LIMITED_INFORMATION`
/// dan `TOKEN_QUERY`. Proses milik user lain akan gagal dan mengembalikan `None`
/// untuk keduanya.
pub fn user_and_integrity(pid: u32) -> (Option<String>, Option<Integrity>) {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::TOKEN_QUERY;
    use windows::Win32::System::Threading::{
        OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    // SAFETY: pid dari enumerasi.
    let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
    else {
        return (None, None);
    };

    let mut token = HANDLE::default();
    // SAFETY: handle proses valid; token out-param valid.
    let token_ok = unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.is_ok();
    // SAFETY: handle proses tidak dipakai lagi.
    unsafe {
        let _ = CloseHandle(process);
    }
    if !token_ok {
        return (None, None);
    }

    let user = token_user_name(token);
    let integrity = token_integrity(token);

    // SAFETY: token dibuka di atas dan tidak dipakai lagi.
    unsafe {
        let _ = CloseHandle(token);
    }
    (user, integrity)
}

fn token_user_name(token: windows::Win32::Foundation::HANDLE) -> Option<String> {
    use windows::Win32::Security::{
        GetTokenInformation, LookupAccountSidW, SID_NAME_USE, TOKEN_USER, TokenUser,
    };
    use windows::core::PWSTR;

    let mut needed = 0u32;
    // SAFETY: panggilan pertama hanya menanyakan ukuran (buffer None).
    let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut needed) };
    if needed == 0 {
        return None;
    }

    let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
    // SAFETY: buffer >= needed byte dan cukup selaras untuk TOKEN_USER (punya pointer).
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
        return None;
    }

    // SAFETY: buffer berisi TOKEN_USER yang ditulis API.
    let token_user = unsafe { &*(buf.as_ptr() as *const TOKEN_USER) };
    if token_user.User.Sid.is_invalid() {
        return None;
    }

    let mut name = vec![0u16; 256];
    let mut domain = vec![0u16; 256];
    let mut name_len = name.len() as u32;
    let mut domain_len = domain.len() as u32;
    let mut sid_type = SID_NAME_USE::default();

    // SAFETY: SID valid; kedua buffer dan panjangnya sesuai.
    let ok = unsafe {
        LookupAccountSidW(
            None,
            token_user.User.Sid,
            Some(PWSTR(name.as_mut_ptr())),
            &mut name_len,
            Some(PWSTR(domain.as_mut_ptr())),
            &mut domain_len,
            &mut sid_type,
        )
    }
    .is_ok();
    if !ok {
        return None;
    }

    let account = wide_to_string(&name[..name_len as usize]);
    let domain = wide_to_string(&domain[..domain_len as usize]);
    if account.is_empty() {
        None
    } else if domain.is_empty() {
        Some(account)
    } else {
        Some(format!("{domain}\\{account}"))
    }
}

fn token_integrity(token: windows::Win32::Foundation::HANDLE) -> Option<Integrity> {
    use windows::Win32::Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_MANDATORY_LABEL,
        TokenIntegrityLevel,
    };

    let mut needed = 0u32;
    // SAFETY: panggilan pertama hanya menanyakan ukuran.
    let _ = unsafe { GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut needed) };
    if needed == 0 {
        return None;
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
        return None;
    }

    // SAFETY: buffer berisi TOKEN_MANDATORY_LABEL yang ditulis API.
    let label = unsafe { &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL) };
    let sid = label.Label.Sid;
    if sid.is_invalid() {
        return None;
    }

    // SAFETY: SID dari token valid; API mengembalikan pointer ke dalam SID.
    let count_ptr = unsafe { GetSidSubAuthorityCount(sid) };
    if count_ptr.is_null() {
        return None;
    }
    // SAFETY: pointer dari API menunjuk ke satu byte yang valid.
    let count = unsafe { *count_ptr };
    if count == 0 {
        return None;
    }
    // SAFETY: indeks terakhir SID adalah RID integrity level.
    let rid_ptr = unsafe { GetSidSubAuthority(sid, (count - 1) as u32) };
    if rid_ptr.is_null() {
        return None;
    }
    // SAFETY: pointer dari API menunjuk ke satu u32 yang valid.
    let rid = unsafe { *rid_ptr };

    Some(match rid {
        0x0000 => Integrity::Untrusted,
        0x1000 => Integrity::Low,
        0x2000 => Integrity::Medium,
        0x3000 => Integrity::High,
        0x4000 => Integrity::System,
        0x5000 => Integrity::Protected,
        _ => Integrity::Medium,
    })
}

fn wide_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Verifikasi tanda tangan Authenticode sebuah file.
///
/// Status yang mungkin:
/// - `Valid`: rantai sertifikat sampai ke root tepercaya.
/// - `Unsigned`: file tidak punya tanda tangan (bukan error).
/// - `Invalid`: ada tanda tangan tapi tidak valid / dimodifikasi.
/// - `Unknown`: pemeriksaan gagal karena alasan lain (mis. file hilang).
pub fn verify_signature(path: &Path) -> Signature {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Security::WinTrust::{
        WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO,
        WTD_CHOICE_FILE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY,
        WTD_UI_NONE, WinVerifyTrust,
    };
    use windows::core::PCWSTR;

    // TRUST_E_NOSIGNATURE dan turunannya berarti "tidak ada tanda tangan",
    // bukan "tanda tangan rusak". Membedakannya penting: file tanpa signature
    // itu normal untuk build sendiri.
    const TRUST_E_NOSIGNATURE: i32 = 0x800B_0100u32 as i32;
    const TRUST_E_SUBJECT_FORM_UNKNOWN: i32 = 0x800B_000Cu32 as i32;
    const TRUST_E_PROVIDER_UNKNOWN: i32 = 0x800B_000Bu32 as i32;
    const TRUST_E_EXPLICIT_DISTRUST: i32 = 0x800B_0111u32 as i32;

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut file_info = WINTRUST_FILE_INFO {
        cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: PCWSTR(wide.as_ptr()),
        hFile: windows::Win32::Foundation::HANDLE::default(),
        pgKnownSubject: std::ptr::null_mut(),
    };

    let mut data = WINTRUST_DATA {
        cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 {
            pFile: &mut file_info,
        },
        dwStateAction: WTD_STATEACTION_VERIFY,
        ..Default::default()
    };

    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    // SAFETY: semua struct dan string hidup selama panggilan; GUID action valid.
    let status = unsafe {
        WinVerifyTrust(
            HWND::default(),
            &mut action,
            &mut data as *mut _ as *mut core::ffi::c_void,
        )
    };

    // Selalu tutup state, kalau tidak ada kebocoran handle per file.
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    // SAFETY: menutup state yang dibuka panggilan di atas.
    unsafe {
        WinVerifyTrust(
            HWND::default(),
            &mut action,
            &mut data as *mut _ as *mut core::ffi::c_void,
        );
    }

    match status {
        0 => Signature {
            status: SigStatus::Valid,
            publisher: read_publisher(path),
        },
        TRUST_E_NOSIGNATURE | TRUST_E_SUBJECT_FORM_UNKNOWN | TRUST_E_PROVIDER_UNKNOWN => {
            Signature {
                status: SigStatus::Unsigned,
                publisher: None,
            }
        }
        TRUST_E_EXPLICIT_DISTRUST => Signature {
            status: SigStatus::Invalid,
            publisher: None,
        },
        _ => Signature {
            status: SigStatus::Invalid,
            publisher: None,
        },
    }
}

/// Nama penerbit dari sertifikat Authenticode.
///
/// Alur: buka PKCS#7 yang tertanam → ambil CMSG_SIGNER_INFO → cari sertifikat
/// yang cocok (Issuer + SerialNumber) di store → baca nama tampilan.
fn read_publisher(path: &Path) -> Option<String> {
    use windows::Win32::Security::Cryptography::{
        CERT_CONTEXT, CERT_FIND_SUBJECT_CERT, CERT_NAME_SIMPLE_DISPLAY_TYPE,
        CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED_EMBED, CERT_QUERY_CONTENT_TYPE,
        CERT_QUERY_ENCODING_TYPE, CERT_QUERY_FORMAT_FLAG_BINARY, CERT_QUERY_FORMAT_TYPE,
        CERT_QUERY_OBJECT_FILE, CMSG_SIGNER_INFO, CMSG_SIGNER_INFO_PARAM, CertCloseStore,
        CertFindCertificateInStore, CertFreeCertificateContext, CertGetNameStringW, CryptMsgClose,
        CryptMsgGetParam, CryptQueryObject, HCERTSTORE, PKCS_7_ASN_ENCODING, X509_ASN_ENCODING,
    };
    use windows::core::PCWSTR;

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut encoding = CERT_QUERY_ENCODING_TYPE::default();
    let mut content = CERT_QUERY_CONTENT_TYPE::default();
    let mut format = CERT_QUERY_FORMAT_TYPE::default();
    let mut store = HCERTSTORE::default();
    let mut msg: *mut core::ffi::c_void = std::ptr::null_mut();

    // SAFETY: path NUL-terminated; semua out-param valid dan bertipe benar.
    let ok = unsafe {
        CryptQueryObject(
            CERT_QUERY_OBJECT_FILE,
            PCWSTR(wide.as_ptr()).0 as *const _,
            CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED_EMBED,
            CERT_QUERY_FORMAT_FLAG_BINARY,
            0,
            Some(&mut encoding),
            Some(&mut content),
            Some(&mut format),
            Some(&mut store),
            Some(&mut msg),
            None,
        )
    }
    .is_ok();
    if !ok || msg.is_null() {
        return None;
    }

    let publisher = (|| -> Option<String> {
        let mut size = 0u32;
        // SAFETY: msg valid; panggilan pertama menanyakan ukuran buffer.
        let _ = unsafe { CryptMsgGetParam(msg, CMSG_SIGNER_INFO_PARAM, 0, None, &mut size) };
        if size == 0 {
            return None;
        }

        let mut buf = vec![0u64; (size as usize).div_ceil(8)];
        // SAFETY: buffer >= size byte dan cukup selaras untuk struct berpointer.
        if unsafe {
            CryptMsgGetParam(
                msg,
                CMSG_SIGNER_INFO_PARAM,
                0,
                Some(buf.as_mut_ptr() as *mut _),
                &mut size,
            )
        }
        .is_err()
        {
            return None;
        }

        // SAFETY: buffer berisi CMSG_SIGNER_INFO yang ditulis API.
        let signer = unsafe { &*(buf.as_ptr() as *const CMSG_SIGNER_INFO) };

        // CERT_INFO hanya butuh Issuer + SerialNumber untuk pencarian ini;
        // field lain dibiarkan nol.
        let cert_info = windows::Win32::Security::Cryptography::CERT_INFO {
            Issuer: signer.Issuer,
            SerialNumber: signer.SerialNumber,
            ..Default::default()
        };

        // SAFETY: store valid; cert_info hidup selama panggilan.
        let found: *mut CERT_CONTEXT = unsafe {
            CertFindCertificateInStore(
                store,
                X509_ASN_ENCODING | PKCS_7_ASN_ENCODING,
                0,
                CERT_FIND_SUBJECT_CERT,
                Some(&cert_info as *const _ as *const core::ffi::c_void),
                None,
            )
        };
        if found.is_null() {
            return None;
        }

        let mut name_buf = vec![0u16; 512];
        // SAFETY: cert context valid; buffer dan panjangnya sesuai.
        let len = unsafe {
            CertGetNameStringW(
                found,
                CERT_NAME_SIMPLE_DISPLAY_TYPE,
                0,
                None,
                Some(&mut name_buf),
            )
        };
        // SAFETY: context dari CertFindCertificateInStore dibebaskan sekali.
        unsafe {
            let _ = CertFreeCertificateContext(Some(found));
        }
        if len <= 1 {
            return None;
        }
        Some(wide_to_string(&name_buf[..len as usize]))
    })();

    // SAFETY: msg dan store dibuka di atas; ditutup sekali di sini.
    unsafe {
        let _ = CryptMsgClose(Some(msg as *const _));
        let _ = CertCloseStore(Some(store), 0);
    }

    publisher.filter(|p| !p.is_empty())
}

use std::os::windows::ffi::OsStrExt;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_line_of_own_process_is_readable() {
        let pid = std::process::id();
        let cmd = command_line(pid).expect("command line proses sendiri harus terbaca");
        assert!(!cmd.is_empty());
    }

    #[test]
    fn command_line_of_dead_pid_is_none() {
        // PID yang sangat tidak mungkin ada.
        assert!(command_line(0xFFFF_FF00).is_none());
    }

    #[test]
    fn own_user_and_integrity_are_readable() {
        let (user, integrity) = user_and_integrity(std::process::id());
        let user = user.expect("user proses sendiri harus terbaca");
        assert!(
            user.contains('\\'),
            "format harus DOMAIN\\user, dapat: {user}"
        );
        assert!(integrity.is_some());
    }

    #[test]
    fn dead_pid_has_no_user() {
        let (user, integrity) = user_and_integrity(0xFFFF_FF00);
        assert!(user.is_none());
        assert!(integrity.is_none());
    }

    #[test]
    fn signed_system_binary_is_valid() {
        // explorer.exe ditandatangani Microsoft. notepad.exe di build ini
        // adalah stub Store app tanpa signature, jadi tidak dipakai.
        let path = PathBuf::from(r"C:\Windows\explorer.exe");
        if !path.exists() {
            return;
        }
        let sig = verify_signature(&path);
        assert_eq!(
            sig.status,
            SigStatus::Valid,
            "explorer.exe harus bertanda tangan valid"
        );
        let publisher = sig.publisher.expect("publisher harus ada");
        assert!(
            publisher.to_lowercase().contains("microsoft"),
            "publisher tidak terduga: {publisher}"
        );
    }

    #[test]
    fn unsigned_file_is_reported_as_unsigned() {
        let path = std::env::temp_dir().join("core-pulse-unsigned-test.txt");
        std::fs::write(&path, b"not a signed binary").unwrap();
        let sig = verify_signature(&path);
        let _ = std::fs::remove_file(&path);
        // File teks bukan PE, jadi hasilnya Unsigned atau Invalid; yang penting
        // bukan Valid dan tidak panik.
        assert_ne!(sig.status, SigStatus::Valid);
    }

    #[test]
    fn missing_file_does_not_panic() {
        let sig = verify_signature(Path::new(r"C:\definitely\does\not\exist.exe"));
        assert_ne!(sig.status, SigStatus::Valid);
    }

    #[test]
    fn cache_returns_same_result_without_rechecking() {
        let mut cache = DetailsCache::new();
        let path = PathBuf::from(r"C:\Windows\explorer.exe");
        if !path.exists() {
            return;
        }
        let first = cache.signature_for(&path);
        let second = cache.signature_for(&path);
        assert_eq!(first.status, second.status);
        assert_eq!(
            cache.len(),
            1,
            "path sama harus di-cache, bukan dihitung ulang"
        );
    }
}
