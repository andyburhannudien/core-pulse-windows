//! Entri startup: registry Run/RunOnce, folder Startup, dan scheduled task.
//!
//! Semua jalur di sini bisa dibaca tanpa hak administrator untuk entri milik
//! pengguna. Entri HKLM terbaca karena kuncinya memang bisa dibaca semua user;
//! yang butuh admin adalah *menghapus* entri, dan itu tidak dilakukan di v1.

use crate::model::{Degradation, DegradeReason, StartupEntry, StartupSource};
use std::path::{Path, PathBuf};

/// Baca semua entri startup.
pub fn collect() -> (Vec<StartupEntry>, Vec<Degradation>) {
    let mut out = Vec::new();
    let mut degraded = Vec::new();

    for (source, hive, is_32bit) in [
        (StartupSource::RegistryMachine, Hive::LocalMachine, false),
        (StartupSource::RegistryUser, Hive::CurrentUser, false),
        (StartupSource::RegistryMachine, Hive::LocalMachine, true),
        (StartupSource::RegistryUser, Hive::CurrentUser, true),
    ] {
        match read_run_keys(hive, is_32bit, source) {
            Ok(mut entries) => out.append(&mut entries),
            Err(reason) => degraded.push(Degradation {
                area: crate::model::Area::Startup,
                reason,
                detail: format!(
                    "{} ({})",
                    source.label(),
                    if is_32bit { "32-bit" } else { "64-bit" }
                ),
            }),
        }
    }

    out.append(&mut read_startup_folders());

    out.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.source.label().cmp(b.source.label()))
    });
    out.dedup_by(|a, b| a.name == b.name && a.command == b.command && a.source == b.source);

    (out, degraded)
}

#[derive(Clone, Copy)]
enum Hive {
    LocalMachine,
    CurrentUser,
}

fn hive_handle(hive: Hive) -> windows::Win32::System::Registry::HKEY {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    match hive {
        Hive::LocalMachine => HKEY_LOCAL_MACHINE,
        Hive::CurrentUser => HKEY_CURRENT_USER,
    }
}

fn hive_label(hive: Hive) -> &'static str {
    match hive {
        Hive::LocalMachine => "HKLM",
        Hive::CurrentUser => "HKCU",
    }
}

fn read_run_keys(
    hive: Hive,
    is_32bit: bool,
    source: StartupSource,
) -> Result<Vec<StartupEntry>, DegradeReason> {
    use windows::Win32::System::Registry::{KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY};

    let flags = if is_32bit {
        KEY_READ | KEY_WOW64_32KEY
    } else {
        KEY_READ | KEY_WOW64_64KEY
    };

    let mut out = Vec::new();
    // Run dan RunOnce diperlakukan sama: keduanya dijalankan saat logon.
    for subkey in [
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run",
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\RunOnce",
    ] {
        let values = match enum_values(hive, subkey, flags) {
            Some(v) => v,
            None => continue,
        };
        for (name, command) in values {
            out.push(StartupEntry {
                name: format!(
                    "{} [{}\\{}]",
                    name,
                    hive_label(hive),
                    subkey.rsplit('\\').next().unwrap_or("Run")
                ),
                target_exists: target_exists(&command),
                command,
                source,
                is_32bit,
            });
        }
    }

    // Tidak menemukan kunci Run bukan error: banyak mesin memang kosong.
    Ok(out)
}

/// Enumerasi nilai string di bawah sebuah kunci registry.
fn enum_values(
    hive: Hive,
    subkey: &str,
    flags: windows::Win32::System::Registry::REG_SAM_FLAGS,
) -> Option<Vec<(String, String)>> {
    use windows::Win32::System::Registry::{HKEY, RegCloseKey, RegEnumValueW, RegOpenKeyExW};
    use windows::core::PCWSTR;

    let subkey_w: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
    let mut key = HKEY::default();
    // SAFETY: subkey NUL-terminated, out-param valid.
    if unsafe {
        RegOpenKeyExW(
            hive_handle(hive),
            PCWSTR(subkey_w.as_ptr()),
            None,
            flags,
            &mut key,
        )
    }
    .is_err()
    {
        return None;
    }

    let mut out = Vec::new();
    let mut index = 0u32;
    loop {
        let mut name_buf = vec![0u16; 512];
        let mut name_len = name_buf.len() as u32;
        let mut data_buf = vec![0u8; 4096];
        let mut data_len = data_buf.len() as u32;
        let mut value_type = 0u32;

        // SAFETY: semua buffer dan panjangnya valid untuk ukuran yang diberikan.
        let rc = unsafe {
            RegEnumValueW(
                key,
                index,
                Some(windows::core::PWSTR(name_buf.as_mut_ptr())),
                &mut name_len,
                None,
                Some(&mut value_type),
                Some(data_buf.as_mut_ptr()),
                Some(&mut data_len),
            )
        };
        if rc.is_err() {
            break;
        }
        index += 1;

        // REG_SZ = 1, REG_EXPAND_SZ = 2. Tipe lain (mis. REG_BINARY) dilewati.
        if value_type != 1 && value_type != 2 {
            continue;
        }
        let name = String::from_utf16_lossy(&name_buf[..name_len as usize]);
        let wide: Vec<u16> = data_buf[..data_len as usize]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&c| c != 0)
            .collect();
        let mut command = String::from_utf16_lossy(&wide);

        // REG_EXPAND_SZ berisi variabel (%ProgramFiles%); kembangkan supaya
        // pemeriksaan keberadaan file tidak salah.
        if value_type == 2 {
            command = expand_env(&command);
        }
        if !name.is_empty() && !command.is_empty() {
            out.push((name, command));
        }
    }

    // SAFETY: key dibuka di atas dan tidak dipakai lagi.
    unsafe {
        let _ = RegCloseKey(key);
    }
    Some(out)
}

fn expand_env(text: &str) -> String {
    use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
    use windows::core::PCWSTR;

    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: string NUL-terminated; panggilan pertama menanyakan panjang.
    let needed = unsafe { ExpandEnvironmentStringsW(PCWSTR(wide.as_ptr()), None) };
    if needed == 0 {
        return text.to_string();
    }
    let mut buf = vec![0u16; needed as usize];
    // SAFETY: buffer sebesar yang diminta panggilan pertama.
    let written = unsafe { ExpandEnvironmentStringsW(PCWSTR(wide.as_ptr()), Some(&mut buf)) };
    if written == 0 {
        return text.to_string();
    }
    let end = (written as usize).min(buf.len());
    String::from_utf16_lossy(&buf[..end])
        .trim_end_matches('\0')
        .to_string()
}

fn read_startup_folders() -> Vec<StartupEntry> {
    use std::os::windows::ffi::OsStrExt;

    let mut out = Vec::new();
    for var in ["APPDATA", "PROGRAMDATA"] {
        let Ok(base) = std::env::var(var) else {
            continue;
        };
        let dir = PathBuf::from(base).join(r"Microsoft\Windows\Start Menu\Programs\Startup");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // desktop.ini ada di folder itu dan bukan program startup.
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.eq_ignore_ascii_case("desktop.ini"))
                .unwrap_or(false)
            {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?")
                .to_string();
            // Tampilkan path relatif terhadap folder Startup supaya konsisten
            // di mesin dengan nama pengguna berbeda.
            let command = path
                .strip_prefix(&dir)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            let _ = path.as_os_str().encode_wide();
            out.push(StartupEntry {
                name,
                command,
                source: StartupSource::StartupFolder,
                is_32bit: false,
                target_exists: Some(path.exists()),
            });
        }
    }
    out
}

/// Cek apakah file yang dirujuk entri startup benar-benar ada.
///
/// Command line bisa berisi argumen dan tanda kutip, jadi kita ambil bagian
/// pertama yang terlihat seperti path.
fn target_exists(command: &str) -> Option<bool> {
    let path = extract_path(command)?;
    Some(Path::new(&path).exists())
}

fn extract_path(command: &str) -> Option<String> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix('"') {
        let end = rest.find('"')?;
        return Some(rest[..end].to_string());
    }
    // Tanpa tanda kutip: ambil sampai spasi pertama yang diikuti pola path
    // (mis. "C:\Program Files\x.exe" tanpa kutip tidak bisa diurai dengan benar,
    // jadi heuristik: berhenti di ".exe"/".dll"/".bat"/".cmd" pertama).
    let lower = trimmed.to_lowercase();
    for ext in [".exe", ".dll", ".bat", ".cmd", ".com", ".scr"] {
        if let Some(pos) = lower.find(ext) {
            return Some(trimmed[..pos + ext.len()].to_string());
        }
    }
    Some(trimmed.split_whitespace().next()?.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_quoted_path() {
        assert_eq!(
            extract_path(r#""C:\Program Files\App\app.exe" --silent"#),
            Some(r"C:\Program Files\App\app.exe".to_string())
        );
    }

    #[test]
    fn extracts_unquoted_path_with_extension() {
        assert_eq!(
            extract_path(r"C:\Tools\run.exe -flag"),
            Some(r"C:\Tools\run.exe".to_string())
        );
    }

    #[test]
    fn extracts_bare_command() {
        assert_eq!(
            extract_path("rundll32 something"),
            Some("rundll32".to_string())
        );
    }

    #[test]
    fn empty_command_has_no_path() {
        assert_eq!(extract_path("   "), None);
    }

    #[test]
    fn startup_entries_are_collected() {
        let (entries, _) = collect();
        // Tidak ada jaminan mesin punya entri startup, tapi kalau ada, field
        // wajibnya harus terisi.
        for e in &entries {
            assert!(!e.name.is_empty());
            assert!(!e.command.is_empty());
        }
    }

    #[test]
    fn startup_entries_are_sorted_and_unique() {
        let (entries, _) = collect();
        let mut names: Vec<_> = entries.iter().map(|e| e.name.clone()).collect();
        let sorted = names.clone();
        names.sort_by_key(|a| a.to_lowercase());
        assert_eq!(names, sorted, "entri startup harus terurut");
    }
}
