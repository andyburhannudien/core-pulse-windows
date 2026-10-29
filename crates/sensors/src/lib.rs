//! Backend sensor untuk core-pulse.
//!
//! Setiap sumber (ACPI, SMART, NVML, IGCL, bridge pihak ketiga) adalah
//! implementasi dari [`SensorProvider`]. Lihat docs/SENSORS.md.

#![deny(unsafe_op_in_unsafe_fn)]

use core_pulse_core::model::{SensorReading, SensorSource};

/// Status ketersediaan sebuah provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    Available,
    NeedsElevation,
    NotAvailable(&'static str),
    Failed(String),
}

/// Sumber data sensor.
pub trait SensorProvider: Send {
    fn id(&self) -> &'static str;
    fn source(&self) -> SensorSource;
    fn probe(&self) -> Availability;
    fn read(&mut self, out: &mut Vec<SensorReading>);

    fn init(&mut self) -> Result<(), String> {
        Ok(())
    }
}

/// Registry sederhana berisi semua provider yang tersedia di mesin ini.
#[derive(Default)]
pub struct SensorRegistry {
    providers: Vec<Box<dyn SensorProvider>>,
    availability: Vec<Availability>,
}

impl SensorRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, provider: Box<dyn SensorProvider>) {
        self.providers.push(provider);
        self.availability
            .push(Availability::NotAvailable("belum diprobe"));
    }

    /// Probe ulang semua provider. Dipanggil saat startup dan setiap 30 detik.
    pub fn probe_all(&mut self) {
        for (i, p) in self.providers.iter_mut().enumerate() {
            let availability = p.probe();
            if availability == Availability::Available
                && !matches!(self.availability.get(i), Some(Availability::Available))
            {
                if let Err(e) = p.init() {
                    tracing::warn!(provider = p.id(), error = %e, "init gagal");
                    self.availability[i] = Availability::Failed(e);
                    continue;
                }
            }
            self.availability[i] = availability;
        }
    }

    /// Baca semua provider yang tersedia. Provider yang tidak tersedia tidak
    /// dipanggil sama sekali (nol biaya).
    pub fn read_all(&mut self, out: &mut Vec<SensorReading>) {
        for (i, p) in self.providers.iter_mut().enumerate() {
            if matches!(self.availability.get(i), Some(Availability::Available)) {
                p.read(out);
            }
        }
    }

    pub fn status(&self) -> impl Iterator<Item = (&'static str, &Availability)> {
        self.providers
            .iter()
            .enumerate()
            .map(|(i, p)| (p.id(), &self.availability[i]))
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}
