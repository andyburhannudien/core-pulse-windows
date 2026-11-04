//! Alert engine: thresholds, deduplication, and cooldown.
//!
//! Binding rules:
//!
//! - An alert only fires after the condition has been sustained for a while.
//!   A one-second CPU spike while opening an app is not a problem.
//! - An active condition is never notified twice.
//! - After clearing, the same condition cannot fire again during the cooldown,
//!   so a threshold that oscillates cannot flood the user.
//! - No alert is ever raised from missing data. A `None` temperature (no
//!   sensor) is never treated as 0 °C.

use crate::model::{SensorKind, SensorSource, Snapshot};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

const CPU_WARN: f32 = 90.0;
const CPU_CRITICAL: f32 = 97.0;
const MEM_WARN: f32 = 90.0;
const MEM_CRITICAL: f32 = 97.0;
const DISK_TEMP_WARN: f32 = 55.0;
const DISK_TEMP_CRITICAL: f32 = 60.0;
const CPU_TEMP_WARN: f32 = 90.0;
const GPU_TEMP_WARN: f32 = 85.0;
const OTHER_TEMP_WARN: f32 = 80.0;
const FREE_SPACE_WARN: f32 = 5.0;
const FREE_SPACE_CRITICAL: f32 = 2.0;

const CPU_SUSTAIN_MS: u64 = 5_000;
const MEM_SUSTAIN_MS: u64 = 5_000;
const TEMP_SUSTAIN_MS: u64 = 10_000;
const DISK_SUSTAIN_MS: u64 = 60_000;
const COOLDOWN_MS: u64 = 5 * 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AlertKind {
    HighCpu,
    HighMemory,
    HighTemperature,
    LowDiskSpace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AlertSeverity {
    Warning,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    /// Kunci stabil untuk dedup, mis. `cpu`, `mem`, `temp:disk.0.temp`.
    pub key: String,
    pub kind: AlertKind,
    pub severity: AlertSeverity,
    pub title: String,
    pub detail: String,
    pub value: f32,
    pub threshold: f32,
    pub since_unix_ms: u64,
    pub ts_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AlertChange {
    Raised,
    Cleared,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertEvent {
    pub alert: Alert,
    pub change: AlertChange,
}

/// Alert state read by the UI without locking.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AlertState {
    pub active: Vec<Alert>,
    /// Latest events, at most 20, newest first.
    pub recent: Vec<AlertEvent>,
}

pub struct AlertStore {
    inner: arc_swap::ArcSwap<AlertState>,
}

impl Default for AlertStore {
    fn default() -> Self {
        Self::new()
    }
}

impl AlertStore {
    pub fn new() -> Self {
        Self {
            inner: arc_swap::ArcSwap::from_pointee(AlertState::default()),
        }
    }

    pub fn load(&self) -> arc_swap::Guard<std::sync::Arc<AlertState>> {
        self.inner.load()
    }

    pub fn store(&self, state: AlertState) {
        self.inner.store(std::sync::Arc::new(state));
    }
}

struct Candidate {
    key: String,
    kind: AlertKind,
    severity: AlertSeverity,
    title: String,
    detail: String,
    value: f32,
    threshold: f32,
    sustain_ms: u64,
}

/// Mesin stateful. Satu instance dimiliki thread sampler.
pub struct AlertEngine {
    pending_since: HashMap<String, u64>,
    active: HashMap<String, Alert>,
    cooldown_until: HashMap<String, u64>,
    recent: Vec<AlertEvent>,
}

impl Default for AlertEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl AlertEngine {
    pub fn new() -> Self {
        Self {
            pending_since: HashMap::new(),
            active: HashMap::new(),
            cooldown_until: HashMap::new(),
            recent: Vec::new(),
        }
    }

    /// Evaluasi satu snapshot. Mengembalikan event naik/turun yang terjadi.
    pub fn evaluate(&mut self, snapshot: &Snapshot) -> Vec<AlertEvent> {
        let now = if snapshot.ts_unix_ms > 0 {
            snapshot.ts_unix_ms
        } else {
            crate::sampler::now_unix_ms()
        };

        let candidates = candidates(snapshot);
        let current_keys: HashSet<&str> = candidates.iter().map(|c| c.key.as_str()).collect();
        let mut events = Vec::new();

        for candidate in &candidates {
            if let Some(active) = self.active.get_mut(&candidate.key) {
                // Kondisi masih aktif: perbarui nilai, jangan notifikasi ulang.
                active.value = candidate.value;
                active.detail = candidate.detail.clone();
                active.ts_unix_ms = now;
                continue;
            }

            if self
                .cooldown_until
                .get(&candidate.key)
                .is_some_and(|until| now < *until)
            {
                continue;
            }

            let since = *self
                .pending_since
                .entry(candidate.key.clone())
                .or_insert(now);
            if now.saturating_sub(since) < candidate.sustain_ms {
                continue;
            }

            self.pending_since.remove(&candidate.key);
            let alert = Alert {
                key: candidate.key.clone(),
                kind: candidate.kind,
                severity: candidate.severity,
                title: candidate.title.clone(),
                detail: candidate.detail.clone(),
                value: candidate.value,
                threshold: candidate.threshold,
                since_unix_ms: since,
                ts_unix_ms: now,
            };
            self.active.insert(candidate.key.clone(), alert.clone());
            events.push(AlertEvent {
                alert,
                change: AlertChange::Raised,
            });
        }

        // Kondisi yang tidak lagi muncul (mis. sensor hilang) dianggap reda.
        let stale: Vec<String> = self
            .active
            .keys()
            .filter(|key| !current_keys.contains(key.as_str()))
            .cloned()
            .collect();
        for key in stale {
            if let Some(alert) = self.active.remove(&key) {
                self.cooldown_until.insert(key, now + COOLDOWN_MS);
                events.push(AlertEvent {
                    alert,
                    change: AlertChange::Cleared,
                });
            }
        }

        self.pending_since
            .retain(|key, _| current_keys.contains(key.as_str()));

        for event in &events {
            self.recent.insert(0, event.clone());
        }
        self.recent.truncate(20);

        events
    }

    pub fn active(&self) -> Vec<Alert> {
        let mut alerts: Vec<_> = self.active.values().cloned().collect();
        alerts.sort_by(|a, b| {
            severity_rank(b.severity)
                .cmp(&severity_rank(a.severity))
                .then_with(|| a.title.cmp(&b.title))
        });
        alerts
    }

    pub fn state(&self) -> AlertState {
        AlertState {
            active: self.active(),
            recent: self.recent.clone(),
        }
    }
}

fn severity_rank(s: AlertSeverity) -> u8 {
    match s {
        AlertSeverity::Warning => 1,
        AlertSeverity::Critical => 2,
    }
}

fn candidates(snapshot: &Snapshot) -> Vec<Candidate> {
    let mut out = Vec::new();

    let cpu = snapshot.cpu.total_percent;
    if cpu.is_finite() && cpu >= CPU_WARN {
        out.push(Candidate {
            key: "cpu".into(),
            kind: AlertKind::HighCpu,
            severity: if cpu >= CPU_CRITICAL {
                AlertSeverity::Critical
            } else {
                AlertSeverity::Warning
            },
            title: "High CPU".into(),
            detail: format!("Total CPU {cpu:.0}% sustained for at least 5 seconds"),
            value: cpu,
            threshold: CPU_WARN,
            sustain_ms: CPU_SUSTAIN_MS,
        });
    }

    if snapshot.memory.total_bytes > 0 {
        let used = snapshot.memory.in_use_bytes as f32 / snapshot.memory.total_bytes as f32 * 100.0;
        if used >= MEM_WARN {
            out.push(Candidate {
                key: "mem".into(),
                kind: AlertKind::HighMemory,
                severity: if used >= MEM_CRITICAL {
                    AlertSeverity::Critical
                } else {
                    AlertSeverity::Warning
                },
                title: "Memory almost full".into(),
                detail: format!(
                    "RAM in use {used:.0}% ({} of {})",
                    crate::model::format_bytes(snapshot.memory.in_use_bytes),
                    crate::model::format_bytes(snapshot.memory.total_bytes)
                ),
                value: used,
                threshold: MEM_WARN,
                sustain_ms: MEM_SUSTAIN_MS,
            });
        }
    }

    for reading in snapshot
        .sensors
        .iter()
        .filter(|r| r.kind == SensorKind::Temperature)
    {
        if !reading.value.is_finite() {
            continue;
        }
        let (threshold, critical) = match reading.source {
            SensorSource::Smart => (DISK_TEMP_WARN, DISK_TEMP_CRITICAL),
            _ if reading.label.to_lowercase().contains("cpu") => {
                (CPU_TEMP_WARN, CPU_TEMP_WARN + 5.0)
            }
            _ if reading.label.to_lowercase().contains("gpu") => {
                (GPU_TEMP_WARN, GPU_TEMP_WARN + 5.0)
            }
            _ => (OTHER_TEMP_WARN, OTHER_TEMP_WARN + 5.0),
        };
        if reading.value >= threshold {
            out.push(Candidate {
                key: format!("temp:{}", reading.id),
                kind: AlertKind::HighTemperature,
                severity: if reading.value >= critical {
                    AlertSeverity::Critical
                } else {
                    AlertSeverity::Warning
                },
                title: format!("High temperature: {}", reading.label),
                detail: format!(
                    "{:.1} °C (ambang {:.0} °C, bertahan minimal 10 detik)",
                    reading.value, threshold
                ),
                value: reading.value,
                threshold,
                sustain_ms: TEMP_SUSTAIN_MS,
            });
        }
    }

    for volume in &snapshot.volumes {
        // Volume kecil (mis. partisi recovery 500 MB) tidak layak diperingatkan.
        if volume.total_bytes < 1024 * 1024 * 1024 {
            continue;
        }
        let free_pct = volume.free_bytes as f32 / volume.total_bytes as f32 * 100.0;
        if free_pct < FREE_SPACE_WARN {
            out.push(Candidate {
                key: format!("disk:{}", volume.mount),
                kind: AlertKind::LowDiskSpace,
                severity: if free_pct < FREE_SPACE_CRITICAL {
                    AlertSeverity::Critical
                } else {
                    AlertSeverity::Warning
                },
                title: format!("Low disk space: {}", volume.mount),
                detail: format!(
                    "{:.1}% free ({} of {})",
                    free_pct,
                    crate::model::format_bytes(volume.free_bytes),
                    crate::model::format_bytes(volume.total_bytes)
                ),
                value: free_pct,
                threshold: FREE_SPACE_WARN,
                sustain_ms: DISK_SUSTAIN_MS,
            });
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(ts: u64) -> Snapshot {
        let mut s = Snapshot::empty();
        s.ts_unix_ms = ts;
        s.memory.total_bytes = 100;
        s.memory.in_use_bytes = 10;
        s
    }

    #[test]
    fn short_spike_does_not_alert() {
        let mut engine = AlertEngine::new();
        let mut s = snapshot(1_000);
        s.cpu.total_percent = 99.0;
        assert!(engine.evaluate(&s).is_empty());
    }

    #[test]
    fn sustained_cpu_raises_once() {
        let mut engine = AlertEngine::new();
        let mut s = snapshot(1_000);
        s.cpu.total_percent = 95.0;
        engine.evaluate(&s);
        s.ts_unix_ms = 6_000;
        let events = engine.evaluate(&s);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].change, AlertChange::Raised);
        assert_eq!(events[0].alert.kind, AlertKind::HighCpu);

        // Masih tinggi: tidak ada event baru.
        s.ts_unix_ms = 7_000;
        assert!(engine.evaluate(&s).is_empty());
        assert_eq!(engine.active().len(), 1);
    }

    #[test]
    fn clearing_sets_cooldown() {
        let mut engine = AlertEngine::new();
        let mut s = snapshot(1_000);
        s.cpu.total_percent = 95.0;
        engine.evaluate(&s);
        s.ts_unix_ms = 6_000;
        engine.evaluate(&s);

        s.cpu.total_percent = 10.0;
        s.ts_unix_ms = 7_000;
        let events = engine.evaluate(&s);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].change, AlertChange::Cleared);
        assert!(engine.active().is_empty());

        // Naik lagi dalam cooldown: tidak ada notifikasi.
        s.cpu.total_percent = 95.0;
        s.ts_unix_ms = 8_000;
        engine.evaluate(&s);
        s.ts_unix_ms = 14_000;
        assert!(engine.evaluate(&s).is_empty());
    }

    #[test]
    fn missing_sensor_never_alerts() {
        let mut engine = AlertEngine::new();
        let s = snapshot(1_000);
        assert!(engine.evaluate(&s).is_empty());
        assert!(engine.active().is_empty());
    }

    #[test]
    fn hot_disk_uses_smart_threshold() {
        use crate::model::{Confidence, SensorReading, SensorSource, Unit};
        let mut engine = AlertEngine::new();
        let mut s = snapshot(1_000);
        s.sensors.push(SensorReading {
            id: "disk.0.temp".into(),
            kind: SensorKind::Temperature,
            label: "Disk 0 (SATA)".into(),
            value: 57.0,
            unit: Unit::Celsius,
            min: 57.0,
            max: 57.0,
            avg: 57.0,
            source: SensorSource::Smart,
            confidence: Confidence::Direct,
            ts_unix_ms: 1_000,
            disk_index: Some(0),
        });
        engine.evaluate(&s);
        s.ts_unix_ms = 11_000;
        let events = engine.evaluate(&s);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].alert.kind, AlertKind::HighTemperature);
    }

    #[test]
    fn tiny_volume_is_ignored() {
        use crate::model::VolumeData;
        let mut engine = AlertEngine::new();
        let mut s = snapshot(1_000);
        s.volumes.push(VolumeData {
            mount: "R:\\".into(),
            label: "Recovery".into(),
            fs: "NTFS".into(),
            total_bytes: 500 * 1024 * 1024,
            free_bytes: 0,
            disk_index: None,
        });
        assert!(engine.evaluate(&s).is_empty());
    }
}
