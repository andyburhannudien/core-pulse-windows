//! Backend sensor untuk core-pulse.
//!
//! Setiap sumber (ACPI/PDH, SMART, GPU vendor, bridge pihak ketiga) adalah
//! implementasi dari [`SensorProvider`]. Lihat docs/SENSORS.md.
//!
//! Aturan yang mengikat:
//!
//! 1. Provider yang `NotAvailable` **tidak dipanggil** `read()` sama sekali.
//! 2. `read()` tidak boleh `panic` dan tidak boleh memblokir > 100 ms.
//! 3. Tidak ada angka pengganti: kalau sumber tidak ada, hasilnya kosong dan
//!    statusnya dilaporkan, bukan `0 °C` (ADR-0008).
//! 4. Nilai di luar rentang fisik dibuang, bukan ditampilkan.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod acpi;
pub mod bridge;
pub mod gpu_vendor;
pub mod helper;
pub mod smart;
use core_pulse_core::model::{
    Area, Confidence, Degradation, DegradeReason, SensorReading, SensorSource, Snapshot,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Status ketersediaan sebuah provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    Available,
    NeedsElevation,
    NotAvailable(String),
    Failed(String),
}

impl Availability {
    /// Petakan ke bentuk yang bisa diserialisasi untuk snapshot.
    pub fn to_status(&self) -> core_pulse_core::model::SensorAvailability {
        use core_pulse_core::model::SensorAvailability as S;
        match self {
            Self::Available => S::Available,
            Self::NeedsElevation => S::NeedsElevation,
            Self::NotAvailable(reason) => S::NotAvailable {
                reason: reason.clone(),
            },
            Self::Failed(e) => S::Failed { error: e.clone() },
        }
    }

    /// Entri degradasi untuk snapshot, kalau provider tidak tersedia.
    pub fn to_degradation(&self, provider: &str) -> Option<Degradation> {
        let (reason, detail) = match self {
            Self::Available => return None,
            Self::NeedsElevation => (
                DegradeReason::NeedsElevation,
                format!("{provider}: butuh hak administrator (helper, P4)"),
            ),
            Self::NotAvailable(why) => (DegradeReason::NotAvailable, format!("{provider}: {why}")),
            Self::Failed(e) => (DegradeReason::ApiFailed, format!("{provider}: {e}")),
        };
        Some(Degradation {
            area: Area::Sensors,
            reason,
            detail,
        })
    }
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

/// Satu putaran pembacaan sensor.
#[derive(Debug, Clone, Default)]
pub struct SensorReport {
    pub readings: Vec<SensorReading>,
    pub degraded: Vec<Degradation>,
    pub statuses: Vec<(String, Availability)>,
}

/// Registry semua provider yang tersedia di mesin ini.
pub struct Sensors {
    providers: Vec<Box<dyn SensorProvider>>,
    status: Vec<Availability>,
    readings: Vec<SensorReading>,
    last_probe: Option<std::time::Instant>,
    report: SensorReport,
    /// Kalau true, setiap `poll()` juga mencoba helper elevated. Default mati
    /// supaya test tidak bergantung pada pipe di mesin.
    helper_enabled: bool,
}

/// Probe ulang status provider setiap 30 detik. Perangkat keras bisa dicolok
/// atau driver bisa dimuat setelah aplikasi berjalan.
const PROBE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

impl Default for Sensors {
    fn default() -> Self {
        Self::new()
    }
}

impl Sensors {
    /// Daftar provider default. Bridge pihak ketiga **tidak** didaftarkan di
    /// sini karena harus diaktifkan pengguna secara eksplisit.
    pub fn new() -> Self {
        Self::with_bridge(false)
    }

    pub fn with_bridge(enable_bridge: bool) -> Self {
        Self::with_options(enable_bridge, false)
    }

    /// Konstruktor lengkap: bridge pihak ketiga dan helper elevated.
    pub fn with_options(enable_bridge: bool, enable_helper: bool) -> Self {
        let mut providers: Vec<Box<dyn SensorProvider>> = vec![
            Box::new(acpi::PdhThermalProvider::new()),
            Box::new(smart::SmartProvider::new()),
            Box::new(gpu_vendor::GpuVendorProvider::new()),
        ];
        if enable_bridge {
            providers.push(Box::new(bridge::LhmBridgeProvider::new()));
        }
        let status = vec![Availability::NotAvailable("not probed yet".into()); providers.len()];
        Self {
            providers,
            status,
            readings: Vec::with_capacity(32),
            last_probe: None,
            report: SensorReport::default(),
            helper_enabled: enable_helper,
        }
    }

    pub fn enable_helper(&mut self, enabled: bool) {
        self.helper_enabled = enabled;
    }

    pub fn helper_enabled(&self) -> bool {
        self.helper_enabled
    }

    /// Daftarkan provider tambahan (dipakai test dan CLI `--bridge`).
    pub fn register(&mut self, provider: Box<dyn SensorProvider>) {
        self.providers.push(provider);
        self.status
            .push(Availability::NotAvailable("not probed yet".into()));
    }

    /// Probe semua provider. Dipanggil saat startup dan setiap 30 detik, atau
    /// paksa dengan `force`.
    pub fn probe_all(&mut self, force: bool) {
        let due = self
            .last_probe
            .map(|t| t.elapsed() >= PROBE_INTERVAL)
            .unwrap_or(true);
        if !due && !force {
            return;
        }
        self.last_probe = Some(std::time::Instant::now());

        for i in 0..self.providers.len() {
            let mut availability = self.providers[i].probe();
            // init hanya dipanggil saat transisi ke Available. Setelah init,
            // probe diulang karena init bisa mengubah jawaban (mis. thermal zone
            // terdaftar tapi tidak punya instance → NotAvailable).
            if availability == Availability::Available
                && !matches!(self.status.get(i), Some(Availability::Available))
            {
                if let Err(e) = self.providers[i].init() {
                    tracing::warn!(provider = self.providers[i].id(), error = %e, "init gagal");
                    availability = Availability::Failed(e);
                } else {
                    availability = self.providers[i].probe();
                }
            }
            self.status[i] = availability;
        }
    }

    /// Baca semua provider yang tersedia. Hasil disimpan dan bisa diambil
    /// dengan [`Self::report`].
    pub fn poll(&mut self) -> &SensorReport {
        self.probe_all(false);

        self.readings.clear();
        let mut degraded = Vec::new();
        let mut statuses = Vec::new();

        for i in 0..self.providers.len() {
            let id = self.providers[i].id();
            let availability = self.status[i].clone();
            if let Some(d) = availability.to_degradation(id) {
                degraded.push(d);
            }
            statuses.push((id.to_string(), availability.clone()));

            if matches!(availability, Availability::Available) {
                self.providers[i].read(&mut self.readings);
            }
        }

        let mut report = SensorReport {
            readings: self.readings.clone(),
            degraded,
            statuses,
        };

        // Helper elevated: satu koneksi berumur pendek per putaran. Kalau
        // helper mati, merge hanya menambahkan status "helper", bukan error.
        if self.helper_enabled {
            let outcome = helper::fetch_once();
            if let Err(error) = &outcome {
                tracing::debug!(error = %error, "helper tidak tersedia");
            }
            helper::merge_into(&mut report, outcome);
        }

        // Buang nilai yang tidak masuk akal. Sumber yang salah lebih berbahaya
        // daripada tidak ada sumber, karena terlihat meyakinkan. Ini juga
        // berlaku untuk data dari helper.
        let before = report.readings.len();
        report.readings.retain(is_plausible);
        if report.readings.len() < before {
            tracing::warn!(
                dropped = before - report.readings.len(),
                "membuang bacaan sensor di luar rentang fisik"
            );
        }

        // Urutkan stabil: suhu dulu, lalu per label. Membuat UI tidak "melompat"
        // setiap detik saat urutan instance berubah.
        report.readings.sort_by(|a, b| {
            (a.kind as u8)
                .cmp(&(b.kind as u8))
                .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
        });

        self.report = report;
        &self.report
    }

    pub fn report(&self) -> &SensorReport {
        &self.report
    }

    pub fn statuses(&self) -> impl Iterator<Item = (&'static str, &Availability)> {
        self.providers
            .iter()
            .enumerate()
            .map(|(i, p)| (p.id(), &self.status[i]))
    }

    /// Tulis hasil ke snapshot. Satu tempat supaya UI dan CLI konsisten.
    pub fn apply(&mut self, snapshot: &mut Snapshot) {
        let report = self.poll();
        snapshot.sensors = report.readings.clone();
        snapshot.degraded.extend(report.degraded.iter().cloned());
        snapshot.sensor_status = report
            .statuses
            .iter()
            .map(|(provider, a)| core_pulse_core::model::SensorStatus {
                provider: provider.clone(),
                availability: a.to_status(),
            })
            .collect();
    }
}

/// Wadah `SensorReport` yang aman dibaca tanpa lock, untuk dibagikan dari
/// thread sensor ke UI/sampler.
pub struct SensorStore {
    inner: arc_swap::ArcSwap<SensorReport>,
}

impl Default for SensorStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SensorStore {
    pub fn new() -> Self {
        Self {
            inner: arc_swap::ArcSwap::from_pointee(SensorReport::default()),
        }
    }

    pub fn load(&self) -> arc_swap::Guard<std::sync::Arc<SensorReport>> {
        self.inner.load()
    }

    pub fn store(&self, report: SensorReport) {
        self.inner.store(std::sync::Arc::new(report));
    }

    /// Salin hasil terakhir ke snapshot. Dipakai sampler setiap tick, tanpa
    /// memblokir dan tanpa menyentuh hardware.
    pub fn apply_to(&self, snapshot: &mut Snapshot) {
        let report = self.load();
        snapshot.sensors = report.readings.clone();
        snapshot.degraded.extend(report.degraded.iter().cloned());
        snapshot.sensor_status = report
            .statuses
            .iter()
            .map(|(provider, a)| core_pulse_core::model::SensorStatus {
                provider: provider.clone(),
                availability: a.to_status(),
            })
            .collect();
    }
}

/// Kontrol runtime untuk poller sensor. Dipakai UI untuk toggle bridge/helper
/// tanpa mengakses `Sensors` langsung dari thread lain.
#[derive(Clone)]
pub struct SensorControl {
    bridge_enabled: Arc<AtomicBool>,
    helper_enabled: Arc<AtomicBool>,
}

impl SensorControl {
    pub fn new(bridge_enabled: bool, helper_enabled: bool) -> Self {
        Self {
            bridge_enabled: Arc::new(AtomicBool::new(bridge_enabled)),
            helper_enabled: Arc::new(AtomicBool::new(helper_enabled)),
        }
    }

    pub fn set_bridge_enabled(&self, enabled: bool) {
        self.bridge_enabled.store(enabled, Ordering::Release);
    }

    pub fn set_helper_enabled(&self, enabled: bool) {
        self.helper_enabled.store(enabled, Ordering::Release);
    }

    pub fn bridge_enabled(&self) -> bool {
        self.bridge_enabled.load(Ordering::Acquire)
    }

    pub fn helper_enabled(&self) -> bool {
        self.helper_enabled.load(Ordering::Acquire)
    }
}

/// Jalankan pembacaan sensor di thread sendiri.
///
/// Interval 2 detik: sensor hardware sendiri jarang memperbarui lebih cepat,
/// dan SMART butuh 5–40 ms per disk. Sampler 1 Hz tidak boleh menunggu itu.
///
/// `control`, bila ada, memungkinkan UI mengubah bridge/helper saat runtime.
/// Perubahan bridge menyebabkan `Sensors` dibuat ulang karena daftar provider
/// bersifat statis; ini jarang terjadi dan aman dilakukan di thread sensor.
pub fn spawn_sensor_poller(
    store: std::sync::Arc<SensorStore>,
    sensors: Sensors,
    on_read: Option<Box<dyn Fn() + Send>>,
) -> std::thread::JoinHandle<()> {
    spawn_sensor_poller_with_control(store, sensors, None, on_read)
}

pub fn spawn_sensor_poller_with_control(
    store: std::sync::Arc<SensorStore>,
    mut sensors: Sensors,
    control: Option<SensorControl>,
    on_read: Option<Box<dyn Fn() + Send>>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("sensors".into())
        .spawn(move || {
            // Jika ada control, control adalah sumber kebenaran untuk opsi runtime.
            if let Some(ctrl) = &control {
                let want_bridge = ctrl.bridge_enabled();
                let want_helper = ctrl.helper_enabled();
                sensors = Sensors::with_options(want_bridge, want_helper);
            }

            let mut current_bridge = control
                .as_ref()
                .map(SensorControl::bridge_enabled)
                .unwrap_or_else(|| has_bridge_provider(&sensors));
            let mut current_helper = sensors.helper_enabled();

            loop {
                if let Some(ctrl) = &control {
                    let want_bridge = ctrl.bridge_enabled();
                    let want_helper = ctrl.helper_enabled();
                    if want_bridge != current_bridge {
                        sensors = Sensors::with_options(want_bridge, want_helper);
                        current_bridge = want_bridge;
                        current_helper = want_helper;
                        tracing::info!(
                            bridge = want_bridge,
                            helper = want_helper,
                            "sensor providers reconfigured"
                        );
                    } else if want_helper != current_helper {
                        sensors.enable_helper(want_helper);
                        current_helper = want_helper;
                    }
                }

                sensors.probe_all(false);
                let report = sensors.poll().clone();
                store.store(report);
                if let Some(cb) = &on_read {
                    cb();
                }
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
        })
        .expect("gagal membuat thread sensor")
}

fn has_bridge_provider(sensors: &Sensors) -> bool {
    sensors.statuses().any(|(id, _)| id == "lhm-bridge")
}

/// Convenience: buat control, bangun sensors darinya, dan spawn poller.
pub fn spawn_sensor_poller_controlled(
    store: std::sync::Arc<SensorStore>,
    control: SensorControl,
    on_read: Option<Box<dyn Fn() + Send>>,
) -> std::thread::JoinHandle<()> {
    let sensors = Sensors::with_options(control.bridge_enabled(), control.helper_enabled());
    spawn_sensor_poller_with_control(store, sensors, Some(control), on_read)
}

/// Rentang fisik yang masuk akal untuk setiap jenis sensor.
///
/// Suhu di bawah -40 °C atau di atas 150 °C bukan bacaan sensor, itu bug
/// (atau payload perangkat yang rusak). Nilai seperti itu dibuang, bukan
/// ditampilkan dengan label "suhu".
fn is_plausible(r: &SensorReading) -> bool {
    match r.kind {
        core_pulse_core::model::SensorKind::Temperature => {
            (-40.0..=150.0).contains(&r.value) && r.value.is_finite()
        }
        core_pulse_core::model::SensorKind::Fan => (0.0..=20_000.0).contains(&r.value),
        core_pulse_core::model::SensorKind::Voltage => (0.0..=30.0).contains(&r.value),
        core_pulse_core::model::SensorKind::Power => (0.0..=2_000.0).contains(&r.value),
        core_pulse_core::model::SensorKind::Clock => (0.0..=20_000.0).contains(&r.value),
        core_pulse_core::model::SensorKind::Load => (0.0..=100.0).contains(&r.value),
    }
}

/// Bangun `SensorReading` dengan min/max/avg yang diisi rata dengan nilai
/// sekarang. Provider yang punya riwayat bisa menimpanya sendiri.
pub(crate) fn reading(
    id: String,
    kind: core_pulse_core::model::SensorKind,
    label: String,
    value: f32,
    unit: core_pulse_core::model::Unit,
    source: SensorSource,
    confidence: Confidence,
) -> SensorReading {
    SensorReading {
        id,
        kind,
        label,
        value,
        unit,
        min: value,
        max: value,
        avg: value,
        source,
        confidence,
        ts_unix_ms: core_pulse_core::sampler::now_unix_ms(),
        disk_index: None,
    }
}

/// Tambah catatan ke label (mis. "zona chassis ACPI").
pub(crate) fn with_note(mut reading: SensorReading, note: &str) -> SensorReading {
    reading.label = format!("{} ({note})", reading.label);
    reading
}

/// Tandai bacaan ini milik disk fisik tertentu.
pub(crate) fn with_disk(mut reading: SensorReading, index: u32) -> SensorReading {
    reading.disk_index = Some(index);
    reading
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_pulse_core::model::{SensorKind, Unit};

    struct Fixed(f32);

    impl SensorProvider for Fixed {
        fn id(&self) -> &'static str {
            "test-fixed"
        }
        fn source(&self) -> SensorSource {
            SensorSource::Acpi
        }
        fn probe(&self) -> Availability {
            Availability::Available
        }
        fn read(&mut self, out: &mut Vec<SensorReading>) {
            out.push(reading(
                "test.cpu".into(),
                SensorKind::Temperature,
                "CPU".into(),
                self.0,
                Unit::Celsius,
                SensorSource::Acpi,
                Confidence::Direct,
            ));
        }
    }

    struct Unavailable;

    impl SensorProvider for Unavailable {
        fn id(&self) -> &'static str {
            "test-unavailable"
        }
        fn source(&self) -> SensorSource {
            SensorSource::Smart
        }
        fn probe(&self) -> Availability {
            Availability::NeedsElevation
        }
        fn read(&mut self, _out: &mut Vec<SensorReading>) {
            panic!("provider yang tidak tersedia tidak boleh dipanggil read()");
        }
    }

    #[test]
    fn unavailable_provider_is_never_read() {
        let mut s = Sensors::new();
        // Kosongkan provider default supaya test tidak bergantung mesin.
        s.providers.clear();
        s.status.clear();
        s.register(Box::new(Unavailable));
        let report = s.poll().clone();
        assert!(report.readings.is_empty());
        assert_eq!(report.degraded.len(), 1);
        assert_eq!(report.degraded[0].reason, DegradeReason::NeedsElevation);
    }

    #[test]
    fn implausible_temperature_is_dropped() {
        let mut s = Sensors::new();
        s.providers.clear();
        s.status.clear();
        s.register(Box::new(Fixed(1234.0)));
        let report = s.poll().clone();
        assert!(report.readings.is_empty(), "suhu 1234 °C harus dibuang");
    }

    #[test]
    fn plausible_temperature_is_kept() {
        let mut s = Sensors::new();
        s.providers.clear();
        s.status.clear();
        s.register(Box::new(Fixed(62.0)));
        let report = s.poll().clone();
        assert_eq!(report.readings.len(), 1);
        assert_eq!(report.readings[0].value, 62.0);
    }

    #[test]
    fn degradations_name_the_provider() {
        let d = Availability::NeedsElevation
            .to_degradation("smart")
            .expect("harus menghasilkan degradasi");
        assert!(d.detail.contains("smart"));
        assert!(d.detail.contains("administrator"));
        assert!(Availability::Available.to_degradation("x").is_none());
    }

    #[test]
    fn apply_fills_snapshot() {
        let mut s = Sensors::new();
        s.providers.clear();
        s.status.clear();
        s.register(Box::new(Fixed(50.0)));
        let mut snap = Snapshot::empty();
        s.apply(&mut snap);
        assert_eq!(snap.sensors.len(), 1);
        assert_eq!(snap.sensor_status.len(), 1);
    }
}
