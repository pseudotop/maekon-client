//! Shared gRPC `Authorization` header injection (Consumer Contract F-RC-C22-04).
//!
//! The server's `AuthenticatedServiceWrapper` requires `authorization: Bearer
//! <token>` on every authenticated RPC; calling without it is rejected
//! `UNAUTHENTICATED`. This is the SINGLE injection point shared by every gRPC
//! client method, so no sibling endpoint can silently ship without auth
//! (ADR-075 P-3 "Sibling-Endpoint Blind Spot").

use maekon_core::error::CoreError;

/// Inject `authorization: Bearer <token>` into a tonic request's metadata.
///
/// An empty `token` omits the header (backward-compat / test paths). A token
/// containing characters invalid for an HTTP header value (RFC 7230) yields
/// `CoreError::Auth` rather than silently dropping the header.
pub(super) fn inject_bearer_auth<T>(
    request: &mut tonic::Request<T>,
    token: &str,
) -> Result<(), CoreError> {
    if token.is_empty() {
        return Ok(());
    }
    let bearer = format!("Bearer {token}");
    request.metadata_mut().insert(
        "authorization",
        bearer.parse().map_err(|_| CoreError::Auth {
            code: maekon_core::error_codes::AuthCode::Failed,
            message: "authorization header value contains invalid characters".to_string(),
        })?,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::client_v1::HeartbeatRequest;

    fn req() -> tonic::Request<HeartbeatRequest> {
        tonic::Request::new(HeartbeatRequest {
            session_id: "sess-1".to_string(),
        })
    }

    #[test]
    fn injects_bearer_for_nonempty_token() {
        let mut request = req();
        inject_bearer_auth(&mut request, "jwt_abc123").unwrap();
        assert_eq!(
            request
                .metadata()
                .get("authorization")
                .expect("authorization header present")
                .to_str()
                .unwrap(),
            "Bearer jwt_abc123"
        );
    }

    #[test]
    fn omits_header_for_empty_token() {
        let mut request = req();
        inject_bearer_auth(&mut request, "").unwrap();
        assert!(
            request.metadata().get("authorization").is_none(),
            "empty token must not inject an authorization header"
        );
    }

    #[test]
    fn rejects_token_with_invalid_header_chars() {
        // \u{0001} (SOH) is not a valid HTTP/1.1 header value char (RFC 7230).
        let mut request = req();
        let err = inject_bearer_auth(&mut request, "\u{0001}bad-token").unwrap_err();
        assert!(
            matches!(err, CoreError::Auth { .. }),
            "invalid header chars must map to CoreError::Auth"
        );
        assert!(request.metadata().get("authorization").is_none());
    }
}
