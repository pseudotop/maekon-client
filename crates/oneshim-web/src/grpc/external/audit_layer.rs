//! `AuditLayer` tower middleware — records Started before handler call and
//! Completed/Denied/Timeout/Failed after the handler returns.
//!
//! Positioned INNER of AuthLayer (and OUTER of the tonic service) in the
//! `Server::builder()` chain. AuthLayer still records `Failed` on auth
//! rejection; AuditLayer owns the terminal-status record for auth-passed
//! requests.
//!
//! # Terminal status (spec §5.5 / D28 / CR1 fix)
//!
//! The terminal status is derived from the gRPC `grpc-status` code via
//! [`map_code_to_audit_status`]. Two observation paths are wired:
//!
//! 1. **Header-first** (trailers-only): tonic emits `grpc-status` in initial
//!    response headers for handler `Err(Status)` returns (empty body, no
//!    trailer frame). `AuditLayer::call` inspects `response.headers()` BEFORE
//!    wrapping the body and fires the oneshot synchronously.
//! 2. **Body-trailer** (normal-trailers / streaming): `TrailerCapturingBody`
//!    observes the trailer frame as the body is polled and fires the oneshot
//!    on first observation. If the body is dropped without emitting a
//!    trailer, `Drop` fires `None` (mapped to `Completed` per D7).
//!
//! A deferred `tokio::spawn`ed task awaits the oneshot, calls
//! `bridge.record_completion` with `grpc_status_code: Option<u32>` (D26), and
//! bumps `metrics.request_bump` with one of 4 status labels
//! (`ok`/`denied`/`timeout`/`failed`). The `metrics.deferred_audit_in_flight`
//! gauge brackets the spawn body for observability (D32).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use oneshim_core::models::audit::AuditStatus;
use tower::{Layer, Service};

use super::audit_bridge::AuditBridge;
use super::conn_info::{AuthContext, PeerInfo};
use super::metrics::ExternalMetrics;
use super::request_id_layer::RequestId;
use super::trailer_body::{map_code_to_audit_status, TrailerCapturingBody};

#[derive(Clone)]
pub(crate) struct AuditLayer {
    pub bridge: Arc<AuditBridge>,
    pub metrics: Arc<ExternalMetrics>,
}

impl<S: Clone> Layer<S> for AuditLayer {
    type Service = AuditService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        AuditService {
            inner,
            bridge: self.bridge.clone(),
            metrics: self.metrics.clone(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct AuditService<S> {
    inner: S,
    bridge: Arc<AuditBridge>,
    metrics: Arc<ExternalMetrics>,
}

impl<S, B, RespBody> Service<http::Request<B>> for AuditService<S>
where
    S: Service<
            http::Request<B>,
            Response = http::Response<RespBody>,
            Error = std::convert::Infallible,
        > + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
    B: Send + 'static,
    RespBody: http_body::Body + Send + 'static,
{
    type Response = http::Response<TrailerCapturingBody<RespBody>>;
    type Error = std::convert::Infallible;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: http::Request<B>) -> Self::Future {
        let mut inner = self.inner.clone();
        let bridge = self.bridge.clone();
        let metrics = self.metrics.clone();

        // RequestIdLayer is outermost (U5); if present, its ID overrides
        // ctx.command_id in audit entries so fail-path (AuthLayer Failed)
        // and success-path (AuditLayer Started/Completed) rows correlate.
        let request_id = req.extensions().get::<RequestId>().map(|r| r.0.clone());
        let auth_ctx: Option<AuthContext> = req.extensions().get::<AuthContext>().cloned();
        let peer: Option<PeerInfo> = req.extensions().get::<PeerInfo>().cloned();
        let operation = req.uri().path().to_string();

        // Insert counter for streaming handlers to populate via CountingStream.
        let msg_counter = Arc::new(AtomicU64::new(0));
        req.extensions_mut().insert(msg_counter.clone());

        Box::pin(async move {
            // Fallthrough: if either extension is missing, the request was not
            // auth-processed (direct handler invocation in tests, internal
            // plumbing) — skip audit entirely. We still return a wrapped body
            // for type-uniformity with the audited path.
            let Some(ctx) = auth_ctx else {
                let response = inner.call(req).await?;
                let (parts, body) = response.into_parts();
                let wrapped = TrailerCapturingBody::new_already_fired(body, None);
                return Ok(http::Response::from_parts(parts, wrapped));
            };
            let Some(peer) = peer else {
                let response = inner.call(req).await?;
                let (parts, body) = response.into_parts();
                let wrapped = TrailerCapturingBody::new_already_fired(body, None);
                return Ok(http::Response::from_parts(parts, wrapped));
            };
            let remote = peer.remote_addr.to_string();

            // Started — record synchronously before handler.
            let _ = bridge
                .record(
                    &ctx,
                    remote.clone(),
                    &operation,
                    "ok",
                    AuditStatus::Started,
                    std::time::Duration::ZERO,
                    None, // request_size
                    None, // response_size
                    None, // failure_reason
                    request_id.clone(),
                )
                .await;

            let start = Instant::now();
            let response = inner.call(req).await?;

            // ── Header-first grpc-status observation (D28) ──────────────────
            // Tonic constructs a "trailers-only" HTTP response for handler
            // Err(Status): grpc-status in initial HEADERS, empty body, no
            // trailer frame. Must be inspected BEFORE wrapping the body.
            let header_code = response
                .headers()
                .get("grpc-status")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<i32>().ok())
                .map(tonic::Code::from_i32);

            let (tx, rx) = tokio::sync::oneshot::channel::<Option<tonic::Code>>();
            let (parts, body) = response.into_parts();

            let wrapped = if let Some(code) = header_code {
                // Fire immediately; body will not emit a trailer for
                // trailers-only. Still wrap for type-uniformity.
                let _ = tx.send(Some(code));
                TrailerCapturingBody::new_already_fired(body, Some(code))
            } else {
                // Normal-trailers / streaming case: observe trailer via body wrap.
                TrailerCapturingBody::new(body, tx)
            };
            let response = http::Response::from_parts(parts, wrapped);

            // ── Deferred completion record (D32 gauge bracket) ──────────────
            metrics
                .deferred_audit_in_flight
                .fetch_add(1, Ordering::Relaxed);
            let metrics_for_task = metrics.clone();
            tokio::spawn(async move {
                let observed = rx.await.ok().flatten();
                let audit_status = map_code_to_audit_status(observed);
                // Capture label BEFORE the enum is moved into record_completion.
                let status_label = audit_status_label(&audit_status);
                // D26: persist raw tonic::Code as u32 so dashboards can
                // disambiguate e.g. PermissionDenied(7) vs Unauthenticated(16).
                let grpc_status_code: Option<u32> = observed.map(|c| c as i32 as u32);
                let duration = start.elapsed();
                let msg_count = msg_counter.load(Ordering::Relaxed);
                let msg_count_opt = (msg_count > 0).then_some(msg_count);

                let _ = bridge
                    .record_completion(
                        &ctx,
                        remote,
                        &operation,
                        audit_status,
                        duration,
                        msg_count_opt,
                        None,             // failure_reason
                        request_id,       // command_id override (U5)
                        grpc_status_code, // D26
                    )
                    .await;

                metrics_for_task.request_bump("external", ctx.auth_type.as_str(), status_label);
                metrics_for_task
                    .deferred_audit_in_flight
                    .fetch_sub(1, Ordering::Relaxed);
            });

            Ok(response)
        })
    }
}

/// Map `AuditStatus` to its stable metric-label string.
///
/// Used by `metrics.request_bump` so Prometheus counters are bucketed into
/// 4 meaningful status labels (not just `"ok"`). `Started` should never
/// reach here (AuditLayer's Started record is separate from completion
/// metric); kept exhaustive for pattern-match safety.
///
/// Borrows `AuditStatus` because the caller still needs to pass it by
/// value to `record_completion` afterward.
fn audit_status_label(s: &AuditStatus) -> &'static str {
    match s {
        AuditStatus::Completed => "ok",
        AuditStatus::Denied => "denied",
        AuditStatus::Timeout => "timeout",
        AuditStatus::Failed => "failed",
        AuditStatus::Started => "started",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grpc::external::audit_bridge::AuditBridge;
    use crate::grpc::external::conn_info::{AuthContext, AuthType, PeerInfo};
    use crate::grpc::external::request_id_layer::RequestId;
    use crate::grpc::external::test_support::{
        fixture_bridge, fixture_metrics, EchoBody, InnerEcho,
    };
    use async_trait::async_trait;
    use bytes::Bytes;
    use http::{HeaderMap, Request};
    use http_body::Frame;
    use http_body_util::BodyExt;
    use oneshim_core::models::ai_session::SessionAuditEntry;
    use oneshim_core::models::audit::{AuditEntry, AuditLevel, AuditStats};
    use oneshim_core::ports::audit_log::AuditLogPort;
    use std::convert::Infallible;
    use std::net::SocketAddr;
    use std::pin::Pin;
    use std::sync::Mutex;
    use tower::ServiceExt;
    use ulid::Ulid;

    // ── Body fixtures ─────────────────────────────────────────────────────
    // Local bodies tailored to each test's needs. `EchoBody` from
    // test_support covers the InnerEcho case; tests that use their own
    // inner service need their own body type.

    /// Empty `Body` used by CheckExt / SlowInner — no data, no trailers.
    struct EmptyBody;
    impl http_body::Body for EmptyBody {
        type Data = Bytes;
        type Error = std::io::Error;
        fn poll_frame(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            Poll::Ready(None)
        }
        fn is_end_stream(&self) -> bool {
            true
        }
    }

    // ── Mock inner service — returns a Response<EchoBody> ────────────────
    #[derive(Clone)]
    struct MockInner;

    impl Service<Request<Vec<u8>>> for MockInner {
        type Response = http::Response<EchoBody>;
        type Error = Infallible;
        type Future = std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
        >;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
        fn call(&mut self, _req: Request<Vec<u8>>) -> Self::Future {
            // Use InnerEcho's EchoBody layout with grpc-status=0 (Ok) trailer.
            let mut service = InnerEcho::with_trailer_status(0);
            Box::pin(async move {
                service
                    .call(Request::builder().body(Vec::<u8>::new()).unwrap())
                    .await
            })
        }
    }

    // ── Capturing audit port — records every log_complete_with_time call ───
    struct CapturingAudit {
        entries: Mutex<Vec<AuditEntry>>,
    }
    impl CapturingAudit {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                entries: Mutex::new(vec![]),
            })
        }
    }
    #[async_trait]
    impl AuditLogPort for CapturingAudit {
        async fn pending_count(&self) -> usize {
            0
        }
        async fn recent_entries(&self, _l: usize) -> Vec<AuditEntry> {
            vec![]
        }
        async fn entries_by_status(&self, _s: &AuditStatus, _l: usize) -> Vec<AuditEntry> {
            vec![]
        }
        async fn entries_by_action_prefix(&self, _p: &str, _l: usize) -> Vec<AuditEntry> {
            vec![]
        }
        async fn entries_by_command_id(&self, _cmd_id: &str, _limit: usize) -> Vec<AuditEntry> {
            vec![]
        }
        async fn stats(&self) -> AuditStats {
            AuditStats::default()
        }
        async fn has_pending_batch(&self) -> bool {
            false
        }
        async fn log_event(&self, _a: &str, _s: &str, _d: &str) {}
        async fn log_start_if(&self, _l: AuditLevel, _c: &str, _s: &str, _a: &str) {}
        async fn log_complete_with_time(
            &self,
            _level: AuditLevel,
            command_id: &str,
            session_id: &str,
            details: &str,
            execution_time_ms: u64,
        ) {
            // Infer status from the JSON `result` field so tests can branch on it.
            let status = serde_json::from_str::<serde_json::Value>(details)
                .ok()
                .and_then(|v| {
                    v.get("result").and_then(|r| r.as_str()).map(|r| match r {
                        "ok" => AuditStatus::Completed,
                        "denied" => AuditStatus::Denied,
                        "timeout" => AuditStatus::Timeout,
                        _ => AuditStatus::Failed,
                    })
                })
                .unwrap_or(AuditStatus::Completed);

            self.entries.lock().unwrap().push(AuditEntry {
                entry_id: Ulid::new().to_string(),
                timestamp: chrono::Utc::now(),
                action_type: "external_grpc".into(),
                command_id: command_id.into(),
                session_id: session_id.into(),
                status,
                details: Some(details.into()),
                execution_time_ms: Some(execution_time_ms),
            });
        }
        async fn drain_batch(&self) -> Vec<AuditEntry> {
            vec![]
        }
        async fn drain_all(&self) -> Vec<AuditEntry> {
            vec![]
        }
        async fn record_session_event(&self, _e: SessionAuditEntry) {}
    }

    fn mk_ctx() -> AuthContext {
        AuthContext {
            command_id: Ulid::new().to_string(),
            client_id: "client-1".into(),
            auth_type: AuthType::Jwt,
            jti: None,
        }
    }

    fn mk_peer() -> PeerInfo {
        PeerInfo {
            remote_addr: "127.0.0.1:5000".parse::<SocketAddr>().unwrap(),
            peer_cert_der: None,
            cert_subject_cn: None,
            tls_version: "TLSv1.3".into(),
        }
    }

    fn mk_layer(audit: Arc<CapturingAudit>) -> AuditLayer {
        AuditLayer {
            bridge: Arc::new(AuditBridge::new(audit as Arc<dyn AuditLogPort>)),
            metrics: Arc::new(ExternalMetrics::new()),
        }
    }

    /// Helper: poll the response body to completion so the oneshot trailer
    /// signal fires; then wait briefly for the deferred task to record.
    async fn drain_and_wait<B: http_body::Body + Unpin>(body: B) {
        let _ = body.collect().await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    #[tokio::test]
    async fn ok_response_records_started_then_completed() {
        let audit = CapturingAudit::new();
        let layer = mk_layer(audit.clone());
        let svc = layer.layer(MockInner);

        let mut req = Request::builder()
            .uri("/dashboard.v1.DashboardService/GetSessionStats")
            .body(vec![])
            .unwrap();
        req.extensions_mut().insert(mk_ctx());
        req.extensions_mut().insert(mk_peer());

        let resp = svc.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        // Drain body so the trailer frame fires the oneshot.
        drain_and_wait(resp.into_body()).await;

        let entries = audit.entries.lock().unwrap();
        assert_eq!(entries.len(), 2, "expected Started + Completed audit rows");
        let first: serde_json::Value =
            serde_json::from_str(entries[0].details.as_ref().unwrap()).unwrap();
        let second: serde_json::Value =
            serde_json::from_str(entries[1].details.as_ref().unwrap()).unwrap();
        // Started row uses result = "ok" and has no message_count.
        assert_eq!(first["result"], "ok");
        assert!(first.get("response_message_count").is_none());
        // Completed row mirrors result = "ok" with no count (non-streaming).
        assert_eq!(second["result"], "ok");
    }

    #[tokio::test]
    async fn missing_auth_context_skips_audit() {
        let audit = CapturingAudit::new();
        let layer = mk_layer(audit.clone());
        let svc = layer.layer(MockInner);

        // No AuthContext inserted — fallthrough path.
        let req = Request::builder().uri("/svc/op").body(vec![]).unwrap();
        let resp = svc.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        drain_and_wait(resp.into_body()).await;
        assert_eq!(audit.entries.lock().unwrap().len(), 0, "no auth → no audit");
    }

    #[tokio::test]
    async fn missing_peer_info_skips_audit() {
        let audit = CapturingAudit::new();
        let layer = mk_layer(audit.clone());
        let svc = layer.layer(MockInner);
        let mut req = Request::builder().uri("/svc/op").body(vec![]).unwrap();
        req.extensions_mut().insert(mk_ctx());
        // No PeerInfo extension.
        let resp = svc.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        drain_and_wait(resp.into_body()).await;
        assert_eq!(audit.entries.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn message_counter_extension_is_inserted_before_handler() {
        #[derive(Clone)]
        struct CheckExt;
        impl Service<Request<Vec<u8>>> for CheckExt {
            type Response = http::Response<EmptyBody>;
            type Error = Infallible;
            type Future = std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
            >;
            fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
                Poll::Ready(Ok(()))
            }
            fn call(&mut self, req: Request<Vec<u8>>) -> Self::Future {
                let has = req.extensions().get::<Arc<AtomicU64>>().is_some();
                Box::pin(async move {
                    // Use response status to signal presence (200 ok, 500 missing).
                    let status = if has { 200 } else { 500 };
                    Ok(http::Response::builder()
                        .status(status)
                        .body(EmptyBody)
                        .unwrap())
                })
            }
        }
        let audit = CapturingAudit::new();
        let layer = mk_layer(audit.clone());
        let svc = layer.layer(CheckExt);

        let mut req = Request::builder().uri("/svc/op").body(vec![]).unwrap();
        req.extensions_mut().insert(mk_ctx());
        req.extensions_mut().insert(mk_peer());
        let resp = svc.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200, "handler observed msg_counter extension");
    }

    #[tokio::test]
    async fn completed_entry_has_duration_ge_handler_elapsed() {
        #[derive(Clone)]
        struct SlowInner;
        impl Service<Request<Vec<u8>>> for SlowInner {
            type Response = http::Response<EchoBody>;
            type Error = Infallible;
            type Future = std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
            >;
            fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
                Poll::Ready(Ok(()))
            }
            fn call(&mut self, _req: Request<Vec<u8>>) -> Self::Future {
                Box::pin(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    // Use InnerEcho's body layout (data+trailer) so AuditLayer
                    // observes a grpc-status code via the body-trailer path.
                    let mut trailers = HeaderMap::new();
                    trailers.insert("grpc-status", http::HeaderValue::from(0));
                    let body = EchoBody {
                        data: Some(Bytes::from_static(b"x")),
                        trailers: Some(trailers),
                    };
                    Ok(http::Response::builder().status(200).body(body).unwrap())
                })
            }
        }

        let audit = CapturingAudit::new();
        let layer = mk_layer(audit.clone());
        let svc = layer.layer(SlowInner);

        let mut req = Request::builder().uri("/svc/op").body(vec![]).unwrap();
        req.extensions_mut().insert(mk_ctx());
        req.extensions_mut().insert(mk_peer());
        let resp = svc.oneshot(req).await.unwrap();
        drain_and_wait(resp.into_body()).await;

        let entries = audit.entries.lock().unwrap();
        assert_eq!(entries.len(), 2);
        // Completed entry is the second — execution_time_ms ≥ 5 ms.
        let completed_ms = entries[1].execution_time_ms.unwrap_or(0);
        assert!(
            completed_ms >= 5,
            "expected ≥5ms elapsed, got {completed_ms}"
        );
    }

    // ── Task 3.1 NEW tests (spec §5.5 + CR1 + D26) ────────────────────────

    /// Verifies the body-trailer code path: response body carries a
    /// `grpc-status` trailer frame, `TrailerCapturingBody` observes it on
    /// poll, deferred task records Completed with grpc_status_code=0 and
    /// command_id from RequestId.
    ///
    /// Note: `MockRecorder` infers `AuditStatus` from the JSON `result`
    /// field; both the Started and Completed rows use `result: "ok"`, so
    /// MockRecorder maps both to `AuditStatus::Completed`. We disambiguate
    /// by looking for the row that carries `grpc_status_code` (only the
    /// completion row does — Started omits the field).
    #[tokio::test]
    async fn deferred_task_records_completion_after_body_drop() {
        let (bridge, recorder) = fixture_bridge();
        let layer = AuditLayer {
            bridge: Arc::new(bridge),
            metrics: fixture_metrics(),
        };
        // InnerEcho::with_trailer_status(0) — non-empty body + grpc-status:0 trailer.
        let service = layer.layer(InnerEcho::with_trailer_status(0));

        let mut req = Request::builder()
            .uri("/Service/Method")
            .body(Vec::<u8>::new())
            .unwrap();
        req.extensions_mut().insert(AuthContext::fixture());
        req.extensions_mut().insert(PeerInfo::fixture());
        req.extensions_mut().insert(RequestId("req-abc".into()));

        let resp = service.oneshot(req).await.unwrap();
        // Poll body to completion so the trailer fires the oneshot.
        drain_and_wait(resp.into_body()).await;

        let entries = recorder.snapshot();
        assert_eq!(entries.len(), 2, "Started + Completed");
        assert!(
            entries.iter().all(|e| e.command_id == "req-abc"),
            "both entries must carry the request_id as command_id"
        );
        // The completion row is identifiable by the presence of
        // grpc_status_code (Started path calls record(...) which passes
        // grpc_status_code=None → skip_serializing_if omits the field).
        let (completion_idx, completion_details) = entries
            .iter()
            .enumerate()
            .find_map(|(i, e)| {
                let parsed: serde_json::Value =
                    serde_json::from_str(e.details.as_ref().unwrap()).ok()?;
                if parsed.get("grpc_status_code").is_some() {
                    Some((i, parsed))
                } else {
                    None
                }
            })
            .expect("completion row with grpc_status_code");
        assert_eq!(completion_idx, 1, "completion row must follow Started");
        assert_eq!(
            completion_details["grpc_status_code"], 0,
            "grpc_status_code=0 (Ok) must be persisted"
        );
    }

    /// Regression guard: verify that `test_support::EchoBody` wrapped by
    /// `TrailerCapturingBody` actually observes the trailer frame and fires
    /// the oneshot with `Some(Code::Ok)`. Keeps the AuditLayer + EchoBody
    /// contract in sync (this is the happy path that every other test
    /// depends on).
    #[tokio::test]
    async fn echo_body_trailer_observed_through_wrapper() {
        use crate::grpc::external::trailer_body::TrailerCapturingBody;

        let mut trailers = HeaderMap::new();
        trailers.insert("grpc-status", http::HeaderValue::from(0_i32));
        let body = EchoBody {
            data: Some(Bytes::from_static(b"x")),
            trailers: Some(trailers),
        };
        let (tx, rx) = tokio::sync::oneshot::channel::<Option<tonic::Code>>();
        let wrapped = TrailerCapturingBody::new(body, tx);
        let _ = wrapped.collect().await;
        let observed = rx.await.expect("signal fired");
        assert_eq!(observed, Some(tonic::Code::Ok));
    }

    /// Verifies the header-first code path (CR1 fix): response is
    /// trailers-only (empty body + `grpc-status: 7` in initial headers).
    /// `AuditLayer::call` must observe the code BEFORE wrapping the body,
    /// fire the oneshot synchronously, and record `Denied` (not the
    /// pre-fix `Completed` default).
    #[tokio::test]
    async fn header_first_records_denied_for_trailers_only_permission_denied() {
        let (bridge, recorder) = fixture_bridge();
        let layer = AuditLayer {
            bridge: Arc::new(bridge),
            metrics: fixture_metrics(),
        };
        // InnerEcho::trailers_only_with_status(7) — empty body, grpc-status=7 in headers.
        let service = layer.layer(InnerEcho::trailers_only_with_status(7));

        let mut req = Request::builder()
            .uri("/Service/Method")
            .body(Vec::<u8>::new())
            .unwrap();
        req.extensions_mut().insert(AuthContext::fixture());
        req.extensions_mut().insert(PeerInfo::fixture());
        req.extensions_mut().insert(RequestId("req-pd".into()));

        let resp = service.oneshot(req).await.unwrap();
        // Body is empty; drop directly. The oneshot was fired synchronously
        // inside call() for the trailers-only path.
        drop(resp);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let entries = recorder.snapshot();
        let denied = entries
            .iter()
            .find(|e| e.status == AuditStatus::Denied)
            .cloned();
        assert!(
            denied.is_some(),
            "handler Err(PermissionDenied) must audit as Denied, got entries: {entries:?}"
        );
        let details: serde_json::Value =
            serde_json::from_str(denied.unwrap().details.as_ref().unwrap()).unwrap();
        assert_eq!(
            details["grpc_status_code"], 7,
            "grpc_status_code=7 (PermissionDenied) must be persisted for disambiguation"
        );
        // command_id correlation: both Started + Denied rows carry "req-pd".
        assert!(
            entries.iter().all(|e| e.command_id == "req-pd"),
            "request_id correlation across Started + Denied: {entries:?}"
        );
    }

    /// Verifies the `metrics.deferred_audit_in_flight` gauge is bracketed
    /// by the deferred task: starts at 0, peaks at 1, returns to 0 after
    /// the task completes (D32 observability).
    #[tokio::test]
    async fn deferred_audit_in_flight_gauge_brackets_spawn() {
        let (bridge, _recorder) = fixture_bridge();
        let metrics = fixture_metrics();
        let layer = AuditLayer {
            bridge: Arc::new(bridge),
            metrics: metrics.clone(),
        };
        let service = layer.layer(InnerEcho::with_trailer_status(0));

        let initial = metrics.deferred_audit_in_flight.load(Ordering::Relaxed);
        assert_eq!(initial, 0, "gauge starts at 0");

        let mut req = Request::builder()
            .uri("/Service/Method")
            .body(Vec::<u8>::new())
            .unwrap();
        req.extensions_mut().insert(AuthContext::fixture());
        req.extensions_mut().insert(PeerInfo::fixture());
        req.extensions_mut().insert(RequestId("req-gauge".into()));

        let resp = service.oneshot(req).await.unwrap();
        drain_and_wait(resp.into_body()).await;

        // After drain_and_wait's 50ms sleep, the deferred task should be done.
        let after = metrics.deferred_audit_in_flight.load(Ordering::Relaxed);
        assert_eq!(after, 0, "gauge returns to 0 after deferred task completes");
    }

    /// Verifies `metrics.request_bump` receives the mapped status label
    /// (not hardcoded "ok"). Trailers-only PermissionDenied → "denied".
    #[tokio::test]
    async fn request_bump_uses_mapped_status_label_for_denied() {
        let (bridge, _recorder) = fixture_bridge();
        let metrics = fixture_metrics();
        let layer = AuditLayer {
            bridge: Arc::new(bridge),
            metrics: metrics.clone(),
        };
        let service = layer.layer(InnerEcho::trailers_only_with_status(7));

        let mut req = Request::builder()
            .uri("/Service/Method")
            .body(Vec::<u8>::new())
            .unwrap();
        req.extensions_mut().insert(AuthContext::fixture());
        req.extensions_mut().insert(PeerInfo::fixture());
        req.extensions_mut().insert(RequestId("req-metric".into()));

        let resp = service.oneshot(req).await.unwrap();
        drop(resp);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert_eq!(
            metrics.get_request_count("external|jwt|denied"),
            1,
            "PermissionDenied must bump the denied label (not ok)"
        );
        assert_eq!(
            metrics.get_request_count("external|jwt|ok"),
            0,
            "ok label must not be bumped for a denied response"
        );
    }

    #[test]
    fn audit_status_label_covers_all_variants() {
        assert_eq!(audit_status_label(&AuditStatus::Completed), "ok");
        assert_eq!(audit_status_label(&AuditStatus::Denied), "denied");
        assert_eq!(audit_status_label(&AuditStatus::Timeout), "timeout");
        assert_eq!(audit_status_label(&AuditStatus::Failed), "failed");
        assert_eq!(audit_status_label(&AuditStatus::Started), "started");
    }
}
