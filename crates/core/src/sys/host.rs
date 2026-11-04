//! Informasi host: nama mesin, OS, uptime, hak akses.

use crate::model::{Degradation, HostInfo};

/// Baca informasi host yang bersifat statis (berubah sangat jarang).
pub fn collect() -> (HostInfo, Vec<Degradation>) {
    let mut degraded = Vec::new();

    let computer_name = std::env::var("COMPUTERNAME").unwrap_or_default();
    let os_name = os_name();
    let os_build = os_build();
    let (cpu_brand, physical_cores, logical_cores, base_clock_mhz) =
        crate::sys::cpu::brand_and_topology();
    let total_ram_bytes = crate::sys::mem::total_physical_bytes();

    if cpu_brand.is_empty() {
        degraded.push(Degradation {
            area: crate::model::Area::Host,
            reason: crate::model::DegradeReason::ApiFailed,
            detail: "CPU name could not be read from the registry".into(),
        });
    }

    (
        HostInfo {
            computer_name,
            os_name,
            os_build,
            cpu_brand,
            physical_cores,
            logical_cores,
            base_clock_mhz,
            total_ram_bytes,
            uptime_secs: uptime_secs(),
            is_elevated: is_elevated(),
        },
        degraded,
    )
}

/// Uptime sistem dalam detik, dari `GetTickCount64`.
pub fn uptime_secs() -> u64 {
    use windows::Win32::System::SystemInformation::GetTickCount64;
    // SAFETY: fungsi murni tanpa parameter.
    unsafe { GetTickCount64() / 1000 }
}

/// Apakah proses ini berjalan dengan token elevated.
pub fn is_elevated() -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token = HANDLE::default();
    // SAFETY: handle pseudo-proses selalu valid; `token` out-param valid.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
        return false;
    }

    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0u32;
    // SAFETY: buffer sesuai tipe yang diminta (TokenElevation) dan ukurannya benar.
    let ok = unsafe {
        GetTokenInformation(
            token,
            windows::Win32::Security::TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    }
    .is_ok();
    // SAFETY: token dibuka di atas dan tidak dipakai lagi.
    unsafe {
        let _ = CloseHandle(token);
    }
    ok && elevation.TokenIsElevated != 0
}

fn os_name() -> String {
    let raw = read_registry_string_pub(
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "ProductName",
    )
    .unwrap_or_else(|| "Windows".to_string());
    // Registry masih melaporkan "Windows 10 ..." di Windows 11; build number
    // adalah sumber kebenaran.
    let build = os_build();
    if build >= 22000 && raw.starts_with("Windows 10") {
        raw.replacen("Windows 10", "Windows 11", 1)
    } else {
        raw
    }
}

fn os_build() -> u32 {
    read_registry_string_pub(
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "CurrentBuildNumber",
    )
    .and_then(|s| s.parse().ok())
    .unwrap_or(0)
}

/// Baca nilai string dari HKLM. Dipakai untuk data statis saja (bukan di loop).
pub fn read_registry_string_pub(subkey: &str, value: &str) -> Option<String> {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        HKEY, HKEY_LOCAL_MACHINE, KEY_READ, REG_SZ, RegCloseKey, RegOpenKeyExW, RegQueryValueExW,
    };
    use windows::core::PCWSTR;

    let subkey_w: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
    let value_w: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();

    let mut hkey = HKEY::default();
    // SAFETY: kedua string NUL-terminated dan hidup selama panggilan; `hkey`
    // adalah out-param yang valid.
    let rc = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(subkey_w.as_ptr()),
            None,
            KEY_READ,
            &mut hkey,
        )
    };
    if rc != ERROR_SUCCESS {
        return None;
    }

    let mut buf = vec![0u8; 1024];
    let mut size = buf.len() as u32;
    let mut kind = REG_SZ;
    // SAFETY: buffer valid untuk `size` byte; `kind` dan `size` out-param valid.
    let rc = unsafe {
        RegQueryValueExW(
            hkey,
            PCWSTR(value_w.as_ptr()),
            None,
            Some(&mut kind),
            Some(buf.as_mut_ptr()),
            Some(&mut size),
        )
    };
    // SAFETY: handle dibuka di atas dan tidak dipakai lagi.
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    if rc != ERROR_SUCCESS {
        return None;
    }

    let wide: Vec<u16> = buf[..size as usize]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&c| c != 0)
        .collect();
    if wide.is_empty() {
        return None;
    }
    Some(String::from_utf16_lossy(&wide))
}

/// Baca nilai DWORD dari HKLM.
pub fn read_registry_dword(subkey: &str, value: &str) -> Option<u32> {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        HKEY, HKEY_LOCAL_MACHINE, KEY_READ, REG_DWORD, RegCloseKey, RegOpenKeyExW, RegQueryValueExW,
    };
    use windows::core::PCWSTR;

    let subkey_w: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
    let value_w: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();

    let mut hkey = HKEY::default();
    // SAFETY: string NUL-terminated, handle out-param valid.
    let rc = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(subkey_w.as_ptr()),
            None,
            KEY_READ,
            &mut hkey,
        )
    };
    if rc != ERROR_SUCCESS {
        return None;
    }

    let mut data = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let mut kind = REG_DWORD;
    // SAFETY: buffer u32 valid dan ukurannya sesuai.
    let rc = unsafe {
        RegQueryValueExW(
            hkey,
            PCWSTR(value_w.as_ptr()),
            None,
            Some(&mut kind),
            Some(&mut data as *mut u32 as *mut u8),
            Some(&mut size),
        )
    };
    // SAFETY: handle tidak dipakai lagi.
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    if rc != ERROR_SUCCESS || kind != REG_DWORD {
        return None;
    }
    Some(data)
}
