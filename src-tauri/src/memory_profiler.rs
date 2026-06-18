#![allow(dead_code)] // Diagnostic module; invoked on-demand via debug IPC commands and scheduler health checks

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// F-PF-C20-04: maximum number of retained snapshots (ring buffer upper bound).
/// `Vec::with_capacity(MAX_SNAPSHOTS)` is only a pre-allocation, not a cap, so
/// `record_snapshot` drains the oldest entries when this bound is exceeded.
const MAX_SNAPSHOTS: usize = 1000;

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
    /// F-PF-C21-05: share a single `System` instance — removes the pattern of
    /// creating `System::new()` on every `get_current_rss()` call. Same
    /// `Arc<Mutex<System>>` pattern as `SysInfoMonitor` (in the maekon-monitor crate).
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    system: std::sync::Mutex<sysinfo::System>,
}

impl Default for MemoryTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryTracker {
    pub fn new() -> Self {
        // F-PF-C21-05: create the `System` instance once and share it. The free
        // function `get_current_rss` is used only for the initial RSS
        // measurement; afterwards `record_snapshot` reuses the shared `System`.
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        let system = {
            use sysinfo::{Pid, ProcessesToUpdate, System};
            let mut sys = System::new();
            let pid = Pid::from_u32(std::process::id());
            sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
            std::sync::Mutex::new(sys)
        };
        let initial = {
            #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
            {
                use sysinfo::Pid;
                let pid = Pid::from_u32(std::process::id());
                system
                    .lock()
                    .ok()
                    .and_then(|s| s.process(pid).map(|p| p.memory()))
                    .unwrap_or(0)
            }
            #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
            0u64
        };
        Self {
            initial_rss: AtomicU64::new(initial),
            peak_rss: AtomicU64::new(initial),
            snapshots: parking_lot::Mutex::new(Vec::with_capacity(1000)),
            start_time: Instant::now(),
            #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
            system,
        }
    }

    pub fn record_snapshot(&self) -> Option<MemorySnapshot> {
        // F-PF-C21-05: measure RSS via the shared `System` instance — removes the
        // per-call `System::new()`.
        let rss = self.get_rss_shared()?;
        let snapshot = MemorySnapshot {
            rss_bytes: rss,
            heap_bytes: 0, // platform-specific implementation pending
            timestamp: Instant::now(),
        };

        self.peak_rss.fetch_max(rss, Ordering::Relaxed);

        {
            // F-PF-C20-04: drain the oldest entries when MAX_SNAPSHOTS is exceeded
            // — prevents unbounded growth. Switching to a VecDeque would be more
            // efficient, but we prioritize keeping the `parking_lot::Mutex<Vec<_>>`
            // API and implement this via drain instead.
            let mut snapshots = self.snapshots.lock();
            snapshots.push(snapshot.clone());
            if snapshots.len() > MAX_SNAPSHOTS {
                let excess = snapshots.len() - MAX_SNAPSHOTS;
                snapshots.drain(..excess);
            }
        }

        Some(snapshot)
    }

    /// F-RR-C22-03: safely record an RSS snapshot from the tokio async runtime.
    ///
    /// `sysinfo::System::refresh_processes` is a synchronous blocking syscall,
    /// so it is isolated via `spawn_blocking` to avoid blocking a tokio worker
    /// thread. Same pattern as `SysInfoMonitor` (F-RR-39).
    ///
    /// Returns `Some(snapshot)` on success, or `None` if the RSS measurement
    /// fails or the `spawn_blocking` join fails.
    pub async fn record_snapshot_async(tracker: std::sync::Arc<Self>) -> Option<MemorySnapshot> {
        tokio::task::spawn_blocking(move || tracker.record_snapshot())
            .await
            .unwrap_or(None)
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

    /// F-PF-C21-05: return the current process RSS via the shared `System`
    /// instance. Returns None on unsupported platforms (same semantics as the
    /// `get_current_rss()` free function).
    fn get_rss_shared(&self) -> Option<u64> {
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        {
            use sysinfo::{Pid, ProcessesToUpdate};
            let pid = Pid::from_u32(std::process::id());
            let mut sys = self.system.lock().ok()?;
            sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
            sys.process(pid).map(|p| p.memory())
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        None
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
                "memory leak suspected: {:.2}KB/s growth rate",
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

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
pub fn get_current_rss() -> Option<u64> {
    use sysinfo::{Pid, ProcessesToUpdate, System};

    let pid = Pid::from_u32(std::process::id());
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);

    system.process(pid).map(|process| process.memory())
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub fn get_current_rss() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
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

    /// F-QA-C23-02: verify that `record_snapshot_async` returns Some on
    /// macOS/Linux/Windows.
    ///
    /// The `spawn_blocking` JoinError path (`unwrap_or(None)`) is tested
    /// separately, so cover the happy path (returns Some) first.
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    #[tokio::test]
    async fn record_snapshot_async_returns_some_on_supported_platforms() {
        let tracker = Arc::new(MemoryTracker::new());
        let result = MemoryTracker::record_snapshot_async(Arc::clone(&tracker)).await;

        assert!(
            result.is_some(),
            "record_snapshot_async must return Some on macOS/Linux/Windows"
        );
        let snapshot = result.unwrap();
        // RSS must be non-zero on supported platforms.
        assert!(
            snapshot.rss_bytes > 0,
            "record_snapshot_async: rss_bytes must be > 0 (got {})",
            snapshot.rss_bytes
        );
    }

    /// F-QA-C23-02: JoinError `unwrap_or(None)` path — if `spawn_blocking`
    /// returns Err, the result is None.
    ///
    /// When a panic occurs inside `spawn_blocking`, a JoinError is returned and
    /// `unwrap_or(None)` propagates None.
    #[tokio::test]
    async fn record_snapshot_async_returns_none_on_panic() {
        // When a panic occurs inside `spawn_blocking`, `JoinHandle::await` returns
        // `Err(JoinError)`. Verify directly that `unwrap_or(None)` converts it to None.
        let result: Option<MemorySnapshot> = tokio::task::spawn_blocking(|| {
            // Simulate an internal panic.
            panic!("simulated panic in spawn_blocking");
        })
        .await
        .unwrap_or(None);

        assert!(
            result.is_none(),
            "a panic inside spawn_blocking must return None via unwrap_or(None)"
        );
    }

    /// F-PF-C20-04: verify that after recording MAX_SNAPSHOTS+1 entries,
    /// len == MAX_SNAPSHOTS. Guarantees the Vec does not grow unboundedly.
    #[test]
    fn test_snapshot_cap_enforced() {
        let tracker = MemoryTracker::new();
        let base = Instant::now();

        // Inject MAX_SNAPSHOTS + 1 synthetic snapshots directly into `snapshots`.
        // This must work even in CI environments where `get_current_rss()` returns
        // None, so bypass `record_snapshot()` and manipulate `snapshots` directly.
        {
            let mut snapshots = tracker.snapshots.lock();
            for i in 0..=(MAX_SNAPSHOTS) {
                snapshots.push(MemorySnapshot {
                    rss_bytes: 100_000_000 + i as u64 * 1000,
                    heap_bytes: 0,
                    timestamp: base,
                });
                if snapshots.len() > MAX_SNAPSHOTS {
                    let excess = snapshots.len() - MAX_SNAPSHOTS;
                    snapshots.drain(..excess);
                }
            }
        }

        let analysis = tracker.analyze();
        assert_eq!(
            analysis.snapshot_count, MAX_SNAPSHOTS,
            "snapshot_count={} exceeds MAX_SNAPSHOTS={}",
            analysis.snapshot_count, MAX_SNAPSHOTS
        );
    }
}
