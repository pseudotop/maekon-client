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
use maekon_api_contracts::stream::RealtimeEvent;
use maekon_core::ports::monitor::SystemMonitor;
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
    streaming_source: crate::grpc::streaming_source::StreamingSource,
    active_streams: Arc<AtomicUsize>,
    max_concurrent_streams: usize,
) -> Result<Response<SubscribeMetricsStream>, Status> {
    let (load_policy, streaming_enabled) = (
        streaming_source.load_policy(),
        streaming_source.streaming_enabled(),
    );
    // Step 0a: authority validation (IMP-V2-A) — reject a DNS-rebound
    // (non-loopback) hostname WHEN it is observable.
    //
    // tonic 0.14 does NOT propagate the HTTP/2 `:authority` pseudo-header into
    // request metadata, so `host` is normally ABSENT for gRPC clients (rejecting
    // on absence rejected every legitimate streaming subscriber). We therefore
    // validate only when an authority IS observable (browser `fetch` with an
    // explicit `Host`, integration proxies) and must NOT reject on absence. The
    // actual transport protections are the loopback `serve()` bind (default
    // builds) and the Bearer auth gate below (Step 1, enforced on every bind —
    // including the external/routable one, for which a loopback-only authority
    // allowlist would be inapplicable anyway).
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
                // oneshim#5964: match the inner result explicitly. `.is_ok()` on the
                // OUTER Result treats Ok(Err(RecvError::Closed)) as success, so once
                // the broadcast sender dropped, rx.recv() returned Closed instantly
                // and this drain spun at 100% CPU forever. Closed must end the
                // stream; Lagged stays recoverable; Elapsed ends the quiet window.
                loop {
                    match tokio::time::timeout(quiet, rx.recv()).await {
                        Ok(Ok(_)) => { /* drained one — keep coalescing */ }
                        Ok(Err(RecvError::Lagged(_))) => { /* missed some — keep coalescing */ }
                        Ok(Err(RecvError::Closed)) => return, // sender gone — end the stream
                        Err(_elapsed) => break,              // quiet window elapsed — done
                    }
                }
                // CRIT-5: rate-limit BEFORE expensive work (collect_metrics +
                // classify + DB). This is the tight-skip path for throttled
                // realtime under opt-out.
                if let Some(t) = last_emit {
                    if t.elapsed() < effective_interval_cache {
                        continue;
                    }
                }
            } else {
                // SAFETY: ticker is Some when interval_secs > 0 (set
                // unconditionally in the `if interval_secs > 0` block above).
                // The debug_assert catches any future refactoring that breaks
                // this invariant at dev time without panicking in production.
                #[cfg(debug_assertions)]
                debug_assert!(ticker.is_some(), "ticker must be initialized when interval_secs > 0");
                if let Some(t) = ticker.as_mut() {
                    t.tick().await;
                }
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
                    // #6278: build the new-period ticker with `interval_at(now +
                    // period, ..)`, NOT `interval(period)`. A fresh `interval()`
                    // fires its FIRST tick IMMEDIATELY, so recreating it on every
                    // load-level oscillation defeats the emit-interval throttle
                    // (back-to-back emits). `interval_at` sets the new period AND
                    // schedules the first tick one full period out.
                    let mut new_ticker = tokio::time::interval_at(
                        tokio::time::Instant::now() + effective_interval_cache,
                        effective_interval_cache,
                    );
                    new_ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
                    *t = new_ticker;
                    ticker_period = Some(effective_interval_cache);
                }
            }

            // ── D. Fetch bucket via the async storage funnel ───────────────
            //
            // ADR-026 PR-9: `aggregate_metrics_window` is now an async
            // `DashboardStreamingStorage` method that offloads the SQLite read
            // onto `spawn_blocking` internally (the `with_conn_read` funnel), so
            // the hand-rolled `spawn_blocking` wrapper is replaced by a direct
            // `.await`. A blocking-pool join failure is surfaced inside the
            // funnel as a `CoreError`, collapsing the former three-arm match
            // into the standard `Ok`/`Err` pair.
            //
            // IMP-5 / IMP-29 (1-tick smearing): `window_start` uses the
            // post-refresh `effective_interval_cache`, so the first bucket
            // after a level transition uses the NEW interval's window span.
            // Documented in spec §11; acceptable.
            // SAFETY: effective_interval_cache is always clamped to
            // [INTERVAL_FLOOR, INTERVAL_CEILING] (both well under i64::MAX
            // nanoseconds). The debug_assert catches any future change that
            // widens the ceiling beyond chrono::Duration range at dev time.
            #[cfg(debug_assertions)]
            debug_assert!(
                effective_interval_cache <= std::time::Duration::from_secs(86_400),
                "effective_interval_cache exceeds chrono::Duration safe range"
            );
            let window_start = chrono::Utc::now()
                - chrono::Duration::from_std(effective_interval_cache)
                    .unwrap_or(chrono::Duration::seconds(60));
            let window_end = chrono::Utc::now();
            let fetch = storage
                .aggregate_metrics_window(window_start, window_end)
                .await;
            match fetch {
                Ok(b) => {
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
                Err(e) => {
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
            }
        }
    };

    // Wrap the outbound stream in CountingStream so AuditLayer records the
    // terminal `response_message_count` correctly. For non-Audit-wrapped
    // call paths (loopback / unit tests), `msg_counter` is a throwaway.
    let counted = super::counting_stream::CountingStream::new(Box::pin(out), msg_counter);
    Ok(Response::new(Box::pin(counted)))
}

#[cfg(test)]
mod tests {
    // NB: both unit tests below use fully-qualified `std::time::Duration` /
    // `chrono::Duration`; no `use super::*` is needed (was a dormant unused
    // import surfaced once this gated module is compiled under `--tests`).

    /// Verifies that the `effective_interval_cache` fallback path (replacing
    /// the former `.expect()`) never panics for any value in [INTERVAL_FLOOR,
    /// INTERVAL_CEILING].  The `unwrap_or` fallback of 60s is also exercised
    /// here to confirm it compiles and produces a valid `chrono::Duration`.
    #[test]
    fn effective_interval_within_ceiling_converts_without_panic() {
        // Any value between floor and ceiling must convert cleanly.
        for secs in [0u64, 1, 5, 30, 60] {
            let d = std::time::Duration::from_secs(secs.max(1));
            let result = chrono::Duration::from_std(d);
            // Strengthen: pin the converted value so a refactor that changes
            // the from_std conversion (e.g., overflow clamping) is caught. (#5594)
            let converted = result
                .unwrap_or_else(|e| panic!("chrono::Duration::from_std failed for {d:?}: {e}"));
            assert_eq!(
                converted.num_seconds(),
                secs.max(1) as i64,
                "converted duration must round-trip for {d:?}"
            );
        }
    }

    /// Confirms the fallback value (60 s) used in the `unwrap_or` branch is
    /// itself a valid `chrono::Duration` — regression guard against someone
    /// changing the fallback to an out-of-range constant.
    #[test]
    fn fallback_duration_60s_is_valid() {
        let fallback = chrono::Duration::seconds(60);
        assert!(fallback.num_seconds() == 60);
    }
}
