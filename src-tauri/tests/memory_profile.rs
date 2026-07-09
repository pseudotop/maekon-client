//! ```
//! cargo test -p maekon-app --test memory_profile_test -- --nocapture --ignored
//! ```
//! ```
//! cargo test -p maekon-app --test memory_profile_test --release -- --nocapture --ignored
//! ```

use image::{DynamicImage, Rgba, RgbaImage};
use maekon_core::models::event::{Event, UserEvent, UserEventType};
use maekon_core::models::frame::FrameMetadata;
use maekon_core::resource_budget;
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

/// Measures the current process RSS (Resident Set Size, in bytes) via sysinfo.
///
/// The previous implementation relied on the external `ps -o rss=` process and
/// did not work on Windows. Since production `src/memory_profiler.rs` measures
/// RSS via sysinfo (`process.memory()`), the test uses the same measurement path
/// (sysinfo) for consistency. Returns 0 on measurement failure (e.g. unsupported
/// platform).
fn get_rss() -> u64 {
    use sysinfo::{Pid, ProcessesToUpdate, System};

    let pid = Pid::from_u32(std::process::id());
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    system.process(pid).map(|p| p.memory()).unwrap_or(0)
}

/// Measures the current process CPU usage (%) via sysinfo.
///
/// sysinfo computes CPU usage from the interval between two refreshes, so this
/// performs the second refresh after waiting at least `MINIMUM_CPU_UPDATE_INTERVAL`.
/// Returns 0.0 on measurement failure. The return value can exceed 100% relative
/// to a single core (multi-threaded process).
fn sample_cpu_percent() -> f32 {
    use sysinfo::{Pid, ProcessesToUpdate, System, MINIMUM_CPU_UPDATE_INTERVAL};

    let pid = Pid::from_u32(std::process::id());
    let mut system = System::new();
    // 1st refresh — establish the baseline for CPU-usage calculation.
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    std::thread::sleep(MINIMUM_CPU_UPDATE_INTERVAL);
    // 2nd refresh — derive CPU usage based on elapsed time.
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    system.process(pid).map(|p| p.cpu_usage()).unwrap_or(0.0)
}

// ============================================================================
// #4829 / #7918: absolute resource-budget assertions (RSS / CPU)
// ----------------------------------------------------------------------------
// The budget numbers are the SINGLE SOURCE OF TRUTH in
// `maekon_core::resource_budget` (#7918). They used to be test-local
// `PROVISIONAL_*` constants here; they were promoted to that SSOT module so the
// scheduler health loop and the `get_resource_usage_snapshot` diagnostics IPC
// enforce the exact same numbers. They remain provisional (generous, to catch
// gross regressions/leaks without CI flakiness) — see the SSOT module docs for
// the aspirational-target-vs-enforcement-ceiling rationale.
//
// This test is promoted to a NIGHTLY scheduled workflow lane
// (`.github/workflows/maekon-client-budget-nightly.yml`), not a per-PR check
// (repo CI cost policy). A failing assert fails the nightly run — that is the
// signal — but never blocks a PR.
// ============================================================================

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

/// #4829: absolute resource-budget (RSS / CPU) assertion test.
///
/// All existing tests only verified *relative* leaks (growth rate / variance).
/// This test compares the process RSS (peak) and CPU usage right after running a
/// real vision + storage workload against an **absolute upper bound**, verifying
/// that the workload does not exceed the provisional budget.
///
/// - The measurement is REAL: `get_rss()`/`sample_cpu_percent()` sample the actual
///   RSS/CPU of the currently running process via sysinfo (not a hardcoded-value
///   assertion).
/// - The budget numbers are provisional (see the constant-definition comment).
///   They must be replaced with the confirmed SSOT.
/// - Long-running + environment-dependent, so it stays opt-in via `#[ignore]`
///   (`--ignored`).
#[test]
#[ignore = "absolute resource-budget assertion - run with cargo test --ignored"]
fn test_absolute_resource_budget() {
    const ITERATIONS: usize = 100;

    println!("\n=== #4829 absolute resource-budget test ===");

    // --- measurability guard ---
    // If get_rss() returns 0 on an unsupported platform (or where process
    // visibility is restricted in a CI container), the absolute assertion cannot
    // be trusted, so panic with a meaningful message.
    // (Supported platforms: macOS/Linux/Windows. This is an ignored test, so
    // regular CI is unaffected.)
    let baseline_rss = get_rss();
    assert!(
        baseline_rss > 0,
        "RSS measurement failed (returned 0) — sysinfo cannot see the current \
         process in this environment. Run on a supported platform \
         (macOS/Linux/Windows)."
    );

    // --- run the real workload ---
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("budget.db");
    let storage = SqliteStorage::open(&db_path, 30, None).unwrap();

    let img1 = create_test_image(1920, 1080, 42);
    let img2 = create_test_image(1920, 1080, 43);

    let mut peak_rss = baseline_rss;
    let start = Instant::now();

    for i in 0..ITERATIONS {
        // vision pipeline (delta + thumbnail + webp encoding)
        let _delta = delta::compute_delta(&img1, &img2);
        let thumb = thumbnail::fast_resize(&img2, 480, 270).unwrap();
        let _encoded = encoder::encode_webp(&thumb, WebPQuality::Medium).unwrap();

        // storage pipeline (batch event save)
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

        // Sample the real RSS every 10 iterations to update the peak (REAL measurement)
        if i.is_multiple_of(10) {
            peak_rss = peak_rss.max(get_rss());
        }
    }

    // Final RSS / CPU sample right after the workload ends (REAL measurement)
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
        "provisional budgets (SSOT: maekon_core::resource_budget) — RSS <= {:.0} MB, CPU <= {:.0}%",
        resource_budget::RSS_BUDGET_BYTES as f64 / 1024.0 / 1024.0,
        resource_budget::CPU_BUDGET_PERCENT,
    );

    // --- absolute budget assertion (provisional SSOT ceiling) ---
    assert!(
        resource_budget::rss_within_budget(peak_rss),
        "peak RSS {:.2} MB exceeds the provisional budget of {:.0} MB. \
         (Could be a real regression or an overly tight provisional budget — \
         re-tune the SSOT in maekon_core::resource_budget)",
        peak_rss as f64 / 1024.0 / 1024.0,
        resource_budget::RSS_BUDGET_BYTES as f64 / 1024.0 / 1024.0,
    );

    // If the CPU measurement is 0.0 (happens in some environments), skip the
    // assertion — prefer avoiding a false-fail over preventing a false-pass. Only
    // verify the upper bound when it is greater than 0.0.
    if cpu_percent > 0.0 {
        assert!(
            resource_budget::cpu_within_budget(cpu_percent),
            "CPU {:.1}% exceeds the provisional budget of {:.0}%. \
             (Re-tune the SSOT in maekon_core::resource_budget)",
            cpu_percent,
            resource_budget::CPU_BUDGET_PERCENT,
        );
    } else {
        println!("[INFO] CPU measurement is 0.0 — skipping the CPU assertion in this environment");
    }
}
