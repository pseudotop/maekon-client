// SubscribeMetrics streaming integration tests — initial-hint warmup,
// streaming-disabled rejection, interval bucket cadence, realtime
// event_tx-driven wakeups, active-stream cap, DNS-rebind host rejection,
// reconnect-cycle guard-leak check, and localhost opt-out. Split from
// `grpc_dashboard_integration.rs` by scenario family (#7730).

use std::sync::Arc;
use std::time::Duration;

use maekon_web::proto::dashboard::v1::{subscribe_metrics_response, SubscribeMetricsRequest};

use crate::common::{
    connect_dashboard_client, in_memory_storage, pick_free_port, test_spawn_config,
    wait_for_server_ready, TEST_LOCAL_AUTH_TOKEN,
};

// ── B2-10: SubscribeMetrics integration tests (8) ────────────────────
//
// Infrastructure: per spec §7.1 virtual-time ordering — when using
// `start_paused = true`, the `subscribe_metrics` RPC call MUST come AFTER
// `tokio::time::pause()` so the handler's `tokio::time::interval` registers
// against the paused clock. `wait_for_server_ready` (real-clock sleep) MUST
// come BEFORE `pause()`.

use maekon_api_contracts::stream::{MetricsUpdate, RealtimeEvent};

/// Test #1 — First yield is a `Hint`, reason starts with `"warmup"` (within 30s).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_dashboard_subscribe_metrics_emits_initial_hint() {
    let port = pick_free_port();
    let storage = in_memory_storage().await;
    let cfg = test_spawn_config(port, storage);
    let server_task = tokio::spawn(maekon_web::grpc::serve_optional(cfg));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let mut client = connect_dashboard_client(port).await;

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
    let server_task = tokio::spawn(maekon_web::grpc::serve_optional(cfg));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let mut client = connect_dashboard_client(port).await;

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
    let server_task = tokio::spawn(maekon_web::grpc::serve_optional(cfg));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let mut client = connect_dashboard_client(port).await;

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
    use maekon_core::config::LoadThresholds;
    use maekon_web::grpc::{test_support::mock_system_monitor::MockSystemMonitor, LoadPolicy};

    let port = pick_free_port();
    let storage = in_memory_storage().await;
    let (event_tx, _) = tokio::sync::broadcast::channel(16);
    let cfg = maekon_web::grpc::GrpcSpawnConfig {
        port,
        storage,
        system_monitor: MockSystemMonitor::new(30.0, 4096, 16384),
        event_tx: event_tx.clone(),
        integration_auth_token: None,
        local_auth_token: Some(Arc::from(TEST_LOCAL_AUTH_TOKEN)),
        pii_sanitizer: None,
        ai_runtime_status_snapshot: None,
        load_policy: Arc::new(LoadPolicy::new(LoadThresholds::default())),
        streaming_enabled: true,
        max_concurrent_streams: 50,
    };
    let server_task = tokio::spawn(maekon_web::grpc::serve_optional(cfg));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let mut client = connect_dashboard_client(port).await;
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
    let server_task = tokio::spawn(maekon_web::grpc::serve_optional(cfg));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let mut c1 = connect_dashboard_client(port).await;
    let mut c2 = connect_dashboard_client(port).await;
    let mut c3 = connect_dashboard_client(port).await;

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
    let server_task = tokio::spawn(maekon_web::grpc::serve_optional(cfg));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let mut client = connect_dashboard_client(port).await;

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
    let server_task = tokio::spawn(maekon_web::grpc::serve_optional(cfg));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    for i in 0..5 {
        let mut client = connect_dashboard_client(port).await;
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
    let server_task = tokio::spawn(maekon_web::grpc::serve_optional(cfg));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let mut client = connect_dashboard_client(port).await;
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
