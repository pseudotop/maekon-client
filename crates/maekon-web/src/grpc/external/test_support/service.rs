// ── InnerEcho — minimal tower::Service returning preset HTTP responses ────────

use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use http::HeaderMap;
use http_body::{Body, Frame};

/// `http_body::Body` impl used by [`InnerEcho`] so `AuditLayer::call` can
/// wrap the response body with `TrailerCapturingBody<RespBody>` (which
/// requires `RespBody: http_body::Body`).
///
/// Two shapes:
/// - **Body + trailer**: `data = Some(...)`, `trailers = Some(...)` — the
///   `poll_frame` returns one `Frame::data` then one `Frame::trailers`.
/// - **Trailers-only (empty body)**: `data = None`, `trailers = None` — the
///   `poll_frame` immediately returns `Ready(None)`. The grpc-status lives
///   in INITIAL headers per the tonic `Err(Status)` trailers-only convention.
///
/// Fields are `pub` so tests can construct bespoke bodies directly (e.g.
/// `SlowInner` in audit_layer tests that need a known delay before the
/// trailer frame is emitted).
pub struct EchoBody {
    pub data: Option<Bytes>,
    pub trailers: Option<HeaderMap>,
}

impl Body for EchoBody {
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

    fn is_end_stream(&self) -> bool {
        self.data.is_none() && self.trailers.is_none()
    }
}

/// Minimal `tower::Service` test double that echoes a preset HTTP response
/// with a `grpc-status` code encoded per one of two tonic conventions:
///
/// - [`InnerEcho::with_trailer_status`] — non-empty body + a **trailer frame**
///   carrying `grpc-status`. Simulates an Ok unary response or streaming
///   response terminating cleanly. `AuditLayer` observes the code via
///   `TrailerCapturingBody::poll_frame` when the body is polled.
///
/// - [`InnerEcho::trailers_only_with_status`] — empty body + `grpc-status`
///   in **initial response headers**, no body trailer frame. Simulates
///   `tonic::Status::from` (handler `Err(Status)`) which constructs a
///   trailers-only HTTP/2 response. `AuditLayer` observes the code via
///   `response.headers().get("grpc-status")` BEFORE wrapping the body
///   (spec §5.5 D28 header-first path).
#[derive(Clone)]
pub struct InnerEcho {
    grpc_status: i32,
    body_bytes: &'static [u8],
    /// When `true`, emit `grpc-status` in INITIAL headers + empty body (no
    /// trailer frame). When `false`, emit `grpc-status` as a body trailer.
    trailers_only: bool,
}

impl InnerEcho {
    /// Non-empty body + a `grpc-status` **trailer frame** (normal-trailers path).
    pub fn with_trailer_status(grpc_status: i32) -> Self {
        Self {
            grpc_status,
            body_bytes: b"body",
            trailers_only: false,
        }
    }

    /// Empty body + `grpc-status` in **initial headers** (tonic Err(Status) path).
    pub fn trailers_only_with_status(grpc_status: i32) -> Self {
        Self {
            grpc_status,
            body_bytes: b"",
            trailers_only: true,
        }
    }
}

impl<B> tower::Service<http::Request<B>> for InnerEcho
where
    B: Send + 'static,
{
    type Response = http::Response<EchoBody>;
    type Error = Infallible;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::from(Ok(()))
    }

    fn call(&mut self, _req: http::Request<B>) -> Self::Future {
        let grpc_status = self.grpc_status;
        let body_bytes = self.body_bytes;
        let trailers_only = self.trailers_only;
        Box::pin(async move {
            if trailers_only {
                // Tonic `Err(Status)` trailers-only convention: grpc-status
                // in INITIAL headers, empty body, no trailer frame.
                let body = EchoBody {
                    data: None,
                    trailers: None,
                };
                let resp = {
                    let mut response = http::Response::new(body);
                    response.headers_mut().insert(
                        "content-type",
                        http::HeaderValue::from_static("application/grpc"),
                    );
                    response
                        .headers_mut()
                        .insert("grpc-status", http::HeaderValue::from(grpc_status));
                    response
                };
                Ok(resp)
            } else {
                // Normal-trailers path: non-empty body frame + trailer frame
                // carrying grpc-status. No grpc-status header.
                let mut trailers = HeaderMap::new();
                trailers.insert("grpc-status", http::HeaderValue::from(grpc_status));
                let body = EchoBody {
                    data: Some(Bytes::copy_from_slice(body_bytes)),
                    trailers: Some(trailers),
                };
                let resp = {
                    let mut response = http::Response::new(body);
                    response.headers_mut().insert(
                        "content-type",
                        http::HeaderValue::from_static("application/grpc"),
                    );
                    response
                };
                Ok(resp)
            }
        })
    }
}

// ── PassthroughInner — transparent Service that records call counts ───────────

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Transparent `tower::Service` that delegates to the inner service and counts
/// how many times `call` was invoked. Used by Task 6.1 to verify that
/// the audit or auth layer does (or does not) forward requests to the handler.
#[derive(Clone)]
pub struct PassthroughInner {
    /// Number of times `call` has been invoked.
    pub call_count: Arc<AtomicUsize>,
}

impl PassthroughInner {
    /// Create a new `PassthroughInner` with the counter at zero.
    pub fn new() -> Self {
        Self {
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Snapshot the call count at this instant.
    pub fn count(&self) -> usize {
        self.call_count.load(Ordering::Relaxed)
    }
}

impl Default for PassthroughInner {
    fn default() -> Self {
        Self::new()
    }
}

impl<B> tower::Service<http::Request<B>> for PassthroughInner
where
    B: Send + 'static,
{
    type Response = http::Response<Vec<u8>>;
    type Error = Infallible;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::from(Ok(()))
    }

    fn call(&mut self, _req: http::Request<B>) -> Self::Future {
        self.call_count.fetch_add(1, Ordering::Relaxed);
        Box::pin(async move { Ok(http::Response::new(vec![])) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use tower::Service;
    use tower::ServiceExt;

    #[tokio::test]
    async fn status_headers_and_trailers_preserve_all_integer_values() {
        for code in [i32::MIN, -1, 0, 7, 16, i32::MAX] {
            let response = InnerEcho::trailers_only_with_status(code)
                .oneshot(http::Request::new(()))
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            assert_eq!(response.headers()["content-type"], "application/grpc");
            assert_eq!(response.headers()["grpc-status"], code.to_string());
            assert!(response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .is_empty());
            let response = InnerEcho::with_trailer_status(code)
                .oneshot(http::Request::new(()))
                .await
                .unwrap();
            assert!(!response.headers().contains_key("grpc-status"));
            assert_eq!(response.status(), 200);
            assert_eq!(response.headers()["content-type"], "application/grpc");
            let body = response.into_body().collect().await.unwrap();
            assert_eq!(body.trailers().unwrap()["grpc-status"], code.to_string());
            assert_eq!(body.to_bytes(), Bytes::from_static(b"body"));
        }
    }

    #[tokio::test]
    async fn body_frames_preserve_empty_data_and_exhaust_exactly_once() {
        let mut trailers = HeaderMap::new();
        trailers.insert("grpc-status", http::HeaderValue::from_static("0"));
        let mut body = EchoBody {
            data: Some(Bytes::new()),
            trailers: Some(trailers.clone()),
        };
        assert!(!body.is_end_stream());
        let data = body.frame().await.unwrap().unwrap();
        assert_eq!(data.into_data().unwrap(), Bytes::new());
        assert!(!body.is_end_stream());
        let last = body.frame().await.unwrap().unwrap();
        assert_eq!(last.into_trailers().unwrap(), trailers);
        assert!(body.is_end_stream());
        assert!(body.frame().await.is_none());
        assert!(body.frame().await.is_none());
    }

    #[tokio::test]
    async fn empty_and_trailer_only_bodies_keep_their_stream_boundaries() {
        let mut empty = EchoBody {
            data: None,
            trailers: None,
        };
        assert!(empty.is_end_stream());
        assert!(empty.frame().await.is_none());
        let mut trailer_only = EchoBody {
            data: None,
            trailers: Some(HeaderMap::new()),
        };
        assert!(!trailer_only.is_end_stream());
        assert_eq!(
            trailer_only
                .frame()
                .await
                .unwrap()
                .unwrap()
                .into_trailers()
                .unwrap(),
            HeaderMap::new()
        );
        assert!(trailer_only.is_end_stream());
        assert!(trailer_only.frame().await.is_none());
    }

    #[tokio::test]
    async fn passthrough_counts_calls_across_clones_and_cancelled_futures() {
        let mut service = PassthroughInner::default();
        assert_eq!(service.count(), 0);
        std::future::poll_fn(|cx| {
            <PassthroughInner as Service<http::Request<()>>>::poll_ready(&mut service, cx)
        })
        .await
        .unwrap();
        assert_eq!(service.count(), 0);
        let cancelled = service.call(http::Request::new(()));
        assert_eq!(service.count(), 1);
        drop(cancelled);
        assert_eq!(service.count(), 1);
        let mut calls = tokio::task::JoinSet::new();
        for _ in 0..16 {
            let inner = service.clone();
            calls.spawn(async move { inner.oneshot(http::Request::new(())).await.unwrap() });
        }
        while let Some(result) = calls.join_next().await {
            let response = result.unwrap();
            assert_eq!(response.status(), 200);
            assert!(response.headers().is_empty());
            assert!(response.into_body().is_empty());
        }
        assert_eq!(service.count(), 17);
        assert_eq!(service.clone().count(), 17);
        assert_eq!(PassthroughInner::new().count(), 0);
    }
}
