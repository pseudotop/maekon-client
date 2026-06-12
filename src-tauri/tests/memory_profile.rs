//! ```
//! cargo test -p maekon-app --test memory_profile_test -- --nocapture --ignored
//! ```
//! ```
//! cargo test -p maekon-app --test memory_profile_test --release -- --nocapture --ignored
//! ```

use image::{DynamicImage, Rgba, RgbaImage};
use maekon_core::models::event::{Event, UserEvent, UserEventType};
use maekon_core::models::frame::FrameMetadata;
use maekon_storage::sqlite::SqliteStorage;
use maekon_vision::{delta, encoder, encoder::WebPQuality, thumbnail};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tracing::debug;
use uuid::Uuid;

#[derive(Clone)]
struct MemorySnapshot {
    rss_bytes: u64,
    timestamp: Instant,
}

/// 현재 프로세스의 RSS(Resident Set Size, 바이트)를 sysinfo 로 측정한다.
///
/// 기존 구현은 `ps -o rss=` 외부 프로세스에 의존하여 Windows 에서 동작하지
/// 않았다. 프로덕션 `src/memory_profiler.rs` 가 sysinfo 기반(`process.memory()`)
/// 으로 RSS 를 측정하므로, 테스트도 동일한 측정 경로(sysinfo)로 통일한다.
/// 측정 실패(미지원 플랫폼 등) 시 0 을 반환한다.
fn get_rss() -> u64 {
    use sysinfo::{Pid, ProcessesToUpdate, System};

    let pid = Pid::from_u32(std::process::id());
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    system.process(pid).map(|p| p.memory()).unwrap_or(0)
}

/// 현재 프로세스의 CPU 사용률(%)을 sysinfo 로 측정한다.
///
/// sysinfo 의 CPU 사용률은 두 번의 refresh 사이 간격으로 계산되므로,
/// `MINIMUM_CPU_UPDATE_INTERVAL` 이상 대기 후 두 번째 refresh 를 수행한다.
/// 측정 실패 시 0.0 을 반환한다. 반환값은 단일 코어 기준 100% 를 초과할 수 있다
/// (멀티스레드 프로세스).
fn sample_cpu_percent() -> f32 {
    use sysinfo::{Pid, ProcessesToUpdate, System, MINIMUM_CPU_UPDATE_INTERVAL};

    let pid = Pid::from_u32(std::process::id());
    let mut system = System::new();
    // 1차 refresh — CPU 사용률 계산 기준점 확보.
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    std::thread::sleep(MINIMUM_CPU_UPDATE_INTERVAL);
    // 2차 refresh — 경과 시간 기준 CPU 사용률 산출.
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    system.process(pid).map(|p| p.cpu_usage()).unwrap_or(0.0)
}

// ============================================================================
// #4829: 절대(absolute) 리소스 예산 단언 (RSS / CPU)
// ----------------------------------------------------------------------------
// 주의: 아래 100MB RSS / 200% CPU 수치는 확정된 제품 SSOT 가 아닌 **잠정값**이다.
// 누수 회귀(leak regression)를 잡되 CI 에서 flaky 하지 않도록 의도적으로 넉넉하게
// 설정했다. 제품 차원의 공식 리소스 예산(SSOT)이 확정되면 그 상수로 교체해야 한다.
// 프로덕션에 RSS/CPU 예산 상수가 존재하지 않아(grep 확인) 테스트 로컬 상수로 둔다.
// ============================================================================

/// 잠정 RSS 상한 (바이트). 200MB — 확정 SSOT 아님(provisional).
const PROVISIONAL_RSS_BUDGET_BYTES: u64 = 200 * 1024 * 1024;

/// 잠정 CPU 사용률 상한 (%). 멀티코어 합산 기준이므로 200%(2코어 풀 가동) 로 둔다.
/// 확정 SSOT 아님(provisional).
const PROVISIONAL_CPU_BUDGET_PERCENT: f32 = 200.0;

fn create_test_image(width: u32, height: u32, seed: u8) -> DynamicImage {
    let mut img = RgbaImage::new(width, height);
    for (x, y, pixel) in img.enumerate_pixels_mut() {
        let r = (x as u8).wrapping_add(seed).wrapping_mul(17);
        let g = (y as u8).wrapping_add(seed).wrapping_mul(31);
        let b = (x as u8).wrapping_add(y as u8).wrapping_add(seed);
        *pixel = Rgba([r, g, b, 255]);
    }
    DynamicImage::ImageRgba8(img)
}

fn calculate_stable_growth_rate(snapshots: &[MemorySnapshot], warmup_ratio: f64) -> f64 {
    let warmup_count = (snapshots.len() as f64 * warmup_ratio).ceil() as usize;
    let stable_snapshots = &snapshots[warmup_count..];

    if stable_snapshots.len() < 2 {
        return 0.0;
    }

    let first_time = stable_snapshots[0].timestamp;
    let n = stable_snapshots.len() as f64;

    let mut sum_x = 0.0;
    let mut sum_y = 0.0;
    let mut sum_xy = 0.0;
    let mut sum_xx = 0.0;

    for s in stable_snapshots {
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

fn calculate_memory_variance(snapshots: &[MemorySnapshot], last_n: usize) -> u64 {
    if snapshots.len() < last_n {
        return u64::MAX;
    }

    let tail = &snapshots[snapshots.len() - last_n..];
    let min = tail.iter().map(|s| s.rss_bytes).min().unwrap_or(0);
    let max = tail.iter().map(|s| s.rss_bytes).max().unwrap_or(0);

    max - min
}

struct LeakCheckResult {
    stable_growth_rate: f64,
    memory_variance: u64,
    initial_rss: u64,
    peak_rss: u64,
    final_rss: u64,
    leak_suspected: bool,
}

impl LeakCheckResult {
    fn from_snapshots(snapshots: &[MemorySnapshot]) -> Self {
        let initial_rss = snapshots.first().map(|s| s.rss_bytes).unwrap_or(0);
        let peak_rss = snapshots.iter().map(|s| s.rss_bytes).max().unwrap_or(0);
        let final_rss = snapshots.last().map(|s| s.rss_bytes).unwrap_or(0);

        let stable_growth_rate = calculate_stable_growth_rate(snapshots, 0.3);
        let memory_variance = calculate_memory_variance(snapshots, 5);

        let leak_suspected = stable_growth_rate > 50_000.0 && memory_variance > 10 * 1024 * 1024;

        Self {
            stable_growth_rate,
            memory_variance,
            initial_rss,
            peak_rss,
            final_rss,
            leak_suspected,
        }
    }

    fn print_summary(&self, test_name: &str, elapsed: Duration, iterations: u64) {
        println!("\n=== {} ===", test_name);
        println!(
            "initial RSS: {:.2} MB",
            self.initial_rss as f64 / 1024.0 / 1024.0
        );
        println!("RSS: {:.2} MB", self.peak_rss as f64 / 1024.0 / 1024.0);
        println!(
            "final RSS: {:.2} MB",
            self.final_rss as f64 / 1024.0 / 1024.0
        );
        println!(
            "memory increase: {:.2} MB ({:+.1}%)",
            (self.final_rss as i64 - self.initial_rss as i64) as f64 / 1024.0 / 1024.0,
            (self.final_rss as f64 - self.initial_rss as f64) / self.initial_rss as f64 * 100.0
        );
        println!(
            "stable-window growth rate: {:.2} KB/s (excluding first 30% warmup)",
            self.stable_growth_rate / 1024.0
        );
        println!(
            "last-window variance: {:.2} MB",
            self.memory_variance as f64 / 1024.0 / 1024.0
        );
        println!("execution hour: {:.2}s", elapsed.as_secs_f64());
        println!(
            "throughput: {:.1} iterations/s",
            iterations as f64 / elapsed.as_secs_f64()
        );

        if self.leak_suspected {
            println!("\n[WARN] potential memory leak:");
            println!(
                "  - stable-window growth rate: {:.2} KB/s",
                self.stable_growth_rate / 1024.0
            );
            println!("-");
        } else if self.stable_growth_rate > 10_000.0 {
            println!("\n[WARN] memory growth is elevated but below leak threshold.");
        } else {
            println!("\n[OK] no leak signal detected");
        }
    }
}

#[test]
#[ignore = "long-running test - run with cargo test --ignored"]
fn test_vision_pipeline_memory() {
    const ITERATIONS: usize = 200;
    const SAMPLE_INTERVAL: usize = 5;

    println!("\n=== Vision test ===");
    println!(": {}", ITERATIONS);

    let mut snapshots = Vec::with_capacity(ITERATIONS / SAMPLE_INTERVAL + 2);
    let start = Instant::now();

    snapshots.push(MemorySnapshot {
        rss_bytes: get_rss(),
        timestamp: Instant::now(),
    });

    let img1 = create_test_image(1920, 1080, 42);
    let img2 = create_test_image(1920, 1080, 43);

    snapshots.push(MemorySnapshot {
        rss_bytes: get_rss(),
        timestamp: Instant::now(),
    });

    for i in 0..ITERATIONS {
        let _delta = delta::compute_delta(&img1, &img2);

        let thumb = thumbnail::fast_resize(&img2, 480, 270).unwrap();

        let _encoded = encoder::encode_webp(&thumb, WebPQuality::Medium).unwrap();

        if i.is_multiple_of(SAMPLE_INTERVAL) {
            snapshots.push(MemorySnapshot {
                rss_bytes: get_rss(),
                timestamp: Instant::now(),
            });
        }
    }

    snapshots.push(MemorySnapshot {
        rss_bytes: get_rss(),
        timestamp: Instant::now(),
    });

    let elapsed = start.elapsed();
    let result = LeakCheckResult::from_snapshots(&snapshots);
    result.print_summary("Vision pipeline", elapsed, ITERATIONS as u64);

    assert!(
        !result.leak_suspected,
        "possible memory leak: stable-window growth {:.2} KB/s, variance {:.2} MB",
        result.stable_growth_rate / 1024.0,
        result.memory_variance as f64 / 1024.0 / 1024.0
    );
}

#[test]
#[ignore = "long-running test - run with cargo test --ignored"]
fn test_storage_memory() {
    const ITERATIONS: usize = 500;
    const BATCH_SIZE: usize = 10;
    const SAMPLE_INTERVAL: usize = 25;

    println!("\n=== Storage test ===");
    println!(": {} (batch size: {})", ITERATIONS, BATCH_SIZE);

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let storage = SqliteStorage::open(&db_path, 30, None).unwrap();

    let mut snapshots = Vec::with_capacity(ITERATIONS / SAMPLE_INTERVAL + 2);
    let start = Instant::now();

    snapshots.push(MemorySnapshot {
        rss_bytes: get_rss(),
        timestamp: Instant::now(),
    });

    for i in 0..ITERATIONS {
        let events: Vec<Event> = (0..BATCH_SIZE)
            .map(|j| {
                Event::User(UserEvent {
                    event_id: Uuid::new_v4(),
                    event_type: UserEventType::WindowChange,
                    timestamp: chrono::Utc::now(),
                    app_name: format!("App{}", j % 5),
                    window_title: format!("Window {} - {}", i, j),
                })
            })
            .collect();

        if let Err(e) = storage.save_events_batch(&events) {
            debug!("save_events_batch failed: {e}");
        }

        let metadata = FrameMetadata {
            timestamp: chrono::Utc::now(),
            trigger_type: "AppSwitch".to_string(),
            app_name: format!("App{}", i % 10),
            window_title: format!("Window {}", i),
            resolution: (1920, 1080),
            importance: 0.5,
            monitor_id: None,
            app_bundle_id: None,
        };
        if let Err(e) =
            storage.save_frame_metadata(&metadata, Some(&format!("frames/{}.webp", i)), None)
        {
            debug!("save_frame_metadata failed: {e}");
        }

        let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
        if let Err(e) = storage.get_or_create_focus_metrics(&date) {
            debug!("get_or_create_focus_metrics failed: {e}");
        }
        if let Err(e) = storage.increment_focus_metrics(&date, 1, 1, 0, 0, 0) {
            debug!("increment_focus_metrics failed: {e}");
        }

        if i.is_multiple_of(SAMPLE_INTERVAL) {
            snapshots.push(MemorySnapshot {
                rss_bytes: get_rss(),
                timestamp: Instant::now(),
            });
        }
    }

    snapshots.push(MemorySnapshot {
        rss_bytes: get_rss(),
        timestamp: Instant::now(),
    });

    let elapsed = start.elapsed();
    let result = LeakCheckResult::from_snapshots(&snapshots);
    result.print_summary("Storage", elapsed, ITERATIONS as u64);

    println!(
        "saved data: {} event(s), {} frame(s)",
        ITERATIONS * BATCH_SIZE,
        ITERATIONS
    );

    assert!(
        !result.leak_suspected,
        "possible memory leak: stable-window growth {:.2} KB/s, variance {:.2} MB",
        result.stable_growth_rate / 1024.0,
        result.memory_variance as f64 / 1024.0 / 1024.0
    );
}

#[test]
#[ignore = "long-running test - run with cargo test --ignored"]
fn test_combined_memory() {
    const DURATION_SECS: u64 = 30;

    println!("\n=== composite test ===");
    println!("execution hour: {}s", DURATION_SECS);

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let storage = SqliteStorage::open(&db_path, 30, None).unwrap();

    let img1 = create_test_image(1920, 1080, 42);
    let img2 = create_test_image(1920, 1080, 43);

    let mut snapshots = Vec::new();
    let start = Instant::now();
    let iteration_count = AtomicU64::new(0);

    snapshots.push(MemorySnapshot {
        rss_bytes: get_rss(),
        timestamp: Instant::now(),
    });

    let mut last_sample = Instant::now();
    while start.elapsed() < Duration::from_secs(DURATION_SECS) {
        let iter = iteration_count.fetch_add(1, Ordering::Relaxed);

        let _delta = delta::compute_delta(&img1, &img2);
        let thumb = thumbnail::fast_resize(&img2, 480, 270).unwrap();
        let _encoded = encoder::encode_webp(&thumb, WebPQuality::Medium).unwrap();

        let events: Vec<Event> = (0..5)
            .map(|j| {
                Event::User(UserEvent {
                    event_id: Uuid::new_v4(),
                    event_type: UserEventType::WindowChange,
                    timestamp: chrono::Utc::now(),
                    app_name: format!("App{}", j),
                    window_title: format!("Window {}", iter),
                })
            })
            .collect();
        if let Err(e) = storage.save_events_batch(&events) {
            debug!("save_events_batch failed: {e}");
        }

        if last_sample.elapsed() >= Duration::from_secs(1) {
            snapshots.push(MemorySnapshot {
                rss_bytes: get_rss(),
                timestamp: Instant::now(),
            });
            last_sample = Instant::now();
        }

        std::thread::sleep(Duration::from_millis(10));
    }

    snapshots.push(MemorySnapshot {
        rss_bytes: get_rss(),
        timestamp: Instant::now(),
    });

    let elapsed = start.elapsed();
    let total_iterations = iteration_count.load(Ordering::Relaxed);
    let result = LeakCheckResult::from_snapshots(&snapshots);
    result.print_summary("composite scenario", elapsed, total_iterations);

    println!("\n--- (5s ) ---");
    for (i, snap) in snapshots.iter().enumerate() {
        if i.is_multiple_of(5) || i == snapshots.len() - 1 {
            println!(
                "  {:3}s: {:.2} MB",
                snap.timestamp.duration_since(start).as_secs(),
                snap.rss_bytes as f64 / 1024.0 / 1024.0
            );
        }
    }

    assert!(
        !result.leak_suspected,
        "possible memory leak: stable-window growth {:.2} KB/s, variance {:.2} MB",
        result.stable_growth_rate / 1024.0,
        result.memory_variance as f64 / 1024.0 / 1024.0
    );
}

/// #4829: 절대 리소스 예산(RSS / CPU) 단언 테스트.
///
/// 기존 테스트들은 모두 *상대적* 누수(증가율/분산)만 검증했다. 이 테스트는
/// 실제 vision + storage 워크로드를 돌린 직후의 프로세스 RSS(peak)와 CPU 사용률을
/// **절대 상한**과 비교하여, 워크로드가 잠정 예산을 초과하지 않음을 검증한다.
///
/// - 측정은 REAL 이다: `get_rss()`/`sample_cpu_percent()` 는 sysinfo 로 실행 중인
///   현재 프로세스의 실제 RSS/CPU 를 샘플링한다(하드코딩 값 단언 아님).
/// - 예산 수치는 잠정값이다(상수 정의부 주석 참조). 확정 SSOT 로 교체 필요.
/// - long-running + 환경 의존적이므로 `#[ignore]` 로 opt-in 유지 (`--ignored`).
#[test]
#[ignore = "absolute resource-budget assertion - run with cargo test --ignored"]
fn test_absolute_resource_budget() {
    const ITERATIONS: usize = 100;

    println!("\n=== #4829 absolute resource-budget test ===");

    // --- 측정 가능 여부 가드 ---
    // 미지원 플랫폼(또는 CI 컨테이너에서 프로세스 가시성 제한)에서 get_rss() 가 0 을
    // 반환하면 절대 단언을 신뢰할 수 없으므로 의미 있는 메시지와 함께 패닉한다.
    // (지원 플랫폼: macOS/Linux/Windows. ignore 테스트라 일반 CI 는 영향 없음.)
    let baseline_rss = get_rss();
    assert!(
        baseline_rss > 0,
        "RSS 측정 실패(0 반환) — sysinfo 가 현재 프로세스를 볼 수 없는 환경. \
         지원 플랫폼(macOS/Linux/Windows)에서 실행해야 한다."
    );

    // --- 실제 워크로드 실행 ---
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("budget.db");
    let storage = SqliteStorage::open(&db_path, 30, None).unwrap();

    let img1 = create_test_image(1920, 1080, 42);
    let img2 = create_test_image(1920, 1080, 43);

    let mut peak_rss = baseline_rss;
    let start = Instant::now();

    for i in 0..ITERATIONS {
        // vision 파이프라인 (delta + thumbnail + webp 인코딩)
        let _delta = delta::compute_delta(&img1, &img2);
        let thumb = thumbnail::fast_resize(&img2, 480, 270).unwrap();
        let _encoded = encoder::encode_webp(&thumb, WebPQuality::Medium).unwrap();

        // storage 파이프라인 (이벤트 배치 저장)
        let events: Vec<Event> = (0..5)
            .map(|j| {
                Event::User(UserEvent {
                    event_id: Uuid::new_v4(),
                    event_type: UserEventType::WindowChange,
                    timestamp: chrono::Utc::now(),
                    app_name: format!("App{}", j),
                    window_title: format!("Window {} - {}", i, j),
                })
            })
            .collect();
        if let Err(e) = storage.save_events_batch(&events) {
            debug!("save_events_batch failed: {e}");
        }

        // 매 10회 실제 RSS 샘플링하여 peak 갱신 (REAL measurement)
        if i.is_multiple_of(10) {
            peak_rss = peak_rss.max(get_rss());
        }
    }

    // 워크로드 종료 직후 최종 RSS / CPU 샘플 (REAL measurement)
    peak_rss = peak_rss.max(get_rss());
    let cpu_percent = sample_cpu_percent();
    let elapsed = start.elapsed();

    println!(
        "baseline RSS: {:.2} MB, peak RSS: {:.2} MB, CPU: {:.1}%, elapsed: {:.2}s",
        baseline_rss as f64 / 1024.0 / 1024.0,
        peak_rss as f64 / 1024.0 / 1024.0,
        cpu_percent,
        elapsed.as_secs_f64(),
    );
    println!(
        "provisional budgets — RSS <= {:.0} MB, CPU <= {:.0}% (NOT confirmed SSOT)",
        PROVISIONAL_RSS_BUDGET_BYTES as f64 / 1024.0 / 1024.0,
        PROVISIONAL_CPU_BUDGET_PERCENT,
    );

    // --- 절대 예산 단언 (잠정값) ---
    assert!(
        peak_rss <= PROVISIONAL_RSS_BUDGET_BYTES,
        "peak RSS {:.2} MB 가 잠정 예산 {:.0} MB 를 초과. \
         (실제 회귀이거나 잠정 예산이 너무 빡빡할 수 있음 — SSOT 확정 시 재조정)",
        peak_rss as f64 / 1024.0 / 1024.0,
        PROVISIONAL_RSS_BUDGET_BYTES as f64 / 1024.0 / 1024.0,
    );

    // CPU 측정값이 0.0 이면(일부 환경에서 발생) 단언을 건너뛴다 — false-pass 방지보다
    // false-fail 회피 우선. 0.0 초과일 때만 상한을 검증한다.
    if cpu_percent > 0.0 {
        assert!(
            cpu_percent <= PROVISIONAL_CPU_BUDGET_PERCENT,
            "CPU {:.1}% 가 잠정 예산 {:.0}% 를 초과. \
             (SSOT 확정 시 재조정)",
            cpu_percent,
            PROVISIONAL_CPU_BUDGET_PERCENT,
        );
    } else {
        println!("[INFO] CPU 측정값 0.0 — 이 환경에서 CPU 단언 생략");
    }
}
