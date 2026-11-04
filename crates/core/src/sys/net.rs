//! Network per adapter.
//!
//! Identitas dan counter kumulatif dari `GetIfTable2` (satu panggilan, tanpa
//! PDH), rate dihitung dari delta antar sampel. Ini lebih murah dan lebih tepat
//! daripada mencocokkan nama instance PDH `Network Interface`, yang berbeda
//! antar driver.

use crate::model::{Degradation, DegradeReason, NetData};
use std::collections::HashMap;

/// Collector stateful: rate butuh pembacaan sebelumnya.
#[derive(Default)]
pub struct NetCollector {
    prev: HashMap<u64, (u64, u64)>,
}

impl NetCollector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn collect(&mut self, elapsed_secs: f64) -> (Vec<NetData>, Vec<Degradation>) {
        use windows::Win32::NetworkManagement::IpHelper::{
            FreeMibTable, GetIfTable2, MIB_IF_TABLE2,
        };
        use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;

        let mut degraded = Vec::new();
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        // SAFETY: out-param valid; tabel dibebaskan dengan FreeMibTable di bawah.
        let rc = unsafe { GetIfTable2(&mut table) };
        if rc.is_err() || table.is_null() {
            degraded.push(Degradation {
                area: crate::model::Area::Net,
                reason: DegradeReason::ApiFailed,
                detail: format!("GetIfTable2 gagal: {rc:?}"),
            });
            return (Vec::new(), degraded);
        }

        let elapsed = if elapsed_secs <= 0.0 {
            1.0
        } else {
            elapsed_secs
        };
        let mut next = HashMap::new();
        let mut out = Vec::new();

        // SAFETY: `table` valid dan berisi `NumEntries` baris.
        let count = unsafe { (*table).NumEntries };
        for i in 0..count as usize {
            // SAFETY: indeks < NumEntries, jadi masih di dalam alokasi tabel.
            let row = unsafe { &(*table).Table.as_ptr().add(i).read() };

            // 24 = IF_TYPE_SOFTWARE_LOOPBACK; tunnel dilewati karena rate-nya
            // sudah terhitung pada adapter fisik di bawahnya.
            if row.Type == 24 || row.TunnelType.0 != 0 {
                continue;
            }
            // Tanpa filter ini, Windows melaporkan puluhan pseudo-adapter
            // (WFP filter, Hyper-V, dsb). Task Manager hanya menampilkan adapter
            // hardware dan yang benar-benar aktif.
            let flags = row.InterfaceAndOperStatusFlags._bitfield;
            let is_hardware = flags & 0b0000_0001 != 0;
            let is_filter = flags & 0b0000_0010 != 0;
            let is_up = row.OperStatus == IfOperStatusUp;
            if is_filter || (!is_hardware && !is_up) {
                continue;
            }
            let name = from_wide(&row.Alias);
            if name.is_empty() {
                continue;
            }
            let description = from_wide(&row.Description);
            // Adapter virtual (Wi-Fi Direct, Hyper-V vSwitch) terlihat sebagai
            // hardware tetapi tidak ditampilkan Task Manager dan hanya membuat
            // daftar penuh noise.
            if name.starts_with("Local Area Connection*")
                || description.to_lowercase().contains("virtual")
            {
                continue;
            }

            let luid = luid_value(&row.InterfaceLuid);
            let (prev_in, prev_out) = self.prev.get(&luid).copied().unwrap_or((0, 0));
            let rx = row.InOctets.saturating_sub(prev_in);
            let tx = row.OutOctets.saturating_sub(prev_out);
            next.insert(luid, (row.InOctets, row.OutOctets));

            let has_prev = self.prev.contains_key(&luid);
            out.push(NetData {
                name,
                description,
                mac: format_mac(&row.PhysicalAddress[..row.PhysicalAddressLength.min(32) as usize]),
                link_speed_bps: row.TransmitLinkSpeed.max(row.ReceiveLinkSpeed),
                rx_bps: if has_prev {
                    (rx as f64 / elapsed) as u64
                } else {
                    0
                },
                tx_bps: if has_prev {
                    (tx as f64 / elapsed) as u64
                } else {
                    0
                },
                is_up,
                ipv4: None,
            });
        }

        // SAFETY: tabel dialokasikan oleh GetIfTable2 dan hanya dibebaskan sekali.
        unsafe { FreeMibTable(table as *const _) };

        self.prev = next;

        if out.is_empty() {
            degraded.push(Degradation {
                area: crate::model::Area::Net,
                reason: DegradeReason::NotAvailable,
                detail: "tidak ada adapter jaringan non-loopback".into(),
            });
        }

        (out, degraded)
    }
}

fn luid_value(luid: &windows::Win32::NetworkManagement::Ndis::NET_LUID_LH) -> u64 {
    // SAFETY: NET_LUID_LH adalah union POD; Value adalah representasi 64-bit.
    unsafe { luid.Value }
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end]).trim().to_string()
}

fn format_mac(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_mac_with_dashes() {
        assert_eq!(format_mac(&[0x00, 0x1A, 0x2B, 0xFF]), "00-1A-2B-FF");
    }

    #[test]
    fn empty_mac_is_empty_string() {
        assert_eq!(format_mac(&[]), "");
    }

    #[test]
    fn wide_string_is_trimmed_at_nul() {
        let buf = [b'E' as u16, b't' as u16, b'h' as u16, 0, b'x' as u16];
        assert_eq!(from_wide(&buf), "Eth");
    }

    #[test]
    fn adapters_are_enumerated() {
        let mut c = NetCollector::new();
        let (first, _) = c.collect(1.0);
        assert!(!first.is_empty(), "harus ada minimal satu adapter");
        // Sampel pertama belum punya rate.
        assert!(first.iter().all(|a| a.rx_bps == 0 && a.tx_bps == 0));
        let (second, _) = c.collect(1.0);
        assert_eq!(second.len(), first.len());
    }
}
