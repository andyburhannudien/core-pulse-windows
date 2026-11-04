//! Enumerasi Windows Service lewat Service Control Manager.
//!
//! Membaca service **tidak** butuh hak administrator. Menjalankan atau
//! menghentikannya butuh, dan itu ditandai di UI sebagai tombol nonaktif.

use crate::model::{Degradation, DegradeReason, ServiceRow, ServiceRunState, ServiceStartType};

/// Baca semua service (Win32 dan driver).
pub fn collect() -> (Vec<ServiceRow>, Vec<Degradation>) {
    use windows::Win32::System::Services::{
        CloseServiceHandle, ENUM_SERVICE_STATUS_PROCESSW, ENUM_SERVICE_TYPE, EnumServicesStatusExW,
        OpenSCManagerW, SC_ENUM_PROCESS_INFO, SC_MANAGER_ENUMERATE_SERVICE, SERVICE_ACTIVE,
        SERVICE_INACTIVE, SERVICE_STATE_ALL, SERVICE_WIN32,
    };

    let mut degraded = Vec::new();
    let mut out = Vec::new();

    // SAFETY: tidak ada nama mesin/database (lokal), hanya hak enumerasi.
    let scm = unsafe {
        OpenSCManagerW(
            windows::core::PCWSTR::null(),
            windows::core::PCWSTR::null(),
            SC_MANAGER_ENUMERATE_SERVICE,
        )
    };
    let Ok(scm) = scm else {
        degraded.push(Degradation {
            area: crate::model::Area::Services,
            reason: DegradeReason::ApiFailed,
            detail: "OpenSCManagerW gagal".into(),
        });
        return (out, degraded);
    };

    let service_type =
        ENUM_SERVICE_TYPE(SERVICE_WIN32.0 | windows::Win32::System::Services::SERVICE_DRIVER.0);
    let _ = SERVICE_ACTIVE;
    let _ = SERVICE_INACTIVE;

    let mut resume = 0u32;
    // Buffer awal 128 KiB; tumbuh kalau API minta lebih. Dialokasikan sebagai
    // u64 supaya selaras untuk struct yang berisi pointer.
    let mut bytes_capacity = 128 * 1024usize;

    for _ in 0..8 {
        let mut buf = vec![0u64; bytes_capacity.div_ceil(8)];
        let mut returned = 0u32;
        let mut needed = 0u32;

        // SAFETY: buffer valid sebesar `buf.len() * 8` byte dan cukup selaras;
        // semua out-param valid.
        let res = unsafe {
            EnumServicesStatusExW(
                scm,
                SC_ENUM_PROCESS_INFO,
                service_type,
                SERVICE_STATE_ALL,
                Some(std::slice::from_raw_parts_mut(
                    buf.as_mut_ptr() as *mut u8,
                    buf.len() * 8,
                )),
                &mut needed,
                &mut returned,
                Some(&mut resume),
                windows::core::PCWSTR::null(),
            )
        };

        // ERROR_MORE_DATA (234) berarti "buffer penuh, masih ada lagi" — bukan
        // kegagalan. Entri yang sudah ditulis tetap valid dan harus diproses.
        let more_data = res
            .as_ref()
            .err()
            .map(|e| e.code().0 as u32 == 234)
            .unwrap_or(false);

        let stride = std::mem::size_of::<ENUM_SERVICE_STATUS_PROCESSW>();
        for i in 0..returned as usize {
            // SAFETY: `returned` entri ditulis API ke dalam buffer, dan buffer
            // cukup besar untuk jumlah itu.
            let item = unsafe {
                std::ptr::read_unaligned(
                    buf.as_ptr().add(i * stride / 8) as *const ENUM_SERVICE_STATUS_PROCESSW
                )
            };

            let name = pwstr_to_string(item.lpServiceName);
            let display = pwstr_to_string(item.lpDisplayName);
            let status = item.ServiceStatusProcess;

            out.push(ServiceRow {
                name: if name.is_empty() {
                    display.clone()
                } else {
                    name
                },
                display_name: display,
                state: ServiceRunState::from_raw(status.dwCurrentState.0),
                start_type: read_start_type(item.lpServiceName),
                pid: status.dwProcessId,
                accepts_stop: status.dwControlsAccepted & 0x0000_0001 != 0,
            });
        }

        if more_data {
            if needed > bytes_capacity as u32 {
                bytes_capacity = needed as usize;
            }
            // resume == 0 dengan ERROR_MORE_DATA tidak seharusnya terjadi;
            // berhenti supaya tidak loop selamanya.
            if resume == 0 {
                break;
            }
            continue;
        }

        if res.is_err() && returned == 0 {
            degraded.push(Degradation {
                area: crate::model::Area::Services,
                reason: DegradeReason::ApiFailed,
                detail: format!("EnumServicesStatusExW: {res:?}"),
            });
        }
        break;
    }

    // SAFETY: scm dibuka di atas dan tidak dipakai lagi.
    unsafe {
        let _ = CloseServiceHandle(scm);
    }

    out.sort_by_key(|s| s.name.to_lowercase());
    (out, degraded)
}

fn pwstr_to_string(p: windows::core::PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: pointer dari API menunjuk ke string UTF-16 NUL-terminated yang
    // hidup selama buffer hasil enumerasi masih ada (kita menyalinnya sekarang).
    unsafe { p.to_string() }.unwrap_or_default()
}

/// Start type tidak ada di `SERVICE_STATUS_PROCESS`. Membaca registry
/// (`HKLM\SYSTEM\CurrentControlSet\Services\<nama>\Start`) jauh lebih murah
/// daripada `OpenServiceW` + `QueryServiceConfigW` per service: ~15 us vs
/// ~200 us, dan ada 700+ service di mesin ini.
fn read_start_type(name: windows::core::PWSTR) -> ServiceStartType {
    if name.is_null() {
        return ServiceStartType::Unknown;
    }
    // SAFETY: pointer dari API menunjuk ke string UTF-16 NUL-terminated yang
    // hidup selama buffer enumerasi masih ada.
    let Ok(service_name) = (unsafe { name.to_string() }) else {
        return ServiceStartType::Unknown;
    };
    let key = format!(r"SYSTEM\CurrentControlSet\Services\{service_name}");
    let Some(start) = crate::sys::host::read_registry_dword(&key, "Start") else {
        return ServiceStartType::Unknown;
    };
    // `DelayedAutostart` hanya relevan untuk Start=2.
    if start == 2 && crate::sys::host::read_registry_dword(&key, "DelayedAutostart") == Some(1) {
        return ServiceStartType::AutoDelayed;
    }
    ServiceStartType::from_raw(start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn services_are_enumerated() {
        let (services, degraded) = collect();
        assert!(
            services.len() > 10,
            "harus ada banyak service, dapat {}",
            services.len()
        );
        assert!(
            degraded.is_empty(),
            "enumerasi service tidak boleh terdegradasi"
        );
    }

    #[test]
    fn service_names_are_unique_and_nonempty() {
        let (services, _) = collect();
        let mut names: Vec<_> = services.iter().map(|s| s.name.clone()).collect();
        assert!(services.iter().all(|s| !s.name.is_empty()));
        names.sort();
        names.dedup();
        assert_eq!(names.len(), services.len(), "nama service duplikat");
    }

    #[test]
    fn known_service_has_sane_fields() {
        let (services, _) = collect();
        // "EventLog" ada di semua instalasi Windows.
        let ev = services
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case("eventlog"))
            .expect("service EventLog harus ada");
        assert!(!ev.display_name.is_empty());
        assert_ne!(ev.start_type, ServiceStartType::Unknown);
    }
}
