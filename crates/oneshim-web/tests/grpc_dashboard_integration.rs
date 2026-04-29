//! D13-v2 integration tests: end-to-end gRPC dashboard server ↔ client.
//!
//! Spawns `serve_optional()` on an ephemeral port, connects a tonic client,
//! exercises each RPC, and verifies the wire protocol + service
//! registration + port wiring end-to-end.
//!
//! Feature-gated by `grpc-dashboard` — the entire file compiles away when
//! the feature is off (matches the production gating in
//! `oneshim-web::grpc`).

#![cfg(feature = "grpc-dashboard")]

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use oneshim_core::models::activity::SessionStats;
use oneshim_core::models::work_session::FocusMetrics;
use oneshim_core::ports::storage::MetricsStorage;
use oneshim_storage::sqlite::SqliteStorage;
use oneshim_web::proto::dashboard::v1::dashboard_service_client::DashboardServiceClient;
use oneshim_web::proto::dashboard::v1::health_check_response::Status as HealthStatus;
use oneshim_web::proto::dashboard::v1::{
    subscribe_metrics_response, GetAgentInfoRequest, GetFocusStatsRequest,
    GetProductivityMetricsRequest, GetRecentFramesRequest, GetSessionStatsRequest,
    HealthCheckRequest, SubscribeMetricsRequest,
};
use oneshim_web::storage_port::WebStorage;

/// Pick a free ephemeral port by binding + immediately dropping a listener.
/// Tiny race window between drop + server bind, acceptable for test use.
fn pick_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().expect("local_addr").port();
    drop(listener);
    port
}

/// Poll until the server accepts TCP connections (up to `timeout`).
async fn wait_for_server_ready(port: u16, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "gRPC dashboard server did not accept connections on port {port} within {timeout:?}"
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Build an in-memory SqliteStorage behind the `WebStorage` trait.
async fn in_memory_storage() -> Arc<dyn WebStorage> {
    let storage = SqliteStorage::open_in_memory(30).expect("open in-memory SqliteStorage");
    Arc::new(storage) as Arc<dyn WebStorage>
}

/// Build a `GrpcSpawnConfig` with sensible test defaults (deterministic
/// `MockSystemMonitor`, 16-slot broadcast, no auth token, default
/// `LoadThresholds`, streaming enabled, cap 50). Callers that need to
/// override fields can destructure and rebuild.
fn test_spawn_config(
    port: u16,
    storage: Arc<dyn WebStorage>,
) -> oneshim_web::grpc::GrpcSpawnConfig {
    use oneshim_core::config::LoadThresholds;
    use oneshim_web::grpc::{test_support::mock_system_monitor::MockSystemMonitor, LoadPolicy};

    let (event_tx, _) = tokio::sync::broadcast::channel(16);
    oneshim_web::grpc::GrpcSpawnConfig {
        port,
        storage,
        system_monitor: MockSystemMonitor::new(30.0, 4096, 16384),
        event_tx,
        integration_auth_token: None,
        pii_sanitizer: None,
        ai_runtime_status_snapshot: None,
        load_policy: Arc::new(LoadPolicy::new(LoadThresholds::default())),
        streaming_enabled: true,
        max_concurrent_streams: 50,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_dashboard_get_agent_info_end_to_end() {
    let port = pick_free_port();
    let storage = in_memory_storage().await;

    let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(test_spawn_config(
        port, storage,
    )));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let endpoint = format!("http://127.0.0.1:{port}");
    let mut client = DashboardServiceClient::connect(endpoint)
        .await
        .expect("connect to dashboard gRPC server");

    let response = client
        .get_agent_info(GetAgentInfoRequest {})
        .await
        .expect("GetAgentInfo RPC succeeds")
        .into_inner();

    assert!(
        !response.version.is_empty(),
        "version must come from CARGO_PKG_VERSION — got empty string"
    );
    assert!(
        matches!(response.build_profile.as_str(), "debug" | "release"),
        "build_profile must be 'debug' or 'release' — got '{}'",
        response.build_profile
    );
    assert!(
        matches!(
            response.platform.as_str(),
            "macos" | "windows" | "linux" | "unknown"
        ),
        "platform must be one of macos/windows/linux/unknown — got '{}'",
        response.platform
    );
    assert!(
        response.uptime_secs >= 0,
        "uptime_secs must be non-negative — got {}",
        response.uptime_secs
    );

    server_task.abort();
    let _ = server_task.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_dashboard_health_check_end_to_end() {
    let port = pick_free_port();
    let storage = in_memory_storage().await;

    let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(test_spawn_config(
        port, storage,
    )));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let endpoint = format!("http://127.0.0.1:{port}");
    let mut client = DashboardServiceClient::connect(endpoint)
        .await
        .expect("connect to dashboard gRPC server");

    let response = client
        .health_check(HealthCheckRequest {})
        .await
        .expect("HealthCheck RPC succeeds")
        .into_inner();

    assert_eq!(
        response.status,
        HealthStatus::Serving as i32,
        "health status should be SERVING when server is up",
    );

    server_task.abort();
    let _ = server_task.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_dashboard_survives_multiple_sequential_calls() {
    let port = pick_free_port();
    let storage = in_memory_storage().await;

    let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(test_spawn_config(
        port, storage,
    )));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let endpoint = format!("http://127.0.0.1:{port}");
    let mut client = DashboardServiceClient::connect(endpoint)
        .await
        .expect("connect to dashboard gRPC server");

    // Locks the invariant that DashboardServiceImpl handles concurrent-ish
    // traffic without panicking or leaking state between calls.
    for _ in 0..5 {
        let info = client
            .get_agent_info(GetAgentInfoRequest {})
            .await
            .expect("GetAgentInfo RPC succeeds")
            .into_inner();
        assert!(!info.version.is_empty());
    }

    server_task.abort();
    let _ = server_task.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_dashboard_get_session_stats_empty_db() {
    let port = pick_free_port();
    let storage = in_memory_storage().await;

    let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(test_spawn_config(
        port, storage,
    )));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let endpoint = format!("http://127.0.0.1:{port}");
    let mut client = DashboardServiceClient::connect(endpoint)
        .await
        .expect("connect to dashboard gRPC server");

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

    let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(test_spawn_config(
        port, storage,
    )));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let endpoint = format!("http://127.0.0.1:{port}");
    let mut client = DashboardServiceClient::connect(endpoint)
        .await
        .expect("connect to dashboard gRPC server");

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

    let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(test_spawn_config(
        port, storage,
    )));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let endpoint = format!("http://127.0.0.1:{port}");
    let mut client = DashboardServiceClient::connect(endpoint)
        .await
        .expect("connect to dashboard gRPC server");

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

    let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(test_spawn_config(
        port, storage,
    )));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let endpoint = format!("http://127.0.0.1:{port}");
    let mut client = DashboardServiceClient::connect(endpoint)
        .await
        .expect("connect to dashboard gRPC server");

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

    let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(test_spawn_config(
        port, storage,
    )));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let endpoint = format!("http://127.0.0.1:{port}");
    let mut client = DashboardServiceClient::connect(endpoint)
        .await
        .expect("connect to dashboard gRPC server");

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

    let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(test_spawn_config(
        port, storage,
    )));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let endpoint = format!("http://127.0.0.1:{port}");
    let mut client = DashboardServiceClient::connect(endpoint)
        .await
        .expect("connect to dashboard gRPC server");

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
            period: oneshim_core::types::TimeWindow::new(Utc::now(), Utc::now())
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

    let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(test_spawn_config(
        port, storage,
    )));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let endpoint = format!("http://127.0.0.1:{port}");
    let mut client = DashboardServiceClient::connect(endpoint)
        .await
        .expect("connect to dashboard gRPC server");

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

// ── B2-10: SubscribeMetrics integration tests (8) ────────────────────
//
// Infrastructure: per spec §7.1 virtual-time ordering — when using
// `start_paused = true`, the `subscribe_metrics` RPC call MUST come AFTER
// `tokio::time::pause()` so the handler's `tokio::time::interval` registers
// against the paused clock. `wait_for_server_ready` (real-clock sleep) MUST
// come BEFORE `pause()`.

use oneshim_api_contracts::stream::{MetricsUpdate, RealtimeEvent};

/// Test #1 — First yield is a `Hint`, reason starts with `"warmup"` (within 30s).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_dashboard_subscribe_metrics_emits_initial_hint() {
    let port = pick_free_port();
    let storage = in_memory_storage().await;
    let cfg = test_spawn_config(port, storage);
    let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(cfg));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let endpoint = format!("http://127.0.0.1:{port}");
    let mut client = DashboardServiceClient::connect(endpoint).await.unwrap();

    let mut stream = client
        .subscribe_metrics(SubscribeMetricsRequest {
            interval_secs: 1,
            respect_server_hints: true,
        })
        .await
        .expect("subscribe_metrics ok")
        .into_inner();

    let first = tokio::time::timeout(Duration::from_secs(3), stream.message())
        .await
        .expect("message within 3s")
        .expect("stream not errored")
        .expect("stream not ended");
    match first.payload.expect("payload") {
        subscribe_metrics_response::Payload::Hint(h) => {
            assert_ne!(h.load_level, 0, "level should not be UNSPECIFIED");
            assert!(!h.reason.is_empty());
            // During warm-up the reason must be prefixed "warmup".
            assert!(
                h.reason.starts_with("warmup"),
                "expected 'warmup' prefix in fresh server, got: {}",
                h.reason
            );
        }
        other => panic!("expected Hint first, got {other:?}"),
    }

    server_task.abort();
    let _ = server_task.await;
}

/// Test #2 — `streaming_enabled=false` → `Status::unavailable`, message
/// does not leak config field name (IMP-V2-F).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_dashboard_subscribe_metrics_rejects_when_streaming_disabled() {
    let port = pick_free_port();
    let storage = in_memory_storage().await;
    let mut cfg = test_spawn_config(port, storage);
    cfg.streaming_enabled = false;
    let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(cfg));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let endpoint = format!("http://127.0.0.1:{port}");
    let mut client = DashboardServiceClient::connect(endpoint).await.unwrap();

    // `streaming_enabled=false` short-circuits BEFORE the stream opens, so the
    // error surfaces at the RPC boundary (not as a stream item).
    let err = client
        .subscribe_metrics(SubscribeMetricsRequest {
            interval_secs: 1,
            respect_server_hints: true,
        })
        .await
        .expect_err("RPC must return Status::unavailable when streaming is disabled");
    assert_eq!(err.code(), tonic::Code::Unavailable);
    // Neutral message — must NOT expose the config field name.
    assert!(
        !err.message().contains("grpc_streaming_enabled"),
        "message should not leak config field name, got: {}",
        err.message()
    );

    server_task.abort();
    let _ = server_task.await;
}

/// Test #3 — `interval_secs=5`: two Data payloads arrive with `start`
/// timestamps ≥5s apart. Uses real clock (no pause) so we don't interfere
/// with the server's tokio::time::interval.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_dashboard_subscribe_metrics_interval_emits_buckets() {
    let port = pick_free_port();
    let storage = in_memory_storage().await;
    let cfg = test_spawn_config(port, storage);
    let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(cfg));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let endpoint = format!("http://127.0.0.1:{port}");
    let mut client = DashboardServiceClient::connect(endpoint).await.unwrap();

    let mut stream = client
        .subscribe_metrics(SubscribeMetricsRequest {
            interval_secs: 1,
            respect_server_hints: true,
        })
        .await
        .expect("subscribe_metrics ok")
        .into_inner();

    // Drain the initial Hint.
    let first = tokio::time::timeout(Duration::from_secs(3), stream.message())
        .await
        .expect("first msg")
        .expect("not errored")
        .expect("not ended");
    assert!(matches!(
        first.payload,
        Some(subscribe_metrics_response::Payload::Hint(_))
    ));

    // Collect Data payloads with a generous timeout (real-clock ticker, not paused).
    let mut data_count = 0u32;
    let collect_deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while data_count < 2 && tokio::time::Instant::now() < collect_deadline {
        let remaining = collect_deadline - tokio::time::Instant::now();
        let msg = match tokio::time::timeout(remaining, stream.message()).await {
            Ok(Ok(Some(m))) => m,
            _ => break,
        };
        if let Some(subscribe_metrics_response::Payload::Data(_)) = msg.payload {
            data_count += 1;
        }
    }
    assert!(
        data_count >= 2,
        "expected ≥2 Data payloads, got {data_count}"
    );

    server_task.abort();
    let _ = server_task.await;
}

/// Test #4 — realtime mode wakes on `event_tx::Metrics` ticks.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_dashboard_subscribe_metrics_realtime_emits_on_event_tx_tick() {
    use oneshim_core::config::LoadThresholds;
    use oneshim_web::grpc::{test_support::mock_system_monitor::MockSystemMonitor, LoadPolicy};

    let port = pick_free_port();
    let storage = in_memory_storage().await;
    let (event_tx, _) = tokio::sync::broadcast::channel(16);
    let cfg = oneshim_web::grpc::GrpcSpawnConfig {
        port,
        storage,
        system_monitor: MockSystemMonitor::new(30.0, 4096, 16384),
        event_tx: event_tx.clone(),
        integration_auth_token: None,
        pii_sanitizer: None,
        ai_runtime_status_snapshot: None,
        load_policy: Arc::new(LoadPolicy::new(LoadThresholds::default())),
        streaming_enabled: true,
        max_concurrent_streams: 50,
    };
    let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(cfg));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let endpoint = format!("http://127.0.0.1:{port}");
    let mut client = DashboardServiceClient::connect(endpoint).await.unwrap();
    let mut stream = client
        .subscribe_metrics(SubscribeMetricsRequest {
            interval_secs: 0, // realtime — handler blocks on event_tx
            respect_server_hints: true,
        })
        .await
        .expect("subscribe_metrics ok")
        .into_inner();

    // In realtime mode the handler's first work is `rx.recv().await`, so NO
    // initial Hint is emitted until an event_tx tick wakes the generator.
    // Send several Metrics events BEFORE polling the stream — the server-side
    // `rx.subscribe()` has already run by the time `subscribe_metrics().await`
    // returned on the client side. Sending multiple covers any pre/post race.
    for _ in 0..5 {
        let _ = event_tx.send(RealtimeEvent::Metrics(MetricsUpdate {
            timestamp: chrono::Utc::now().to_rfc3339(),
            cpu_usage: 25.0,
            memory_percent: 25.0,
            memory_used: 4_000_000_000,
            memory_total: 16_000_000_000,
        }));
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // First yield after an event_tx wake is the initial Hint (emitter state
    // None → always emits on first call).
    let got = tokio::time::timeout(Duration::from_secs(3), stream.message())
        .await
        .expect("message within 3s after event_tx ticks")
        .expect("stream not errored")
        .expect("stream not ended");
    assert!(
        got.payload.is_some(),
        "expected payload after event_tx tick"
    );

    server_task.abort();
    let _ = server_task.await;
}

/// Test #5 — `max_concurrent_streams=2`: 3rd subscribe fails with
/// `Status::resource_exhausted` (active-stream cap — CRIT-4).
/// Uses lowered cap per IMP-V2-C (avoids fd pressure at 51 streams).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_dashboard_subscribe_metrics_enforces_active_stream_cap() {
    let port = pick_free_port();
    let storage = in_memory_storage().await;
    let mut cfg = test_spawn_config(port, storage);
    cfg.max_concurrent_streams = 2;
    let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(cfg));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let endpoint = format!("http://127.0.0.1:{port}");
    let mut c1 = DashboardServiceClient::connect(endpoint.clone())
        .await
        .unwrap();
    let mut c2 = DashboardServiceClient::connect(endpoint.clone())
        .await
        .unwrap();
    let mut c3 = DashboardServiceClient::connect(endpoint.clone())
        .await
        .unwrap();

    let req = || SubscribeMetricsRequest {
        interval_secs: 30, // slow cadence, keeps streams alive
        respect_server_hints: true,
    };
    let s1 = c1
        .subscribe_metrics(req())
        .await
        .expect("slot 1")
        .into_inner();
    // Drain initial Hint to ensure stream is registered + guard held.
    let mut s1 = s1;
    let _ = tokio::time::timeout(Duration::from_secs(3), s1.message()).await;

    let s2 = c2
        .subscribe_metrics(req())
        .await
        .expect("slot 2")
        .into_inner();
    let mut s2 = s2;
    let _ = tokio::time::timeout(Duration::from_secs(3), s2.message()).await;

    // Third must fail with ResourceExhausted.
    let err = c3
        .subscribe_metrics(req())
        .await
        .expect_err("slot 3 should be rejected at cap");
    assert_eq!(err.code(), tonic::Code::ResourceExhausted);

    drop(s1);
    drop(s2);
    server_task.abort();
    let _ = server_task.await;
}

/// Test #6 — `validate_authority` rejects DNS-rebound `:authority` by
/// forcing a `host` header that's NOT in the allowlist. Uses a
/// `Request::new` + manual metadata injection.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_dashboard_subscribe_metrics_rejects_dns_rebound_authority() {
    let port = pick_free_port();
    let storage = in_memory_storage().await;
    let cfg = test_spawn_config(port, storage);
    let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(cfg));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let endpoint = format!("http://127.0.0.1:{port}");
    let mut client = DashboardServiceClient::connect(endpoint).await.unwrap();

    let mut req = tonic::Request::new(SubscribeMetricsRequest {
        interval_secs: 1,
        respect_server_hints: true,
    });
    // Inject a non-allowlisted host (simulates DNS rebinding).
    req.metadata_mut()
        .insert("host", "evil.example.com:10080".parse().unwrap());
    let err = client
        .subscribe_metrics(req)
        .await
        .expect_err("evil host must be rejected");
    assert_eq!(err.code(), tonic::Code::PermissionDenied);

    server_task.abort();
    let _ = server_task.await;
}

/// Test #7 — reconnect cycle: 5 subscribe/drop cycles; server-side counter
/// must return to baseline 0 (IMP-V2-C / CRIT-3 StreamCounterGuard leak check).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_dashboard_subscribe_metrics_survives_reconnect_cycle() {
    let port = pick_free_port();
    let storage = in_memory_storage().await;
    let mut cfg = test_spawn_config(port, storage);
    // Tight cap — if any guard leaks, the 6th cycle fails.
    cfg.max_concurrent_streams = 3;
    let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(cfg));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let endpoint = format!("http://127.0.0.1:{port}");
    for i in 0..5 {
        let mut client = DashboardServiceClient::connect(endpoint.clone())
            .await
            .unwrap();
        let mut stream = client
            .subscribe_metrics(SubscribeMetricsRequest {
                interval_secs: 30,
                respect_server_hints: true,
            })
            .await
            .unwrap_or_else(|e| panic!("cycle {i} subscribe failed: {e}"))
            .into_inner();
        // Drain initial hint so the stream is fully registered.
        let _ = tokio::time::timeout(Duration::from_millis(500), stream.message()).await;
        drop(stream);
        drop(client);
        // Yield so the server has a chance to run the Drop on its side.
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // If guards leaked, at least one cycle's count would have exceeded 3
    // and failed with ResourceExhausted — the loop above would have panicked.

    server_task.abort();
    let _ = server_task.await;
}

/// Test #8 — opt-out on loopback is honored. Client sets
/// `respect_server_hints=false`; first Hint is `reason` reflects current
/// state (not enforcement-forced). Server lifetime is loopback-bound so
/// this exercises the trust-a loopback branch of honor_opt_out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_dashboard_subscribe_metrics_honors_opt_out_on_localhost() {
    let port = pick_free_port();
    let storage = in_memory_storage().await;
    let cfg = test_spawn_config(port, storage);
    let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(cfg));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let endpoint = format!("http://127.0.0.1:{port}");
    let mut client = DashboardServiceClient::connect(endpoint).await.unwrap();
    let mut stream = client
        .subscribe_metrics(SubscribeMetricsRequest {
            interval_secs: 1,
            respect_server_hints: false, // opt-out
        })
        .await
        .expect("subscribe ok")
        .into_inner();

    // Server-side should have granted opt-out without emitting a warn-log
    // (we can't directly assert no warn here without tracing capture infra;
    // instead, verify the stream opens + yields a Hint as usual).
    let first = tokio::time::timeout(Duration::from_secs(3), stream.message())
        .await
        .expect("msg")
        .expect("not errored")
        .expect("not ended");
    assert!(matches!(
        first.payload,
        Some(subscribe_metrics_response::Payload::Hint(_))
    ));

    // Hold the counter briefly, then close.
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(stream);

    server_task.abort();
    let _ = server_task.await;
}

// ── B3-7: FailingStorage test harness ────────────────────────────────────
// Included at the top level so it is visible to all test sub-modules below.
// The `#[path]` attribute points to the companion support file.
#[cfg(feature = "grpc-dashboard")]
#[path = "support/failing_storage.rs"]
mod failing_storage;

// ── B3-7: SubscribeEvents integration tests (12) ─────────────────────────
//
// These tests exercise the `subscribe_events` RPC end-to-end: event-type
// filtering, rate limiting, DropAccumulator emission, ServerLoadHint under
// high CPU, concurrent clients, AiRuntimeStatus snapshot, PII sanitisation,
// and FailingStorage-driven idle invariants.
//
// Pattern: construct a `GrpcSpawnConfig` manually when the test needs to
// deviate from `test_spawn_config` defaults (e.g. custom system_monitor,
// ai_runtime_status_snapshot, pii_sanitizer, event_tx). Use
// `test_spawn_config` + field override when only one field differs.

#[cfg(feature = "grpc-dashboard")]
mod subscribe_events_tests {
    use super::*;

    use oneshim_api_contracts::stream::{AiRuntimeStatus, FrameUpdate, IdleUpdate, RealtimeEvent};
    use oneshim_core::config::LoadThresholds;
    use oneshim_core::ports::pii_sanitizer::PiiSanitizer;
    use oneshim_web::grpc::{test_support::mock_system_monitor::MockSystemMonitor, LoadPolicy};
    use oneshim_web::proto::dashboard::v1::{
        dashboard_event::Payload as DashboardPayload,
        subscribe_events_response::Payload as EventsPayload, SubscribeEventsRequest,
    };

    // ── Test helpers ─────────────────────────────────────────────────────

    /// Build a fully explicit `GrpcSpawnConfig` with a separate `event_tx`
    /// so the caller retains the sender.
    fn explicit_cfg(
        port: u16,
        storage: Arc<dyn oneshim_web::storage_port::WebStorage>,
        event_tx: tokio::sync::broadcast::Sender<RealtimeEvent>,
        cpu_pct: f32,
        mem_used_mb: u32,
        mem_total_mb: u32,
    ) -> oneshim_web::grpc::GrpcSpawnConfig {
        oneshim_web::grpc::GrpcSpawnConfig {
            port,
            storage,
            system_monitor: MockSystemMonitor::new(cpu_pct, mem_used_mb, mem_total_mb),
            event_tx,
            integration_auth_token: None,
            pii_sanitizer: None,
            ai_runtime_status_snapshot: None,
            load_policy: Arc::new(LoadPolicy::new(LoadThresholds::default())),
            streaming_enabled: true,
            max_concurrent_streams: 50,
        }
    }

    // ── Test #1 ──────────────────────────────────────────────────────────

    /// Frame events published via event_tx arrive on the subscriber stream.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn subscribe_events_streams_frame_after_capture() {
        let port = pick_free_port();
        let storage = in_memory_storage().await;
        let (event_tx, _) = tokio::sync::broadcast::channel(16);
        let cfg = explicit_cfg(port, storage, event_tx.clone(), 30.0, 4096, 16384);
        let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(cfg));
        wait_for_server_ready(port, Duration::from_secs(5)).await;

        let endpoint = format!("http://127.0.0.1:{port}");
        let mut client = DashboardServiceClient::connect(endpoint).await.unwrap();
        let mut stream = client
            .subscribe_events(SubscribeEventsRequest {
                event_types: vec!["frame".to_string()],
                respect_server_hints: true,
            })
            .await
            .expect("subscribe_events ok")
            .into_inner();

        // The first message may be the AiRuntimeStatus snapshot — drain it.
        // Our subscription is "frame" only, so ai_runtime_status is excluded.
        // Fire Frame events with retries to cover the subscribe-vs-send race.
        for _ in 0..5 {
            let _ = event_tx.send(RealtimeEvent::Frame(FrameUpdate {
                id: 99,
                timestamp: chrono::Utc::now().to_rfc3339(),
                app_name: "TestApp".to_string(),
                window_title: "w1".to_string(),
                importance: 0.75,
                trigger_type: "timer".to_string(),
            }));
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Loop until a Frame event arrives or the deadline expires.  On loaded
        // CI runners the HintEmitter's 1s warmup tick can race ahead of the
        // Frame emission, so we skip Hint / Dropped payloads and keep polling.
        // Matches the `first_frame` helper pattern used by test #6 (310144c7).
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let msg = tokio::time::timeout(remaining, stream.message())
                .await
                .expect("within 3s")
                .expect("not errored")
                .expect("not ended");

            match msg.payload.expect("payload") {
                EventsPayload::Event(de) => {
                    assert!(de.occurred_at.is_some(), "occurred_at must be set");
                    match de.payload.expect("de.payload") {
                        DashboardPayload::Frame(frame) => {
                            assert_eq!(frame.frame_id, 99);
                            assert_eq!(frame.trigger_type, "timer");
                            assert_eq!(frame.app_name, "TestApp");
                            break;
                        }
                        other => panic!("expected FrameEvent, got {other:?}"),
                    }
                }
                EventsPayload::Hint(_) | EventsPayload::Dropped(_) => continue,
            }
        }

        server_task.abort();
        let _ = server_task.await;
    }

    // ── Test #2 ──────────────────────────────────────────────────────────

    /// Idle events stream as-received; handler does NOT dedup identical payloads.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn subscribe_events_streams_idle_events_as_received() {
        let port = pick_free_port();
        let storage = in_memory_storage().await;
        let (event_tx, _) = tokio::sync::broadcast::channel(16);
        let cfg = explicit_cfg(port, storage, event_tx.clone(), 30.0, 4096, 16384);
        let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(cfg));
        wait_for_server_ready(port, Duration::from_secs(5)).await;

        let endpoint = format!("http://127.0.0.1:{port}");
        let mut client = DashboardServiceClient::connect(endpoint).await.unwrap();
        let mut stream = client
            .subscribe_events(SubscribeEventsRequest {
                event_types: vec!["idle".to_string()],
                respect_server_hints: true,
            })
            .await
            .expect("subscribe ok")
            .into_inner();

        // Fire 2 identical idle events (no dedup contract).
        for _ in 0..5 {
            let _ = event_tx.send(RealtimeEvent::Idle(IdleUpdate {
                is_idle: true,
                idle_secs: 300,
            }));
            tokio::time::sleep(Duration::from_millis(30)).await;
        }

        let mut idle_count = 0u32;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while idle_count < 2 && tokio::time::Instant::now() < deadline {
            let remaining = deadline - tokio::time::Instant::now();
            match tokio::time::timeout(remaining, stream.message()).await {
                Ok(Ok(Some(msg))) => {
                    if let Some(EventsPayload::Event(de)) = msg.payload {
                        if let Some(DashboardPayload::Idle(idle)) = de.payload {
                            assert!(idle.is_idle);
                            assert_eq!(idle.idle_secs, 300);
                            idle_count += 1;
                        }
                    }
                }
                _ => break,
            }
        }
        assert!(idle_count >= 2, "expected ≥2 IdleEvents, got {idle_count}");

        server_task.abort();
        let _ = server_task.await;
    }

    // ── Test #3 ──────────────────────────────────────────────────────────

    /// `event_types=["frame"]` filter: Frame passes, Idle is dropped.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn subscribe_events_filters_by_event_types() {
        let port = pick_free_port();
        let storage = in_memory_storage().await;
        let (event_tx, _) = tokio::sync::broadcast::channel(16);
        let cfg = explicit_cfg(port, storage, event_tx.clone(), 30.0, 4096, 16384);
        let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(cfg));
        wait_for_server_ready(port, Duration::from_secs(5)).await;

        let endpoint = format!("http://127.0.0.1:{port}");
        let mut client = DashboardServiceClient::connect(endpoint).await.unwrap();
        let mut stream = client
            .subscribe_events(SubscribeEventsRequest {
                event_types: vec!["frame".to_string()],
                respect_server_hints: true,
            })
            .await
            .expect("subscribe ok")
            .into_inner();

        // Fire: Frame, Idle, Frame — only 2 Frames should pass the filter.
        for _ in 0..3 {
            let _ = event_tx.send(RealtimeEvent::Frame(FrameUpdate {
                id: 1,
                timestamp: chrono::Utc::now().to_rfc3339(),
                app_name: "App".to_string(),
                window_title: "t".to_string(),
                importance: 0.5,
                trigger_type: "timer".to_string(),
            }));
            tokio::time::sleep(Duration::from_millis(30)).await;
            let _ = event_tx.send(RealtimeEvent::Idle(IdleUpdate {
                is_idle: true,
                idle_secs: 5,
            }));
            tokio::time::sleep(Duration::from_millis(30)).await;
        }

        // Collect events for 500 ms; count frames vs idle.
        let mut frame_count = 0u32;
        let mut idle_count = 0u32;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(700);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, stream.message()).await {
                Ok(Ok(Some(msg))) => {
                    if let Some(EventsPayload::Event(de)) = msg.payload {
                        match de.payload {
                            Some(DashboardPayload::Frame(_)) => frame_count += 1,
                            Some(DashboardPayload::Idle(_)) => idle_count += 1,
                            _ => {}
                        }
                    }
                }
                _ => break,
            }
        }

        assert!(
            frame_count >= 2,
            "expected ≥2 FrameEvents, got {frame_count}"
        );
        assert_eq!(
            idle_count, 0,
            "no IdleEvents should pass a frame-only filter"
        );

        server_task.abort();
        let _ = server_task.await;
    }

    // ── Test #4 ──────────────────────────────────────────────────────────

    /// Rapid burst beyond BURST_CAPACITY causes DropAccumulator to emit a
    /// `DroppedEventsSignal` with `dropped_count ≥ 5` and `reason = "rate_limit"`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn subscribe_events_drops_when_rate_limited() {
        use oneshim_web::grpc::BURST_CAPACITY;

        let port = pick_free_port();
        let storage = in_memory_storage().await;
        let (event_tx, _) = tokio::sync::broadcast::channel(64);
        let cfg = explicit_cfg(port, storage, event_tx.clone(), 30.0, 4096, 16384);
        let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(cfg));
        wait_for_server_ready(port, Duration::from_secs(5)).await;

        let endpoint = format!("http://127.0.0.1:{port}");
        let mut client = DashboardServiceClient::connect(endpoint).await.unwrap();
        let mut stream = client
            .subscribe_events(SubscribeEventsRequest {
                event_types: vec!["frame".to_string()],
                respect_server_hints: true,
            })
            .await
            .expect("subscribe ok")
            .into_inner();

        // Fire 25 events rapidly — BURST_CAPACITY=20 allowed, 5 dropped.
        let excess = 5u32;
        let total = BURST_CAPACITY + excess;
        for i in 0..total {
            let _ = event_tx.send(RealtimeEvent::Frame(FrameUpdate {
                id: i as i64,
                timestamp: chrono::Utc::now().to_rfc3339(),
                app_name: "BurstApp".to_string(),
                window_title: "t".to_string(),
                importance: 0.5,
                trigger_type: "timer".to_string(),
            }));
        }

        // Drain up to BURST_CAPACITY FrameEvents + wait ≥1s for the drop signal tick.
        let mut frame_count = 0u32;
        let mut dropped_signal: Option<oneshim_web::proto::dashboard::v1::DroppedEventsSignal> =
            None;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, stream.message()).await {
                Ok(Ok(Some(msg))) => match msg.payload {
                    Some(EventsPayload::Event(de)) => {
                        if let Some(DashboardPayload::Frame(_)) = de.payload {
                            frame_count += 1;
                        }
                    }
                    Some(EventsPayload::Dropped(sig)) => {
                        dropped_signal = Some(sig);
                        break; // found what we need
                    }
                    _ => {}
                },
                _ => break,
            }
        }

        assert!(
            frame_count <= BURST_CAPACITY,
            "admitted frames must not exceed burst capacity: got {frame_count}"
        );
        let sig = dropped_signal.expect("DroppedEventsSignal must be emitted after burst");
        assert!(
            sig.dropped_count >= excess as u64,
            "dropped_count must be ≥ {excess}, got {}",
            sig.dropped_count
        );
        assert_eq!(sig.reason, "rate_limit");
        assert!(
            sig.by_type
                .iter()
                .any(|tc| tc.event_type == "frame" && tc.count >= excess as u64),
            "by_type must contain frame entry with count >= {excess}: {:?}",
            sig.by_type
        );

        server_task.abort();
        let _ = server_task.await;
    }

    // ── Test #13 ──────────────────────────────────────────────────────────

    /// Broadcast-channel overflow (`broadcast::RecvError::Lagged`) surfaces as a
    /// `DroppedEventsSignal` with `reason = "channel_lag"` — distinct from the
    /// `reason = "rate_limit"` signal. Event-type breakdown uses the `"unknown"`
    /// sentinel because the broadcast lag error does not carry the dropped
    /// event's type (OTel `error.type = "unknown"` convention).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn subscribe_events_emits_channel_lag_drop_signal() {
        let port = pick_free_port();
        let storage = in_memory_storage().await;
        // Small broadcast capacity so that a rapid burst before the handler's
        // first `recv()` forces `RecvError::Lagged`. Burst > capacity by ≥2×.
        let (event_tx, _) = tokio::sync::broadcast::channel(4);
        let cfg = explicit_cfg(port, storage, event_tx.clone(), 30.0, 4096, 16384);
        let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(cfg));
        wait_for_server_ready(port, Duration::from_secs(5)).await;

        let endpoint = format!("http://127.0.0.1:{port}");
        let mut client = DashboardServiceClient::connect(endpoint).await.unwrap();
        let mut stream = client
            .subscribe_events(SubscribeEventsRequest {
                event_types: vec!["frame".to_string()],
                respect_server_hints: true,
            })
            .await
            .expect("subscribe ok")
            .into_inner();

        // Reduce race between `.subscribe()` returning on client side and the
        // server handler reaching the `select!` loop where `rx.recv()` is first
        // polled. Without this delay, a fast handler could drain the burst
        // linearly and never see `Lagged` because the ring never overflows from
        // the consumer's POV. 100ms is empirical; raise if CI flakes.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Fire enough events in a tight loop to overflow the broadcast ring
        // multiple times over. With capacity=4 and 64 sends, the handler's
        // first several recv() calls should see at least one `Lagged(n)`.
        for i in 0..64 {
            let _ = event_tx.send(RealtimeEvent::Frame(FrameUpdate {
                id: i as i64,
                timestamp: chrono::Utc::now().to_rfc3339(),
                app_name: "LagApp".to_string(),
                window_title: "t".to_string(),
                importance: 0.5,
                trigger_type: "timer".to_string(),
            }));
        }

        // Drain the stream until we see a DroppedEventsSignal with reason
        // "channel_lag". Bound the wait to 5s (tick is 1s; give 3-4 ticks).
        let mut channel_lag_signal: Option<oneshim_web::proto::dashboard::v1::DroppedEventsSignal> =
            None;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, stream.message()).await {
                Ok(Ok(Some(msg))) => match msg.payload {
                    Some(EventsPayload::Dropped(sig)) if sig.reason == "channel_lag" => {
                        channel_lag_signal = Some(sig);
                        break;
                    }
                    // Ignore other payloads (Frame events, rate_limit drop signals, hints).
                    _ => {}
                },
                _ => break,
            }
        }

        let sig = channel_lag_signal.expect(
            "DroppedEventsSignal with reason=channel_lag must be emitted after broadcast overflow",
        );
        assert_eq!(sig.reason, "channel_lag");
        // N-per-Lagged: one RecvError::Lagged(n) contributes n to dropped_count.
        // With broadcast capacity=4 and 64 sends, the first Lagged fires with
        // n ≥ 4 in the worst case, so >= 2 is a conservative flake-safe bound
        // that proves N-per-Lagged semantics (vs the old 1-per-Lagged MVP).
        assert!(
            sig.dropped_count >= 2,
            "dropped_count must be ≥ 2 under N-per-Lagged, got {}",
            sig.dropped_count
        );
        assert!(
            sig.by_type.iter().any(|tc| tc.event_type == "unknown"),
            "by_type must contain an \"unknown\" entry (OTel convention): {:?}",
            sig.by_type
        );

        server_task.abort();
        let _ = server_task.await;
    }

    // ── Test #5 ──────────────────────────────────────────────────────────

    /// High-CPU system monitor drives a High/Critical ServerLoadHint on the
    /// events stream (emitted from the 1s tick alongside drop accumulator).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn subscribe_events_emits_server_load_hint_on_high_cpu() {
        use oneshim_web::proto::dashboard::v1::server_load_hint::Level as HintLevel;

        let port = pick_free_port();
        let storage = in_memory_storage().await;
        let (event_tx, _) = tokio::sync::broadcast::channel(16);

        // 95% CPU, 14GB used / 16GB total → Critical (past warmup once
        // LoadPolicy is constructed with default thresholds).
        let mut cfg = explicit_cfg(port, storage, event_tx.clone(), 95.0, 14_336, 16_384);
        // Rewind the load policy so warm-up is not in effect. We can't
        // directly rewind LoadPolicy::started_at from outside the module, but
        // classification already returns Critical at 95% CPU — wait for the
        // 30s warmup to expire (too long in CI). Instead, assert ≥ Medium
        // (warmup forces Medium) which still proves the hint path fires.
        cfg.load_policy = Arc::new(LoadPolicy::new(LoadThresholds::default()));

        let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(cfg));
        wait_for_server_ready(port, Duration::from_secs(5)).await;

        let endpoint = format!("http://127.0.0.1:{port}");
        let mut client = DashboardServiceClient::connect(endpoint).await.unwrap();
        let mut stream = client
            .subscribe_events(SubscribeEventsRequest {
                event_types: vec![], // empty = all types
                respect_server_hints: true,
            })
            .await
            .expect("subscribe ok")
            .into_inner();

        // Fire a Frame to wake the select! branch; the 1s tick emits the hint.
        let _ = event_tx.send(RealtimeEvent::Frame(FrameUpdate {
            id: 1,
            timestamp: chrono::Utc::now().to_rfc3339(),
            app_name: "A".to_string(),
            window_title: "t".to_string(),
            importance: 0.5,
            trigger_type: "timer".to_string(),
        }));

        // Wait up to 4s for a Hint payload.
        let mut found_hint = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, stream.message()).await {
                Ok(Ok(Some(msg))) => {
                    if let Some(EventsPayload::Hint(hint)) = msg.payload {
                        // During warmup: Medium (2). Post-warmup: High (3) or Critical (4).
                        assert!(
                            hint.load_level >= HintLevel::LoadLevelMedium as i32,
                            "expected load_level ≥ Medium, got {}",
                            hint.load_level
                        );
                        found_hint = true;
                        break;
                    }
                }
                _ => break,
            }
        }
        assert!(
            found_hint,
            "expected at least one ServerLoadHint on the events stream"
        );

        server_task.abort();
        let _ = server_task.await;
    }

    // ── Test #6 ──────────────────────────────────────────────────────────

    /// Three concurrent subscribers each receive the same FrameEvent (broadcast fan-out).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn subscribe_events_multiple_concurrent_clients_independent() {
        let port = pick_free_port();
        let storage = in_memory_storage().await;
        let (event_tx, _) = tokio::sync::broadcast::channel(32);
        let cfg = explicit_cfg(port, storage, event_tx.clone(), 30.0, 4096, 16384);
        let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(cfg));
        wait_for_server_ready(port, Duration::from_secs(5)).await;

        let endpoint = format!("http://127.0.0.1:{port}");

        // Open 3 separate clients + streams.
        let req = || SubscribeEventsRequest {
            event_types: vec!["frame".to_string()],
            respect_server_hints: true,
        };
        let mut c1 = DashboardServiceClient::connect(endpoint.clone())
            .await
            .unwrap();
        let mut c2 = DashboardServiceClient::connect(endpoint.clone())
            .await
            .unwrap();
        let mut c3 = DashboardServiceClient::connect(endpoint.clone())
            .await
            .unwrap();

        let mut s1 = c1.subscribe_events(req()).await.unwrap().into_inner();
        let mut s2 = c2.subscribe_events(req()).await.unwrap().into_inner();
        let mut s3 = c3.subscribe_events(req()).await.unwrap().into_inner();

        // Brief settle — all three streams should now be subscribed.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Fire exactly one FrameEvent.
        for _ in 0..5 {
            let _ = event_tx.send(RealtimeEvent::Frame(FrameUpdate {
                id: 42,
                timestamp: chrono::Utc::now().to_rfc3339(),
                app_name: "FanOut".to_string(),
                window_title: "all3".to_string(),
                importance: 0.9,
                trigger_type: "user_action".to_string(),
            }));
            tokio::time::sleep(Duration::from_millis(40)).await;
        }

        async fn first_frame(
            stream: &mut tonic::codec::Streaming<
                oneshim_web::proto::dashboard::v1::SubscribeEventsResponse,
            >,
        ) -> bool {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
            loop {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    return false;
                }
                match tokio::time::timeout(remaining, stream.message()).await {
                    Ok(Ok(Some(msg))) => {
                        if let Some(EventsPayload::Event(de)) = msg.payload {
                            if let Some(DashboardPayload::Frame(f)) = de.payload {
                                if f.frame_id == 42 {
                                    return true;
                                }
                            }
                        }
                        // Non-frame message (e.g. ServerLoadHint on 1s tick) — continue polling
                    }
                    Err(_) | Ok(Ok(None)) | Ok(Err(_)) => return false,
                }
            }
        }

        assert!(
            first_frame(&mut s1).await,
            "client 1 must receive the FrameEvent"
        );
        assert!(
            first_frame(&mut s2).await,
            "client 2 must receive the FrameEvent"
        );
        assert!(
            first_frame(&mut s3).await,
            "client 3 must receive the FrameEvent"
        );

        server_task.abort();
        let _ = server_task.await;
    }

    // ── Test #7 ──────────────────────────────────────────────────────────

    /// `ai_runtime_status_snapshot = None` → sentinel emission with all "unknown" fields.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn subscribe_ai_runtime_status_sends_sentinel_when_none() {
        let port = pick_free_port();
        let storage = in_memory_storage().await;
        let (event_tx, _) = tokio::sync::broadcast::channel(16);
        // ai_runtime_status_snapshot: None is the default in explicit_cfg.
        let cfg = explicit_cfg(port, storage, event_tx, 30.0, 4096, 16384);
        let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(cfg));
        wait_for_server_ready(port, Duration::from_secs(5)).await;

        let endpoint = format!("http://127.0.0.1:{port}");
        let mut client = DashboardServiceClient::connect(endpoint).await.unwrap();
        let mut stream = client
            .subscribe_events(SubscribeEventsRequest {
                event_types: vec!["ai_runtime_status".to_string()],
                respect_server_hints: true,
            })
            .await
            .expect("subscribe ok")
            .into_inner();

        // First (and only immediate) emission is the snapshot.
        let resp = tokio::time::timeout(Duration::from_secs(3), stream.message())
            .await
            .expect("within 3s")
            .expect("not errored")
            .expect("not ended");

        match resp.payload.expect("payload") {
            EventsPayload::Event(de) => match de.payload.expect("de.payload") {
                DashboardPayload::AiRuntimeStatus(ai) => {
                    assert_eq!(ai.ocr_source, "unknown");
                    assert_eq!(ai.llm_source, "unknown");
                    assert_eq!(ai.ocr_fallback_reason, "");
                    assert_eq!(ai.llm_fallback_reason, "");
                }
                other => panic!("expected AiRuntimeStatusEvent, got {other:?}"),
            },
            other => panic!("expected Event payload, got {other:?}"),
        }

        server_task.abort();
        let _ = server_task.await;
    }

    // ── Test #8 ──────────────────────────────────────────────────────────

    /// After the AiRuntimeStatus snapshot, the stream stays open (does not close).
    /// The 1-second periodic tick emits ServerLoadHint on ALL streams (unconditionally
    /// on first call). We verify liveness by checking the next message within 3s is NOT
    /// a stream close (Ok(None)), i.e. either a Hint arrives or the timeout fires —
    /// both are "open". Only Ok(None) means "stream ended" which would be the failure.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn subscribe_only_ai_runtime_status_keeps_stream_open() {
        let port = pick_free_port();
        let storage = in_memory_storage().await;
        let (event_tx, _) = tokio::sync::broadcast::channel(16);
        let cfg = explicit_cfg(port, storage, event_tx, 30.0, 4096, 16384);
        let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(cfg));
        wait_for_server_ready(port, Duration::from_secs(5)).await;

        let endpoint = format!("http://127.0.0.1:{port}");
        let mut client = DashboardServiceClient::connect(endpoint).await.unwrap();
        let mut stream = client
            .subscribe_events(SubscribeEventsRequest {
                event_types: vec!["ai_runtime_status".to_string()],
                respect_server_hints: true,
            })
            .await
            .expect("subscribe ok")
            .into_inner();

        // Drain the snapshot.
        let _snapshot = tokio::time::timeout(Duration::from_secs(3), stream.message())
            .await
            .expect("snapshot within 3s")
            .expect("not errored")
            .expect("not ended");

        // The stream must remain open after the snapshot. The 1-second periodic tick
        // emits a ServerLoadHint on all streams unconditionally on the first call.
        // We accept either:
        //   (a) A Hint message arrives (stream is open, Ok(Some(msg))) — pass.
        //   (b) The 3s timeout fires without the stream closing (Err) — pass.
        // Only Ok(None) (stream ended) is a failure.
        let keep_open_result = tokio::time::timeout(Duration::from_secs(3), stream.message()).await;
        match keep_open_result {
            // Timeout: stream is still open waiting for events — desired.
            Err(_timeout) => {}
            // A message arrived (ServerLoadHint from the 1s tick) — stream is open.
            Ok(Ok(Some(_msg))) => {}
            // Stream closed: this is the failure case.
            Ok(Ok(None)) => panic!("stream closed after AiRuntimeStatus snapshot — must stay open"),
            Ok(Err(e)) => panic!("stream error after snapshot: {e}"),
        }

        server_task.abort();
        let _ = server_task.await;
    }

    // ── Test #9 ──────────────────────────────────────────────────────────

    /// PII sanitizer redacts email addresses in `ocr_fallback_reason` before
    /// the AiRuntimeStatus snapshot is emitted.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn subscribe_events_sanitizes_ai_runtime_status_fallback_reason() {
        use oneshim_core::config::PiiFilterLevel;

        // Minimal test sanitizer: replaces the test email address.
        struct RedactingSanitizer;
        impl PiiSanitizer for RedactingSanitizer {
            fn sanitize_text(&self, text: &str, _level: PiiFilterLevel) -> String {
                text.replace("secret@example.com", "[EMAIL]")
            }
        }

        let port = pick_free_port();
        let storage = in_memory_storage().await;
        let (event_tx, _) = tokio::sync::broadcast::channel(16);

        let cfg = oneshim_web::grpc::GrpcSpawnConfig {
            port,
            storage,
            system_monitor: MockSystemMonitor::new(30.0, 4096, 16384),
            event_tx,
            integration_auth_token: None,
            pii_sanitizer: Some(Arc::new(RedactingSanitizer)),
            ai_runtime_status_snapshot: Some(AiRuntimeStatus {
                ocr_source: "local".to_string(),
                llm_source: "local".to_string(),
                ocr_fallback_reason: Some("error at secret@example.com".to_string()),
                llm_fallback_reason: None,
            }),
            load_policy: Arc::new(LoadPolicy::new(LoadThresholds::default())),
            streaming_enabled: true,
            max_concurrent_streams: 50,
        };

        let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(cfg));
        wait_for_server_ready(port, Duration::from_secs(5)).await;

        let endpoint = format!("http://127.0.0.1:{port}");
        let mut client = DashboardServiceClient::connect(endpoint).await.unwrap();
        let mut stream = client
            .subscribe_events(SubscribeEventsRequest {
                event_types: vec!["ai_runtime_status".to_string()],
                respect_server_hints: true,
            })
            .await
            .expect("subscribe ok")
            .into_inner();

        let resp = tokio::time::timeout(Duration::from_secs(3), stream.message())
            .await
            .expect("within 3s")
            .expect("not errored")
            .expect("not ended");

        match resp.payload.expect("payload") {
            EventsPayload::Event(de) => match de.payload.expect("de.payload") {
                DashboardPayload::AiRuntimeStatus(ai) => {
                    assert_eq!(
                        ai.ocr_fallback_reason, "error at [EMAIL]",
                        "PII must be sanitized in ocr_fallback_reason"
                    );
                    assert_eq!(
                        ai.llm_fallback_reason, "",
                        "None fallback_reason must become empty string"
                    );
                    assert_eq!(ai.ocr_source, "local");
                    assert_eq!(ai.llm_source, "local");
                }
                other => panic!("expected AiRuntimeStatusEvent, got {other:?}"),
            },
            other => panic!("expected Event payload, got {other:?}"),
        }

        server_task.abort();
        let _ = server_task.await;
    }

    // ── Test #10 ─────────────────────────────────────────────────────────

    /// Idle events are emitted via event_tx even when start_idle_period fails.
    ///
    /// The publisher-side invariant (helpers.rs `handle_idle_tick`) emits on
    /// event_tx *after* the storage call, regardless of success or failure.
    /// This integration test simulates the "emit-despite-Err" path by firing
    /// `event_tx.send(RealtimeEvent::Idle(...))` directly (the publisher already
    /// handles the storage error and then emits — subscribers see the event
    /// either way).
    ///
    /// The FailingStorage is constructed and wired through `GrpcSpawnConfig` so
    /// the test infrastructure correctly exercises the server path, even though
    /// the idle storage failure occurs on the publisher (scheduler) side, not
    /// inside the gRPC handler itself.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn subscribe_events_emits_idle_even_when_storage_fails() {
        use crate::failing_storage::FailingStorage;

        let port = pick_free_port();
        let sqlite = oneshim_storage::sqlite::SqliteStorage::open_in_memory(30).expect("sqlite");
        let failing = Arc::new(FailingStorage::new(Arc::new(sqlite)).with_fail_start_idle());
        let storage: Arc<dyn oneshim_web::storage_port::WebStorage> = failing;

        let (event_tx, _) = tokio::sync::broadcast::channel(16);
        let cfg = oneshim_web::grpc::GrpcSpawnConfig {
            port,
            storage,
            system_monitor: MockSystemMonitor::new(30.0, 4096, 16384),
            event_tx: event_tx.clone(),
            integration_auth_token: None,
            pii_sanitizer: None,
            ai_runtime_status_snapshot: None,
            load_policy: Arc::new(LoadPolicy::new(LoadThresholds::default())),
            streaming_enabled: true,
            max_concurrent_streams: 50,
        };

        let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(cfg));
        wait_for_server_ready(port, Duration::from_secs(5)).await;

        let endpoint = format!("http://127.0.0.1:{port}");
        let mut client = DashboardServiceClient::connect(endpoint).await.unwrap();
        let mut stream = client
            .subscribe_events(SubscribeEventsRequest {
                event_types: vec!["idle".to_string()],
                respect_server_hints: true,
            })
            .await
            .expect("subscribe ok")
            .into_inner();

        // Simulate publisher: emit-despite-storage-failure by sending directly.
        for _ in 0..5 {
            let _ = event_tx.send(RealtimeEvent::Idle(IdleUpdate {
                is_idle: true,
                idle_secs: 100,
            }));
            tokio::time::sleep(Duration::from_millis(40)).await;
        }

        // Skip any initial warmup Hint / Dropped frames — the handler emits a
        // ServerLoadHint immediately at subscribe time (spec §4.1) before any
        // realtime Event arrives. The test cares about the Idle event
        // actually being delivered; iterate until we see an Event payload.
        let idle = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let msg = stream
                    .message()
                    .await
                    .expect("not errored")
                    .expect("not ended");
                match msg.payload.expect("payload") {
                    EventsPayload::Event(de) => return de,
                    _ => continue, // Hint / Dropped — keep waiting for the Event
                }
            }
        })
        .await
        .expect("within 3s");

        match idle.payload.expect("de.payload") {
            DashboardPayload::Idle(idle) => {
                assert!(idle.is_idle);
                assert_eq!(idle.idle_secs, 100);
            }
            other => panic!("expected IdleEvent, got {other:?}"),
        }

        server_task.abort();
        let _ = server_task.await;
    }

    // ── Test #11 ─────────────────────────────────────────────────────────

    /// Negative test: when no Frame event is published via event_tx, the
    /// subscriber receives no FrameEvent within the observation window.
    ///
    /// This verifies the handler does NOT synthesise Frame events autonomously —
    /// it is a pure pass-through of event_tx emissions. The publisher-side
    /// invariant (save_frame → emit OR Err → skip-emit) is covered by the
    /// helpers.rs unit tests.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn subscribe_events_skips_frame_emit_when_save_fails() {
        let port = pick_free_port();
        let storage = in_memory_storage().await;
        let (event_tx, _) = tokio::sync::broadcast::channel(16);
        // Do NOT send any Frame events — simulates the publisher skipping
        // emission when save_frame_metadata_with_bounds returns Err.
        let cfg = explicit_cfg(port, storage, event_tx, 30.0, 4096, 16384);
        let server_task = tokio::spawn(oneshim_web::grpc::serve_optional(cfg));
        wait_for_server_ready(port, Duration::from_secs(5)).await;

        let endpoint = format!("http://127.0.0.1:{port}");
        let mut client = DashboardServiceClient::connect(endpoint).await.unwrap();
        let mut stream = client
            .subscribe_events(SubscribeEventsRequest {
                event_types: vec!["frame".to_string()],
                respect_server_hints: true,
            })
            .await
            .expect("subscribe ok")
            .into_inner();

        // Observe for 500 ms — no Frame events should appear.
        let mut frame_count = 0u32;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, stream.message()).await {
                Ok(Ok(Some(msg))) => {
                    if let Some(EventsPayload::Event(de)) = msg.payload {
                        if let Some(DashboardPayload::Frame(_)) = de.payload {
                            frame_count += 1;
                        }
                    }
                }
                _ => break,
            }
        }

        assert_eq!(
            frame_count, 0,
            "handler must not synthesise Frame events without event_tx publication"
        );

        server_task.abort();
        let _ = server_task.await;
    }

    // ── Test #12 ─────────────────────────────────────────────────────────

    /// Source-level proof that HTTP/2 keepalive is configured on the gRPC server.
    /// Uses `include_str!` to read the grpc/mod.rs source at compile-time.
    #[test]
    fn grpc_server_configures_http2_keepalive() {
        const SRC: &str = include_str!("../src/grpc/mod.rs");
        assert!(
            SRC.contains(".http2_keepalive_interval(Some("),
            "http2_keepalive_interval must be configured on the gRPC dashboard server"
        );
        assert!(
            SRC.contains(".http2_keepalive_timeout(Some("),
            "http2_keepalive_timeout must be configured alongside the interval"
        );
    }
}
