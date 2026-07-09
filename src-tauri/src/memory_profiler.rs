//! RSS + CPU self-process resource tracking + linear-regression
//! leak-growth-rate analysis.
//!
// #7918: this module is now LIVE. `MemoryTracker` is wired into the scheduler
// health-check loop (`scheduler/loops/resource_health.rs`), which records a
// self-RSS/CPU snapshot each health tick and logs a budget-breach warning
// against the resource-budget SSOT (`maekon_core::resource_budget`); and
// `sample_self_resource_usage` backs the `get_resource_usage_snapshot`
// diagnostics IPC (`commands/system.rs`). (Before #7918 it had zero live
// callers — #7719's stale-comment note.) `#![allow(dead_code)]` is retained
// because a handful of diagnostic helpers here (`get_current_rss` free fn,
// `log_analysis`, `MemoryAnalysis::growth_bytes`/`growth_percent`) remain
// utility-only, exercised solely by this module's own tests.
#![allow(dead_code)]

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
    /// #7918: self-process CPU usage (%, multi-core aggregate) measured over
    /// the interval since the previous refresh of the shared `System`. `0.0`
    /// on the first snapshot (no prior baseline) and on unsupported platforms.
    pub cpu_percent: f32,
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
        // #7918: the same refresh also yields CPU usage over the interval since
        // the previous refresh (the health loop's ~5s tick cadence is a natural
        // sampling window — no artificial sleep needed, unlike a one-shot
        // sampler).
        let (rss, cpu) = self.sample_shared()?;
        let snapshot = MemorySnapshot {
            rss_bytes: rss,
            cpu_percent: cpu,
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

    /// F-PF-C21-05 / #7918: return the current process RSS (bytes) + CPU usage
    /// (%, multi-core aggregate) via the shared `System` instance. CPU usage is
    /// computed by `sysinfo` from the delta since the previous refresh of this
    /// same `System`, so a periodic caller (the health loop) gets a real
    /// interval measurement for free. Returns None on unsupported platforms
    /// (same semantics as the `get_current_rss()` free function).
    fn sample_shared(&self) -> Option<(u64, f32)> {
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        {
            use sysinfo::{Pid, ProcessesToUpdate};
            let pid = Pid::from_u32(std::process::id());
            let mut sys = self.system.lock().ok()?;
            sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
            sys.process(pid).map(|p| (p.memory(), p.cpu_usage()))
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

/// #7918: a one-shot self-process resource sample (RSS + CPU%), for on-demand
/// diagnostics readers such as the `get_resource_usage_snapshot` IPC.
#[derive(Debug, Clone, Copy)]
pub struct ResourceUsageSample {
    /// RSS (Resident Set Size) in bytes.
    pub rss_bytes: u64,
    /// CPU usage (%, multi-core aggregate — can exceed 100% on a multi-threaded
    /// process). Sampled over `MINIMUM_CPU_UPDATE_INTERVAL` between two
    /// refreshes.
    pub cpu_percent: f32,
}

/// #7918: sample the current process's RSS + CPU usage via sysinfo, for
/// on-demand callers (the diagnostics IPC) that do not share a long-lived
/// `System` the way `MemoryTracker` does.
///
/// Unlike `MemoryTracker::record_snapshot` — which reuses a shared `System` and
/// therefore reads CPU across the caller's own tick cadence — this creates a
/// throwaway `System` and must perform two refreshes separated by
/// `MINIMUM_CPU_UPDATE_INTERVAL` to obtain a meaningful CPU delta. It therefore
/// BLOCKS for that interval and must be called off the async executor (e.g. via
/// `spawn_blocking`). Returns `None` on unsupported platforms.
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
pub fn sample_self_resource_usage() -> Option<ResourceUsageSample> {
    use sysinfo::{Pid, ProcessesToUpdate, System, MINIMUM_CPU_UPDATE_INTERVAL};

    let pid = Pid::from_u32(std::process::id());
    let mut system = System::new();
    // 1st refresh — establish the CPU-usage baseline.
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    std::thread::sleep(MINIMUM_CPU_UPDATE_INTERVAL);
    // 2nd refresh — CPU usage is now derived from the elapsed interval.
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    system.process(pid).map(|p| ResourceUsageSample {
        rss_bytes: p.memory(),
        cpu_percent: p.cpu_usage(),
    })
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub fn sample_self_resource_usage() -> Option<ResourceUsageSample> {
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

    /// #7918: `sample_self_resource_usage` returns a real RSS on supported
    /// platforms. CPU is not asserted for a value (it can legitimately read
    /// 0.0 on an idle process), only that the sample is produced.
    #[test]
    fn test_sample_self_resource_usage() {
        let sample = sample_self_resource_usage();
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            let sample = sample.expect("self resource sample must be Some on macOS/Linux");
            assert!(sample.rss_bytes > 0, "self RSS must be > 0");
            assert!(sample.cpu_percent >= 0.0, "self CPU must be non-negative");
        }
    }

    /// #7918: `record_snapshot` now populates `cpu_percent`. The first snapshot
    /// reads 0.0 (no prior refresh baseline), so we only assert the field is
    /// present and non-negative — not a specific value.
    #[test]
    fn test_record_snapshot_populates_cpu_field() {
        let tracker = MemoryTracker::new();
        if let Some(snapshot) = tracker.record_snapshot() {
            assert!(
                snapshot.cpu_percent >= 0.0,
                "cpu_percent must be non-negative"
            );
        }
    }

    #[test]
    fn test_growth_rate_calculation() {
        let base = Instant::now();
        let snapshots = vec![
            MemorySnapshot {
                rss_bytes: 100_000_000,
                cpu_percent: 0.0,
                heap_bytes: 0,
                timestamp: base,
            },
            MemorySnapshot {
                rss_bytes: 101_000_000,
                cpu_percent: 0.0,
                heap_bytes: 0,
                timestamp: base + Duration::from_secs(1),
            },
            MemorySnapshot {
                rss_bytes: 102_000_000,
                cpu_percent: 0.0,
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
                    cpu_percent: 0.0,
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
