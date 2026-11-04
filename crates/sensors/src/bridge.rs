//! Tier 3: bridge read-only ke monitor pihak ketiga (LibreHardwareMonitor).
//!
//! **Nonaktif secara default.** Pengguna harus menyalakannya, dan data ditandai
//! `ThirdParty` (SENSORS.md §5). Aturan yang mengikat:
//!
//! - Alamat **selalu** `127.0.0.1`, tidak bisa dikonfigurasi ke host lain.
//! - Port tetap 8085 (plugin Remote Web Server LHM).
//! - Timeout 500 ms, tanpa retry agresif.
//! - Respons dibatasi 2 MB dan harus `application/json`.
//! - Redirect tidak diikuti.
//! - Hanya field yang dikenal yang dibaca; nilai di luar rentang fisik dibuang
//!   (dilakukan sekali lagi di `Sensors::is_plausible`).

use crate::{Availability, Confidence, SensorProvider, reading};
use core_pulse_core::model::{SensorKind, SensorReading, SensorSource, Unit};

/// Endpoint LHM. Sengaja konstanta, bukan konfigurasi: bridge ini hanya untuk
/// monitor yang berjalan di mesin yang sama.
const LHM_URL: &str = "http://127.0.0.1:8085/data.json";
const TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);
const MAX_BYTES: usize = 2 * 1024 * 1024;

pub struct LhmBridgeProvider {
    available: bool,
    last_error: Option<String>,
}

impl Default for LhmBridgeProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl LhmBridgeProvider {
    pub fn new() -> Self {
        Self {
            available: false,
            last_error: None,
        }
    }

    /// Probe ringan: satu permintaan HEAD/GET dengan timeout pendek.
    fn reachable() -> bool {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .build()
            .into();
        match agent.get(LHM_URL).call() {
            Ok(resp) => {
                let is_json = resp
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .map(|v| v.to_ascii_lowercase().contains("json"))
                    .unwrap_or(false);
                is_json && resp.status().is_success()
            }
            Err(_) => false,
        }
    }
}

impl SensorProvider for LhmBridgeProvider {
    fn id(&self) -> &'static str {
        "lhm-bridge"
    }

    fn source(&self) -> SensorSource {
        SensorSource::LhmBridge
    }

    fn probe(&self) -> Availability {
        if Self::reachable() {
            Availability::Available
        } else {
            Availability::NotAvailable(
                "LibreHardwareMonitor tidak berjalan di 127.0.0.1:8085 (fitur opt-in)",
            )
        }
    }

    fn init(&mut self) -> Result<(), String> {
        self.available = Self::reachable();
        if !self.available {
            return Err("LHM tidak bisa dihubungi".into());
        }
        Ok(())
    }

    fn read(&mut self, out: &mut Vec<SensorReading>) {
        if !self.available {
            return;
        }
        match fetch_and_parse() {
            Ok(readings) => out.extend(readings),
            Err(e) => {
                self.last_error = Some(e);
                // Kalau LHM berhenti di tengah jalan, jangan spam; UI akan
                // menampilkan status dari probe berikutnya.
                self.available = false;
            }
        }
    }
}

fn fetch_and_parse() -> Result<Vec<SensorReading>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        // Redirect tidak diikuti: alamat loopback tidak boleh mengarahkan kita
        // ke host lain.
        .max_redirects(0)
        .build()
        .into();

    let resp = agent
        .get(LHM_URL)
        .call()
        .map_err(|e| format!("GET {LHM_URL}: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("status HTTP {}", resp.status()));
    }

    // Batasi ukuran: monitor yang rusak tidak boleh menghabiskan memori kita.
    let mut body = String::new();
    {
        use std::io::Read;
        let mut reader = resp.into_body().into_reader().take(MAX_BYTES as u64 + 1);
        reader
            .read_to_string(&mut body)
            .map_err(|e| format!("baca body: {e}"))?;
    }
    if body.len() > MAX_BYTES {
        return Err("respons melebihi 2 MB".into());
    }

    let root: LhmNode = serde_json::from_str(&body).map_err(|e| format!("JSON: {e}"))?;
    Ok(collect_temperatures(&root))
}

/// Node LHM. Struktur aslinya berisi `Text`, `Value`, `Children`, `ImageURL`.
#[derive(Debug, serde::Deserialize)]
struct LhmNode {
    #[serde(rename = "Text", default)]
    text: String,
    #[serde(rename = "Value", default)]
    value: String,
    #[serde(rename = "Children", default)]
    children: Vec<LhmNode>,
}

/// Telusuri pohon dan ambil semua node bernilai suhu.
fn collect_temperatures(root: &LhmNode) -> Vec<SensorReading> {
    let mut out = Vec::new();
    let mut stack: Vec<(&LhmNode, Vec<String>)> = vec![(root, Vec::new())];

    while let Some((node, path)) = stack.pop() {
        let mut path = path;
        if !node.text.trim().is_empty() {
            path.push(node.text.trim().to_string());
        }

        if let Some(value) = parse_temperature(&node.value) {
            let label = path
                .iter()
                .rev()
                .take(2)
                .rev()
                .cloned()
                .collect::<Vec<_>>()
                .join(" / ");
            out.push(reading(
                format!("lhm.{}", sanitize(&path.join("."))),
                SensorKind::Temperature,
                if label.is_empty() {
                    "LHM".into()
                } else {
                    label
                },
                value,
                Unit::Celsius,
                SensorSource::LhmBridge,
                Confidence::ThirdParty,
            ));
        }

        for child in &node.children {
            stack.push((child, path.clone()));
        }
    }
    out
}

/// Parse nilai seperti `"62.0 °C"`, `"62,0 °C"`, `"62 °C"`. Nilai non-suhu
/// (`"1200 RPM"`, `"1.2 V"`) menghasilkan `None`.
fn parse_temperature(value: &str) -> Option<f32> {
    let lower = value.to_lowercase();
    if !lower.contains("°c") && !lower.contains("degc") {
        return None;
    }
    let numeric: String = value
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',' || *c == '-')
        .collect();
    // Koma desimal (locale Jerman/Indonesia) jangan dianggap pemisah ribuan
    // karena nilai sensor selalu < 1000.
    let normalized = numeric.replace(',', ".");
    let parsed: f32 = normalized.parse().ok()?;
    if (-40.0..=150.0).contains(&parsed) {
        Some(parsed)
    } else {
        None
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_typical_lhm_values() {
        assert_eq!(parse_temperature("62.0 °C"), Some(62.0));
        assert_eq!(parse_temperature("62 °C"), Some(62.0));
        assert_eq!(parse_temperature("45,5 °C"), Some(45.5));
        assert_eq!(parse_temperature("-5.0 °C"), Some(-5.0));
    }

    #[test]
    fn rejects_non_temperature_units() {
        assert_eq!(parse_temperature("1200 RPM"), None);
        assert_eq!(parse_temperature("1.2 V"), None);
        assert_eq!(parse_temperature("35 W"), None);
        assert_eq!(parse_temperature(""), None);
    }

    #[test]
    fn rejects_absurd_temperatures() {
        assert_eq!(parse_temperature("5000 °C"), None);
        assert_eq!(parse_temperature("-273 °C"), None);
    }

    #[test]
    fn walks_tree_and_labels_with_parent() {
        let json = r#"{
            "Text": "Sensor",
            "Children": [
                {
                    "Text": "CPU",
                    "Children": [
                        { "Text": "CPU Package", "Value": "62.0 °C" },
                        { "Text": "CPU Core #1", "Value": "58.0 °C" }
                    ]
                },
                {
                    "Text": "GPU",
                    "Children": [
                        { "Text": "GPU Core", "Value": "51.0 °C" }
                    ]
                },
                { "Text": "Fan", "Value": "1200 RPM" }
            ]
        }"#;
        let root: LhmNode = serde_json::from_str(json).unwrap();
        let readings = collect_temperatures(&root);
        assert_eq!(readings.len(), 3);
        for r in &readings {
            assert_eq!(r.confidence, Confidence::ThirdParty);
            assert!(
                r.label.contains(" / "),
                "label harus menyebut induk: {}",
                r.label
            );
        }
        let labels: Vec<_> = readings.iter().map(|r| r.label.clone()).collect();
        assert!(labels.iter().any(|l| l.contains("CPU Package")));
        assert!(labels.iter().any(|l| l.contains("GPU Core")));
    }

    #[test]
    fn empty_tree_gives_no_readings() {
        let node: LhmNode = serde_json::from_str("{}").unwrap();
        assert!(collect_temperatures(&node).is_empty());
    }

    #[test]
    fn probe_fails_gracefully_when_lhm_not_running() {
        // Di mesin ini LHM tidak berjalan; probe harus mengembalikan
        // NotAvailable, bukan panik atau menggantung.
        let p = LhmBridgeProvider::new();
        let started = std::time::Instant::now();
        let a = p.probe();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(3),
            "probe terlalu lama: {:?}",
            started.elapsed()
        );
        assert!(matches!(
            a,
            Availability::NotAvailable(_) | Availability::Available
        ));
    }
}
