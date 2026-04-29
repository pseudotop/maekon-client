#![allow(clippy::redundant_closure, clippy::unit_arg)]

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use oneshim_core::models::event::{ContextEvent, Event, UserEvent, UserEventType};
use oneshim_core::models::frame::FrameMetadata;
use oneshim_storage::sqlite::SqliteStorage;
use tempfile::TempDir;
use uuid::Uuid;

fn create_test_user_event(i: usize) -> Event {
    Event::User(UserEvent {
        event_id: Uuid::new_v4(),
        event_type: UserEventType::WindowChange,
        timestamp: chrono::Utc::now(),
        app_name: format!("TestApp{}", i % 10),
        window_title: format!("Test Window {}", i),
    })
}

fn create_test_context_event(i: usize) -> Event {
    Event::Context(ContextEvent {
        app_name: format!("App{}", i % 5),
        window_title: format!("Window {}", i),
        prev_app_name: Some(format!("PrevApp{}", (i + 1) % 5)),
        timestamp: chrono::Utc::now(),
        input_activity_level: 0.0,
    })
}

fn create_temp_storage() -> (SqliteStorage, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temporary directory");
    let db_path = temp_dir.path().join("test.db");
    let storage = SqliteStorage::open(&db_path, 30, None).expect("Failed to create storage");
    (storage, temp_dir)
}

fn bench_event_batch_save(c: &mut Criterion) {
    let mut group = c.benchmark_group("event_batch_save");

    let batch_sizes = [10, 50, 100, 500];

    for batch_size in batch_sizes {
        group.throughput(Throughput::Elements(batch_size as u64));

        let events: Vec<Event> = (0..batch_size).map(create_test_user_event).collect();

        group.bench_with_input(
            BenchmarkId::new("user_events", batch_size),
            &events,
            |b, events| {
                b.iter_with_setup(
                    || create_temp_storage(),
                    |(storage, _temp): (SqliteStorage, TempDir)| {
                        black_box(storage.save_events_batch(events).unwrap());
                    },
                );
            },
        );

        let context_events: Vec<Event> = (0..batch_size).map(create_test_context_event).collect();

        group.bench_with_input(
            BenchmarkId::new("context_events", batch_size),
            &context_events,
            |b, events| {
                b.iter_with_setup(
                    || create_temp_storage(),
                    |(storage, _temp): (SqliteStorage, TempDir)| {
                        black_box(storage.save_events_batch(events).unwrap());
                    },
                );
            },
        );
    }

    group.finish();
}

fn bench_frame_metadata_save(c: &mut Criterion) {
    let mut group = c.benchmark_group("frame_metadata_save");

    let counts = [10, 50, 100];

    for count in counts {
        group.throughput(Throughput::Elements(count as u64));

        group.bench_with_input(BenchmarkId::new("frames", count), &count, |b, &count| {
            b.iter_with_setup(
                || {
                    let (storage, temp) = create_temp_storage();
                    let frames: Vec<_> = (0..count)
                        .map(|i| {
                            let metadata = FrameMetadata {
                                timestamp: chrono::Utc::now(),
                                trigger_type: "AppSwitch".to_string(),
                                app_name: format!("App{}", i % 5),
                                window_title: format!("Window {}", i),
                                resolution: (1920, 1080),
                                importance: 0.5 + (i % 5) as f32 * 0.1,
                            };
                            let path = format!("frames/2026-01-31/{:03}.webp", i);
                            (metadata, path)
                        })
                        .collect();
                    (storage, temp, frames)
                },
                |(storage, _temp, frames): (
                    SqliteStorage,
                    TempDir,
                    Vec<(FrameMetadata, String)>,
                )| {
                    for (metadata, path) in &frames {
                        black_box(
                            storage
                                .save_frame_metadata(metadata, Some(path.as_str()), None)
                                .unwrap(),
                        );
                    }
                },
            );
        });
    }

    group.finish();
}

fn bench_focus_metrics(c: &mut Criterion) {
    let mut group = c.benchmark_group("focus_metrics");

    group.bench_function("get_or_create", |b| {
        b.iter_with_setup(
            || create_temp_storage(),
            |(storage, _temp): (SqliteStorage, TempDir)| {
                black_box(storage.get_or_create_today_focus_metrics().unwrap());
            },
        );
    });

    group.bench_function("increment_100x", |b| {
        b.iter_with_setup(
            || {
                let (storage, temp) = create_temp_storage();
                storage.get_or_create_today_focus_metrics().unwrap();
                (storage, temp)
            },
            |(storage, _temp): (SqliteStorage, TempDir)| {
                for _ in 0..100 {
                    black_box(
                        storage
                            .increment_focus_metrics(
                                &chrono::Utc::now().format("%Y-%m-%d").to_string(),
                                1, // total_active_secs
                                1, // deep_work_secs
                                0, // communication_secs
                                0, // context_switches
                                0, // interruption_count
                            )
                            .unwrap(),
                    );
                }
            },
        );
    });

    group.bench_function("get_recent_7days", |b| {
        b.iter_with_setup(
            || {
                let (storage, temp) = create_temp_storage();
                for i in 0..7 {
                    let date = (chrono::Utc::now() - chrono::Duration::days(i))
                        .format("%Y-%m-%d")
                        .to_string();
                    storage.get_or_create_focus_metrics(&date).unwrap();
                }
                (storage, temp)
            },
            |(storage, _temp): (SqliteStorage, TempDir)| {
                black_box(storage.get_recent_focus_metrics(7).unwrap());
            },
        );
    });

    group.finish();
}

fn bench_tags(c: &mut Criterion) {
    let mut group = c.benchmark_group("tags");

    group.bench_function("create_10", |b| {
        b.iter_with_setup(
            || create_temp_storage(),
            |(storage, _temp): (SqliteStorage, TempDir)| {
                for i in 0..10 {
                    black_box(storage.create_tag(&format!("Tag{}", i), "#FF5733").unwrap());
                }
            },
        );
    });

    group.bench_function("get_all_50tags", |b| {
        b.iter_with_setup(
            || {
                let (storage, temp) = create_temp_storage();
                for i in 0..50 {
                    storage.create_tag(&format!("Tag{}", i), "#FF5733").unwrap();
                }
                (storage, temp)
            },
            |(storage, _temp): (SqliteStorage, TempDir)| {
                black_box(storage.get_all_tags().unwrap());
            },
        );
    });

    group.bench_function("add_tag_to_frame", |b| {
        b.iter_with_setup(
            || {
                let (storage, temp) = create_temp_storage();
                let metadata = FrameMetadata {
                    timestamp: chrono::Utc::now(),
                    trigger_type: "AppSwitch".to_string(),
                    app_name: "TestApp".to_string(),
                    window_title: "Window".to_string(),
                    resolution: (1920, 1080),
                    importance: 0.8,
                };
                let frame_id = storage
                    .save_frame_metadata(&metadata, Some("frames/test.webp"), None)
                    .unwrap();
                let tag = storage.create_tag("TestTag", "#FF5733").unwrap();
                (storage, temp, frame_id, tag.id)
            },
            |(storage, _temp, frame_id, tag_id): (SqliteStorage, TempDir, i64, i64)| {
                let _ = storage.remove_tag_from_frame(frame_id, tag_id);
                black_box(storage.add_tag_to_frame(frame_id, tag_id).unwrap());
            },
        );
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_event_batch_save,
    bench_frame_metadata_save,
    bench_focus_metrics,
    bench_tags
);
criterion_main!(benches);
