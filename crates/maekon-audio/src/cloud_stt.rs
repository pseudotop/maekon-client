//! Cloud STT provider using OpenAI Whisper API.
//!
//! Gated behind `#[cfg(feature = "cloud-stt")]`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::multipart;
use tracing::{debug, warn};
use zeroize::{Zeroize, Zeroizing};

use maekon_core::config::{PiiFilterLevel, SttLanguage};
use maekon_core::error::CoreError;
use maekon_core::models::audio::{AudioBuffer, TranscriptionResult};
use maekon_core::ports::pii_sanitizer::PiiSanitizer;
use maekon_core::ports::stt_provider::SttProvider;

pub struct CloudSttProvider {
    client: reqwest::Client,
    api_key: Zeroizing<String>,
    endpoint: String,
    language: SttLanguage,
    timeout_secs: u32,
    /// D5 iter-4: injected PII sanitizer for transcript sanitization.
    pii_sanitizer: Option<Arc<dyn PiiSanitizer>>,
    pii_level: PiiFilterLevel,
}

/// True if `endpoint` is safe for transmitting a Bearer API key + raw audio:
/// `https://` to any host, or `http://` only to an explicit loopback host (a
/// local proxy/sidecar). A routable `http://` endpoint is rejected. (#6102-20)
///
/// Hand-parsed (no `url` dependency): scheme is the prefix before `://`, and the
/// authority is everything up to the first `/`, `?`, or `#`, with userinfo and
/// port stripped and bracketed IPv6 handled. #7723: the final host-extraction +
/// loopback classification steps delegate to `maekon_core::net_policy`'s
/// pure-string helpers (this crate intentionally carries no URL-parser
/// dependency; those helpers exist for exactly that case).
fn endpoint_is_secure(endpoint: &str) -> bool {
    let lower = endpoint.trim().to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("https://") {
        return !rest.is_empty();
    }
    if let Some(rest) = lower.strip_prefix("http://") {
        let authority = rest.split(['/', '\\', '?', '#']).next().unwrap_or("");
        // Drop any `user:pass@` userinfo prefix.
        let host_port = authority.rsplit('@').next().unwrap_or(authority);
        // Strip the port, handling bracketed IPv6 like `[::1]:8080`.
        let host = maekon_core::net_policy::authority_host(host_port);
        // "localhost", or any IP that parses as loopback (127.0.0.0/8, ::1).
        // Parsing as IpAddr rejects look-alikes like "127.0.0.1.evil.com".
        return maekon_core::net_policy::is_loopback_host_str(host);
    }
    false
}

/// Build the cloud-STT HTTP client — extracted so the redirect-hardening
/// invariant below is directly unit-testable (see `stt_client_does_not_follow_redirects`).
///
/// #7724: disables redirect following, matching the credential-bearing client
/// hardening in `maekon-network::outbound::hardened_client_builder` (#6892).
/// This client sends a Bearer API key (`bearer_auth` in `transcribe`) plus the
/// raw multipart audio body to a user-configured (but `endpoint_is_secure`-gated)
/// endpoint. reqwest's default redirect policy follows up to 10 hops and
/// re-sends the request body verbatim on a 307/308, so a compromised/MITM'd
/// provider endpoint could 30x to an attacker host and receive the recorded
/// audio. `maekon-audio` cannot depend on `maekon-network`
/// (`scripts/check-crate-boundaries.sh` forbids adapter-to-adapter crate edges
/// per the hexagonal architecture), so this hardening is duplicated locally
/// rather than calling `hardened_client_builder` directly.
fn build_stt_client(timeout_secs: u32) -> Result<reqwest::Client, CoreError> {
    // #6102-22: mirror the network helper convention — 0 means "no timeout"
    // (unlimited), not a zero-duration deadline that would fail every request.
    let mut builder = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none());
    if timeout_secs > 0 {
        builder = builder.timeout(Duration::from_secs(u64::from(timeout_secs)));
    }
    builder.build().map_err(|e| CoreError::SpeechToText {
        code: maekon_core::error_codes::AudioCode::SttFailed,
        message: format!("build HTTP client: {e}"),
    })
}

impl CloudSttProvider {
    pub fn new(
        api_key: impl Into<String>,
        endpoint: String,
        language: SttLanguage,
        timeout_secs: u32,
    ) -> Result<Self, CoreError> {
        let api_key = Zeroizing::new(api_key.into());
        if api_key.is_empty() {
            return Err(CoreError::SpeechToText {
                code: maekon_core::error_codes::AudioCode::SttFailed,
                message: "cloud STT API key is empty".into(),
            });
        }

        // #6102-20: a Bearer API key + raw microphone audio must never egress over
        // cleartext http://. Require https, allowing http only for an explicit
        // loopback host (a local proxy/sidecar), never for a routable endpoint.
        if !endpoint_is_secure(&endpoint) {
            return Err(CoreError::SpeechToText {
                code: maekon_core::error_codes::AudioCode::SttFailed,
                message: format!(
                    "cloud STT endpoint must be https:// (or http:// to a loopback host); got: {endpoint}"
                ),
            });
        }

        let client = build_stt_client(timeout_secs)?;

        Ok(Self {
            client,
            api_key,
            endpoint,
            language,
            timeout_secs,
            pii_sanitizer: None,
            pii_level: PiiFilterLevel::Standard,
        })
    }

    /// D5 iter-4: attach a `PiiSanitizer` for transcript sanitization.
    pub fn with_pii_sanitizer(
        mut self,
        sanitizer: Arc<dyn PiiSanitizer>,
        level: PiiFilterLevel,
    ) -> Self {
        self.pii_sanitizer = Some(sanitizer);
        self.pii_level = level;
        self
    }
}

#[cfg(test)]
fn truncate_provider_body(body: &str, max_chars: usize) -> &str {
    match body.char_indices().nth(max_chars) {
        // nth(n) returns the (byte_index, char) of the (n+1)-th character — i.e. the first
        // character that would exceed the budget.  Slicing to that byte index gives exactly
        // `max_chars` characters.
        Some((byte_idx, _)) => &body[..byte_idx],
        // Fewer than max_chars chars in the string: return the whole thing.
        None => body,
    }
}

fn provider_body_log_preview(body: &str) -> String {
    if body.is_empty() {
        return "[redacted provider error body: 0 bytes]".to_string();
    }

    format!("[redacted provider error body: {} bytes]", body.len())
}

fn provider_error_message(status: reqwest::StatusCode, body: &str) -> String {
    let body_summary = provider_body_log_preview(body);
    format!("cloud STT error: HTTP {status} — {body_summary}")
}

/// Maximum cloud-STT response body this provider will buffer (#6989).
///
/// Whisper transcript JSON is kilobytes even for long audio, so a 16 MiB ceiling is
/// generous while bounding memory against a malicious / compromised / MITM endpoint
/// that streams an unbounded body. Same OOM threat model as the #6949 maekon-network
/// response-body caps in `maekon_network::outbound` — `maekon-audio` cannot depend on
/// `maekon-network` at all (`scripts/check-crate-boundaries.sh` forbids
/// adapter-to-adapter crate edges), so this cap is a crate-local, domain-specific
/// value rather than shared with network's own AI/integration/auth caps.
const MAX_STT_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// #7724: pure predicate for the Content-Length early-reject below — extracted so
/// it is directly unit-testable without a real HTTP round-trip.
fn declared_content_length_exceeds_cap(content_length: Option<u64>, max_bytes: usize) -> bool {
    content_length.is_some_and(|len| len > max_bytes as u64)
}

/// Read the cloud-STT response body incrementally, never buffering more than
/// [`MAX_STT_RESPONSE_BYTES`] (#6989).
///
/// #7724: rejects an honestly-oversized declared `Content-Length` up front
/// (mirroring `maekon_network::outbound::read_body_capped` and
/// `updater::install::download::read_body_capped_update`), THEN pulls the body
/// chunk-by-chunk and aborts as soon as the accumulated size would exceed the
/// cap — so a forged/absent `Content-Length` with an unbounded (e.g. chunked)
/// body can never be fully buffered either. Returns `Err` on cap-exceed: the
/// endpoint carries a Bearer key + raw audio and is user-configured, so an oversized
/// response is treated as hostile rather than truncated-and-parsed.
///
/// `max_bytes` is a parameter (not a hardcoded read of the const) so unit tests can
/// exercise the cap with a small ceiling instead of allocating a multi-MiB body.
async fn read_stt_body_capped(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, CoreError> {
    if declared_content_length_exceeds_cap(response.content_length(), max_bytes) {
        return Err(CoreError::SpeechToText {
            code: maekon_core::error_codes::AudioCode::SttFailed,
            message: format!(
                "cloud STT response declared Content-Length exceeds {max_bytes} byte cap"
            ),
        });
    }

    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| CoreError::Network {
        code: maekon_core::error_codes::NetworkCode::Generic,
        message: format!("cloud STT body read: {e}"),
    })? {
        if buf.len() + chunk.len() > max_bytes {
            return Err(CoreError::SpeechToText {
                code: maekon_core::error_codes::AudioCode::SttFailed,
                message: format!("cloud STT response body exceeds {max_bytes} byte cap"),
            });
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// Encode `audio` to WAV bytes for upload, then zeroize the source PCM samples (#7098).
///
/// The returned WAV bytes are moved into the multipart request body and handed to
/// `reqwest`, which owns and drops them internally after transmission with no
/// zeroize hook exposed to this crate — so the actually-transmitted copy cannot be
/// overwritten in place here (the same "ownership leaves our control" boundary as
/// the resampled `AudioBuffer` returned by `maekon-audio::capture`). What we CAN
/// and DO wipe is the source f32 PCM we own: `audio.samples` holds the raw recorded
/// speech and would otherwise drop un-overwritten at the end of `transcribe`,
/// leaving it recoverable in freed heap. Overwriting it here extends the #7077
/// stop/drain/Drop capture-buffer treatment to the STT upload path. Encoding
/// happens BEFORE the wipe so the WAV body carries the real audio, not zeros.
fn encode_wav_and_wipe_source(audio: &mut AudioBuffer) -> Vec<u8> {
    let wav_bytes = audio.to_wav_bytes();
    audio.samples.zeroize();
    wav_bytes
}

#[async_trait]
impl SttProvider for CloudSttProvider {
    async fn transcribe(&self, mut audio: AudioBuffer) -> Result<TranscriptionResult, CoreError> {
        if audio.is_empty() {
            return Ok(TranscriptionResult {
                text: String::new(),
                language: None,
                duration_secs: 0.0,
                processing_secs: 0.0,
            });
        }

        let duration_secs = audio.duration_secs;
        let start = Instant::now();

        // Convert to WAV for upload, then zeroize the source PCM we own (#7098).
        // The WAV bytes are moved into the multipart body below; reqwest owns and
        // drops that transmitted copy with no wipe hook, so we wipe what we can.
        let wav_bytes = encode_wav_and_wipe_source(&mut audio);

        let file_part = multipart::Part::bytes(wav_bytes)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|e| CoreError::SpeechToText {
                code: maekon_core::error_codes::AudioCode::SttFailed,
                message: format!("create multipart: {e}"),
            })?;

        let mut form = multipart::Form::new()
            .part("file", file_part)
            .text("model", "whisper-1");

        // Add language hint if not auto
        match self.language {
            SttLanguage::En => {
                form = form.text("language", "en");
            }
            SttLanguage::Ko => {
                form = form.text("language", "ko");
            }
            SttLanguage::Auto => {}
        }

        debug!(endpoint = %self.endpoint, "sending cloud STT request");

        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(self.api_key.as_str())
            .multipart(form)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    CoreError::RequestTimeout {
                        code: maekon_core::error_codes::NetworkCode::Timeout,
                        timeout_ms: u64::from(self.timeout_secs) * 1000,
                    }
                } else {
                    CoreError::Network {
                        code: maekon_core::error_codes::NetworkCode::Generic,
                        message: format!("cloud STT request: {e}"),
                    }
                }
            })?;

        if !response.status().is_success() {
            let status = response.status();
            // #6989: read the error body under a hard size cap so a hostile endpoint
            // cannot OOM us via an unbounded error body. On cap-exceed / read error,
            // fall back to a short placeholder — this path is best-effort logging.
            let body_bytes = read_stt_body_capped(response, MAX_STT_RESPONSE_BYTES)
                .await
                .unwrap_or_else(|_| {
                    b"<oversized or unreadable provider error body omitted>".to_vec()
                });
            let body = String::from_utf8_lossy(&body_bytes).into_owned();

            // Never log the provider body verbatim: error responses can echo transcript
            // text, credentials, request IDs, or provider internals. Keep structured
            // status/size signals plus a redacted marker for observability.
            let provider_body_preview = provider_body_log_preview(&body);
            warn!(
                http.status = status.as_u16(),
                err.code = "stt_failed",
                provider_body_len = body.len(),
                provider_body_preview = %provider_body_preview,
                "cloud STT provider returned error"
            );

            // The upstream fallback/VAD path re-emits this error string to logs/events,
            // so the provider body is not included even in the error message.
            let message = provider_error_message(status, &body);

            // Semantic HTTP status mapping per iter-54/55/56 pattern — even STT
            // domain errors benefit from differentiating auth/timeout/rate-limit
            // from generic STT failures.
            return Err(match status.as_u16() {
                401 | 403 => CoreError::Auth {
                    code: maekon_core::error_codes::AuthCode::Failed,
                    message,
                },
                408 | 504 => CoreError::RequestTimeout {
                    code: maekon_core::error_codes::NetworkCode::Timeout,
                    timeout_ms: u64::from(self.timeout_secs) * 1000,
                },
                429 => CoreError::RateLimit {
                    code: maekon_core::error_codes::NetworkCode::RateLimit,
                    retry_after_secs: 60,
                },
                502 | 503 => CoreError::ServiceUnavailable {
                    code: maekon_core::error_codes::ServiceCode::Unavailable,
                    message,
                },
                _ => CoreError::SpeechToText {
                    code: maekon_core::error_codes::AudioCode::SttFailed,
                    message,
                },
            });
        }

        #[derive(serde::Deserialize)]
        struct OpenAiResponse {
            text: String,
        }

        // #6989: parse from a size-capped body, never the unbounded reqwest `.json()`.
        let body_bytes = read_stt_body_capped(response, MAX_STT_RESPONSE_BYTES).await?;
        let result: OpenAiResponse =
            serde_json::from_slice(&body_bytes).map_err(|e| CoreError::SpeechToText {
                code: maekon_core::error_codes::AudioCode::SttFailed,
                message: format!("parse cloud response: {e}"),
            })?;

        let processing_secs = start.elapsed().as_secs_f32();
        debug!(
            text_len = result.text.len(),
            processing_secs, "cloud STT complete"
        );

        // D5 iter-4: sanitize transcript at provider boundary.
        let text = self
            .pii_sanitizer
            .as_ref()
            .map(|s| s.sanitize_text(&result.text, self.pii_level))
            .unwrap_or(result.text);

        Ok(TranscriptionResult {
            text,
            language: None,
            duration_secs,
            processing_secs,
        })
    }

    fn provider_name(&self) -> &str {
        "openai-whisper-cloud"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- truncate_provider_body ---

    #[test]
    fn truncate_short_body_is_unchanged() {
        assert_eq!(truncate_provider_body("hello", 200), "hello");
    }

    #[test]
    fn truncate_empty_body_is_unchanged() {
        assert_eq!(truncate_provider_body("", 200), "");
    }

    #[test]
    fn truncate_exact_limit_is_unchanged() {
        let body = "a".repeat(200);
        assert_eq!(truncate_provider_body(&body, 200), body.as_str());
    }

    #[test]
    fn truncate_over_limit_is_capped() {
        let body = "a".repeat(300);
        let result = truncate_provider_body(&body, 200);
        assert_eq!(
            result.len(),
            200,
            "ASCII: truncated length must equal max_chars"
        );
    }

    #[test]
    fn truncate_multibyte_char_boundary_safe() {
        // Each '\u{AC00}' (a Hangul syllable) is 3 UTF-8 bytes.  Building 100 of them gives a
        // 300-byte string that is only 100 Unicode chars.  Requesting 50 chars must yield 50 of
        // them (150 bytes), never splitting in the middle of a char.
        let body: String = "\u{AC00}".repeat(100);
        let result = truncate_provider_body(&body, 50);
        assert_eq!(result.chars().count(), 50);
        assert_eq!(result.len(), 150, "3 bytes x 50 chars = 150 bytes");
        // Verify the result is valid UTF-8 and contains exactly the expected chars.
        // `from_utf8` returns Ok(&str) — assert the decoded value equals result directly.
        assert_eq!(
            std::str::from_utf8(result.as_bytes()).expect("result must be valid UTF-8"),
            "\u{AC00}".repeat(50),
            "truncated content must be exactly 50 '\u{AC00}' characters"
        );
    }

    #[test]
    fn truncate_multibyte_long_body_stays_within_limit() {
        // Mix of ASCII and multi-byte chars to ensure we never go over the char budget.
        // Each Hangul char in "hello \u{C138}\u{ACC4} " is 3 bytes; ASCII chars are 1 byte.
        let body: String = "hello \u{C138}\u{ACC4} ".repeat(50);
        let result = truncate_provider_body(&body, 200);
        assert!(result.chars().count() <= 200);
        // Verify the result is valid UTF-8 by asserting the decoded value equals the slice itself.
        // truncate_provider_body returns &str, so from_utf8 must succeed; expect propagates any
        // future regression as a panic with a clear message rather than a silent is_ok() hedge.
        assert_eq!(
            std::str::from_utf8(result.as_bytes()).expect("result must be valid UTF-8"),
            result,
            "decoded UTF-8 must round-trip to the same &str"
        );
    }

    #[test]
    fn provider_error_message_omits_raw_provider_body() {
        let body =
            "provider echoed email alice@example.com token sk-live-secret phone 010-1234-5678";
        let message = provider_error_message(reqwest::StatusCode::INTERNAL_SERVER_ERROR, body);

        assert!(
            message.contains("HTTP 500"),
            "message must keep status diagnostics: {message}"
        );
        assert!(
            message.contains("[redacted provider error body:"),
            "message must include a redacted provider-body summary: {message}"
        );
        assert!(
            !message.contains("alice@example.com"),
            "error message must not preserve provider-echoed email: {message}"
        );
        assert!(
            !message.contains("sk-live-secret"),
            "error message must not preserve provider token-like text: {message}"
        );
        assert!(
            !message.contains("010-1234-5678"),
            "error message must not preserve provider-echoed phone number: {message}"
        );
    }

    #[test]
    fn provider_body_log_preview_masks_sensitive_values() {
        let body =
            "provider echoed email alice@example.com token sk-live-secret phone 010-1234-5678";
        let preview = provider_body_log_preview(body);

        assert!(
            !preview.contains("alice@example.com"),
            "log preview must not preserve provider-echoed email: {preview}"
        );
        assert!(
            !preview.contains("sk-live-secret"),
            "log preview must not preserve provider token-like text: {preview}"
        );
        assert!(
            !preview.contains("010-1234-5678"),
            "log preview must not preserve provider-echoed phone number: {preview}"
        );
        assert!(
            preview.contains("[redacted"),
            "log preview should retain a diagnostic redaction marker: {preview}"
        );
    }

    #[test]
    fn new_rejects_empty_api_key() {
        // `.err().expect()` instead of `.unwrap_err()`: the Ok type
        // (`CloudSttProvider`) is deliberately not `Debug` (it holds an API key).
        let err = CloudSttProvider::new(String::new(), "http://test".into(), SttLanguage::Auto, 10)
            .err()
            .expect("empty API key must be rejected at construction");
        assert!(
            matches!(err, CoreError::SpeechToText { .. }),
            "empty API key must yield CoreError::SpeechToText"
        );
    }

    #[test]
    fn provider_name_correct() {
        let provider =
            CloudSttProvider::new("sk-test", "https://test".into(), SttLanguage::Auto, 10).unwrap();
        assert_eq!(provider.provider_name(), "openai-whisper-cloud");
    }

    #[test]
    fn new_rejects_cleartext_http_endpoint() {
        // #6102-20: a routable http:// endpoint must be rejected (Bearer key + audio
        // would egress in cleartext).
        let err = CloudSttProvider::new(
            "sk-test",
            "http://api.example.com/v1/transcribe".into(),
            SttLanguage::Auto,
            10,
        )
        .err()
        .expect("cleartext http endpoint must be rejected");
        assert!(matches!(err, CoreError::SpeechToText { .. }));
    }

    #[test]
    fn new_allows_https_and_loopback_http() {
        // https to any host is fine. `.expect()` only needs the Err type
        // (CoreError) to be Debug; the Ok type (CloudSttProvider) intentionally
        // is not. Bind the provider and confirm it is actually usable.
        let provider = CloudSttProvider::new(
            "sk-test",
            "https://api.example.com".into(),
            SttLanguage::Auto,
            10,
        )
        .expect("https endpoint must be accepted");
        assert_eq!(provider.provider_name(), "openai-whisper-cloud");
        // http is allowed only to an explicit loopback host (local proxy).
        for ep in [
            "http://localhost:8080/stt",
            "http://127.0.0.1:9000",
            "http://[::1]:9000/v1",
        ] {
            let provider = CloudSttProvider::new("sk-test", ep.into(), SttLanguage::Auto, 10)
                .unwrap_or_else(|e| {
                    panic!("loopback http endpoint should be allowed: {ep}; got: {e:?}")
                });
            assert_eq!(provider.provider_name(), "openai-whisper-cloud");
        }
    }

    #[test]
    fn new_zero_timeout_builds_unlimited_client() {
        // #6102-22: 0 means "no timeout" (unlimited), not a zero-duration deadline
        // that would fail every request. Construction must succeed and the
        // resulting provider must be usable (not a half-built client).
        let provider =
            CloudSttProvider::new("sk-test", "https://test".into(), SttLanguage::Auto, 0)
                .expect("zero timeout must build an unlimited client, not fail");
        assert_eq!(provider.provider_name(), "openai-whisper-cloud");
        assert_eq!(provider.timeout_secs, 0, "zero timeout must be preserved");
    }

    #[test]
    fn endpoint_is_secure_classifies_correctly() {
        assert!(endpoint_is_secure("https://example.com"));
        assert!(endpoint_is_secure("http://localhost:1234"));
        assert!(endpoint_is_secure("http://127.0.0.1"));
        assert!(endpoint_is_secure("http://[::1]:9000"));
        assert!(!endpoint_is_secure("http://example.com"));
        assert!(!endpoint_is_secure("http://example.com\\@127.0.0.1/stt"));
        assert!(!endpoint_is_secure("http://127.0.0.1.evil.com"));
        assert!(!endpoint_is_secure("ftp://example.com"));
        assert!(!endpoint_is_secure(""));
    }

    // iter-72 regression guards for iter-58 semantic HTTP status mapping
    // in cloud_stt::transcribe. Shared helper pattern matches iter-67..71.
    async fn run_cloud_stt_status_test(status: u16) -> CoreError {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(status as usize)
            .with_body(format!("http {status}"))
            .create_async()
            .await;
        let provider =
            CloudSttProvider::new("sk-test", server.url(), SttLanguage::Auto, 10).unwrap();
        // Non-empty buffer so transcribe doesn't early-return; 1 sec silence
        // at 16kHz = 16_000 samples.
        let audio = maekon_core::models::audio::AudioBuffer::new(vec![0.0f32; 16_000]);
        provider.transcribe(audio).await.unwrap_err()
    }

    #[tokio::test]
    async fn stt_403_maps_to_auth() {
        let err = run_cloud_stt_status_test(403).await;
        assert!(
            matches!(err, CoreError::Auth { .. }),
            "403 → Auth, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn stt_408_maps_to_timeout() {
        let err = run_cloud_stt_status_test(408).await;
        assert!(
            matches!(err, CoreError::RequestTimeout { .. }),
            "408 → RequestTimeout, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn stt_429_maps_to_rate_limit() {
        let err = run_cloud_stt_status_test(429).await;
        assert!(
            matches!(err, CoreError::RateLimit { .. }),
            "429 → RateLimit, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn stt_502_maps_to_service_unavailable() {
        let err = run_cloud_stt_status_test(502).await;
        assert!(
            matches!(err, CoreError::ServiceUnavailable { .. }),
            "502 → ServiceUnavailable, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn stt_504_maps_to_timeout() {
        let err = run_cloud_stt_status_test(504).await;
        assert!(
            matches!(err, CoreError::RequestTimeout { .. }),
            "504 → RequestTimeout, got: {err:?}"
        );
    }

    /// Domain fallback: generic server error remains as SpeechToText / SttFailed.
    #[tokio::test]
    async fn stt_500_falls_back_to_stt_failed() {
        let err = run_cloud_stt_status_test(500).await;
        assert!(
            matches!(err, CoreError::SpeechToText { .. }),
            "500 → SpeechToText (domain fallback), got: {err:?}"
        );
    }

    #[tokio::test]
    async fn stt_provider_error_omits_raw_provider_body_from_core_error() {
        let mut server = mockito::Server::new_async().await;
        let sensitive_body =
            "provider echoed email alice@example.com token sk-live-secret phone 010-1234-5678";
        let _mock = server
            .mock("POST", "/")
            .with_status(500)
            .with_body(sensitive_body)
            .create_async()
            .await;
        let provider =
            CloudSttProvider::new("sk-test", server.url(), SttLanguage::Auto, 10).unwrap();
        let audio = maekon_core::models::audio::AudioBuffer::new(vec![0.0f32; 16_000]);

        let err = provider.transcribe(audio).await.unwrap_err().to_string();
        assert!(
            !err.contains("alice@example.com"),
            "CoreError display must not preserve provider-echoed email: {err}"
        );
        assert!(
            !err.contains("sk-live-secret"),
            "CoreError display must not preserve provider token-like text: {err}"
        );
        assert!(
            !err.contains("010-1234-5678"),
            "CoreError display must not preserve provider-echoed phone number: {err}"
        );
    }

    // --- #6989: response-body size cap (OOM guard) ---

    #[tokio::test]
    async fn read_stt_body_capped_accepts_body_within_cap() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/")
            .with_status(200)
            .with_body("0123456789") // 10 bytes
            .create_async()
            .await;
        let resp = reqwest::Client::new()
            .get(server.url())
            .send()
            .await
            .expect("mock request must succeed");

        let bytes = read_stt_body_capped(resp, 1024)
            .await
            .expect("body within cap must read");
        assert_eq!(
            bytes, b"0123456789",
            "capped read must return the full body"
        );
    }

    #[tokio::test]
    async fn read_stt_body_capped_rejects_oversized_body() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/")
            .with_status(200)
            .with_body("x".repeat(100)) // 100 bytes, well over the 10-byte test cap
            .create_async()
            .await;
        let resp = reqwest::Client::new()
            .get(server.url())
            .send()
            .await
            .expect("mock request must succeed");

        // A hostile endpoint streaming more than the cap must be rejected, not buffered.
        let err = read_stt_body_capped(resp, 10)
            .await
            .expect_err("body over cap must error rather than OOM-buffer");
        assert!(
            matches!(err, CoreError::SpeechToText { .. }),
            "over-cap body → SpeechToText/SttFailed, got: {err:?}"
        );
    }

    // --- #7724: Content-Length early-reject ---
    //
    // A genuinely protocol-inconsistent response (declared `Content-Length`
    // larger than the bytes actually sent) cannot be constructed through a real
    // HTTP round-trip: mockito's hyper-backed server refuses to serve a
    // manually-set `Content-Length` header that disagrees with the actual body
    // ("payload claims content-length of N, custom content-length header claims
    // M" — a server-side panic), and reqwest's own `Response::content_length()`
    // is computed from the same decoded-body size hint the chunk loop reads
    // from (confirmed empirically: a HEAD response reports `content_length() ==
    // Some(0)`, not the value of its `Content-Length` header). So this fix's
    // value is (a) matching the established sibling pattern
    // (`maekon_network::outbound::read_body_capped`,
    // `updater::install::download::read_body_capped_update`) and (b) failing
    // fast on an honestly-oversized declared length instead of buffering up to
    // `max_bytes` before the per-chunk check trips — not a behavior that can be
    // proven "impossible before, possible after" via a real HTTP mock. The pure
    // predicate below is therefore the direct fails-before/passes-after guard
    // for the new logic (it did not exist before this PR); the existing
    // `read_stt_body_capped_accepts_body_within_cap` /
    // `read_stt_body_capped_rejects_oversized_body` integration tests confirm
    // the refactor did not change end-to-end behavior for either path.

    #[test]
    fn declared_content_length_exceeds_cap_pure_predicate() {
        assert!(
            declared_content_length_exceeds_cap(Some(100), 10),
            "a declared length over the cap must be flagged"
        );
        assert!(
            !declared_content_length_exceeds_cap(Some(5), 10),
            "a declared length within the cap must not be flagged"
        );
        assert!(
            !declared_content_length_exceeds_cap(None, 10),
            "an absent Content-Length must fall through to the per-chunk check, not be flagged here"
        );
    }

    // --- #7724: redirect-following hardening on the STT client (same threat
    // class as maekon-network::outbound's #6892 hardened builder) ---

    /// Fails-before regression guard: without `.redirect(Policy::none())` on
    /// `build_stt_client`, this client would follow the 302 to `/leaked` and the
    /// `leaked` mock's `expect(0)` would fail the test.
    #[tokio::test]
    async fn stt_client_does_not_follow_redirects() {
        let mut server = mockito::Server::new_async().await;
        let start = server
            .mock("GET", "/start")
            .with_status(302)
            .with_header("location", "/leaked")
            .create_async()
            .await;
        let leaked = server
            .mock("GET", "/leaked")
            .with_status(200)
            .with_body("LEAKED")
            .expect(0)
            .create_async()
            .await;

        let client = build_stt_client(10).expect("STT client must build");
        let resp = client
            .get(format!("{}/start", server.url()))
            .send()
            .await
            .expect("request must send");

        assert_eq!(
            resp.status().as_u16(),
            302,
            "302 must not be followed and must be returned as-is"
        );
        start.assert_async().await;
        leaked.assert_async().await; // expect(0): verifies /leaked was never called
    }

    // --- #7098: source PCM is wiped after encoding the multipart upload body ---

    #[test]
    fn encode_wav_and_wipe_source_encodes_then_zeroizes_pcm() {
        // Distinctive non-zero PCM so we can prove the WAV was encoded from the real
        // samples BEFORE the source was wiped (encode-then-wipe ordering).
        let mut audio = AudioBuffer::new(vec![0.5f32; 64]);
        let wav = encode_wav_and_wipe_source(&mut audio);
        // 44-byte WAV header + 64 samples * 2 bytes (PCM16) = 172 bytes.
        assert_eq!(
            wav.len(),
            44 + 64 * 2,
            "WAV body must contain the full encoded PCM"
        );
        assert_eq!(&wav[..4], b"RIFF", "body must be a real WAV container");
        // The data sub-chunk must be non-zero — proves the body was encoded from the
        // original 0.5 samples (≈16383 PCM16), not from post-wipe zeros.
        assert!(
            wav[44..].iter().any(|&b| b != 0),
            "WAV PCM data must come from the original samples, not post-wipe zeros"
        );
        // The source PCM we own must be overwritten (zeroize truncates to len 0).
        assert!(
            audio.samples.is_empty(),
            "source PCM samples must be zeroized after WAV encoding (#7098)"
        );
    }
}
