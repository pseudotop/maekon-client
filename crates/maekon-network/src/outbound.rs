//! Shared helper for hardening outbound HTTP clients (#6892).
//!
//! reqwest's default redirect policy follows up to 10 hops, and on a cross-host
//! redirect it strips only the standard sensitive headers
//! (`Authorization`/`Cookie`/`Proxy-Authorization`/`Www-Authenticate`) while
//! keeping **custom auth headers** (`x-api-key`, `x-goog-api-key`, DPoP, etc.)
//! and re-sending the request body verbatim on 307/308. As a result, a 30x
//! response from a compromised/MITM'd provider endpoint can leak BYOK
//! credentials together with captured screen OCR images and LLM prompt bodies
//! to an attacker host (#6892; the same-class precedent on the WebSocket
//! transport is #6824).
//!
//! Every outbound client that carries provider credentials must be created with
//! this builder so redirect following is disabled by construction. Existing
//! guard precedents: `integration/http_transport`,
//! `integration/auth/oidc_device_flow`.

use reqwest::redirect::Policy;

/// Returns a hardened reqwest [`ClientBuilder`] with redirect following disabled.
///
/// Callers chain their own options such as `.timeout(...)` onto it and call
/// `.build()`, mapping build errors to their own error types. Only the redirect
/// policy is enforced in common, so each caller's existing behavior (timeouts,
/// DNS pinning, etc.) is preserved as-is.
///
/// [`ClientBuilder`]: reqwest::ClientBuilder
pub(crate) fn hardened_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder().redirect(Policy::none())
}

/// #6939: response body cap for BYOK external-AI providers. Generous enough
/// (64 MiB) for LLM responses/embedding vectors/OCR results yet within the 24/7
/// agent memory budget (~100 MB). Prevents OOM from a compromised/MITM
/// provider's multi-GB response.
pub(crate) const MAX_AI_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;

/// #6940: response body cap for the integration SaaS HTTP transport
/// (ack/prompt-pull/bootstrap). Generous for legitimate control-plane responses
/// (8 MiB) while preventing OOM.
pub(crate) const MAX_INTEGRATION_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;

/// #6949: response body cap for auth/token/session/server control-plane
/// responses (ONESHIM REST server, OAuth/OIDC token endpoints, session creation,
/// error bodies). These JSON responses are a few KB, so 16 MiB rejects no
/// legitimate response while preventing multi-GB OOM from a compromised/MITM
/// host (especially an external OAuth/OIDC IdP).
pub(crate) const MAX_AUTH_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;

/// #6939/#6940: error from reading an outbound response body under a size cap.
/// Callers map it to their own error type (CoreError / NetworkError), and
/// transport errors are preserved in `Transport` so the `e.is_timeout()` branch
/// can be kept.
pub(crate) enum BodyReadError {
    /// Network/transport read error (original reqwest error preserved — enables timeout branching).
    Transport(reqwest::Error),
    /// Response body exceeded the cap — fail-closed.
    TooLarge { len: u64, cap: u64 },
}

/// #6939/#6940: reads a reqwest response body into memory under a `cap`-byte
/// limit.
///
/// `resp.bytes()`/`.text()`/`.json()` buffer the body without bound, so a
/// compromised/MITM'd provider/SaaS endpoint that omits or understates
/// Content-Length while sending a multi-GB body could exhaust the 24/7 agent
/// heap before parsing. This helper rejects an honestly oversized Content-Length
/// early, then aborts the moment the accumulated chunk total exceeds `cap`
/// (defending against absent/understated Content-Length). fail-closed: both a
/// read error and a cap overflow return `Err`.
///
/// The sync transport (read_body_capped/read_text_capped) is unified onto this
/// helper as well.
pub(crate) async fn read_body_capped(
    mut resp: reqwest::Response,
    cap: u64,
) -> Result<Vec<u8>, BodyReadError> {
    if let Some(len) = resp.content_length() {
        if len > cap {
            return Err(BodyReadError::TooLarge { len, cap });
        }
    }
    let mut bytes: Vec<u8> = Vec::new();
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                let projected = bytes.len() as u64 + chunk.len() as u64;
                if projected > cap {
                    return Err(BodyReadError::TooLarge {
                        len: projected,
                        cap,
                    });
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(e) => return Err(BodyReadError::Transport(e)),
        }
    }
    Ok(bytes)
}

/// Converts the `read_body_capped` result into a UTF-8 lossy string (for text responses).
pub(crate) async fn read_text_capped(
    resp: reqwest::Response,
    cap: u64,
) -> Result<String, BodyReadError> {
    let bytes = read_body_capped(resp, cap).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A client built with the hardened builder must build successfully (guarantees
    /// the redirect=none setting does not break the reqwest build — regression guard).
    #[test]
    fn hardened_client_builder_builds_successfully() {
        hardened_client_builder().build().expect(
            "하드닝 클라이언트 빌드는 성공해야 한다 (redirect=none 이 reqwest 빌드를 깨지 않음)",
        );
    }

    /// Core behavior guard (#6892): the hardened client **does not follow** 30x.
    /// Even if the server redirects 302 from `/start` → `/leaked`, the client must
    /// return the 302 as-is and never call `/leaked`. A client with the default
    /// redirect policy (limited(10)) would call `/leaked` and fail this test.
    #[tokio::test]
    async fn hardened_client_does_not_follow_redirects() {
        let mut server = mockito::Server::new_async().await;
        let start = server
            .mock("GET", "/start")
            .with_status(302)
            .with_header("location", "/leaked")
            .create_async()
            .await;
        // Endpoint that would be called if the redirect were followed — must be called 0 times.
        let leaked = server
            .mock("GET", "/leaked")
            .with_status(200)
            .with_body("LEAKED")
            .expect(0)
            .create_async()
            .await;

        let client = hardened_client_builder()
            .build()
            .expect("하드닝 클라이언트 빌드");
        let resp = client
            .get(format!("{}/start", server.url()))
            .send()
            .await
            .expect("요청 전송");

        // redirect=none → the 302 is returned as-is and the body is not re-sent to the redirect target.
        assert_eq!(
            resp.status().as_u16(),
            302,
            "302 를 추종하지 않고 그대로 반환해야 한다"
        );
        start.assert_async().await;
        leaked.assert_async().await; // expect(0): verifies that /leaked was never called
    }

    /// #6939/#6940: read_body_capped rejects an over-cap body as fail-closed (TooLarge).
    #[tokio::test]
    async fn read_body_capped_rejects_oversized() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("GET", "/big")
            .with_status(200)
            .with_body(vec![b'x'; 100])
            .create_async()
            .await;
        let resp = reqwest::get(format!("{}/big", server.url()))
            .await
            .expect("req");
        let r = read_body_capped(resp, 10).await;
        assert!(
            matches!(r, Err(BodyReadError::TooLarge { .. })),
            "100B body with 10B cap must be TooLarge"
        );
        m.assert_async().await;
    }

    /// #6939/#6940: a body within the cap passes through unchanged.
    #[tokio::test]
    async fn read_body_capped_allows_within_cap() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("GET", "/ok")
            .with_status(200)
            .with_body(vec![b'y'; 100])
            .create_async()
            .await;
        let resp = reqwest::get(format!("{}/ok", server.url()))
            .await
            .expect("req");
        let body = read_body_capped(resp, 1024).await.ok().expect("within cap");
        assert_eq!(body.len(), 100);
        m.assert_async().await;
    }
}
