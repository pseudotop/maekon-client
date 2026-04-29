//! ```
//! cargo test -p oneshim-app --test memory_profile_test -- --nocapture --ignored
//! ```
//! ```
//! cargo test -p oneshim-app --test memory_profile_test --release -- --nocapture --ignored
//! ```

use image::{DynamicImage, Rgba, RgbaImage};
use oneshim_core::models::event::{Event, UserEvent, UserEventType};
use oneshim_core::models::frame::FrameMetadata;
use oneshim_storage::sqlite::SqliteStorage;
use oneshim_vision::{delta, encoder, encoder::WebPQuality, thumbnail};
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

fn get_rss() -> u64 {
    use std::process::Command;

    let pid = std::process::id();
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .expect("failed to run ps command");

    let rss_kb: u64 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0);

    rss_kb * 1024
}

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

        if i % SAMPLE_INTERVAL == 0 {
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

        if i % SAMPLE_INTERVAL == 0 {
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
        if i % 5 == 0 || i == snapshots.len() - 1 {
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
