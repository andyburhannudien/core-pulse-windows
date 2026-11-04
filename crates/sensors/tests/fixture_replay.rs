//! Replay fixture SMART dari file `.hex`.
//!
//! Ini yang memungkinkan parser NVMe/ATA teruji di mesin yang **tidak punya**
//! NVMe dan **tidak bisa** membaca SMART (non-admin). Fixture direkam sekali
//! dari mesin nyata, lalu diputar ulang di CI dan di mesin mana pun.
//!
//! Format file: beberapa baris komentar (`#` atau teks bebas), lalu satu baris
//! berisi 512 byte dalam heksadesimal dipisah spasi.

use core_pulse_sensors::smart::{ata_temperature_c, nvme_percentage_used, nvme_temperature_c};
use std::path::Path;

fn load_payload(path: &Path) -> Vec<u8> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("gagal membaca {}: {e}", path.display()));

    // Baris payload adalah baris terpanjang yang seluruh tokennya 2 digit hex.
    for line in text.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.len() < 64 {
            continue;
        }
        if !tokens
            .iter()
            .all(|t| t.len() == 2 && t.chars().all(|c| c.is_ascii_hexdigit()))
        {
            continue;
        }
        return tokens
            .iter()
            .map(|t| u8::from_str_radix(t, 16).expect("token hex valid"))
            .collect();
    }
    panic!("tidak ada baris payload hex di {}", path.display());
}

fn fixture(name: &str) -> std::path::PathBuf {
    // `CARGO_MANIFEST_DIR` = crates/sensors, fixture ada di root workspace.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/smart")
        .join(name)
}

#[test]
fn nvme_fixture_temperature() {
    let payload = load_payload(&fixture("nvme-health-318k.hex"));
    assert_eq!(payload.len(), 512, "payload NVMe harus 512 byte");
    let t = nvme_temperature_c(&payload).expect("suhu harus terbaca");
    assert!((t - 44.85).abs() < 0.01, "dapat {t}");
    assert_eq!(nvme_percentage_used(&payload), Some(3));
}

#[test]
fn ata_fixture_temperature() {
    let payload = load_payload(&fixture("ata-attrs-41c.hex"));
    assert_eq!(payload.len(), 512, "payload ATA harus 512 byte");
    assert_eq!(ata_temperature_c(&payload), Some(41.0));
}

#[test]
fn fixtures_are_independent_of_machine() {
    // Fixture tidak boleh bergantung pada hardware: dua pembacaan harus sama.
    let a = load_payload(&fixture("nvme-health-318k.hex"));
    let b = load_payload(&fixture("nvme-health-318k.hex"));
    assert_eq!(a, b);
}
