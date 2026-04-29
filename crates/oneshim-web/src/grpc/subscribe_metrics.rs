//! D13-v2b SubscribeMetrics handler — realtime (`interval_secs=0`) or
//! interval-aggregated `MetricBucket` stream. See spec §4.6.
//!
//! Restructured per iter-2 review CRIT-3/4/5:
//! - `StreamCounterGuard` moved into `async_stream!` closure → Drop runs on
//!   every exit path (abrupt disconnect, `yield Err → return`, JoinError).
//! - CAS-style cap with revert-on-over (no TOCTOU; spec §4.6 step 0b).
//! - Realtime rate-limit gate placed BEFORE `collect_metrics().await` to
//!   avoid busy-looping under opt-out + throttle.
//!
//! Per spec §4.6:
//! - Warm-up forces `Medium` classification (LoadPolicy::is_in_warmup).
//! - Hint `reason` prefixed `"warmup"` during first 30s (HintEmitter).
//! - First yield is always a `Hint` (HintEmitter state is None on first call).
//! - Interval mode uses `tokio::time::interval + MissedTickBehavior::Skip`
//!   (drift-free vs `sleep`).
//! - Transient DB errors increment `consecutive_db_failures`; N=5 emits a
//!   degraded `Hint` via `HintEmitter::force_emit_degraded`; N=10 closes the
//!   stream with `Status::internal`.
//! - SystemMonitor failure ends the stream (spec IMP-27 simplification).
//! - Authority validation (IPv6-bracket-aware) + kill-switch + cap fire
//!   BEFORE any auth / hint work.

use std::pin::Pin;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_stream::stream;
use oneshim_api_contracts::stream::RealtimeEvent;
use oneshim_core::ports::monitor::SystemMonitor;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::MissedTickBehavior;
use tokio_stream::Stream;
use tonic::{Request, Response, Status};
use tracing::warn;

use crate::proto::dashboard::v1::subscribe_metrics_response::Payload as MetricsPayload;
use crate::proto::dashboard::v1::{
    MetricBucket, SubscribeMetricsRequest, SubscribeMetricsResponse,
};
use crate::storage_port::WebStorage;

use super::auth_gate::{honor_opt_out, validate_authority};
use super::hint_emitter::HintEmitter;
#[cfg(not(feature = "grpc-dashboard-external"))]
use super::load_policy::LoadPolicy;
use super::load_policy::{LoadLevel, INTERVAL_CEILING, INTERVAL_FLOOR};
use super::stream_counter::StreamCounterGuard;
use super::to_proto_ts;

pub type SubscribeMetricsStream =
    Pin<Box<dyn Stream<Item = Result<SubscribeMetricsResponse, Status>> + Send>>;

#[allow(clippy::too_many_arguments)]
pub async fn subscribe_metrics(
    req: Request<SubscribeMetricsRequest>,
    storage: Arc<dyn WebStorage>,
    system_monitor: Arc<dyn SystemMonitor>,
    event_tx: tokio::sync::broadcast::Sender<RealtimeEvent>,
    integration_auth_token: Option<String>,
    #[cfg(feature = "grpc-dashboard-external")]
    streaming_source: crate::grpc::streaming_source::StreamingSource,
    #[cfg(not(feature = "grpc-dashboard-external"))] load_policy: Arc<LoadPolicy>,
    #[cfg(not(feature = "grpc-dashboard-external"))] streaming_enabled: bool,
    active_streams: Arc<AtomicUsize>,
    max_concurrent_streams: usize,
) -> Result<Response<SubscribeMetricsStream>, Status> {
    // D24 / Task 5.2: under external feature, derive the pair from an atomic
    // snapshot of streaming_source at call entry (spec D21).
    #[cfg(feature = "grpc-dashboard-external")]
    let (load_policy, streaming_enabled) = (
        streaming_source.load_policy(),
        streaming_source.streaming_enabled(),
    );
    // Step 0a: authority validation (IMP-V2-A) — reject DNS-rebound hostnames
    // when the client exposes `:authority` via the `"host"` metadata key.
    // Tonic 0.14 does not uniformly propagate `:authority` into request
    // metadata; when absent, the loopback-only `serve()` bind is our actual
    // protection against DNS rebinding. This layer adds belt-and-braces
    // rejection for the cases where the authority IS observable (browser
    // `fetch` with explicit `Host` header, integration proxies, etc.).
    if let Some(authority) = req.metadata().get("host").and_then(|v| v.to_str().ok()) {
        validate_authority(Some(authority))?;
    }

    // Step 0b: active-stream cap (CRIT-3/4/8) — CAS-style, revert-on-over,
    // BEFORE auth/streaming_enabled/hint work. Unauth floods fail cheaply here.
    let guard = StreamCounterGuard::try_acquire(active_streams, max_concurrent_streams)?;

    // Step 0c: runtime kill switch. Returns Status::unavailable (NOT
    // Unimplemented, per IMP-1 — clients can retry when the operator flips
    // the flag back on).
    if !streaming_enabled {
        return Err(Status::unavailable("streaming disabled"));
    }

    // Step 1: request parse + auth gate.
    let remote_addr = req.remote_addr();
    let auth_header = req
        .metadata()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    // D13 Task 13: extract the CountingStream counter that `AuditLayer` inserted
    // into request extensions. For callers not wrapped in AuditLayer (loopback,
    // unit tests), fall back to a throwaway counter so the wrap below has a
    // consistent return type.
    let msg_counter: std::sync::Arc<std::sync::atomic::AtomicU64> = req
        .extensions()
        .get::<std::sync::Arc<std::sync::atomic::AtomicU64>>()
        .cloned()
        .unwrap_or_else(|| std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)));
    let SubscribeMetricsRequest {
        interval_secs,
        respect_server_hints,
    } = req.into_inner();

    let enforcement_on = honor_opt_out(
        respect_server_hints,
        remote_addr,
        auth_header.as_deref(),
        integration_auth_token.as_deref(),
    );
    if !respect_server_hints && enforcement_on {
        // Downgraded opt-out (untrusted caller). Log WITHOUT echoing the
        // token value — `auth_header_present` is boolean. CRIT-9 explicit
        // field whitelist; no #[tracing::instrument] on this fn.
        let remote_is_loopback = remote_addr
            .map(|a| super::auth_gate::is_local_loopback(&a.ip()))
            .unwrap_or(false);
        warn!(
            remote_is_loopback,
            auth_header_present = auth_header.is_some(),
            "SubscribeMetrics opt-out rejected (untrusted connection)"
        );
    }

    // Per-stream state.
    let mut rx = event_tx.subscribe();
    let mut hint_emitter = HintEmitter::new();
    let mut last_emit: Option<Instant> = None;
    let mut consecutive_db_failures: u32 = 0;

    // MIN-B1: seed `effective_interval_cache` with the warm-up level (Medium)
    // before the loop so step A's skip-if-too-soon check has a defined value
    // on first iteration.
    let mut effective_interval_cache: Duration =
        load_policy.enforced_metrics_interval(LoadLevel::Medium, interval_secs);

    // MIN-B2: `tokio::time::Interval::period()` is not public on all versions;
    // track the period we last set in a sibling `Option<Duration>` and
    // compare against it to decide whether to recreate the ticker on level
    // transitions.
    let mut ticker: Option<tokio::time::Interval> = None;
    let mut ticker_period: Option<Duration> = None;
    if interval_secs > 0 {
        ticker = Some({
            let mut i = tokio::time::interval(effective_interval_cache);
            i.set_missed_tick_behavior(MissedTickBehavior::Skip);
            i
        });
        ticker_period = Some(effective_interval_cache);
    }

    let out = stream! {
        // CRIT-3: capture the counter guard into the generator closure so Drop
        // runs whenever the stream drops (abrupt disconnect, yield-Err-return,
        // join-panic return).
        let _counter_guard = guard;

        loop {
            // ── A. Wait-for-tick ──────────────────────────────────────────
            if interval_secs == 0 {
                // Realtime: block on event_tx::Metrics wake-up.
                match rx.recv().await {
                    Ok(RealtimeEvent::Metrics(_)) => { /* wake */ }
                    Ok(_) => continue, // non-metrics event — ignore
                    Err(RecvError::Lagged(_)) => continue, // metrics tick will refire
                    Err(RecvError::Closed) => return, // server shutdown
                }
                // Coalesce queued wake-ups within a 10ms quiet window.
                let quiet = Duration::from_millis(10);
                while tokio::time::timeout(quiet, rx.recv()).await.is_ok() { /* drain */ }
                // CRIT-5: rate-limit BEFORE expensive work (collect_metrics +
                // classify + DB). This is the tight-skip path for throttled
                // realtime under opt-out.
                if let Some(t) = last_emit {
                    if t.elapsed() < effective_interval_cache {
                        continue;
                    }
                }
            } else {
                ticker
                    .as_mut()
                    .expect("ticker initialized when interval_secs > 0")
                    .tick()
                    .await;
            }

            // ── B. Metrics + classify + maybe emit hint ───────────────────
            let metrics = match system_monitor.collect_metrics().await {
                Ok(m) => m,
                Err(e) => {
                    warn!(err.code = %e.code(), "SubscribeMetrics metrics snapshot failed");
                    yield Err(Status::internal("metrics snapshot failed"));
                    return;
                }
            };
            let level = load_policy.classify(&metrics);
            let is_warmup = load_policy.is_in_warmup();
            let cpu_pct = metrics.cpu_usage;
            let memory_pct = if metrics.memory_total > 0 {
                (metrics.memory_used as f32 / metrics.memory_total as f32) * 100.0
            } else {
                0.0
            };
            if let Some(h) =
                hint_emitter.maybe_emit(level, &load_policy, cpu_pct, memory_pct, is_warmup)
            {
                yield Ok(SubscribeMetricsResponse {
                    payload: Some(MetricsPayload::Hint(h)),
                });
            }

            // ── C. Refresh effective interval + ticker on level transition ─
            effective_interval_cache = if enforcement_on {
                load_policy.enforced_metrics_interval(level, interval_secs)
            } else {
                let r = if interval_secs == 0 {
                    INTERVAL_FLOOR
                } else {
                    Duration::from_secs(u64::from(interval_secs))
                };
                r.max(INTERVAL_FLOOR).min(INTERVAL_CEILING)
            };
            if let Some(t) = ticker.as_mut() {
                if ticker_period != Some(effective_interval_cache) {
                    let mut new_ticker = tokio::time::interval(effective_interval_cache);
                    new_ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
                    *t = new_ticker;
                    ticker_period = Some(effective_interval_cache);
                }
            }

            // ── D. Fetch bucket via spawn_blocking ────────────────────────
            //
            // IMP-5 / IMP-29 (1-tick smearing): `window_start` uses the
            // post-refresh `effective_interval_cache`, so the first bucket
            // after a level transition uses the NEW interval's window span.
            // Documented in spec §11; acceptable.
            let window_start = chrono::Utc::now()
                - chrono::Duration::from_std(effective_interval_cache)
                    .expect("effective_interval_cache bounded by INTERVAL_CEILING");
            let window_end = chrono::Utc::now();
            let storage_clone = storage.clone();
            let fetch = tokio::task::spawn_blocking(move || {
                storage_clone.aggregate_metrics_window(window_start, window_end)
            })
            .await;
            match fetch {
                Ok(Ok(b)) => {
                    consecutive_db_failures = 0;
                    yield Ok(SubscribeMetricsResponse {
                        payload: Some(MetricsPayload::Data(MetricBucket {
                            start: Some(to_proto_ts(b.start)),
                            cpu_avg_pct: b.cpu_avg_pct,
                            memory_avg_mb: b.memory_avg_mb,
                            // IMP-19: `SystemMetrics` has no keystroke/click
                            // counters today; record forwards whatever the
                            // aggregation layer supplies (currently 0 via v2a
                            // `grpc/mod.rs:258-259` parity). Non-zero source
                            // lands in a future task.
                            active_keystrokes: b.active_keystrokes,
                            active_mouse_clicks: b.active_mouse_clicks,
                        })),
                    });
                    last_emit = Some(Instant::now());
                }
                Ok(Err(e)) => {
                    consecutive_db_failures += 1;
                    warn!(
                        err.code = %e.code(),
                        consecutive = consecutive_db_failures,
                        "SubscribeMetrics aggregate_metrics_window failed"
                    );
                    // IMP-6 / IMP-B2: emit a degraded Hint through HintEmitter
                    // at N=5 so the heartbeat clock advances in lockstep.
                    if consecutive_db_failures == 5 {
                        let h = hint_emitter.force_emit_degraded(
                            level,
                            &load_policy,
                            cpu_pct,
                            memory_pct,
                            "db_error_degraded",
                        );
                        yield Ok(SubscribeMetricsResponse {
                            payload: Some(MetricsPayload::Hint(h)),
                        });
                    }
                    if consecutive_db_failures >= 10 {
                        yield Err(Status::internal("persistent storage errors"));
                        return;
                    }
                    // Otherwise skip this tick; the stream stays open, next
                    // iteration retries.
                    continue;
                }
                Err(join_err) => {
                    // spawn_blocking panicked or was cancelled — fatal for
                    // this stream. No task-id leak in the outward message
                    // (M7 advisory); server-side logs have full detail.
                    warn!(error = %join_err, "SubscribeMetrics spawn_blocking join failure");
                    yield Err(Status::internal("task join failure"));
                    return;
                }
            }
        }
    };

    // Wrap the outbound stream in CountingStream so AuditLayer records the
    // terminal `response_message_count` correctly. For non-Audit-wrapped
    // call paths (loopback / unit tests), `msg_counter` is a throwaway.
    let counted = super::counting_stream::CountingStream::new(Box::pin(out), msg_counter);
    Ok(Response::new(Box::pin(counted)))
}
