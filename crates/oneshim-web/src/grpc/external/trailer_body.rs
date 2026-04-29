//! `http_body::Body` wrapper observing the gRPC `grpc-status` trailer.
//!
//! Spec §5.3 / D28. Paired with `AuditLayer::call`'s **header-first**
//! observation: trailers-only responses (handler `Err(Status)`) emit
//! grpc-status in initial HEADERS and no trailer frame — header-first
//! path handles those; this wrapper handles the normal-trailer path
//! (Ok responses + streaming RPCs).

use std::pin::Pin;
use std::task::{Context, Poll};

use http::HeaderMap;
use http_body::{Body, Frame};
use pin_project_lite::pin_project;
use tokio::sync::oneshot;

pin_project! {
    #[allow(dead_code)] // Phase 1 scaffold; consumed in Phase 3 (AuditLayer header-first)
    pub(crate) struct TrailerCapturingBody<B> {
        #[pin]
        inner: B,
        signal: Option<oneshot::Sender<Option<tonic::Code>>>,
        captured: Option<tonic::Code>,
    }

    impl<B> PinnedDrop for TrailerCapturingBody<B> {
        fn drop(this: Pin<&mut Self>) {
            let this = this.project();
            if let Some(tx) = this.signal.take() {
                // Best-effort; receiver may have been dropped (deferred audit
                // task cancelled). Ignore send errors.
                let _ = tx.send(*this.captured);
            }
        }
    }
}

#[allow(dead_code)] // Phase 1 scaffold; consumed in Phase 3 (AuditLayer header-first)
impl<B> TrailerCapturingBody<B> {
    pub fn new(inner: B, signal: oneshot::Sender<Option<tonic::Code>>) -> Self {
        Self {
            inner,
            signal: Some(signal),
            captured: None,
        }
    }

    /// Construct a wrapper where status is already known from initial
    /// response headers (trailers-only fast path per D28). Signal NOT
    /// owned — caller already fired their oneshot.
    pub fn new_already_fired(inner: B, captured: Option<tonic::Code>) -> Self {
        Self {
            inner,
            signal: None,
            captured,
        }
    }
}

/// `Default` impl required by `AuthLayer::status_response<B: Default>` — the
/// outer auth layer constructs a trailers-only gRPC error response via
/// `Status::into_http::<B>()`, which calls `B::default()` to supply an
/// empty body. Because `AuthLayer` now sits outside `AuditLayer` and
/// sees `RespBody = TrailerCapturingBody<…>`, this impl is load-bearing.
impl<B: Default> Default for TrailerCapturingBody<B> {
    fn default() -> Self {
        Self {
            inner: B::default(),
            signal: None,
            captured: None,
        }
    }
}

impl<B: Body> Body for TrailerCapturingBody<B> {
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.project();
        let result = this.inner.poll_frame(cx);
        if let Poll::Ready(Some(Ok(frame))) = &result {
            if let Some(trailers) = frame.trailers_ref() {
                let code = parse_grpc_status(trailers);
                if this.captured.is_none() {
                    *this.captured = code;
                }
                // Fire immediately; don't wait for drop.
                if let Some(tx) = this.signal.take() {
                    let _ = tx.send(*this.captured);
                }
            }
        }
        result
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.size_hint()
    }
}

#[allow(dead_code)] // Phase 1 scaffold; consumed in Phase 3
pub(crate) fn parse_grpc_status(trailers: &HeaderMap) -> Option<tonic::Code> {
    trailers
        .get("grpc-status")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<i32>().ok())
        .map(tonic::Code::from_i32)
}

/// Decision D7 mapping: `None` (no trailer observed) → `Completed` (conservative).
#[allow(dead_code)] // Phase 1 scaffold; consumed in Phase 3
pub(crate) fn map_code_to_audit_status(
    code: Option<tonic::Code>,
) -> oneshim_core::models::audit::AuditStatus {
    use oneshim_core::models::audit::AuditStatus;
    use tonic::Code::*;
    match code {
        None | Some(Ok) => AuditStatus::Completed,
        Some(PermissionDenied) | Some(Unauthenticated) => AuditStatus::Denied,
        Some(Cancelled) | Some(DeadlineExceeded) => AuditStatus::Timeout,
        _ => AuditStatus::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use http::{HeaderMap, HeaderValue};
    use http_body_util::BodyExt;
    use oneshim_core::models::audit::AuditStatus;
    use tonic::Code;

    // Hand-crafted body that emits one data frame + one trailer frame.
    struct FixtureBody {
        data: Option<Bytes>,
        trailers: Option<HeaderMap>,
    }

    impl Body for FixtureBody {
        type Data = Bytes;
        type Error = std::io::Error;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            if let Some(d) = self.data.take() {
                return Poll::Ready(Some(Ok(Frame::data(d))));
            }
            if let Some(t) = self.trailers.take() {
                return Poll::Ready(Some(Ok(Frame::trailers(t))));
            }
            Poll::Ready(None)
        }
    }

    fn trailers_with_status(code: i32) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("grpc-status", HeaderValue::from(code));
        h
    }

    #[tokio::test]
    async fn captures_ok_trailer_fires_some_ok() {
        let (tx, rx) = oneshot::channel();
        let body = FixtureBody {
            data: Some(Bytes::from_static(b"x")),
            trailers: Some(trailers_with_status(0)),
        };
        let wrapped = TrailerCapturingBody::new(body, tx);
        let _ = wrapped.collect().await;
        let observed = rx.await.expect("signal fired").expect("code present");
        assert_eq!(observed, Code::Ok);
    }

    #[tokio::test]
    async fn captures_permission_denied() {
        let (tx, rx) = oneshot::channel();
        let body = FixtureBody {
            data: None,
            trailers: Some(trailers_with_status(7)),
        };
        let wrapped = TrailerCapturingBody::new(body, tx);
        let _ = wrapped.collect().await;
        assert_eq!(rx.await.unwrap().unwrap(), Code::PermissionDenied);
    }

    #[tokio::test]
    async fn captures_deadline_exceeded() {
        let (tx, rx) = oneshot::channel();
        let body = FixtureBody {
            data: None,
            trailers: Some(trailers_with_status(4)),
        };
        let wrapped = TrailerCapturingBody::new(body, tx);
        let _ = wrapped.collect().await;
        assert_eq!(rx.await.unwrap().unwrap(), Code::DeadlineExceeded);
    }

    #[tokio::test]
    async fn drop_without_trailer_sends_none() {
        let (tx, rx) = oneshot::channel();
        let body = FixtureBody {
            data: Some(Bytes::from_static(b"x")),
            trailers: None,
        };
        let wrapped = TrailerCapturingBody::new(body, tx);
        drop(wrapped);
        assert!(rx.await.unwrap().is_none());
    }

    #[tokio::test]
    async fn drop_mid_stream_sends_none() {
        let (tx, rx) = oneshot::channel();
        let body = FixtureBody {
            data: Some(Bytes::from_static(b"partial")),
            trailers: Some(trailers_with_status(0)),
        };
        let wrapped = TrailerCapturingBody::new(body, tx);
        drop(wrapped);
        assert!(rx.await.unwrap().is_none());
    }

    #[test]
    fn parse_grpc_status_ignores_non_numeric() {
        let mut h = HeaderMap::new();
        h.insert("grpc-status", HeaderValue::from_static("notanumber"));
        assert!(parse_grpc_status(&h).is_none());
    }

    #[test]
    fn parse_grpc_status_returns_none_when_absent() {
        let h = HeaderMap::new();
        assert!(parse_grpc_status(&h).is_none());
    }

    #[test]
    fn map_code_table_driven() {
        use Code::*;
        let cases = vec![
            (None, AuditStatus::Completed),
            (Some(Ok), AuditStatus::Completed),
            (Some(PermissionDenied), AuditStatus::Denied),
            (Some(Unauthenticated), AuditStatus::Denied),
            (Some(Cancelled), AuditStatus::Timeout),
            (Some(DeadlineExceeded), AuditStatus::Timeout),
            (Some(Internal), AuditStatus::Failed),
            (Some(Unknown), AuditStatus::Failed),
            (Some(InvalidArgument), AuditStatus::Failed),
            (Some(NotFound), AuditStatus::Failed),
            (Some(AlreadyExists), AuditStatus::Failed),
            (Some(ResourceExhausted), AuditStatus::Failed),
            (Some(FailedPrecondition), AuditStatus::Failed),
            (Some(Aborted), AuditStatus::Failed),
            (Some(OutOfRange), AuditStatus::Failed),
            (Some(Unimplemented), AuditStatus::Failed),
            (Some(Unavailable), AuditStatus::Failed),
            (Some(DataLoss), AuditStatus::Failed),
        ];
        for (code, expected) in cases {
            assert_eq!(map_code_to_audit_status(code), expected, "code = {code:?}");
        }
    }

    #[tokio::test]
    async fn new_already_fired_drop_is_safe() {
        let body = FixtureBody {
            data: None,
            trailers: None,
        };
        let wrapped = TrailerCapturingBody::new_already_fired(body, Some(Code::Ok));
        drop(wrapped);
    }

    #[tokio::test]
    async fn first_trailer_wins_on_multiple() {
        let (tx, rx) = oneshot::channel();
        let body = FixtureBody {
            data: None,
            trailers: Some(trailers_with_status(7)),
        };
        let wrapped = TrailerCapturingBody::new(body, tx);
        let _ = wrapped.collect().await;
        assert_eq!(rx.await.unwrap().unwrap(), Code::PermissionDenied);
    }
}
