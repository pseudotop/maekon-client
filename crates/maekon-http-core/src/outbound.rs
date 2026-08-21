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
//!
//! #7724: `hardened_client_builder`/`read_body_capped`/`read_text_capped`/
//! [`BodyReadError`] are `pub` (not `pub(crate)`) so any crate that depends on
//! `maekon-network` can adopt the same primitives instead of hand-rolling a
//! near-duplicate. Two current near-duplicates — `src-tauri/src/updater` and
//! `maekon-audio::cloud_stt` — cannot actually reach this `pub` API: `updater`
//! is compiled unconditionally (not feature-gated) while `maekon-network` is an
//! optional dependency only pulled in by the `analysis` feature (CI enforces a
//! `--no-default-features` build cell that excludes it), and `maekon-audio`
//! cannot depend on `maekon-network` at all — `scripts/check-crate-boundaries.sh`
//! forbids adapter-to-adapter crate edges. Both keep local copies (documented at
//! their call sites); this promotion exists for any *other* consumer that
//! already depends on `maekon-network` unconditionally.

use reqwest::redirect::Policy;

/// Transport-layer cleartext policy for [`hardened_client_builder`] (#8045 C3).
///
/// The config layer already blocks cleartext egress to remote hosts (e.g.
/// `endpoint_is_loopback` gates in the sync/analysis transports), but sibling
/// first-party transports (`http_client::build_reqwest_client_for_url`) ALSO
/// enforce it at the transport layer via reqwest's `https_only`. This adds the
/// same by-construction backstop to the shared BYOK/provider client builder so a
/// misconfigured or drifted call site cannot ship provider credentials + captured
/// screen/prompt bodies over plaintext `http://` to a remote host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportPolicy {
    /// Enforce HTTPS at the transport layer: reqwest rejects any `http://` URL
    /// at request time. Use for every remote/provider endpoint.
    HttpsOnly,
    /// Permit cleartext `http://` (no `https_only`). Use ONLY when the client
    /// genuinely targets a loopback endpoint (local LLM/OCR/embedding servers
    /// such as Ollama, or loopback dev/test servers), where cleartext never
    /// leaves the machine.
    AllowLoopbackCleartext,
}

impl TransportPolicy {
    /// Derive the policy from the endpoint the client will target: loopback
    /// hosts keep cleartext (`AllowLoopbackCleartext`); everything else is
    /// `HttpsOnly`. Reuses the strict [`host_is_loopback`]
    /// helper so the loopback definition stays consistent with the sibling
    /// transports (full `127.0.0.0/8` + `::1` + literal `localhost`; fail-closed
    /// on an unparseable URL → `HttpsOnly`).
    pub fn for_endpoint(endpoint: &str) -> Self {
        if host_is_loopback(endpoint) {
            Self::AllowLoopbackCleartext
        } else {
            Self::HttpsOnly
        }
    }
}

/// Returns a hardened reqwest [`ClientBuilder`] with redirect following disabled
/// and a transport-layer cleartext policy applied (#6892, #8045 C3).
///
/// Callers chain their own options such as `.timeout(...)` onto it and call
/// `.build()`, mapping build errors to their own error types. Redirect following
/// is always disabled; `policy` additionally sets `https_only(true)` for
/// [`TransportPolicy::HttpsOnly`] so a remote endpoint cannot be reached over
/// cleartext `http://`. Loopback/dev callers pass
/// [`TransportPolicy::AllowLoopbackCleartext`] (or derive via
/// [`TransportPolicy::for_endpoint`]) to keep local `http://` working.
///
/// [`ClientBuilder`]: reqwest::ClientBuilder
pub fn hardened_client_builder(policy: TransportPolicy) -> reqwest::ClientBuilder {
    let builder = reqwest::Client::builder().redirect(Policy::none());
    match policy {
        TransportPolicy::HttpsOnly => builder.https_only(true),
        TransportPolicy::AllowLoopbackCleartext => builder,
    }
}

/// #6939: response body cap for BYOK external-AI providers. Generous enough
/// (64 MiB) for LLM responses/embedding vectors/OCR results yet within the 24/7
/// agent memory budget (~100 MB). Prevents OOM from a compromised/MITM
/// provider's multi-GB response.
pub const MAX_AI_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;

/// #6940: response body cap for the integration SaaS HTTP transport
/// (ack/prompt-pull/bootstrap). Generous for legitimate control-plane responses
/// (8 MiB) while preventing OOM.
pub const MAX_INTEGRATION_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;

/// #6949: response body cap for auth/token/session/server control-plane
/// responses (ONESHIM REST server, OAuth/OIDC token endpoints, session creation,
/// error bodies). These JSON responses are a few KB, so 16 MiB rejects no
/// legitimate response while preventing multi-GB OOM from a compromised/MITM
/// host (especially an external OAuth/OIDC IdP).
pub const MAX_AUTH_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;

/// #6939/#6940: error from reading an outbound response body under a size cap.
/// Callers map it to their own error type (CoreError / NetworkError), and
/// transport errors are preserved in `Transport` so the `e.is_timeout()` branch
/// can be kept.
///
/// `Debug` is derived so a failing assertion can name the variant it got;
/// without it a cap-guard test can only say "not TooLarge".
#[derive(Debug)]
pub enum BodyReadError {
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
pub async fn read_body_capped(
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
pub async fn read_text_capped(resp: reqwest::Response, cap: u64) -> Result<String, BodyReadError> {
    let bytes = read_body_capped(resp, cap).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Strict loopback check for the ADR-023 MG-PII-02 enrichment egress gate.
///
/// Parses the URL and accepts ONLY `localhost` or an IP that is loopback (the
/// full `127.0.0.0/8` range + `::1`, via `IpAddr::is_loopback`). Stronger than
/// [`is_localhost`]'s literal set. **Fail-closed**: an unparseable URL, a missing
/// host, or a non-loopback host returns `false`. Note: the IPv4-mapped IPv6
/// loopback (`::ffff:127.0.0.1`) is NOT loopback per `IpAddr::is_loopback` and is
/// therefore refused (safe default).
pub fn host_is_loopback(url: &str) -> bool {
    match reqwest::Url::parse(url) {
        Ok(parsed) => match parsed.host_str() {
            Some(host) => {
                // `host_str()` serializes IPv6 WITH brackets (e.g. "[::1]"); strip
                // them so the address parses.
                let host = host
                    .strip_prefix('[')
                    .and_then(|h| h.strip_suffix(']'))
                    .unwrap_or(host);
                host.eq_ignore_ascii_case("localhost")
                    || host
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|addr| addr.is_loopback())
            }
            None => false,
        },
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A client built with the hardened builder must build successfully (guarantees
    /// the redirect=none setting does not break the reqwest build — regression guard).
    #[test]
    fn hardened_client_builder_builds_successfully() {
        hardened_client_builder(TransportPolicy::AllowLoopbackCleartext)
            .build()
            .expect("hardened client build must succeed (redirect=none does not break reqwest)");
        hardened_client_builder(TransportPolicy::HttpsOnly)
            .build()
            .expect("hardened client build must succeed with https_only enabled");
    }

    /// #8045 C3: a `HttpsOnly` client rejects a cleartext `http://` request at
    /// dispatch time (the by-construction backstop), while an
    /// `AllowLoopbackCleartext` client permits it. Uses an unroutable loopback
    /// port and asserts the error kind (builder rejects before any real network
    /// I/O) so the test never touches the network.
    #[tokio::test]
    async fn https_only_policy_rejects_cleartext_http() {
        let https_only = hardened_client_builder(TransportPolicy::HttpsOnly)
            .build()
            .expect("build https-only client");
        let err = https_only
            .get("http://127.0.0.1:9/should-be-refused")
            .send()
            .await
            .unwrap_err();
        assert!(
            err.is_builder(),
            "https_only must refuse an http:// URL as a builder error, got: {err:?}"
        );

        // The loopback-cleartext policy must NOT reject the scheme; a failure here
        // (if any) is a connection error to the dead port, never a builder error.
        let cleartext = hardened_client_builder(TransportPolicy::AllowLoopbackCleartext)
            .build()
            .expect("build cleartext client");
        if let Err(e) = cleartext
            .get("http://127.0.0.1:9/allowed-scheme")
            .send()
            .await
        {
            assert!(
                !e.is_builder(),
                "AllowLoopbackCleartext must not reject the http:// scheme, got: {e:?}"
            );
        }
    }

    /// #8045 C3: `for_endpoint` maps loopback hosts to cleartext-allowed and
    /// everything else (including unparseable, fail-closed) to https-only.
    #[test]
    fn for_endpoint_derives_policy_from_loopback() {
        assert_eq!(
            TransportPolicy::for_endpoint("http://127.0.0.1:11434/api"),
            TransportPolicy::AllowLoopbackCleartext
        );
        assert_eq!(
            TransportPolicy::for_endpoint("http://localhost:8000"),
            TransportPolicy::AllowLoopbackCleartext
        );
        assert_eq!(
            TransportPolicy::for_endpoint("https://api.anthropic.com/v1/messages"),
            TransportPolicy::HttpsOnly
        );
        assert_eq!(
            TransportPolicy::for_endpoint("http://api.example.com"),
            TransportPolicy::HttpsOnly
        );
        assert_eq!(
            TransportPolicy::for_endpoint("not a url"),
            TransportPolicy::HttpsOnly,
            "unparseable endpoint must fail closed to HttpsOnly"
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

        // Loopback mockito server → cleartext policy (mirrors a loopback caller).
        let client = hardened_client_builder(TransportPolicy::AllowLoopbackCleartext)
            .build()
            .expect("build hardened client");
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

    /// #11296: the caps ARE the guard, yet no test read them, so a mutant turning
    /// `64 * 1024 * 1024` (67,108,864) into `64 + 1024 + 1024` (2,112) or
    /// `64 / 1024 / 1024` (0) passed the suite untouched. Six such mutants survived
    /// run 32449318499.
    ///
    /// Written as plain byte counts rather than `64 * 1024 * 1024`, so the
    /// assertion cannot drift in sympathy with the expression it guards.
    #[test]
    fn response_caps_hold_their_documented_sizes() {
        assert_eq!(
            MAX_AI_RESPONSE_BYTES, 67_108_864,
            "#6939: the BYOK AI response cap must stay 64 MiB"
        );
        assert_eq!(
            MAX_INTEGRATION_RESPONSE_BYTES, 8_388_608,
            "#6940: the integration transport cap must stay 8 MiB"
        );
        assert_eq!(
            MAX_AUTH_RESPONSE_BYTES, 16_777_216,
            "#6949: the auth/control-plane cap must stay 16 MiB"
        );
    }

    /// #11296: `100 > 10` and `100 >= 10` agree, so the pre-existing pair of tests
    /// could not tell `>` from `>=`. The boundary is the only place the two
    /// disagree, and a body of exactly `cap` is within the cap.
    #[tokio::test]
    async fn read_body_capped_allows_body_exactly_at_cap() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("GET", "/exact")
            .with_status(200)
            .with_body(vec![b'z'; 64])
            .create_async()
            .await;
        let resp = reqwest::get(format!("{}/exact", server.url()))
            .await
            .expect("req");
        let body = read_body_capped(resp, 64)
            .await
            .expect("a body of exactly `cap` bytes is within the cap");
        assert_eq!(body.len(), 64);
        m.assert_async().await;
    }

    /// #11296: the other side of the same boundary. The reported `len` is asserted
    /// too — a guard that rejects but misreports the size cannot be reasoned about
    /// from a log, and pinning it also fixes which of the two checks fired.
    #[tokio::test]
    async fn read_body_capped_rejects_one_byte_over_cap() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("GET", "/over")
            .with_status(200)
            .with_body(vec![b'z'; 65])
            .create_async()
            .await;
        let resp = reqwest::get(format!("{}/over", server.url()))
            .await
            .expect("req");
        match read_body_capped(resp, 64).await {
            Err(BodyReadError::TooLarge { len, cap }) => {
                assert_eq!(len, 65, "the reported length must be the real body length");
                assert_eq!(cap, 64, "the reported cap must be the cap that was applied");
            }
            other => panic!("65B body with a 64B cap must be TooLarge, got {other:?}"),
        }
        m.assert_async().await;
    }

    /// #11296: with a `Content-Length` the pre-check at the top of the function
    /// rejects first and the streaming accumulator never runs — every mutant inside
    /// that loop is unobservable. A chunked response carries no `Content-Length`,
    /// which is the only way to reach it.
    ///
    /// The exact `len` is asserted rather than just the variant: it is what
    /// separates `bytes.len() + chunk.len()` from `bytes.len() * chunk.len()`
    /// under ANY chunk split, whereas the variant alone agrees for most splits.
    #[tokio::test]
    async fn read_body_capped_rejects_over_cap_without_content_length() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("GET", "/chunked-over")
            .with_status(200)
            .with_chunked_body(|w| {
                w.write_all(&[b'a'; 40])?;
                w.write_all(&[b'b'; 40])
            })
            .create_async()
            .await;
        let resp = reqwest::get(format!("{}/chunked-over", server.url()))
            .await
            .expect("req");
        assert!(
            resp.content_length().is_none(),
            "this test is only meaningful without a Content-Length; \
             with one the pre-check fires and the streaming path is never taken"
        );
        match read_body_capped(resp, 64).await {
            Err(BodyReadError::TooLarge { len, cap }) => {
                assert_eq!(len, 80, "the accumulated length must be 40 + 40");
                assert_eq!(cap, 64, "the reported cap must be the cap that was applied");
            }
            other => panic!("80B streamed with a 64B cap must be TooLarge, got {other:?}"),
        }
        m.assert_async().await;
    }

    /// #11296: the streaming side of the boundary. Without this, `>` and `>=`
    /// inside the accumulator loop remain indistinguishable.
    #[tokio::test]
    async fn read_body_capped_allows_body_exactly_at_cap_without_content_length() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("GET", "/chunked-exact")
            .with_status(200)
            .with_chunked_body(|w| {
                w.write_all(&[b'a'; 32])?;
                w.write_all(&[b'b'; 32])
            })
            .create_async()
            .await;
        let resp = reqwest::get(format!("{}/chunked-exact", server.url()))
            .await
            .expect("req");
        assert!(
            resp.content_length().is_none(),
            "this test is only meaningful without a Content-Length"
        );
        let body = read_body_capped(resp, 64)
            .await
            .expect("32 + 32 streamed bytes are exactly at a 64B cap, not over it");
        assert_eq!(body.len(), 64);
        m.assert_async().await;
    }

    // Moved with the function from maekon-network's http_client.rs (ADR-034 P2).
    #[test]
    fn host_is_loopback_strict_table() {
        // Accepted: literal localhost + any loopback IP (full 127.0.0.0/8 + ::1).
        assert!(host_is_loopback("http://localhost:11434"));
        assert!(host_is_loopback("http://127.0.0.1:8000"));
        assert!(host_is_loopback("http://127.0.0.5:1"));
        assert!(host_is_loopback("http://[::1]:50051"));
        // Refused (fail-closed):
        assert!(!host_is_loopback("https://api.example.com"));
        assert!(!host_is_loopback("http://localhost.evil.com")); // not literal localhost
        assert!(!host_is_loopback("http://10.0.0.5:11434")); // private but remote
        assert!(!host_is_loopback("http://[::ffff:127.0.0.1]:1")); // mapped-loopback refused
        assert!(!host_is_loopback("not a url")); // unparseable
        assert!(!host_is_loopback("http://")); // no host
    }
}
