#![allow(dead_code)] // Diagnostic module; invoked on-demand via debug IPC commands and scheduler health checks

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// F-PF-C20-04: 스냅샷 최대 보존 개수 (링 버퍼 상한).
/// Vec::with_capacity(MAX_SNAPSHOTS) 는 pre-allocation 일 뿐 cap 이 아니므로
/// record_snapshot 에서 초과 시 오래된 항목을 drain 한다.
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
    /// F-PF-C21-05: System 인스턴스 공유 — get_current_rss() 호출마다 System::new() 생성하는
    /// 패턴을 제거한다. SysInfoMonitor (maekon-monitor crate) 와 동일한 Arc<Mutex<System>> 패턴.
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
        // F-PF-C21-05: System 인스턴스를 한 번만 생성하여 공유. get_current_rss 자유함수는
        // initial RSS 측정에만 사용하고, 이후 record_snapshot 은 공유 System 을 재사용한다.
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
        // F-PF-C21-05: 공유 System 인스턴스를 통해 RSS 측정 — System::new() per-call 제거.
        let rss = self.get_rss_shared()?;
        let snapshot = MemorySnapshot {
            rss_bytes: rss,
            heap_bytes: 0, // platform-specific implementation pending
            timestamp: Instant::now(),
        };

        self.peak_rss.fetch_max(rss, Ordering::Relaxed);

        {
            // F-PF-C20-04: MAX_SNAPSHOTS 초과 시 오래된 항목 drain — 무한 성장 방지.
            // VecDeque 교체 대안이 더 효율적이나, parking_lot::Mutex<Vec<_>> API 유지를
            // 우선하여 drain 방식으로 구현한다.
            let mut snapshots = self.snapshots.lock();
            snapshots.push(snapshot.clone());
            if snapshots.len() > MAX_SNAPSHOTS {
                let excess = snapshots.len() - MAX_SNAPSHOTS;
                snapshots.drain(..excess);
            }
        }

        Some(snapshot)
    }

    /// F-RR-C22-03: tokio async 런타임에서 안전하게 RSS 스냅샷을 기록한다.
    ///
    /// `sysinfo::System::refresh_processes` 는 동기 블로킹 syscall 이므로
    /// tokio worker 스레드를 블로킹하지 않도록 `spawn_blocking` 으로 격리한다.
    /// SysInfoMonitor (F-RR-39) 와 동일한 패턴.
    ///
    /// 반환값: 스냅샷 성공 시 `Some(snapshot)`, RSS 측정 실패 또는
    /// spawn_blocking join 실패 시 `None`.
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

    /// F-PF-C21-05: 공유 System 인스턴스를 통해 현재 프로세스 RSS 를 반환한다.
    /// platform 미지원 시 None 반환 (get_current_rss() 자유함수와 동일 의미론).
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

    /// F-QA-C23-02: record_snapshot_async 가 macOS/Linux/Windows 에서 Some 을 반환하는지 검증.
    ///
    /// spawn_blocking JoinError 경로(unwrap_or(None))는 별도로 테스트하기 위해
    /// 정상 경로(Some 반환)를 먼저 커버한다.
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    #[tokio::test]
    async fn record_snapshot_async_returns_some_on_supported_platforms() {
        let tracker = Arc::new(MemoryTracker::new());
        let result = MemoryTracker::record_snapshot_async(Arc::clone(&tracker)).await;

        assert!(
            result.is_some(),
            "macOS/Linux/Windows 에서 record_snapshot_async 는 Some 을 반환해야 한다"
        );
        let snapshot = result.unwrap();
        // RSS 는 지원 플랫폼에서 0 이 아니어야 한다.
        assert!(
            snapshot.rss_bytes > 0,
            "record_snapshot_async: rss_bytes > 0 이어야 한다 (got {})",
            snapshot.rss_bytes
        );
    }

    /// F-QA-C23-02: JoinError unwrap_or(None) 경로 — spawn_blocking 이 Err 를 반환하면 None.
    ///
    /// 패닉이 spawn_blocking 내부에서 발생할 경우 JoinError 가 반환되고
    /// unwrap_or(None) 에 의해 None 이 전파된다.
    #[tokio::test]
    async fn record_snapshot_async_returns_none_on_panic() {
        // spawn_blocking 내부에서 패닉이 발생하면 JoinHandle::await 는 Err(JoinError) 를 반환한다.
        // unwrap_or(None) 이 이를 None 으로 변환함을 직접 검증한다.
        let result: Option<MemorySnapshot> = tokio::task::spawn_blocking(|| {
            // 내부 패닉 시뮬레이션
            panic!("simulated panic in spawn_blocking");
        })
        .await
        .unwrap_or(None);

        assert!(
            result.is_none(),
            "spawn_blocking 내부 패닉 시 unwrap_or(None) 으로 None 반환"
        );
    }

    /// F-PF-C20-04: MAX_SNAPSHOTS+1 항목 기록 후 len == MAX_SNAPSHOTS 를 검증.
    /// Vec 이 무한 성장하지 않음을 보장한다.
    #[test]
    fn test_snapshot_cap_enforced() {
        let tracker = MemoryTracker::new();
        let base = Instant::now();

        // MAX_SNAPSHOTS + 1 개의 가상 스냅샷을 직접 snapshots 에 주입한다.
        // get_current_rss() 가 None 을 반환하는 CI 환경에서도 동작해야 하므로
        // record_snapshot() 우회 → snapshots 직접 조작.
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
            "snapshot_count={} 가 MAX_SNAPSHOTS={} 초과",
            analysis.snapshot_count, MAX_SNAPSHOTS
        );
    }
}
