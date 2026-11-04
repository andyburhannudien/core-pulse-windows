//! Klien helper elevated.
//!
//! UI dan CLI **tidak pernah** elevated. Data yang administrator rights required datang
//! dari `core-pulse-helper.exe` lewat named pipe baca-saja. Modul ini:
//!
//! 1. menghubungkan diri dengan batas waktu (tidak pernah menggantung),
//! 2. memverifikasi bahwa pipe dipegang executable helper yang kita harapkan,
//! 3. menegosiasikan versi protokol,
//! 4. mengambil bacaan sensor, lalu
//! 5. menggabungkannya tanpa menimpa data lokal yang sudah valid.
//!
//! Kalau helper mati, hasilnya [`HelperError::NotRunning`] dan UI kembali ke
//! status "butuh helper" — bukan error, bukan angka palsu.

use crate::{Availability, SensorReport};
use core_pulse_core::model::{
    Area, DegradeReason, SensorAvailability, SensorReading, SensorStatus,
};
use core_pulse_ipc::{PIPE_NAME, PROTO_VERSION, Request, Response, pipe::PipeClient};
use std::path::{Path, PathBuf};

/// Informasi helper yang sedang terhubung.
#[derive(Debug, Clone)]
pub struct HelperInfo {
    pub version: String,
    pub elevated: bool,
    pub pid: u32,
}

/// Hasil satu putaran pembacaan dari helper.
#[derive(Debug, Clone)]
pub struct HelperData {
    pub info: HelperInfo,
    pub readings: Vec<SensorReading>,
    pub statuses: Vec<SensorStatus>,
}

#[derive(Debug, thiserror::Error)]
pub enum HelperError {
    #[error("helper not running: {0}")]
    NotRunning(String),
    #[error("untrusted helper: {0}")]
    Untrusted(String),
    #[error("helper protocol version {helper} does not match client {client}")]
    Incompatible { helper: u16, client: u16 },
    #[error("helper rejected the request (code {code}): {message}")]
    Rejected { code: u16, message: String },
    #[error("invalid helper protocol: {0}")]
    Protocol(String),
    #[error("helper I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("helper frame: {0}")]
    Frame(#[from] core_pulse_ipc::FrameError),
}

/// Klien berumur pendek: satu koneksi untuk satu putaran pembacaan.
struct HelperClient {
    pipe: PipeClient,
    info: HelperInfo,
}

impl HelperClient {
    fn connect() -> Result<Self, HelperError> {
        let pipe = PipeClient::connect(PIPE_NAME).map_err(classify_connect)?;

        let pid = pipe
            .server_pid()
            .ok_or_else(|| HelperError::Protocol("could not read the pipe server PID".into()))?;
        verify_server_path(pid)?;

        let mut client = Self {
            pipe,
            info: HelperInfo {
                version: String::new(),
                elevated: false,
                pid,
            },
        };
        client.negotiate()?;
        Ok(client)
    }

    fn negotiate(&mut self) -> Result<(), HelperError> {
        self.pipe.request(&Request::Hello {
            proto_version: PROTO_VERSION,
        })?;

        match self.pipe.response::<Response>()? {
            Response::Hello {
                proto_version,
                helper_version,
                elevated,
            } => {
                if proto_version != PROTO_VERSION {
                    return Err(HelperError::Incompatible {
                        helper: proto_version,
                        client: PROTO_VERSION,
                    });
                }
                self.info.version = helper_version;
                self.info.elevated = elevated;
                Ok(())
            }
            Response::Error { code, message } if code == core_pulse_ipc::codes::BAD_VERSION => {
                // Helper lama menjawab dengan pesan error, bukan Hello.
                tracing::debug!(%message, "helper menolak versi");
                Err(HelperError::Incompatible {
                    helper: 0,
                    client: PROTO_VERSION,
                })
            }
            Response::Error { code, message } => Err(HelperError::Rejected { code, message }),
            other => Err(HelperError::Protocol(format!(
                "unexpected negotiation response: {other:?}"
            ))),
        }
    }

    fn sensors(&mut self) -> Result<HelperData, HelperError> {
        self.pipe.request(&Request::GetSensors)?;
        match self.pipe.response::<Response>()? {
            Response::Sensors {
                readings, statuses, ..
            } => Ok(HelperData {
                info: self.info.clone(),
                readings,
                statuses,
            }),
            Response::Error { code, message } => Err(HelperError::Rejected { code, message }),
            other => Err(HelperError::Protocol(format!(
                "unexpected sensor response: {other:?}"
            ))),
        }
    }
}

/// Ambil satu putaran sensor dari helper. Membuka koneksi baru setiap kali
/// supaya helper tidak "tertahan" oleh UI dan CLI tetap bisa terhubung.
pub fn fetch_once() -> Result<HelperData, HelperError> {
    let mut client = HelperClient::connect()?;
    client.sensors()
}

/// Path helper yang diharapkan. Default: saudara executable saat ini.
/// `CORE_PULSE_HELPER_PATH` menimpa untuk layout pengembangan/instalasi khusus.
pub fn expected_helper_path() -> PathBuf {
    if let Some(path) = std::env::var_os("CORE_PULSE_HELPER_PATH") {
        return PathBuf::from(path);
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("core-pulse-helper.exe")))
        .unwrap_or_else(|| PathBuf::from("core-pulse-helper.exe"))
}

/// THREAT-MODEL T1: pipe bisa saja dibuat proses lain. Cocokkan PID server
/// dengan executable helper yang kita harapkan sebelum memakai datanya.
fn verify_server_path(pid: u32) -> Result<(), HelperError> {
    let actual = core_pulse_ipc::pipe::process_image_path(pid).ok_or_else(|| {
        HelperError::Untrusted(format!(
            "pipe server executable path (PID {pid}) could not be read"
        ))
    })?;
    let expected = expected_helper_path();

    if same_path(&actual, &expected) {
        return Ok(());
    }

    Err(HelperError::Untrusted(format!(
        "pipe is held by {} (expected {})",
        actual.display(),
        expected.display()
    )))
}

fn same_path(a: &Path, b: &Path) -> bool {
    let a = std::fs::canonicalize(a).unwrap_or_else(|_| a.to_path_buf());
    let b = std::fs::canonicalize(b).unwrap_or_else(|_| b.to_path_buf());
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

fn classify_connect(error: std::io::Error) -> HelperError {
    match error.kind() {
        std::io::ErrorKind::NotFound => {
            HelperError::NotRunning("named pipe not found; helper is not running".into())
        }
        std::io::ErrorKind::TimedOut => {
            HelperError::NotRunning("helper is busy or not responding".into())
        }
        _ => HelperError::NotRunning(format!("cannot connect to the pipe: {error}")),
    }
}

/// Gabungkan hasil helper ke laporan lokal.
///
/// Aturan:
/// - bacaan lokal menang jika `id` sama (data langsung lebih dekat ke sumber),
/// - status `smart` diganti status helper karena helper yang benar-benar
///   menjalankan IOCTL SMART,
/// - status `helper` selalu dilaporkan apa adanya,
/// - nilai di luar rentang fisik tetap dibuang.
pub fn merge_into(report: &mut SensorReport, outcome: Result<HelperData, HelperError>) {
    report.statuses.retain(|(id, _)| id != "helper");

    match outcome {
        Ok(data) => {
            report.statuses.push((
                "helper".into(),
                if data.info.elevated {
                    Availability::Available
                } else {
                    Availability::NeedsElevation
                },
            ));

            if let Some(smart) = data.statuses.iter().find(|s| s.provider == "smart") {
                let availability = from_status(&smart.availability);
                set_status(report, "smart", availability.clone());
                // Status lokal "smart: administrator rights required" sudah tidak berlaku
                // kalau helper menjawab. Ganti dengan status helper.
                report
                    .degraded
                    .retain(|d| !(d.area == Area::Sensors && d.detail.starts_with("smart:")));
                if let Some(d) = availability.to_degradation("smart (helper)") {
                    report.degraded.push(d);
                }
            }

            for reading in data.readings {
                if !crate::is_plausible(&reading) {
                    tracing::warn!(id = %reading.id, value = reading.value, "helper sent an implausible value; dropped");
                    continue;
                }
                if report.readings.iter().any(|local| local.id == reading.id) {
                    continue;
                }
                report.readings.push(reading);
            }
        }
        Err(error) => {
            // Helper tidak tersedia bukan kegagalan aplikasi. Statusnya
            // NeedsElevation saat helper belum berjalan, karena solusinya
            // memang menjalankan helper dengan hak administrator.
            let availability = match &error {
                HelperError::NotRunning(_) => Availability::NeedsElevation,
                HelperError::Untrusted(_)
                | HelperError::Incompatible { .. }
                | HelperError::Rejected { .. }
                | HelperError::Protocol(_) => Availability::Failed(error.to_string()),
                HelperError::Io(_) | HelperError::Frame(_) => {
                    Availability::Failed(error.to_string())
                }
            };
            report.statuses.push(("helper".into(), availability));
        }
    }
}

fn set_status(report: &mut SensorReport, provider: &str, availability: Availability) {
    if let Some(entry) = report
        .statuses
        .iter_mut()
        .find(|(candidate, _)| candidate == provider)
    {
        entry.1 = availability;
    } else {
        report.statuses.push((provider.to_string(), availability));
    }
}

fn from_status(status: &SensorAvailability) -> Availability {
    match status {
        SensorAvailability::Available => Availability::Available,
        SensorAvailability::NeedsElevation => Availability::NeedsElevation,
        SensorAvailability::NotAvailable { reason } => Availability::NotAvailable(reason.clone()),
        SensorAvailability::Failed { error } => Availability::Failed(error.clone()),
    }
}

/// Degradasi tambahan saat helper tidak ada. Dipakai UI untuk menjelaskan
/// tindakan yang bisa dilakukan tanpa mengulang pesan yang sama.
pub fn missing_helper_note(error: &HelperError) -> (DegradeReason, String) {
    match error {
        HelperError::NotRunning(_) => (
            DegradeReason::NeedsElevation,
            "helper is not running; start core-pulse-helper.exe as administrator".to_string(),
        ),
        HelperError::Untrusted(detail) => (
            DegradeReason::NotAvailable,
            format!("untrusted helper: {detail}"),
        ),
        HelperError::Incompatible { .. } => (
            DegradeReason::NotAvailable,
            "helper version mismatch; update the helper".to_string(),
        ),
        other => (DegradeReason::ApiFailed, format!("helper failed: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SensorReport;
    use core_pulse_core::model::{Confidence, SensorKind, SensorSource, SensorStatus, Unit};

    fn reading(id: &str, value: f32) -> SensorReading {
        SensorReading {
            id: id.into(),
            kind: SensorKind::Temperature,
            label: id.into(),
            value,
            unit: Unit::Celsius,
            min: value,
            max: value,
            avg: value,
            source: SensorSource::Smart,
            confidence: Confidence::Direct,
            ts_unix_ms: 0,
            disk_index: Some(0),
        }
    }

    fn data(elevated: bool, readings: Vec<SensorReading>, smart: SensorAvailability) -> HelperData {
        HelperData {
            info: HelperInfo {
                version: "0.1.0".into(),
                elevated,
                pid: 1234,
            },
            readings,
            statuses: vec![SensorStatus {
                provider: "smart".into(),
                availability: smart,
            }],
        }
    }

    #[test]
    fn helper_readings_fill_empty_report() {
        let mut report = SensorReport::default();
        merge_into(
            &mut report,
            Ok(data(
                true,
                vec![reading("disk.0.temp", 41.0)],
                SensorAvailability::Available,
            )),
        );
        assert_eq!(report.readings.len(), 1);
        assert_eq!(report.readings[0].value, 41.0);
        assert!(
            report
                .statuses
                .iter()
                .any(|(id, a)| id == "helper" && *a == Availability::Available)
        );
        assert!(
            report
                .statuses
                .iter()
                .any(|(id, a)| id == "smart" && *a == Availability::Available)
        );
    }

    #[test]
    fn local_reading_wins_on_id_collision() {
        let mut report = SensorReport {
            readings: vec![reading("disk.0.temp", 40.0)],
            ..Default::default()
        };
        merge_into(
            &mut report,
            Ok(data(
                true,
                vec![reading("disk.0.temp", 99.0)],
                SensorAvailability::Available,
            )),
        );
        assert_eq!(report.readings.len(), 1);
        assert_eq!(report.readings[0].value, 40.0);
    }

    #[test]
    fn implausible_helper_reading_is_dropped() {
        let mut report = SensorReport::default();
        merge_into(
            &mut report,
            Ok(data(
                true,
                vec![reading("disk.0.temp", 1234.0)],
                SensorAvailability::Available,
            )),
        );
        assert!(report.readings.is_empty());
    }

    #[test]
    fn missing_helper_is_not_an_error() {
        let mut report = SensorReport::default();
        merge_into(
            &mut report,
            Err(HelperError::NotRunning("pipe does not exist".into())),
        );
        assert!(matches!(report.statuses[0].1, Availability::NeedsElevation));
    }

    #[test]
    fn untrusted_helper_is_reported_and_not_merged() {
        let mut report = SensorReport::default();
        merge_into(
            &mut report,
            Err(HelperError::Untrusted("wrong path".into())),
        );
        assert!(matches!(report.statuses[0].1, Availability::Failed(_)));
        assert!(report.readings.is_empty());
    }

    #[test]
    fn non_elevated_helper_is_flagged() {
        let mut report = SensorReport::default();
        merge_into(
            &mut report,
            Ok(data(false, vec![], SensorAvailability::NeedsElevation)),
        );
        assert!(
            report
                .statuses
                .iter()
                .any(|(id, a)| id == "helper" && *a == Availability::NeedsElevation)
        );
    }

    #[test]
    fn smart_degradation_is_replaced_when_helper_answers() {
        let mut report = SensorReport {
            degraded: vec![core_pulse_core::model::Degradation {
                area: Area::Sensors,
                reason: DegradeReason::NeedsElevation,
                detail: "smart: administrator rights required".into(),
            }],
            ..Default::default()
        };
        merge_into(
            &mut report,
            Ok(data(
                true,
                vec![reading("disk.0.temp", 41.0)],
                SensorAvailability::Available,
            )),
        );
        assert!(report.degraded.is_empty());
    }
}
