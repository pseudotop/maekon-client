// Read-model RPC tests — GetSessionStats, GetRecentFrames,
// GetProductivityMetrics, and GetFocusStats, each with an empty-DB baseline
// plus a seeded-aggregation case. Split from `grpc_dashboard_integration.rs`
// by scenario family (#7730).

use std::sync::Arc;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use maekon_core::models::activity::SessionStats;
use maekon_core::models::work_session::FocusMetrics;
use maekon_core::ports::storage::MetricsStorage;
use maekon_storage::sqlite::SqliteStorage;
use maekon_web::proto::dashboard::v1::{
    GetFocusStatsRequest, GetProductivityMetricsRequest, GetRecentFramesRequest,
    GetSessionStatsRequest,
};
use maekon_web::storage_port::WebStorage;

use crate::common::{
    connect_dashboard_client, in_memory_storage, pick_free_port, test_spawn_config,
    wait_for_server_ready,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_dashboard_get_session_stats_empty_db() {
    let port = pick_free_port();
    let storage = in_memory_storage().await;

    let server_task = tokio::spawn(maekon_web::grpc::serve_optional(test_spawn_config(
        port, storage,
    )));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let mut client = connect_dashboard_client(port).await;

    let response = client
        .get_session_stats(GetSessionStatsRequest { limit: 0 })
        .await
        .expect("GetSessionStats RPC succeeds")
        .into_inner();

    // Empty DB: all counters zero, avg duration 0.
    assert_eq!(response.total_sessions, 0);
    assert_eq!(response.ended_sessions, 0);
    assert_eq!(response.avg_duration_secs, 0.0);
    assert_eq!(response.total_events, 0);
    assert_eq!(response.total_frames, 0);
    assert_eq!(response.total_idle_secs, 0);

    server_task.abort();
    let _ = server_task.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_dashboard_get_session_stats_aggregates_seeded_sessions() {
    let port = pick_free_port();

    // Seed: 3 sessions, 2 ended. Aggregate expectations below.
    let storage = SqliteStorage::open_in_memory(30).expect("open in-memory SqliteStorage");

    // Session 1 — ended, duration 120s
    let now = Utc::now();
    let s1 = SessionStats {
        session_id: "s1".into(),
        started_at: now - ChronoDuration::seconds(300),
        ended_at: Some(now - ChronoDuration::seconds(180)),
        total_events: 10,
        total_frames: 5,
        total_idle_secs: 20,
    };
    // Session 2 — ended, duration 60s
    let s2 = SessionStats {
        session_id: "s2".into(),
        started_at: now - ChronoDuration::seconds(200),
        ended_at: Some(now - ChronoDuration::seconds(140)),
        total_events: 7,
        total_frames: 3,
        total_idle_secs: 10,
    };
    // Session 3 — still running
    let s3 = SessionStats {
        session_id: "s3".into(),
        started_at: now - ChronoDuration::seconds(30),
        ended_at: None,
        total_events: 2,
        total_frames: 1,
        total_idle_secs: 0,
    };

    storage.upsert_session(&s1).await.unwrap();
    storage.upsert_session(&s2).await.unwrap();
    storage.upsert_session(&s3).await.unwrap();

    let storage: Arc<dyn WebStorage> = Arc::new(storage);

    let server_task = tokio::spawn(maekon_web::grpc::serve_optional(test_spawn_config(
        port, storage,
    )));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let mut client = connect_dashboard_client(port).await;

    let response = client
        .get_session_stats(GetSessionStatsRequest { limit: 10 })
        .await
        .expect("GetSessionStats RPC succeeds")
        .into_inner();

    assert_eq!(response.total_sessions, 3);
    assert_eq!(response.ended_sessions, 2);
    // (120s + 60s) / 2 = 90s. Allow ±1s slack for chrono rounding.
    assert!(
        (response.avg_duration_secs - 90.0).abs() < 2.0,
        "avg_duration_secs: expected ~90, got {}",
        response.avg_duration_secs
    );
    assert_eq!(response.total_events, 10 + 7 + 2);
    assert_eq!(response.total_frames, 5 + 3 + 1);
    assert_eq!(response.total_idle_secs, 20 + 10);

    server_task.abort();
    let _ = server_task.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_dashboard_get_recent_frames_empty_db() {
    let port = pick_free_port();
    let storage = in_memory_storage().await;

    let server_task = tokio::spawn(maekon_web::grpc::serve_optional(test_spawn_config(
        port, storage,
    )));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let mut client = connect_dashboard_client(port).await;

    let response = client
        .get_recent_frames(GetRecentFramesRequest {
            limit: 0,
            since_hours: 0,
        })
        .await
        .expect("GetRecentFrames RPC succeeds")
        .into_inner();

    assert!(
        response.frames.is_empty(),
        "empty DB should return 0 frames"
    );

    server_task.abort();
    let _ = server_task.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_dashboard_get_recent_frames_clamps_limit() {
    let port = pick_free_port();
    let storage = in_memory_storage().await;

    let server_task = tokio::spawn(maekon_web::grpc::serve_optional(test_spawn_config(
        port, storage,
    )));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let mut client = connect_dashboard_client(port).await;

    // Request limit=9999, since_hours=9999 — server should hard-cap but not
    // reject. Empty DB, so we just verify no error.
    let response = client
        .get_recent_frames(GetRecentFramesRequest {
            limit: 9999,
            since_hours: 9999,
        })
        .await
        .expect("GetRecentFrames clamps instead of erroring")
        .into_inner();

    assert!(response.frames.is_empty());

    server_task.abort();
    let _ = server_task.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_dashboard_get_productivity_metrics_empty_db() {
    let port = pick_free_port();
    let storage = in_memory_storage().await;

    let server_task = tokio::spawn(maekon_web::grpc::serve_optional(test_spawn_config(
        port, storage,
    )));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let mut client = connect_dashboard_client(port).await;

    let response = client
        .get_productivity_metrics(GetProductivityMetricsRequest { since_hours: 0 })
        .await
        .expect("GetProductivityMetrics RPC succeeds")
        .into_inner();

    assert!(
        response.buckets.is_empty(),
        "empty DB should return 0 buckets"
    );

    server_task.abort();
    let _ = server_task.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_dashboard_get_focus_stats_empty_db() {
    let port = pick_free_port();
    let storage = in_memory_storage().await;

    let server_task = tokio::spawn(maekon_web::grpc::serve_optional(test_spawn_config(
        port, storage,
    )));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let mut client = connect_dashboard_client(port).await;

    let response = client
        .get_focus_stats(GetFocusStatsRequest { days: 0 })
        .await
        .expect("GetFocusStats RPC succeeds")
        .into_inner();

    // Empty DB: 0 buckets + all counters zero + avg_focus_score = 0.
    assert_eq!(response.bucket_count, 0);
    assert_eq!(response.total_active_secs, 0);
    assert_eq!(response.total_deep_work_secs, 0);
    assert_eq!(response.total_communication_secs, 0);
    assert_eq!(response.total_interruptions, 0);
    assert_eq!(response.avg_focus_score, 0.0);
    assert_eq!(response.longest_focus_secs, 0);

    server_task.abort();
    let _ = server_task.await;
}

/// Seed 3 daily focus buckets, verify GetFocusStats aggregates each dimension
/// (sum, average, max). Mirrors the seeded-aggregation pattern used for
/// GetSessionStats so the focus-side math doesn't drift silently.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_dashboard_get_focus_stats_aggregates_seeded_days() {
    let port = pick_free_port();

    let storage = SqliteStorage::open_in_memory(30).expect("open in-memory SqliteStorage");

    // Three distinct dates so update_focus_metrics writes each row independently.
    // Aggregate expectations (computed in-line below) lock each response field
    // to the handler's reduction rule.
    let seeds: &[(&str, u64, u64, u64, u32, u64, f32)] = &[
        // (date,  total_active, deep_work, communication, interruptions, longest_focus, score)
        ("2026-04-20", 3_600, 2_400, 600, 2, 1_800, 0.80),
        ("2026-04-19", 1_800, 1_200, 300, 1, 2_700, 0.60),
        ("2026-04-18", 900, 600, 150, 4, 900, 0.40),
    ];

    for (date, active, deep, comm, interruptions, longest, score) in seeds {
        // Must materialize the row first — update_focus_metrics is UPDATE, not UPSERT.
        let _ = storage
            .get_or_create_focus_metrics(date)
            .expect("seed focus_metrics row");

        let metrics = FocusMetrics {
            period: maekon_core::types::TimeWindow::new(Utc::now(), Utc::now())
                .expect("zero-duration window valid for trusted test fixture"),
            total_active_secs: *active,
            deep_work_secs: *deep,
            communication_secs: *comm,
            context_switches: 0,
            interruption_count: *interruptions,
            avg_focus_duration_secs: 0,
            max_focus_duration_secs: *longest,
            focus_score: *score,
        };
        storage
            .update_focus_metrics(date, &metrics)
            .expect("update seeded focus_metrics");
    }

    let storage: Arc<dyn WebStorage> = Arc::new(storage);

    let server_task = tokio::spawn(maekon_web::grpc::serve_optional(test_spawn_config(
        port, storage,
    )));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let mut client = connect_dashboard_client(port).await;

    // days=0 → server default (7). All 3 seeded dates fall within the window.
    let response = client
        .get_focus_stats(GetFocusStatsRequest { days: 0 })
        .await
        .expect("GetFocusStats RPC succeeds")
        .into_inner();

    assert_eq!(response.bucket_count, 3);
    assert_eq!(response.total_active_secs, 3_600 + 1_800 + 900);
    assert_eq!(response.total_deep_work_secs, 2_400 + 1_200 + 600);
    assert_eq!(response.total_communication_secs, 600 + 300 + 150);
    assert_eq!(response.total_interruptions, 2 + 1 + 4);
    // avg_focus_score = (0.80 + 0.60 + 0.40) / 3 = 0.60. Tolerance for f32 rounding.
    assert!(
        (response.avg_focus_score - 0.60).abs() < 1e-5,
        "avg_focus_score: expected ~0.60, got {}",
        response.avg_focus_score
    );
    // longest_focus_secs = max(1_800, 2_700, 900) = 2_700.
    assert_eq!(response.longest_focus_secs, 2_700);

    server_task.abort();
    let _ = server_task.await;
}
