//! Tier 0: thermal zone ACPI lewat PDH, dengan fallback WMI.
//!
//! PDH dipakai lebih dulu karena jauh lebih murah daripada WMI (COM + query
//! per instance). Kalau kedua jalur kosong, berarti firmware memang tidak
//! mengekspos thermal zone — bukan bug kita (lihat docs/SENSORS.md).
//!
//! Penting: pada banyak laptop thermal zone ACPI mengukur **zona chassis**,
//! bukan die CPU. Nilainya bisa 15–25 °C di bawah suhu die. Karena itu
//! `confidence` di sini adalah `Derived`, dan labelnya "Zona ACPI", bukan "CPU".

use crate::{Availability, Confidence, SensorProvider, reading};
use core_pulse_core::model::{SensorKind, SensorReading, SensorSource, Unit};
use core_pulse_core::sys::pdh::{Counter, Query};

pub struct PdhThermalProvider {
    query: Option<Query>,
    temperature: Option<Counter>,
    precision: Option<Counter>,
    /// Jumlah instance thermal zone. `None` = belum diperiksa.
    instances: Option<usize>,
}

impl Default for PdhThermalProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl PdhThermalProvider {
    pub fn new() -> Self {
        let mut query = Query::new();
        // `Temperature` dalam Kelvin (×10 pada beberapa driver), `High Precision
        // Temperature` dalam Kelvin. Keduanya opsional.
        let temperature = query
            .as_mut()
            .and_then(|q| q.add(r"\Thermal Zone Information(*)\Temperature"));
        let precision = query
            .as_mut()
            .and_then(|q| q.add(r"\Thermal Zone Information(*)\High Precision Temperature"));
        Self {
            query,
            temperature,
            precision,
            instances: None,
        }
    }

    /// Satu kali collect untuk mengetahui apakah ada instance thermal zone.
    fn count_instances(&mut self) -> usize {
        let Some(query) = self.query.as_mut() else {
            return 0;
        };
        query.collect();
        let count = self
            .temperature
            .as_ref()
            .map(|c| c.array().len())
            .unwrap_or(0)
            .max(
                self.precision
                    .as_ref()
                    .map(|c| c.array().len())
                    .unwrap_or(0),
            );
        self.instances = Some(count);
        count
    }
}

impl SensorProvider for PdhThermalProvider {
    fn id(&self) -> &'static str {
        "acpi-pdh"
    }

    fn source(&self) -> SensorSource {
        SensorSource::Pdh
    }

    fn probe(&self) -> Availability {
        if self.query.is_none() {
            return Availability::Failed("PdhOpenQueryW gagal".into());
        }
        if self.temperature.is_none() && self.precision.is_none() {
            return Availability::NotAvailable(
                "counter Thermal Zone Information tidak terdaftar di mesin ini",
            );
        }
        // Counter terdaftar belum tentu punya instance. Kalau `init` sudah
        // menghitung 0, laporkan NotAvailable — bukan "tersedia" lalu diam.
        match self.instances {
            Some(0) => Availability::NotAvailable(
                "firmware tidak mengekspos ACPI thermal zone (0 instance)",
            ),
            _ => Availability::Available,
        }
    }

    fn init(&mut self) -> Result<(), String> {
        let count = self.count_instances();
        if count == 0 {
            // Bukan error: firmware memang tidak mengekspos thermal zone.
            // Dilaporkan sebagai NotAvailable, bukan Failed.
            tracing::info!("thermal zone ACPI tidak punya instance di mesin ini");
        } else {
            tracing::info!(count, "thermal zone ACPI ditemukan");
        }
        Ok(())
    }

    fn read(&mut self, out: &mut Vec<SensorReading>) {
        let Some(query) = self.query.as_mut() else {
            return;
        };
        query.collect();

        // High precision lebih akurat (0,1 K vs 1 K); pakai sebagai sumber utama.
        let mut values: Vec<(String, f64)> = self
            .precision
            .as_ref()
            .map(|c| c.array())
            .unwrap_or_default();
        if values.is_empty() {
            values = self
                .temperature
                .as_ref()
                .map(|c| c.array())
                .unwrap_or_default();
        }

        for (instance, kelvin) in values {
            // PDH melaporkan Kelvin. Nilai < 100 hampir pasti Celsius (beberapa
            // driver lama), nilai 2000–4000 adalah Kelvin × 10.
            let celsius = normalize_temperature(kelvin);
            let Some(celsius) = celsius else {
                continue;
            };
            let label = friendly_zone_name(&instance);
            out.push(crate::with_note(
                reading(
                    format!("acpi.{}", sanitize(&instance)),
                    SensorKind::Temperature,
                    label,
                    celsius,
                    Unit::Celsius,
                    SensorSource::Pdh,
                    Confidence::Derived,
                ),
                "zona chassis ACPI, biasanya di bawah suhu die CPU",
            ));
        }
    }
}

/// Normalisasi satuan suhu dari PDH ke Celsius.
///
/// Tiga bentuk yang benar-benar ditemui di lapangan:
/// - Kelvin × 10 (paling umum untuk `Temperature`)
/// - Kelvin (untuk `High Precision Temperature`)
/// - Celsius (driver lama / vendor)
fn normalize_temperature(raw: f64) -> Option<f32> {
    if !raw.is_finite() {
        return None;
    }
    let celsius = if (2000.0..=4000.0).contains(&raw) {
        raw / 10.0 - 273.15
    } else if (150.0..=400.0).contains(&raw) {
        raw - 273.15
    } else if (-40.0..=150.0).contains(&raw) {
        raw
    } else {
        return None;
    };
    // Setelah normalisasi, harus tetap masuk akal.
    if (-40.0..=150.0).contains(&celsius) {
        Some(celsius as f32)
    } else {
        None
    }
}

fn sanitize(instance: &str) -> String {
    instance
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// Nama instance PDH thermal zone biasanya `\_TZ.TZ00` atau `\_SB.PCI0.LPCB.EC0`.
/// Ambil bagian terakhir supaya bisa dibaca manusia.
fn friendly_zone_name(instance: &str) -> String {
    let tail = instance
        .rsplit('.')
        .next()
        .unwrap_or(instance)
        .trim_matches('_');
    if tail.is_empty() {
        format!("Zona ACPI {instance}")
    } else {
        format!("Zona ACPI {tail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_kelvin_times_ten() {
        // 3000 = 300.0 K = 26.85 °C
        let c = normalize_temperature(3000.0).unwrap();
        assert!((c - 26.85).abs() < 0.1, "dapat {c}");
    }

    #[test]
    fn normalizes_kelvin() {
        let c = normalize_temperature(318.15).unwrap();
        assert!((c - 45.0).abs() < 0.1, "dapat {c}");
    }

    #[test]
    fn normalizes_celsius() {
        let c = normalize_temperature(42.0).unwrap();
        assert!((c - 42.0).abs() < 0.01);
    }

    #[test]
    fn rejects_absurd_values() {
        assert!(
            normalize_temperature(0.0).is_none() || normalize_temperature(0.0).unwrap() >= -40.0
        );
        assert!(normalize_temperature(99_999.0).is_none());
        assert!(normalize_temperature(f64::NAN).is_none());
        assert!(normalize_temperature(f64::INFINITY).is_none());
    }

    #[test]
    fn friendly_name_takes_last_component() {
        assert_eq!(friendly_zone_name(r"\_TZ.TZ00"), "Zona ACPI TZ00");
        assert_eq!(friendly_zone_name(r"\_SB.PCI0.LPCB.EC0_"), "Zona ACPI EC0");
    }

    #[test]
    fn sanitize_produces_stable_id() {
        assert_eq!(sanitize(r"\_TZ.TZ00"), "__tz_tz00");
        assert_eq!(sanitize(r"\_TZ.TZ00"), sanitize(r"\_TZ.TZ00"));
    }

    #[test]
    fn provider_probes_without_panicking() {
        let mut p = PdhThermalProvider::new();
        let availability = p.probe();
        // Di mesin ini PDH punya counter tapi nol instance; di mesin lain bisa
        // Available. Yang penting tidak panik dan bukan Failed karena kode kita.
        assert!(matches!(
            availability,
            Availability::Available | Availability::NotAvailable(_)
        ));
        if matches!(availability, Availability::Available) {
            let _ = p.init();
            let mut out = Vec::new();
            p.read(&mut out);
            for r in &out {
                assert!(
                    (-40.0..=150.0).contains(&r.value),
                    "suhu tidak masuk akal: {r:?}"
                );
                assert_eq!(r.confidence, Confidence::Derived);
            }
        }
    }
}
