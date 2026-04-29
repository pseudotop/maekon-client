//! D13-v2b SubscribeEvents handler — Frame/Idle live subscription +
//! AiRuntimeStatus snapshot-on-subscribe. See spec §A-§E.
//!
//! Design invariants:
//! - Frame events: payload transform only; no DB round-trip (FrameUpdate
//!   carries all proto FrameEvent fields per §B).
//! - Idle events: payload transform only.
//! - AiRuntimeStatus: snapshot-on-subscribe, ONE emission per stream,
//!   sanitised via PiiSanitizer if configured (§A.sec).
//! - Rate limiter applies ONLY to Frame/Idle. AiRuntimeStatus snapshot
//!   bypasses both limiter and drop accumulator (spec §A + iter-2 I4).
//! - event_types empty = all three types. Unknown types silently ignored
//!   (forward-compat per proto comment L168).
//! - After AiRuntimeStatus snapshot, stream stays OPEN for other types.
//! - Drop reason "channel_lag" when broadcast::RecvError::Lagged fires.
//! - Pre-stream gates mirror subscribe_metrics: authority validation,
//!   StreamCounterGuard, streaming_enabled kill switch.

use std::pin::Pin;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use async_stream::stream;
use oneshim_api_contracts::stream::RealtimeEvent;
use oneshim_core::config::PiiFilterLevel;
use oneshim_core::ports::pii_sanitizer::PiiSanitizer;
use tokio::sync::broadcast::error::RecvError;
use tokio_stream::Stream;
use tonic::{Request, Response, Status};
use tracing::warn;

use crate::proto::dashboard::v1::dashboard_event::Payload as DashboardEventPayload;
use crate::proto::dashboard::v1::subscribe_events_response::Payload as EventsPayload;
use crate::proto::dashboard::v1::{
    AiRuntimeStatusEvent, DashboardEvent, FrameEvent, IdleEvent, SubscribeEventsRequest,
    SubscribeEventsResponse,
};

use super::auth_gate::{honor_opt_out, validate_authority};
use super::drop_accumulator::DropAccumulator;
use super::hint_emitter::HintEmitter;
#[cfg(not(feature = "grpc-dashboard-external"))]
use super::load_policy::LoadPolicy;
use super::rate_limiter::EventRateLimiter;
use super::stream_counter::StreamCounterGuard;
use super::to_proto_ts;

pub type SubscribeEventsStream =
    Pin<Box<dyn Stream<Item = Result<SubscribeEventsResponse, Status>> + Send>>;

const EVENT_FRAME: &str = "frame";
const EVENT_IDLE: &str = "idle";
const EVENT_AI_RUNTIME_STATUS: &str = "ai_runtime_status";

#[allow(clippy::too_many_arguments)]
pub async fn subscribe_events(
    req: Request<SubscribeEventsRequest>,
    system_monitor: Arc<dyn oneshim_core::ports::monitor::SystemMonitor>,
    event_tx: tokio::sync::broadcast::Sender<RealtimeEvent>,
    integration_auth_token: Option<String>,
    #[cfg(feature = "grpc-dashboard-external")]
    streaming_source: crate::grpc::streaming_source::StreamingSource,
    #[cfg(not(feature = "grpc-dashboard-external"))] load_policy: Arc<LoadPolicy>,
    #[cfg(not(feature = "grpc-dashboard-external"))] streaming_enabled: bool,
    active_streams: Arc<AtomicUsize>,
    max_concurrent_streams: usize,
    pii_sanitizer: Option<Arc<dyn PiiSanitizer>>,
    ai_runtime_status_snapshot: Option<oneshim_api_contracts::stream::AiRuntimeStatus>,
) -> Result<Response<SubscribeEventsStream>, Status> {
    // D24 / Task 5.2: under external feature, derive the pair from an atomic
    // snapshot of streaming_source at call entry (spec D21).
    #[cfg(feature = "grpc-dashboard-external")]
    let (load_policy, streaming_enabled) = (
        streaming_source.load_policy(),
        streaming_source.streaming_enabled(),
    );
    // Step 0a: authority validation (IMP-V2-A parity with subscribe_metrics)
    if let Some(authority) = req.metadata().get("host").and_then(|v| v.to_str().ok()) {
        validate_authority(Some(authority))?;
    }

    // Step 0b: active-stream cap (CRIT-3/4/8 parity)
    let guard = StreamCounterGuard::try_acquire(active_streams, max_concurrent_streams)?;

    // Step 0c: streaming kill switch
    if !streaming_enabled {
        return Err(Status::unavailable("streaming disabled"));
    }

    // Step 1: request parse + auth gate
    let remote_addr = req.remote_addr();
    let auth_header = req
        .metadata()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    // D13 Task 13: extract the CountingStream counter that `AuditLayer` inserted
    // into request extensions. For callers not wrapped in AuditLayer (loopback,
    // unit tests), fall back to a throwaway counter.
    let msg_counter: std::sync::Arc<std::sync::atomic::AtomicU64> = req
        .extensions()
        .get::<std::sync::Arc<std::sync::atomic::AtomicU64>>()
        .cloned()
        .unwrap_or_else(|| std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)));
    let SubscribeEventsRequest {
        event_types,
        respect_server_hints,
    } = req.into_inner();

    // Opt-out signal is computed for parity with subscribe_metrics but not
    // consumed here: SubscribeEvents applies per-type rate limiting
    // unconditionally, regardless of the client's respect_server_hints flag.
    // The call retains its side effect (warn log on downgrade scenarios) for
    // consistency across dashboard RPCs.
    let _enforcement_on = honor_opt_out(
        respect_server_hints,
        remote_addr,
        auth_header.as_deref(),
        integration_auth_token.as_deref(),
    );

    // Step 2: normalize filter. Empty = all three. Unknown types silently dropped.
    let want_frame = event_types.is_empty() || event_types.iter().any(|t| t == EVENT_FRAME);
    let want_idle = event_types.is_empty() || event_types.iter().any(|t| t == EVENT_IDLE);
    let want_ai =
        event_types.is_empty() || event_types.iter().any(|t| t == EVENT_AI_RUNTIME_STATUS);

    // Step 3: per-stream state
    let mut rx = event_tx.subscribe();
    let mut rate_limiter = EventRateLimiter::new();
    let mut rate_limit_drops = DropAccumulator::new("rate_limit");
    let mut channel_lag_drops = DropAccumulator::new("channel_lag");
    let mut hint_emitter = HintEmitter::new();

    // Step 4: build output stream
    let stream = stream! {
        let _counter_guard = guard;

        // Step 4a: AiRuntimeStatus SNAPSHOT — ONE emission per stream.
        if want_ai {
            let ai_event = build_ai_runtime_status_event(
                ai_runtime_status_snapshot.as_ref(),
                pii_sanitizer.as_deref(),
            );
            let dash_event = DashboardEvent {
                occurred_at: Some(to_proto_ts(chrono::Utc::now())),
                payload: Some(DashboardEventPayload::AiRuntimeStatus(ai_event)),
            };
            yield Ok(SubscribeEventsResponse {
                payload: Some(EventsPayload::Event(dash_event)),
            });
        }

        // Step 4b: main loop — tokio::select! with biased rx + 1s tick.
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                biased;
                rx_result = rx.recv() => {
                    // Exhaustive match — RealtimeEvent has 5 variants.
                    match rx_result {
                        Ok(RealtimeEvent::Frame(frame)) => {
                            if !want_frame { continue; }
                            if rate_limiter.try_admit(EVENT_FRAME) {
                                let ts = chrono::DateTime::parse_from_rfc3339(&frame.timestamp)
                                    .map(|dt| dt.with_timezone(&chrono::Utc))
                                    .unwrap_or_else(|_| chrono::Utc::now());
                                let dash = DashboardEvent {
                                    occurred_at: Some(to_proto_ts(ts)),
                                    payload: Some(DashboardEventPayload::Frame(FrameEvent {
                                        frame_id: frame.id,
                                        app_name: frame.app_name,
                                        window_title: frame.window_title,
                                        importance: frame.importance,
                                        trigger_type: frame.trigger_type,
                                    })),
                                };
                                yield Ok(SubscribeEventsResponse {
                                    payload: Some(EventsPayload::Event(dash)),
                                });
                            } else {
                                rate_limit_drops.record_drop(EVENT_FRAME);
                            }
                        }
                        Ok(RealtimeEvent::Idle(idle)) => {
                            if !want_idle { continue; }
                            if rate_limiter.try_admit(EVENT_IDLE) {
                                let dash = DashboardEvent {
                                    occurred_at: Some(to_proto_ts(chrono::Utc::now())),
                                    payload: Some(DashboardEventPayload::Idle(IdleEvent {
                                        is_idle: idle.is_idle,
                                        idle_secs: idle.idle_secs,
                                    })),
                                };
                                yield Ok(SubscribeEventsResponse {
                                    payload: Some(EventsPayload::Event(dash)),
                                });
                            } else {
                                rate_limit_drops.record_drop(EVENT_IDLE);
                            }
                        }
                        // Metrics: not exposed on SubscribeEvents. Skip silently.
                        Ok(RealtimeEvent::Metrics(_)) => continue,
                        // AiRuntimeStatus: snapshot-only — emitted once at Step 4a.
                        Ok(RealtimeEvent::AiRuntimeStatus(_)) => continue,
                        // Ping: transport-layer liveness. Not surfaced.
                        Ok(RealtimeEvent::Ping) => continue,
                        Err(RecvError::Lagged(n)) => {
                            // broadcast::RecvError::Lagged does not carry the dropped
                            // event's type; use "unknown" per OTel convention.
                            // Record `n` drops to reflect actual missed events (v2c
                            // upgrade from the PR-B3 MVP 1-per-Lagged undercounting).
                            for _ in 0..n {
                                channel_lag_drops.record_drop("unknown");
                            }
                            warn!(lagged_by = n, "subscribe_events broadcast lagged");
                            continue;
                        }
                        Err(RecvError::Closed) => return,
                    }
                }
                _ = tick.tick() => {
                    // Emit accumulated drop signals (throttled per-reason). Each
                    // accumulator keeps its own throttle cadence; a single tick can
                    // yield 0, 1, or 2 drop signals. Ordering is handler-local
                    // (rate_limit first, then channel_lag) — not a proto contract;
                    // clients must not depend on it.
                    //
                    // Note: `tokio::time::interval(1s)` fires its first tick at
                    // t=0 — both `maybe_emit()` calls will return None on that
                    // initial tick since no drops have accumulated yet. Benign.
                    if let Some(signal) = rate_limit_drops.maybe_emit() {
                        yield Ok(SubscribeEventsResponse {
                            payload: Some(EventsPayload::Dropped(signal)),
                        });
                    }
                    if let Some(signal) = channel_lag_drops.maybe_emit() {
                        yield Ok(SubscribeEventsResponse {
                            payload: Some(EventsPayload::Dropped(signal)),
                        });
                    }

                    // Periodic ServerLoadHint.
                    let metrics = match system_monitor.collect_metrics().await {
                        Ok(m) => m,
                        Err(e) => {
                            warn!(
                                err.code = %e.code(),
                                "subscribe_events collect_metrics failed, skipping hint tick"
                            );
                            continue;
                        }
                    };
                    let level = load_policy.classify(&metrics);
                    let is_warmup = load_policy.is_in_warmup();
                    let cpu_pct = metrics.cpu_usage;
                    let mem_pct = if metrics.memory_total > 0 {
                        (metrics.memory_used as f32 / metrics.memory_total as f32) * 100.0
                    } else {
                        0.0
                    };
                    if let Some(h) =
                        hint_emitter.maybe_emit(level, &load_policy, cpu_pct, mem_pct, is_warmup)
                    {
                        yield Ok(SubscribeEventsResponse {
                            payload: Some(EventsPayload::Hint(h)),
                        });
                    }
                }
            }
        }
    };

    // Wrap the outbound stream in CountingStream so AuditLayer records the
    // terminal `response_message_count` correctly. For non-Audit-wrapped
    // call paths, `msg_counter` is a throwaway.
    let counted = super::counting_stream::CountingStream::new(Box::pin(stream), msg_counter);
    Ok(Response::new(Box::pin(counted) as SubscribeEventsStream))
}

/// Build the proto AiRuntimeStatusEvent, applying PII sanitisation when configured.
/// Returns a sentinel (all "unknown" / "") when status is None.
fn build_ai_runtime_status_event(
    status: Option<&oneshim_api_contracts::stream::AiRuntimeStatus>,
    pii_sanitizer: Option<&dyn PiiSanitizer>,
) -> AiRuntimeStatusEvent {
    match status {
        None => AiRuntimeStatusEvent {
            ocr_source: "unknown".to_string(),
            llm_source: "unknown".to_string(),
            ocr_fallback_reason: String::new(),
            llm_fallback_reason: String::new(),
        },
        Some(s) => {
            let sanitize = |raw: &str| -> String {
                match pii_sanitizer {
                    Some(sanitizer) => sanitizer.sanitize_text(raw, PiiFilterLevel::Standard),
                    None => raw.to_string(),
                }
            };
            AiRuntimeStatusEvent {
                ocr_source: s.ocr_source.clone(),
                llm_source: s.llm_source.clone(),
                ocr_fallback_reason: s
                    .ocr_fallback_reason
                    .as_deref()
                    .map(sanitize)
                    .unwrap_or_default(),
                llm_fallback_reason: s
                    .llm_fallback_reason
                    .as_deref()
                    .map(sanitize)
                    .unwrap_or_default(),
            }
        }
    }
}
