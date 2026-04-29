#![allow(dead_code)] // Diagnostic module; invoked on-demand via debug IPC commands and scheduler health checks

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct MemorySnapshot {
    /// RSS (Resident Set Size) in bytes
    pub rss_bytes: u64,
    pub heap_bytes: u64,
    pub timestamp: Instant,
}

#[derive(Debug)]
pub struct MemoryTracker {
    initial_rss: AtomicU64,
    peak_rss: AtomicU64,
    snapshots: parking_lot::Mutex<Vec<MemorySnapshot>>,
    start_time: Instant,
}

impl Default for MemoryTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryTracker {
    pub fn new() -> Self {
        let initial = get_current_rss().unwrap_or(0);
        Self {
            initial_rss: AtomicU64::new(initial),
            peak_rss: AtomicU64::new(initial),
            snapshots: parking_lot::Mutex::new(Vec::with_capacity(1000)),
            start_time: Instant::now(),
        }
    }

    pub fn record_snapshot(&self) -> Option<MemorySnapshot> {
        let rss = get_current_rss()?;
        let snapshot = MemorySnapshot {
            rss_bytes: rss,
            heap_bytes: 0, // platform-specific implementation pending
            timestamp: Instant::now(),
        };

        self.peak_rss.fetch_max(rss, Ordering::Relaxed);

        self.snapshots.lock().push(snapshot.clone());

        Some(snapshot)
    }

    pub fn analyze(&self) -> MemoryAnalysis {
        let snapshots = self.snapshots.lock();
        let initial = self.initial_rss.load(Ordering::Relaxed);
        let peak = self.peak_rss.load(Ordering::Relaxed);
        let current = snapshots.last().map(|s| s.rss_bytes).unwrap_or(initial);
        let elapsed = self.start_time.elapsed();

        let growth_rate = if snapshots.len() >= 2 {
            calculate_growth_rate(&snapshots)
        } else {
            0.0
        };

        MemoryAnalysis {
            initial_rss: initial,
            current_rss: current,
            peak_rss: peak,
            elapsed,
            growth_rate_bytes_per_sec: growth_rate,
            snapshot_count: snapshots.len(),
            leak_suspected: growth_rate > 1024.0, // suspicious above 1 KB/s
        }
    }

    pub fn log_analysis(&self) {
        let analysis = self.analyze();

        info!(
            "memory analysis: initial={:.2}MB, current={:.2}MB, peak={:.2}MB, growth={:.2}KB/s, elapsed={:.1}s",
            analysis.initial_rss as f64 / 1024.0 / 1024.0,
            analysis.current_rss as f64 / 1024.0 / 1024.0,
            analysis.peak_rss as f64 / 1024.0 / 1024.0,
            analysis.growth_rate_bytes_per_sec / 1024.0,
            analysis.elapsed.as_secs_f64()
        );

        if analysis.leak_suspected {
            warn!(
                "⚠️ 메모리 누수 의심: {:.2}KB/s 증가율",
                analysis.growth_rate_bytes_per_sec / 1024.0
            );
        }
    }
}

#[derive(Debug, Clone)]
pub struct MemoryAnalysis {
    pub initial_rss: u64,
    pub current_rss: u64,
    pub peak_rss: u64,
    pub elapsed: Duration,
    pub growth_rate_bytes_per_sec: f64,
    pub snapshot_count: usize,
    pub leak_suspected: bool,
}

impl MemoryAnalysis {
    pub fn growth_bytes(&self) -> i64 {
        self.current_rss as i64 - self.initial_rss as i64
    }

    pub fn growth_percent(&self) -> f64 {
        if self.initial_rss == 0 {
            return 0.0;
        }
        (self.current_rss as f64 - self.initial_rss as f64) / self.initial_rss as f64 * 100.0
    }
}

fn calculate_growth_rate(snapshots: &[MemorySnapshot]) -> f64 {
    if snapshots.len() < 2 {
        return 0.0;
    }

    let first_time = snapshots[0].timestamp;
    let n = snapshots.len() as f64;

    let mut sum_x = 0.0;
    let mut sum_y = 0.0;
    let mut sum_xy = 0.0;
    let mut sum_xx = 0.0;

    for s in snapshots {
        let x = s.timestamp.duration_since(first_time).as_secs_f64();
        let y = s.rss_bytes as f64;
        sum_x += x;
        sum_y += y;
        sum_xy += x * y;
        sum_xx += x * x;
    }

    let denominator = n * sum_xx - sum_x * sum_x;
    if denominator.abs() < f64::EPSILON {
        return 0.0;
    }

    (n * sum_xy - sum_x * sum_y) / denominator
}

#[cfg(target_os = "macos")]
pub fn get_current_rss() -> Option<u64> {
    use std::process::Command;

    let pid = std::process::id();
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;

    let rss_kb: u64 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .ok()?;

    Some(rss_kb * 1024) // KB to bytes
}

#[cfg(target_os = "linux")]
pub fn get_current_rss() -> Option<u64> {
    use std::fs;

    let status = fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if line.starts_with("VmRSS:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let rss_kb: u64 = parts[1].parse().ok()?;
                return Some(rss_kb * 1024);
            }
        }
    }
    None
}

#[cfg(target_os = "windows")]
pub fn get_current_rss() -> Option<u64> {
    None
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub fn get_current_rss() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_memory_tracker_basic() {
        let tracker = MemoryTracker::new();

        let snap1 = tracker.record_snapshot();
        assert!(snap1.is_some());

        thread::sleep(Duration::from_millis(10));

        let snap2 = tracker.record_snapshot();
        assert!(snap2.is_some());

        let analysis = tracker.analyze();
        assert_eq!(analysis.snapshot_count, 2);
        assert!(analysis.initial_rss > 0);
    }

    #[test]
    fn test_get_current_rss() {
        let rss = get_current_rss();
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            assert!(rss.is_some(), "RSS query failure");
            assert!(rss.unwrap() > 0, "RSS is 0");
        }
    }

    #[test]
    fn test_growth_rate_calculation() {
        let base = Instant::now();
        let snapshots = vec![
            MemorySnapshot {
                rss_bytes: 100_000_000,
                heap_bytes: 0,
                timestamp: base,
            },
            MemorySnapshot {
                rss_bytes: 101_000_000,
                heap_bytes: 0,
                timestamp: base + Duration::from_secs(1),
            },
            MemorySnapshot {
                rss_bytes: 102_000_000,
                heap_bytes: 0,
                timestamp: base + Duration::from_secs(2),
            },
        ];

        let rate = calculate_growth_rate(&snapshots);
        assert!((rate - 1_000_000.0).abs() < 10_000.0, "rate: {}", rate);
    }
}
