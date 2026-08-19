// D22 streaming-enabled fallback-semantics integration tests — loopback
// isolation from the external live-reload channel, `external_grpc.streaming_enabled
// = None` fall-through to `web.grpc_streaming_enabled`, and explicit-override
// precedence. Split from `external_grpc_integration.rs` by scenario family
// (#7730).

use std::time::Duration;

use tonic::Code;

use maekon_web::grpc::external::test_support::{server_cert_pem, test_mint_jwt};
use maekon_web::proto::dashboard::v1::dashboard_service_client::DashboardServiceClient;

use crate::common::{make_tls_channel, LiveReloadHarness};

// ── Task 9.6 — Fallback semantics integration tests (D22) ───────────────────
//
// Spec §9.2 L1419-1422 mandates three integration tests pinning the
// `external_grpc.streaming_enabled` override semantics introduced by D22:
//   1. NG1: external toggle does NOT mutate the loopback `web.grpc_streaming_enabled`
//      field — they remain independent at the AppConfig level.
//   2. Fall-through: when `external_grpc.streaming_enabled = None`, the resolved
//      external streaming value comes from `web.grpc_streaming_enabled`.
//   3. Override-beats-parent (NV4): when `external_grpc.streaming_enabled = Some(_)`,
//      the override wins regardless of `web.grpc_streaming_enabled`.
//
// Tests use real `ConfigReloadTask` wiring (via `spawn_server_with_config_manager`)
// to exercise the resolution code path in `config_reload::apply_config` end-to-end.

/// Test 1 (D22 / NG1) — toggling `external_grpc.streaming_enabled = Some(false)`
/// disables external streaming WITHOUT mutating `web.grpc_streaming_enabled`.
///
/// The loopback gRPC server captures `web.grpc_streaming_enabled` at boot via
/// `StreamingSource::Fixed` (boot-time captured value, no live reload — see
/// `streaming_source.rs`). The external server uses `StreamingSource::Live`,
/// which reads from `LiveSnapshot.streaming_enabled` resolved by `apply_config`.
///
/// NG1 mandates that external live-reload must not mutate the shared
/// `web.grpc_streaming_enabled` field. We verify NG1 at the configuration
/// layer: after the toggle, `cfg_mgr.snapshot().web.grpc_streaming_enabled`
/// must remain unchanged. Since the loopback server reads its `streaming_enabled`
/// from this AppConfig field at boot AND has no live-reload pathway, an
/// unchanged AppConfig field is equivalent to "loopback streaming is unaffected".
/// We don't spawn a separate loopback server here — that would test the
/// `StreamingSource::Fixed` capture (covered in `streaming_source.rs` unit tests),
/// not the NG1 invariant on external live-reload.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_streaming_enabled_is_not_live_reloaded() {
    // Seed: loopback streaming enabled (web.grpc_streaming_enabled=true);
    // external override left at None so external falls through to the same
    // value (initial sanity = enabled on both sides).
    let harness = LiveReloadHarness::builder()
        .seed(|c| {
            c.web.grpc_streaming_enabled = true;
            c.external_grpc.streaming_enabled = None;
        })
        .build()
        .await;

    // Sanity: external streaming initially resolves to true.
    assert!(
        harness.live.snapshot().streaming_enabled,
        "sanity: live snapshot must start with streaming_enabled=true"
    );
    assert!(
        harness.cfg_mgr.snapshot().web.grpc_streaming_enabled,
        "sanity: web.grpc_streaming_enabled must start at true"
    );

    // Flip the EXTERNAL override; loopback config must remain untouched.
    harness
        .cfg_mgr
        .update_with(|c| {
            c.external_grpc.streaming_enabled = Some(false);
            Ok(())
        })
        .expect("update_with apply");

    // Wait for the ConfigReloadTask to converge (mirrors Task 9.4 cap).
    harness
        .wait_for_streaming(
            false,
            Duration::from_secs(1),
            "convergence timeout: external streaming did not flip to false",
        )
        .await;

    // Assert NG1: external is now disabled, but loopback config field is untouched.
    assert!(
        !harness.live.snapshot().streaming_enabled,
        "external override must disable external streaming"
    );
    assert!(
        harness.cfg_mgr.snapshot().web.grpc_streaming_enabled,
        "NG1 violation: external toggle must NOT mutate web.grpc_streaming_enabled \
         (loopback config must remain untouched)"
    );
    // And the override field is now Some(false), confirming the toggle landed.
    assert_eq!(
        harness.cfg_mgr.snapshot().external_grpc.streaming_enabled,
        Some(false),
        "external override must reflect the operator's flip"
    );

    harness.shutdown().await;
}

/// Test 2 (D22 fall-through) — when `external_grpc.streaming_enabled = None`,
/// the resolved external streaming value comes from `web.grpc_streaming_enabled`.
///
/// Seeds initial state with the override enabled, then mutates to the
/// fall-through scenario (`None` + `web=false`). After convergence, the
/// `LiveSnapshot.streaming_enabled` must reflect the shared `web` field, and
/// `subscribe_metrics` must return `Unavailable` to confirm the resolution
/// took effect end-to-end through the running server.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_streaming_falls_back_to_web_field_when_external_none() {
    // Seed initial config with override enabled so external is initially streaming.
    let harness = LiveReloadHarness::builder()
        .seed(|c| {
            c.web.grpc_streaming_enabled = true;
            c.external_grpc.streaming_enabled = Some(true);
        })
        .build()
        .await;

    // Sanity: seed landed — external override of Some(true) → streaming enabled.
    assert!(
        harness.live.snapshot().streaming_enabled,
        "sanity: seed → external streaming initially true (external=Some(true))"
    );

    // Mint a JWT + TLS channel for end-to-end verification via subscribe_metrics.
    let token = test_mint_jwt(
        &harness.jwt_kp.enc_key,
        "user-9-6-fallback",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(harness.port, &cert_pem, None).await;

    // Apply the fall-through scenario: external=None, web=false.
    // Since `apply_config` resolves `streaming_enabled = external.unwrap_or(web)`,
    // the resolved value should now be `false`.
    harness
        .cfg_mgr
        .update_with(|c| {
            c.external_grpc.streaming_enabled = None;
            c.web.grpc_streaming_enabled = false;
            Ok(())
        })
        .expect("update_with apply");

    // Wait for ConfigReloadTask to converge.
    harness
        .wait_for_streaming(
            false,
            Duration::from_secs(1),
            "fall-through timeout: live.streaming_enabled did not converge to false",
        )
        .await;

    // Assert resolution: external=None + web=false → resolved=false.
    assert!(
        !harness.live.snapshot().streaming_enabled,
        "fall-through: external=None + web=false must resolve to streaming_enabled=false"
    );

    // End-to-end check: subscribe_metrics must return Unavailable.
    let mut req =
        tonic::Request::new(maekon_web::proto::dashboard::v1::SubscribeMetricsRequest::default());
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}").parse().expect("valid header"),
    );
    let result = DashboardServiceClient::new(channel)
        .subscribe_metrics(req)
        .await;
    match result {
        Err(s) if s.code() == Code::Unavailable => {
            // Expected: streaming disabled via fall-through.
        }
        other => panic!(
            "expected Unavailable from subscribe_metrics under fall-through; got {:?}",
            other
        ),
    }

    harness.shutdown().await;
}

/// Test 3 (D22 / NV4 — override-beats-parent) — when
/// `external_grpc.streaming_enabled = Some(true)` and
/// `web.grpc_streaming_enabled = false`, the override wins and the external
/// server keeps streaming.
///
/// Seeds initial state with both fields false (external matches), then
/// mutates external to `Some(true)` while leaving web at false. After
/// convergence, `LiveSnapshot.streaming_enabled` must be `true` and a real
/// `subscribe_metrics` RPC must succeed (not return Unavailable).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_streaming_override_wins_over_web_field_when_some() {
    // Seed: web=false and external=Some(false) — initial resolved=false.
    let harness = LiveReloadHarness::builder()
        .seed(|c| {
            c.web.grpc_streaming_enabled = false;
            c.external_grpc.streaming_enabled = Some(false);
        })
        .initial_streaming(false)
        .build()
        .await;

    // Sanity: seed landed — both flags false, resolved streaming disabled.
    assert!(
        !harness.live.snapshot().streaming_enabled,
        "sanity: seed → external streaming initially false (external=Some(false), web=false)"
    );

    let token = test_mint_jwt(
        &harness.jwt_kp.enc_key,
        "user-9-6-override",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(harness.port, &cert_pem, None).await;

    // Apply the override-beats-parent scenario: external=Some(true), web=false.
    // `apply_config` resolves `streaming_enabled = external.unwrap_or(web) = true`
    // even though web stays false — proving NV4.
    harness
        .cfg_mgr
        .update_with(|c| {
            c.external_grpc.streaming_enabled = Some(true);
            // web.grpc_streaming_enabled deliberately stays false.
            Ok(())
        })
        .expect("update_with apply");

    // Wait for ConfigReloadTask to converge.
    harness
        .wait_for_streaming(
            true,
            Duration::from_secs(1),
            "override-beats-parent timeout: live.streaming_enabled did not converge to true \
             (web=false but external=Some(true) should win)",
        )
        .await;

    // Assert NV4: even though web=false, the Some(true) override wins.
    assert!(
        harness.live.snapshot().streaming_enabled,
        "NV4 violation: external=Some(true) must override web=false"
    );
    assert!(
        !harness.cfg_mgr.snapshot().web.grpc_streaming_enabled,
        "sanity: web.grpc_streaming_enabled must remain false (override is the only enabler)"
    );

    // End-to-end check: subscribe_metrics must succeed (auth returns the stream).
    let mut req =
        tonic::Request::new(maekon_web::proto::dashboard::v1::SubscribeMetricsRequest::default());
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}").parse().expect("valid header"),
    );
    let result = DashboardServiceClient::new(channel)
        .subscribe_metrics(req)
        .await;
    // Security-relevant (NV4): when external_grpc.streaming_enabled=Some(true) overrides
    // web.grpc_streaming_enabled=false, the stream gate MUST open. An Unavailable response
    // here would indicate the override did not propagate through the ArcSwap substrate,
    // which is a correctness and security regression. Pin the gRPC code on failure. (#5594)
    let nv4_stream = result.unwrap_or_else(|e| {
        panic!(
            "subscribe_metrics must succeed when Some(true) override beats web=false (NV4); \
             got status {:?} (code={:?})",
            e,
            e.code()
        )
    });
    drop(nv4_stream);

    harness.shutdown().await;
}
