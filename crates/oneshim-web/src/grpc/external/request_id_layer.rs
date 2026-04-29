//! tower Layer — x-request-id ingress validation / generation + egress injection.
//!
//! Spec §5.2. Outermost layer in external gRPC stack (D14 revised / U5):
//! runs BEFORE AuthLayer so auth-rejected audit rows still carry the
//! client's correlation ID.
//!
//! Validation rule: ASCII graphic 0x21..=0x7E, length 1..=128. Invalid
//! values trigger UUIDv4 generation (never reject the request — the header
//! is informational).

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use http::HeaderValue;
use tower::{Layer, Service};
use uuid::Uuid;

pub(crate) const REQUEST_ID_HEADER: &str = "x-request-id";

/// Wrapper type for request-ID extension — gives strong static typing at read sites.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Phase 1 scaffold; consumed in Phase 6+ (AuthLayer Failed-path) and Phase 3 (AuditLayer header-first)
pub(crate) struct RequestId(pub String);

/// Tower Layer placing `RequestIdService` around the inner service.
#[derive(Clone, Default)]
#[allow(dead_code)] // Phase 1 scaffold; consumed in Phase 8 (serve_external layer order wiring)
pub(crate) struct RequestIdLayer;

impl<S: Clone> Layer<S> for RequestIdLayer {
    type Service = RequestIdService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        RequestIdService { inner }
    }
}

#[derive(Clone)]
#[allow(dead_code)] // Phase 1 scaffold; consumed via RequestIdLayer application
pub(crate) struct RequestIdService<S> {
    inner: S,
}

impl<S, B, RespBody> Service<http::Request<B>> for RequestIdService<S>
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
    RespBody: Send + 'static,
{
    type Response = http::Response<RespBody>;
    type Error = std::convert::Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: http::Request<B>) -> Self::Future {
        let incoming = req
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|h| h.to_str().ok());
        let request_id = match incoming {
            Some(raw) if is_valid(raw) => raw.to_string(),
            Some(raw) => {
                tracing::warn!(
                    incoming = %raw.chars().take(32).collect::<String>(),
                    reason = "validation_failed",
                    "external_grpc: invalid x-request-id, generating new UUID"
                );
                Uuid::new_v4().to_string()
            }
            None => Uuid::new_v4().to_string(),
        };
        req.extensions_mut().insert(RequestId(request_id.clone()));

        let mut inner = self.inner.clone();
        Box::pin(async move {
            let mut response = inner.call(req).await?;
            // D31 conditional overwrite: respect handler-set matching value,
            // insert ours otherwise.
            let should_insert = match response.headers().get(REQUEST_ID_HEADER) {
                Some(existing) => existing.to_str().map(|s| s != request_id).unwrap_or(true),
                None => true,
            };
            if should_insert {
                if let Ok(hv) = HeaderValue::from_str(&request_id) {
                    response.headers_mut().insert(REQUEST_ID_HEADER, hv);
                }
            }
            Ok(response)
        })
    }
}

/// Validation: ASCII graphic bytes only, length 1..=128.
///
/// Safely UUIDv4-compatible by construction (UUIDv4 is 36 chars of [0-9a-f-]).
/// Rejects whitespace (0x20, \t, \n, \r), control chars, and non-ASCII.
fn is_valid(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.bytes().all(|b| (0x21..=0x7E).contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::{Request, Response};
    use std::convert::Infallible;
    use tower::ServiceExt;

    // ── Test-local inner service: echoes any Response with empty body ──
    #[derive(Clone)]
    struct EchoService {
        preset_response_header: Option<(String, String)>,
    }
    impl Service<Request<Vec<u8>>> for EchoService {
        type Response = Response<Vec<u8>>;
        type Error = Infallible;
        type Future = std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Response<Vec<u8>>, Infallible>> + Send>,
        >;
        fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
            Poll::Ready(Ok(()))
        }
        fn call(&mut self, _req: Request<Vec<u8>>) -> Self::Future {
            let preset = self.preset_response_header.clone();
            Box::pin(async move {
                let mut r = Response::builder()
                    .status(200)
                    .body(Vec::<u8>::new())
                    .unwrap();
                if let Some((k, v)) = preset {
                    r.headers_mut().insert(
                        http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                        HeaderValue::from_str(&v).unwrap(),
                    );
                }
                Ok(r)
            })
        }
    }

    #[tokio::test]
    async fn accepts_valid_incoming_header() {
        let svc = RequestIdLayer.layer(EchoService {
            preset_response_header: None,
        });
        let req = Request::builder()
            .header(REQUEST_ID_HEADER, "test-req-123")
            .body(Vec::<u8>::new())
            .unwrap();
        let resp = svc.oneshot(req).await.unwrap();
        assert_eq!(
            resp.headers().get(REQUEST_ID_HEADER).unwrap(),
            "test-req-123"
        );
    }

    #[tokio::test]
    async fn generates_uuid_when_missing() {
        let svc = RequestIdLayer.layer(EchoService {
            preset_response_header: None,
        });
        let req = Request::builder().body(Vec::<u8>::new()).unwrap();
        let resp = svc.oneshot(req).await.unwrap();
        let id = resp
            .headers()
            .get(REQUEST_ID_HEADER)
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(id.len(), 36, "UUIDv4 text is 36 chars");
        assert_eq!(
            id.chars().filter(|c| *c == '-').count(),
            4,
            "UUIDv4 has 4 hyphens"
        );
        Uuid::parse_str(id).expect("valid UUID");
    }

    #[tokio::test]
    async fn rejects_invalid_characters_generates_new() {
        // Use a tab byte (0x09) — http HeaderValue accepts it as a valid
        // header byte (HTAB), but our is_valid range 0x21..=0x7E excludes it.
        // to_str() succeeds for tab (valid ASCII), so the warn+UUID path runs.
        let svc = RequestIdLayer.layer(EchoService {
            preset_response_header: None,
        });
        let req = Request::builder()
            .header(REQUEST_ID_HEADER, "bad\tchar") // 0x09 tab fails is_valid
            .body(Vec::<u8>::new())
            .unwrap();
        let resp = svc.oneshot(req).await.unwrap();
        let id = resp
            .headers()
            .get(REQUEST_ID_HEADER)
            .unwrap()
            .to_str()
            .unwrap();
        assert_ne!(id, "bad\tchar");
        assert_eq!(id.len(), 36, "fell back to UUID");
    }

    #[tokio::test]
    async fn rejects_too_long() {
        let svc = RequestIdLayer.layer(EchoService {
            preset_response_header: None,
        });
        let long = "a".repeat(200);
        let req = Request::builder()
            .header(REQUEST_ID_HEADER, &long)
            .body(Vec::<u8>::new())
            .unwrap();
        let resp = svc.oneshot(req).await.unwrap();
        let id = resp
            .headers()
            .get(REQUEST_ID_HEADER)
            .unwrap()
            .to_str()
            .unwrap();
        assert_ne!(id, long);
    }

    #[tokio::test]
    async fn rejects_empty() {
        let svc = RequestIdLayer.layer(EchoService {
            preset_response_header: None,
        });
        let req = Request::builder()
            .header(REQUEST_ID_HEADER, "")
            .body(Vec::<u8>::new())
            .unwrap();
        let resp = svc.oneshot(req).await.unwrap();
        let id = resp
            .headers()
            .get(REQUEST_ID_HEADER)
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(id.len(), 36);
    }

    #[tokio::test]
    async fn rejects_whitespace() {
        let svc = RequestIdLayer.layer(EchoService {
            preset_response_header: None,
        });
        let req = Request::builder()
            .header(REQUEST_ID_HEADER, "abc def") // contains 0x20
            .body(Vec::<u8>::new())
            .unwrap();
        let resp = svc.oneshot(req).await.unwrap();
        let id = resp
            .headers()
            .get(REQUEST_ID_HEADER)
            .unwrap()
            .to_str()
            .unwrap();
        assert_ne!(id, "abc def");
        assert_eq!(id.len(), 36);
    }

    #[tokio::test]
    async fn boundary_128_chars_accepted() {
        let svc = RequestIdLayer.layer(EchoService {
            preset_response_header: None,
        });
        let boundary = "x".repeat(128);
        let req = Request::builder()
            .header(REQUEST_ID_HEADER, &boundary)
            .body(Vec::<u8>::new())
            .unwrap();
        let resp = svc.oneshot(req).await.unwrap();
        assert_eq!(
            resp.headers().get(REQUEST_ID_HEADER).unwrap(),
            boundary.as_str()
        );
    }

    #[tokio::test]
    async fn conditional_overwrite_preserves_matching_handler_value() {
        let svc = RequestIdLayer.layer(EchoService {
            preset_response_header: Some((REQUEST_ID_HEADER.to_string(), "test-xyz".to_string())),
        });
        let req = Request::builder()
            .header(REQUEST_ID_HEADER, "test-xyz")
            .body(Vec::<u8>::new())
            .unwrap();
        let resp = svc.oneshot(req).await.unwrap();
        assert_eq!(resp.headers().get(REQUEST_ID_HEADER).unwrap(), "test-xyz");
        assert_eq!(resp.headers().get_all(REQUEST_ID_HEADER).iter().count(), 1);
    }

    #[tokio::test]
    async fn conditional_overwrite_replaces_mismatched_handler_value() {
        let svc = RequestIdLayer.layer(EchoService {
            preset_response_header: Some((
                REQUEST_ID_HEADER.to_string(),
                "wrong-value".to_string(),
            )),
        });
        let req = Request::builder()
            .header(REQUEST_ID_HEADER, "correct-value")
            .body(Vec::<u8>::new())
            .unwrap();
        let resp = svc.oneshot(req).await.unwrap();
        assert_eq!(
            resp.headers().get(REQUEST_ID_HEADER).unwrap(),
            "correct-value"
        );
    }

    #[test]
    fn is_valid_rejects_control_and_high_bytes() {
        assert!(!is_valid("\tfoo")); // tab
        assert!(!is_valid("foo\nbar")); // newline
        assert!(!is_valid("foo\rbar")); // CR
        assert!(!is_valid("foo\x7F")); // DEL
        assert!(!is_valid("foo\u{00A0}")); // non-ASCII
    }
}
