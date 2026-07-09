// D33 / Task 9.4 live-config-reload integration tests — streaming-toggle
// convergence, warmup preservation, malformed-threshold rejection, rapid
// coalescing, in-flight-stream propagation, and reload-task shutdown.
// Split from `external_grpc_integration.rs` by scenario family (#7730).

use std::sync::Arc;
use std::time::Duration;

use tonic::Code;

use maekon_web::grpc::external::test_support::{server_cert_pem, test_mint_jwt};
use maekon_web::grpc::LoadPolicy;
use maekon_web::proto::dashboard::v1::dashboard_service_client::DashboardServiceClient;

use crate::common::{make_tls_channel, LiveReloadHarness};

/// G3 gate test — streaming toggle reflects within 1 second.
///
/// Spec §9.2 L1407, D33 (CI convergence bound). Seeds the config with
/// `streaming_enabled = true`, verifies a sanity `subscribe_metrics` call
/// succeeds, then flips `external_grpc.streaming_enabled = Some(false)`
/// and polls until the next `subscribe_metrics` returns `Unavailable`.
/// Panics if convergence takes ≥ 1s.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_live_streaming_toggle_reflects_within_1s() {
    use std::time::Instant;

    // Seed: web=true, external=None — exercise fallback path.
    let harness = LiveReloadHarness::builder()
        .seed(|c| {
            c.web.grpc_streaming_enabled = true;
            c.external_grpc.streaming_enabled = None;
        })
        .build()
        .await;

    // Mint a JWT + build a TLS channel (external server requires both).
    let token = test_mint_jwt(
        &harness.jwt_kp.enc_key,
        "user-g3",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(harness.port, &cert_pem, None).await;

    // Sanity: initial subscribe_metrics succeeds (streaming_enabled = true).
    let mut req =
        tonic::Request::new(maekon_web::proto::dashboard::v1::SubscribeMetricsRequest::default());
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}").parse().expect("valid header"),
    );
    let sanity = DashboardServiceClient::new(channel.clone())
        .subscribe_metrics(req)
        .await;
    // Security-relevant: streaming_enabled=true with a valid JWT must open the stream
    // (not be rejected as Unavailable). Pin the gRPC status code so a regression that
    // changes the gate to Unavailable/PermissionDenied is caught explicitly.
    let sanity_stream = sanity.unwrap_or_else(|e| {
        panic!(
            "initial subscribe must succeed with streaming_enabled=true; got status {:?} (code={:?})",
            e,
            e.code()
        )
    });
    drop(sanity_stream);

    // Flip streaming_enabled to false; ConfigReloadTask observes the watch
    // change and swaps the LiveSnapshot atomically. The per-request entry
    // in `subscribe_metrics` will see the new snapshot next.
    let start = Instant::now();
    harness
        .cfg_mgr
        .update_with(|c| {
            c.external_grpc.streaming_enabled = Some(false);
            Ok(())
        })
        .expect("update_with apply");

    // Poll until subscribe_metrics returns Unavailable. Cap at 1s (G3).
    let timeout = Duration::from_secs(1);
    loop {
        let mut req = tonic::Request::new(
            maekon_web::proto::dashboard::v1::SubscribeMetricsRequest::default(),
        );
        req.metadata_mut().insert(
            "authorization",
            format!("Bearer {token}").parse().expect("valid header"),
        );
        let result = DashboardServiceClient::new(channel.clone())
            .subscribe_metrics(req)
            .await;
        if let Err(status) = &result {
            if status.code() == Code::Unavailable {
                let elapsed = start.elapsed();
                assert!(
                    elapsed < timeout,
                    "G3 violation: convergence {elapsed:?} >= 1s cap"
                );
                harness.shutdown().await;
                return; // PASS
            }
        }
        if start.elapsed() > timeout {
            panic!("G3 violation: streaming toggle did not reflect within 1s (D33 CI bound)");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// D27 — warmup preservation. Seeds an initial `LiveSnapshot` whose
/// `started_at` is 60s in the past (well out of the 30s warmup window),
/// then reloads with new thresholds. After reload, `is_in_warmup()` must
/// remain `false` AND the new thresholds must be visible.
///
/// Uses `LoadPolicy::try_new_with_started_at` to construct the past-warmup
/// policy without waiting 30s of real time.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_live_load_thresholds_applied_without_warmup_reset() {
    // Build an initial load_policy whose started_at is 60s in the past —
    // well beyond the 30s WARMUP. `try_new_with_started_at` is the API
    // that ConfigReloadTask uses internally to preserve warmup across
    // reloads (D27); we use it here to bootstrap the test snapshot.
    let past_anchor = std::time::Instant::now() - std::time::Duration::from_secs(60);
    let initial_policy = Arc::new(
        LoadPolicy::try_new_with_started_at(
            maekon_core::config::LoadThresholds::default(),
            past_anchor,
        )
        .expect("valid initial thresholds"),
    );
    assert!(
        !initial_policy.is_in_warmup(),
        "precondition: initial policy must already be out of warmup"
    );

    let harness = LiveReloadHarness::builder()
        .initial_load_policy(initial_policy)
        .build()
        .await;

    // Reload with new (still valid) thresholds. ConfigReloadTask must
    // preserve the original `started_at` per D27.
    let new_thresholds = maekon_core::config::LoadThresholds {
        min_free_mem_gb: 1.5,
        cpu_low_pct: 25.0,
        cpu_medium_pct: 55.0,
        cpu_high_pct: 80.0,
    };
    harness
        .cfg_mgr
        .update_with(|c| {
            c.web.grpc_load_thresholds = Some(new_thresholds.clone());
            Ok(())
        })
        .expect("reload new thresholds");

    // Give the reload task a moment to observe the watch change + apply.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let snap = harness.live.snapshot();
    let post_thresholds = snap.load_policy.thresholds();
    assert!(
        (post_thresholds.cpu_low_pct - 25.0).abs() < f32::EPSILON,
        "new cpu_low_pct must apply; got {}",
        post_thresholds.cpu_low_pct
    );
    assert!(
        (post_thresholds.cpu_medium_pct - 55.0).abs() < f32::EPSILON,
        "new cpu_medium_pct must apply; got {}",
        post_thresholds.cpu_medium_pct
    );
    assert!(
        !snap.load_policy.is_in_warmup(),
        "D27: warmup anchor must carry over across reloads"
    );
    assert_eq!(
        snap.load_policy.started_at(),
        past_anchor,
        "D27: started_at must be bit-identical to the pre-reload anchor"
    );

    harness.shutdown().await;
}

/// Partial-apply invariant — a malformed thresholds reload is rejected and
/// the previous policy is preserved, while `streaming_enabled` (trivially
/// valid) still updates. Task 2.1 commit db1d1252 guarantees this via
/// `apply_config` keeping `current.load_policy` when `try_new_with_started_at`
/// errors.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_live_reload_rejects_malformed_thresholds_and_continues() {
    let initial_thresholds = maekon_core::config::LoadThresholds {
        min_free_mem_gb: 1.0,
        cpu_low_pct: 30.0,
        cpu_medium_pct: 60.0,
        cpu_high_pct: 85.0,
    };
    let initial_policy = Arc::new(LoadPolicy::new(initial_thresholds.clone()));

    let seed_thresholds = initial_thresholds.clone();
    let harness = LiveReloadHarness::builder()
        .seed(move |c| {
            c.web.grpc_streaming_enabled = true;
            c.external_grpc.streaming_enabled = Some(true);
            // Seed explicit valid thresholds so we can assert they survive.
            c.web.grpc_load_thresholds = Some(seed_thresholds);
        })
        .initial_load_policy(initial_policy.clone())
        .build()
        .await;

    // Reload with invalid thresholds (low > medium violates ordering) AND
    // flip streaming_enabled. Partial-apply: streaming flips, policy does
    // NOT.
    harness
        .cfg_mgr
        .update_with(|c| {
            c.external_grpc.streaming_enabled = Some(false);
            c.web.grpc_load_thresholds = Some(maekon_core::config::LoadThresholds {
                min_free_mem_gb: 1.0,
                cpu_low_pct: 90.0, // invalid: low > medium
                cpu_medium_pct: 50.0,
                cpu_high_pct: 85.0,
            });
            Ok(())
        })
        .expect("update_with (malformed thresholds)");

    tokio::time::sleep(Duration::from_millis(100)).await;

    let snap = harness.live.snapshot();
    assert!(
        !snap.streaming_enabled,
        "streaming_enabled update MUST apply despite malformed thresholds (partial-apply)"
    );
    let post_thresholds = snap.load_policy.thresholds();
    assert!(
        (post_thresholds.cpu_low_pct - 30.0).abs() < f32::EPSILON,
        "invalid thresholds rejected; previous cpu_low_pct must survive; got {}",
        post_thresholds.cpu_low_pct
    );
    assert!(
        (post_thresholds.cpu_medium_pct - 60.0).abs() < f32::EPSILON,
        "invalid thresholds rejected; previous cpu_medium_pct must survive; got {}",
        post_thresholds.cpu_medium_pct
    );
    assert!(
        Arc::ptr_eq(&snap.load_policy, &initial_policy),
        "invalid policy rejected; Arc identity must equal the initial policy"
    );
    assert!(
        harness
            .metrics
            .config_reload_task_alive
            .load(std::sync::atomic::Ordering::Relaxed),
        "reload task must remain alive after rejecting a malformed update"
    );

    // Follow-up valid reload must still apply — the task survived the
    // invalid one and keeps draining events.
    harness
        .cfg_mgr
        .update_with(|c| {
            c.external_grpc.streaming_enabled = Some(true);
            Ok(())
        })
        .expect("follow-up valid reload");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        harness.live.snapshot().streaming_enabled,
        "follow-up valid reload must still apply after the rejected one"
    );

    harness.shutdown().await;
}

/// Watch coalescing — 100 rapid `update_with` calls must not panic the
/// reload task AND the live snapshot must match the LAST update's value.
/// `tokio::sync::watch` has latest-wins semantics: the reload task's
/// `changed().await` may coalesce intermediate transitions, but the
/// final observed state must equal the final sent state.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_live_reload_coalesces_rapid_updates() {
    let harness = LiveReloadHarness::builder()
        .seed(|c| {
            c.external_grpc.streaming_enabled = Some(true);
        })
        .build()
        .await;

    // Fire 100 updates as fast as `update_with` will accept them. Alternate
    // streaming_enabled so every call genuinely mutates state — the last
    // update wins (even iterations flip true, odd false; i=99 is odd →
    // final streaming_enabled = false).
    for i in 0..100 {
        let enabled = i % 2 == 0;
        harness
            .cfg_mgr
            .update_with(move |c| {
                c.external_grpc.streaming_enabled = Some(enabled);
                Ok(())
            })
            .expect("rapid update");
    }

    // Replace fixed sleep with convergence poll — waits for the reload task
    // to drain up to the final update without relying on a fixed timeout.
    // i=99 is odd → final update set streaming_enabled = Some(false).
    harness
        .wait_for_streaming(
            false,
            Duration::from_secs(2),
            "reload task did not converge to final update",
        )
        .await;

    // Defensive re-read: guards against the reload task doing one more
    // update between the convergence break and this assertion.
    assert!(
        !harness.live.snapshot().streaming_enabled,
        "final snapshot must match the last update (streaming_enabled=false)"
    );
    assert!(
        !harness.reload_handle.is_finished(),
        "reload task must still be running after coalescing 100 rapid updates"
    );
    assert!(
        harness
            .metrics
            .config_reload_task_alive
            .load(std::sync::atomic::Ordering::Relaxed),
        "reload task liveness flag must still be set"
    );
    // Coalescing invariant: reload_total is bounded by 100 (≤ sends) and
    // must be ≥ 1 (at least one apply observed the final state).
    let total = harness
        .metrics
        .config_reload_total
        .load(std::sync::atomic::Ordering::Relaxed);
    assert!(
        (1..=100).contains(&total),
        "config_reload_total must be within [1, 100] after coalescing; got {total}"
    );

    harness.shutdown().await;
}

/// Live reload affects the next per-request decision after the reload
/// lands. Already-open streams snapshot `load_policy` at call entry
/// (spec D21) — this is intentional — so we verify the *next* RPC's
/// entry-point sees the new policy via `live.snapshot()`.
///
/// Opens a `SubscribeMetrics` stream (which stays alive using the
/// fixture's 1-msg-then-idle handler), mutates thresholds mid-stream,
/// then asserts the live snapshot reflects the new thresholds — which
/// is what a fresh RPC entry would observe per D21.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_reload_affects_long_running_stream() {
    // Past-warmup policy so the classify branch isn't forced-Medium (WARMUP=30s).
    let past_anchor = std::time::Instant::now() - std::time::Duration::from_secs(60);
    let baseline_thresholds = maekon_core::config::LoadThresholds {
        min_free_mem_gb: 1.0,
        cpu_low_pct: 30.0,
        cpu_medium_pct: 60.0,
        cpu_high_pct: 85.0,
    };
    let initial_policy = Arc::new(
        LoadPolicy::try_new_with_started_at(baseline_thresholds.clone(), past_anchor)
            .expect("valid initial thresholds"),
    );

    let seed_thresholds = baseline_thresholds.clone();
    let harness = LiveReloadHarness::builder()
        .seed(move |c| {
            c.external_grpc.streaming_enabled = Some(true);
            // Seed wide thresholds — "no shed" baseline.
            c.web.grpc_load_thresholds = Some(seed_thresholds);
        })
        .initial_load_policy(initial_policy.clone())
        .build()
        .await;

    // Open a SubscribeMetrics stream. The real handler in
    // `DashboardServiceImpl` stays alive and emits periodically; we don't
    // need to drain — we just need the stream call to have been made.
    let token = test_mint_jwt(
        &harness.jwt_kp.enc_key,
        "user-longstream",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(harness.port, &cert_pem, None).await;
    let mut req =
        tonic::Request::new(maekon_web::proto::dashboard::v1::SubscribeMetricsRequest::default());
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}").parse().expect("valid header"),
    );
    let stream_response = DashboardServiceClient::new(channel.clone())
        .subscribe_metrics(req)
        .await
        .expect("initial stream open");
    // Keep the inner stream alive for the rest of the test — dropping it
    // would release the server's per-stream guard; we want the stream to
    // be concurrent with the reload.
    let _keep_stream_alive = stream_response.into_inner();

    // Mid-stream: reload with tight "shed" thresholds. The existing stream
    // keeps its captured policy per D21; new per-request decisions observe
    // the new policy via `live.snapshot()`.
    let shed_thresholds = maekon_core::config::LoadThresholds {
        min_free_mem_gb: 100.0, // require ≥100 GB free — always Critical
        cpu_low_pct: 1.0,
        cpu_medium_pct: 2.0,
        cpu_high_pct: 3.0,
    };
    harness
        .cfg_mgr
        .update_with(|c| {
            c.web.grpc_load_thresholds = Some(shed_thresholds.clone());
            Ok(())
        })
        .expect("mid-stream reload");

    tokio::time::sleep(Duration::from_millis(150)).await;

    // A fresh `live.snapshot()` (what a new RPC entry would observe) must
    // reflect the tight shed thresholds — this is the "next per-request
    // decision" observability point per D21.
    let post_snap = harness.live.snapshot();
    let post_thresholds = post_snap.load_policy.thresholds();
    assert!(
        (post_thresholds.cpu_high_pct - 3.0).abs() < f32::EPSILON,
        "post-reload cpu_high_pct must reflect shed thresholds; got {}",
        post_thresholds.cpu_high_pct
    );
    assert!(
        (post_thresholds.min_free_mem_gb - 100.0).abs() < f32::EPSILON,
        "post-reload min_free_mem_gb must reflect shed thresholds; got {}",
        post_thresholds.min_free_mem_gb
    );
    // Classify a realistic-load metrics snapshot under the new policy —
    // it MUST come back Critical (cpu > 3 and free_mem_gb < 100).
    let mk_metrics =
        |cpu: f32, used_gib: u64, total_gib: u64| maekon_core::models::system::SystemMetrics {
            timestamp: chrono::Utc::now(),
            cpu_usage: cpu,
            memory_used: used_gib * 1_073_741_824,
            memory_total: total_gib * 1_073_741_824,
            disk_used: 0,
            disk_total: 0,
            network: None,
            typing_wpm: 0.0,
        };
    let shed_level = post_snap.load_policy.classify(&mk_metrics(50.0, 8, 16));
    assert_eq!(
        shed_level,
        maekon_web::grpc::LoadLevel::Critical,
        "under shed thresholds, moderate metrics must classify as Critical"
    );

    // D21: already-open streams keep their captured policy reference —
    // represented here by the `initial_policy` Arc that preceded the
    // reload. That Arc must be a DIFFERENT instance from the live
    // snapshot's current `load_policy` (the ConfigReloadTask built a
    // fresh Arc in `apply_config`). The initial policy's thresholds
    // must also still be the pre-reload values.
    assert!(
        !Arc::ptr_eq(&initial_policy, &post_snap.load_policy),
        "D21: post-reload live policy must be a distinct Arc from the pre-reload one"
    );
    let initial_thresholds = initial_policy.thresholds();
    assert!(
        (initial_thresholds.cpu_high_pct - 85.0).abs() < f32::EPSILON,
        "already-captured initial policy must still carry pre-reload cpu_high_pct=85.0; got {}",
        initial_thresholds.cpu_high_pct
    );

    // End-to-end: a 2nd RPC entering the server observes the new policy via
    // streaming_source.load_policy() at subscribe_metrics.rs:72-75 (D21
    // snapshot-at-call-entry). Opening a fresh stream proves the server-stack
    // propagation works — not just the ArcSwap substrate.
    let mut req2 =
        tonic::Request::new(maekon_web::proto::dashboard::v1::SubscribeMetricsRequest::default());
    req2.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}").parse().expect("auth header"),
    );
    let second_open = DashboardServiceClient::new(channel.clone())
        .subscribe_metrics(req2)
        .await;
    // Security-relevant: a policy reload (load thresholds change) must not close the gate —
    // shed affects tick cadence, not the auth/streaming-enabled decision. Pin the gRPC code
    // on failure so Unavailable vs PermissionDenied is distinguishable. (#5594)
    let second_stream = second_open.unwrap_or_else(|e| {
        panic!(
            "2nd RPC must still open post-reload; shed affects tick cadence not the gate; \
             got status {:?} (code={:?})",
            e,
            e.code()
        )
    });
    // Verify the snapshot substrate is stable across the 2nd entry (identity,
    // not rebuild) — proves the fresh Arc assembled during apply_config is
    // what the new RPC would observe.
    let post_2nd_snap = harness.live.snapshot();
    assert!(
        Arc::ptr_eq(&post_2nd_snap.load_policy, &post_snap.load_policy),
        "snapshot stable across 2nd RPC entry; live.snapshot() should return \
         the same Arc until the next reload"
    );
    // Drop the 2nd stream to release its per-stream guard.
    drop(second_stream.into_inner());

    drop(_keep_stream_alive);
    harness.shutdown().await;
}

/// Shutdown — the `ConfigReloadTask` must exit within 5 seconds of the
/// shutdown signal. The task's `tokio::select!` biases on `shutdown_rx`
/// (spec §5.4) so it will notice the flip even when a config update is
/// queued concurrently.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_config_reload_task_exits_on_shutdown() {
    let harness = LiveReloadHarness::builder().build().await;

    // Sanity: reload task alive-flag is set after startup.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        harness
            .metrics
            .config_reload_task_alive
            .load(std::sync::atomic::Ordering::Relaxed),
        "reload task must be alive before shutdown"
    );

    // Signal shutdown.
    harness.shutdown_tx.send_replace(true);

    // Destructure to take ownership of the handles for the graceful-exit
    // verification (await consumes JoinHandle, which can't be done through
    // `harness.shutdown()`'s abort path).
    let LiveReloadHarness {
        metrics,
        server_handle,
        reload_handle,
        ..
    } = harness;

    // The reload task MUST complete within 5s of the signal landing.
    let joined = tokio::time::timeout(Duration::from_secs(5), reload_handle)
        .await
        .expect("reload task must exit within 5s of shutdown signal");
    // Pin clean exit: the ConfigReloadTask must return Ok(()) — not panic — on shutdown.
    // A panic would produce Err(JoinError::panic), masking a live bug in the reload loop. (#5594)
    joined.expect("reload task must exit without panic on shutdown");
    assert!(
        !metrics
            .config_reload_task_alive
            .load(std::sync::atomic::Ordering::Relaxed),
        "reload task liveness flag must clear on exit"
    );

    // Server may still be draining in-flight work; abort to end the test.
    server_handle.abort();
    let _ = server_handle.await;
}
