//! Sampling loop: membangun `Snapshot` secara berkala dan membagikannya.

use crate::model::Snapshot;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Sumber snapshot yang aman dibaca dari thread UI tanpa lock.
pub struct SnapshotStore {
    inner: arc_swap::ArcSwap<Snapshot>,
    seq: AtomicU64,
}

impl Default for SnapshotStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SnapshotStore {
    pub fn new() -> Self {
        Self {
            inner: arc_swap::ArcSwap::from_pointee(Snapshot::empty()),
            seq: AtomicU64::new(0),
        }
    }

    pub fn load(&self) -> arc_swap::Guard<Arc<Snapshot>> {
        self.inner.load()
    }

    pub fn store(&self, snapshot: Snapshot) {
        self.seq.store(snapshot.seq, Ordering::Relaxed);
        self.inner.store(Arc::new(snapshot));
    }

    pub fn seq(&self) -> u64 {
        self.seq.load(Ordering::Relaxed)
    }
}

/// Interval sampling yang dipakai sampler.
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

/// Deteksi lompatan waktu (mis. setelah sleep/resume) yang membuat delta CPU
/// tidak masuk akal. Sampel pertama setelah lompatan dibuang.
pub fn is_time_jump(elapsed: Duration, expected: Duration) -> bool {
    elapsed > expected * 5
}

/// Helper untuk menghitung delta rate (byte/detik) dari dua pembacaan counter.
pub fn rate_per_sec(delta: u64, elapsed: Duration) -> u64 {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 {
        return 0;
    }
    (delta as f64 / secs) as u64
}

/// Waktu sekarang dalam milidetik sejak epoch.
pub fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Penghitung waktu untuk loop sampling dengan koreksi drift sederhana.
pub struct Ticker {
    next: Instant,
    last_tick: Instant,
    interval: Duration,
}

impl Ticker {
    pub fn new(interval: Duration) -> Self {
        let now = Instant::now();
        Self {
            next: now + interval,
            last_tick: now,
            interval,
        }
    }

    /// Tidur sampai tick berikutnya. Mengembalikan interval sebenarnya sejak
    /// tick terakhir (berguna kalau thread tertunda).
    pub fn tick(&mut self) -> Duration {
        let now = Instant::now();
        if self.next > now {
            std::thread::sleep(self.next - now);
        }
        let actual = Instant::now();
        let elapsed = actual.duration_since(self.last_tick);
        self.last_tick = actual;
        self.next += self.interval;
        if self.next < actual {
            // Tertinggal jauh (sleep/resume): jangan mengejar dengan burst.
            self.next = actual + self.interval;
        }
        elapsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_is_zero_for_zero_elapsed() {
        assert_eq!(rate_per_sec(1000, Duration::ZERO), 0);
    }

    #[test]
    fn rate_handles_one_second() {
        assert_eq!(rate_per_sec(1024, Duration::from_secs(1)), 1024);
    }

    #[test]
    fn rate_scales_with_interval() {
        assert_eq!(rate_per_sec(1024, Duration::from_millis(500)), 2048);
    }

    #[test]
    fn detects_sleep_resume_jump() {
        assert!(is_time_jump(Duration::from_secs(60), SAMPLE_INTERVAL));
        assert!(!is_time_jump(Duration::from_millis(1050), SAMPLE_INTERVAL));
    }

    #[test]
    fn store_and_load_roundtrip() {
        let store = SnapshotStore::new();
        let mut s = Snapshot::empty();
        s.seq = 7;
        store.store(s);
        assert_eq!(store.load().seq, 7);
        assert_eq!(store.seq(), 7);
    }
}
