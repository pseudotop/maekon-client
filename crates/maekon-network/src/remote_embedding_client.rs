use async_trait::async_trait;
use maekon_core::config::PiiFilterLevel;
use maekon_core::error::CoreError;
use maekon_core::ports::credential_source::CredentialSource;
use maekon_core::ports::embedding_provider::EmbeddingProvider;
use maekon_core::ports::pii_sanitizer::PiiSanitizer;
use serde::Deserialize;
use std::sync::Arc;
use tracing::{debug, warn};

use crate::circuit_breaker::{CircuitBreaker, CircuitBreakerRegistry, CircuitState};
use crate::provider_error_body::provider_error_message;
use crate::resilience::{classify_for_breaker, endpoint_authority, BreakerSignal};

/// Remote embedding adapter using OpenAI-compatible embedding API.
///
/// Sends `POST {endpoint}` with `Authorization: Bearer {api_key}` and body
/// `{"model": model, "input": [texts]}`.  Response format:
/// `{"data": [{"embedding": [f32...]}]}`.
///
/// D7: Guarded by a per-endpoint `CircuitBreaker` resolved from a shared
/// `CircuitBreakerRegistry` — a persistent outage at the embedding endpoint
/// fast-fails in microseconds instead of blocking every request.
pub struct RemoteEmbeddingProvider {
    http_client: reqwest::Client,
    endpoint: String,
    /// Credential source resolved at request time.
    ///
    /// `new` wraps the caller-supplied `api_key` in `CredentialSource::ApiKey`
    /// so all existing call sites continue to compile and behave identically.
    /// `new_with_credential` accepts a pre-built source (e.g. `StoredSecret`)
    /// so that Settings-persisted keys reach the embedding API.
    credential: CredentialSource,
    model: String,
    dimensions: usize,
    breaker: Arc<CircuitBreaker>,
    pii_sanitizer: Option<Arc<dyn PiiSanitizer>>,
    pii_level: PiiFilterLevel,
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

impl RemoteEmbeddingProvider {
    /// Build a provider from a plain inline API key.
    ///
    /// Wraps `api_key` in `CredentialSource::ApiKey` so all existing call
    /// sites compile unchanged.  The key is sent as-is at request time —
    /// no keychain lookup occurs.  Prefer `new_with_credential` when the
    /// key has been persisted via Settings.
    pub fn new(
        endpoint: String,
        api_key: String,
        model: String,
        dimensions: usize,
        timeout_secs: u64,
        breaker_registry: Arc<CircuitBreakerRegistry>,
    ) -> Self {
        if api_key.is_empty() {
            warn!("RemoteEmbeddingProvider: empty API key for endpoint {endpoint}");
        }
        let credential = CredentialSource::ApiKey(api_key);
        Self::new_with_credential(
            endpoint,
            credential,
            model,
            dimensions,
            timeout_secs,
            breaker_registry,
        )
    }

    /// Build a provider with a pre-resolved credential source.
    ///
    /// Use this constructor when the key has been persisted in the OS keychain
    /// or another secret backend (e.g. `CredentialSource::StoredSecret`), so
    /// that the actual secret is only loaded at request time rather than being
    /// copied as a plain `String` at construction time.
    pub fn new_with_credential(
        endpoint: String,
        credential: CredentialSource,
        model: String,
        dimensions: usize,
        timeout_secs: u64,
        breaker_registry: Arc<CircuitBreakerRegistry>,
    ) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .unwrap_or_default();

        // D7: resolve per-endpoint breaker; malformed endpoint falls back to
        // a "none" key so at least the construction succeeds and runtime
        // errors surface via request-time URL parsing instead.
        let breaker_key =
            endpoint_authority(&endpoint).unwrap_or_else(|_| format!("malformed::{endpoint}"));
        let breaker = breaker_registry.get(&breaker_key);

        Self {
            http_client,
            endpoint,
            credential,
            model,
            dimensions,
            breaker,
            pii_sanitizer: None,
            pii_level: PiiFilterLevel::Standard,
        }
    }

    /// Attach the privacy sanitizer used at the final remote embedding egress boundary.
    pub fn with_pii_sanitizer(
        mut self,
        sanitizer: Arc<dyn PiiSanitizer>,
        level: PiiFilterLevel,
    ) -> Self {
        self.pii_sanitizer = Some(sanitizer);
        self.pii_level = level;
        self
    }

    fn sanitize_remote_embedding_input(&self, text: &str) -> String {
        match self.pii_sanitizer.as_ref() {
            Some(sanitizer) => sanitizer.sanitize_text(text, self.pii_level),
            None => "[REDACTED_REMOTE_EMBEDDING_INPUT]".to_string(),
        }
    }

    async fn request_embeddings(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, CoreError> {
        // D7: pre-flight circuit breaker check.
        if matches!(self.breaker.check(), CircuitState::Open { .. }) {
            return Err(CoreError::ServiceUnavailable {
                code: maekon_core::error_codes::ServiceCode::CircuitOpen,
                message: format!("circuit open for {}", self.endpoint),
            });
        }

        // Resolve the bearer token at request time so StoredSecret keys are
        // read from the keychain on each call rather than copied as a plain
        // String at construction time.
        let bearer_token = self.credential.resolve_bearer_token().await.map_err(|e| {
            warn!(
                "RemoteEmbeddingProvider: credential resolution failed (treating as auth error): {e}"
            );
            CoreError::Auth {
                code: maekon_core::error_codes::AuthCode::Failed,
                message: format!("embedding credential resolution failed: {e}"),
            }
        })?;

        let sanitized_texts: Vec<String> = texts
            .iter()
            .map(|text| self.sanitize_remote_embedding_input(text))
            .collect();

        let body = serde_json::json!({
            "model": self.model,
            "input": sanitized_texts,
        });

        debug!(
            "RemoteEmbeddingProvider: requesting {} embeddings from {}",
            texts.len(),
            self.endpoint
        );

        // Only attach Authorization header when the bearer token is non-empty.
        // For loopback Ollama endpoints using CredentialSource::NoAuth the token
        // resolves to an empty string — sending `Authorization: Bearer ` would
        // be a no-op but is hygienic to omit.
        let request = self
            .http_client
            .post(&self.endpoint)
            .header("Content-Type", "application/json")
            .json(&body);
        let request = if bearer_token.is_empty() {
            request
        } else {
            request.header("Authorization", format!("Bearer {}", bearer_token))
        };
        let send_result = request.send().await;

        // D7: classify for breaker accounting before error mapping.
        let signal = match &send_result {
            Ok(resp) => classify_for_breaker(Some(resp.status().as_u16()), false),
            Err(_) => classify_for_breaker(None, true),
        };
        match signal {
            BreakerSignal::Success => self.breaker.record_success(),
            BreakerSignal::Failure => self.breaker.record_failure(),
            BreakerSignal::Neutral => {}
        }

        let response = send_result.map_err(|e| {
            // Iter-90: split timeout vs generic per canonical pattern.
            if e.is_timeout() {
                CoreError::RequestTimeout {
                    code: maekon_core::error_codes::NetworkCode::Timeout,
                    timeout_ms: 0, // sentinel; client-level timeout is in reqwest builder
                }
            } else {
                CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    message: format!("Embedding API request failed: {e}"),
                }
            }
        })?;

        let status = response.status();
        if !status.is_success() {
            let error_body = response.text().await.ok();
            let message = provider_error_message("Embedding API", status, error_body.as_deref());
            // Semantic HTTP status mapping per iter-54/55 pattern.
            return Err(match status.as_u16() {
                401 | 403 => CoreError::Auth {
                    code: maekon_core::error_codes::AuthCode::Failed,
                    message,
                },
                408 | 504 => CoreError::RequestTimeout {
                    code: maekon_core::error_codes::NetworkCode::Timeout,
                    timeout_ms: 0,
                },
                429 => CoreError::RateLimit {
                    code: maekon_core::error_codes::NetworkCode::RateLimit,
                    retry_after_secs: 60,
                },
                502 | 503 => CoreError::ServiceUnavailable {
                    code: maekon_core::error_codes::ServiceCode::Unavailable,
                    message,
                },
                _ => CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    message,
                },
            });
        }

        let parsed: EmbeddingResponse = response.json().await.map_err(|e| CoreError::Network {
            code: maekon_core::error_codes::NetworkCode::Generic,
            message: format!("Failed to parse embedding response: {e}"),
        })?;

        let target_dims = self.dimensions;
        let embeddings: Vec<Vec<f32>> = parsed
            .data
            .into_iter()
            .map(|d| truncate_and_renormalize(d.embedding, target_dims))
            .collect();
        Ok(embeddings)
    }
}

/// Matryoshka Representation Learning (MRL) truncation with L2 re-normalisation.
///
/// When `target_dims < response.len()`: truncate to `target_dims`, then L2
/// re-normalise so the shortened vector is still a unit vector.  This is the
/// standard MRL inference procedure described in the Matryoshka paper (§3.2).
///
/// When `target_dims >= response.len()`: the response is returned as-is and a
/// warning is emitted.  The caller's `dimensions()` getter will then disagree
/// with the actual vector length — the pipeline logs should surface this quickly.
///
/// When the response vector is all-zeros after truncation (degenerate model
/// output), re-normalisation is skipped to avoid NaN/Inf.
pub(crate) fn truncate_and_renormalize(mut v: Vec<f32>, target_dims: usize) -> Vec<f32> {
    if target_dims >= v.len() {
        if target_dims > v.len() {
            warn!(
                "RemoteEmbeddingProvider: response length {} < target dims {} \
                 — returning response length as-is (dimensions() will disagree)",
                v.len(),
                target_dims
            );
        }
        return v;
    }

    // Truncate to target_dims.
    v.truncate(target_dims);

    // L2 re-normalise.
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    }
    // If norm == 0 the vector is degenerate; return the zero vector unchanged.
    v
}

#[async_trait]
impl EmbeddingProvider for RemoteEmbeddingProvider {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, CoreError> {
        let texts = vec![text.to_string()];
        let mut results = self.request_embeddings(&texts).await?;
        results.pop().ok_or_else(|| CoreError::Network {
            code: maekon_core::error_codes::NetworkCode::Generic,
            message: "Embedding API returned empty data".to_string(),
        })
    }

    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, CoreError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        self.request_embeddings(texts).await
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn model_id(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Matcher;

    struct MarkerSanitizer;

    impl PiiSanitizer for MarkerSanitizer {
        fn sanitize_text(&self, text: &str, _level: PiiFilterLevel) -> String {
            text.replace("user@example.com", "[EMAIL]")
                .replace("555-123-4567", "[PHONE]")
        }
    }

    #[tokio::test]
    async fn parse_valid_response() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "data": [
                        {"embedding": [0.1, 0.2, 0.3]},
                        {"embedding": [0.4, 0.5, 0.6]}
                    ]
                })
                .to_string(),
            )
            .create_async()
            .await;

        let provider = RemoteEmbeddingProvider::new(
            server.url(),
            "test-key".to_string(),
            "text-embedding-3-small".to_string(),
            3,
            30,
            CircuitBreakerRegistry::new(),
        );

        let result = provider
            .embed_batch(&["hello".to_string(), "world".to_string()])
            .await
            .unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0], vec![0.1, 0.2, 0.3]);
        assert_eq!(result[1], vec![0.4, 0.5, 0.6]);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn single_embed() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "data": [{"embedding": [1.0, 2.0, 3.0]}]
                })
                .to_string(),
            )
            .create_async()
            .await;

        let provider = RemoteEmbeddingProvider::new(
            server.url(),
            "test-key".to_string(),
            "text-embedding-3-small".to_string(),
            3,
            30,
            CircuitBreakerRegistry::new(),
        );

        let result = provider.embed("test text").await.unwrap();
        assert_eq!(result, vec![1.0, 2.0, 3.0]);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn remote_input_uses_configured_pii_sanitizer() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/")
            .match_body(Matcher::PartialJson(serde_json::json!({
                "input": ["call [PHONE] about [EMAIL]"]
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "data": [{"embedding": [1.0, 2.0, 3.0]}]
                })
                .to_string(),
            )
            .create_async()
            .await;

        let provider = RemoteEmbeddingProvider::new(
            server.url(),
            "test-key".to_string(),
            "text-embedding-3-small".to_string(),
            3,
            30,
            CircuitBreakerRegistry::new(),
        )
        .with_pii_sanitizer(Arc::new(MarkerSanitizer), PiiFilterLevel::Standard);

        let result = provider
            .embed("call 555-123-4567 about user@example.com")
            .await
            .unwrap();
        assert_eq!(result, vec![1.0, 2.0, 3.0]);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn missing_pii_sanitizer_redacts_remote_input_by_default() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/")
            .match_body(Matcher::PartialJson(serde_json::json!({
                "input": ["[REDACTED_REMOTE_EMBEDDING_INPUT]"]
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "data": [{"embedding": [1.0, 2.0, 3.0]}]
                })
                .to_string(),
            )
            .create_async()
            .await;

        let provider = RemoteEmbeddingProvider::new(
            server.url(),
            "test-key".to_string(),
            "text-embedding-3-small".to_string(),
            3,
            30,
            CircuitBreakerRegistry::new(),
        );

        let result = provider.embed("raw user@example.com").await.unwrap();
        assert_eq!(result, vec![1.0, 2.0, 3.0]);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn handle_api_error() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(401)
            .with_body(r#"{"error": "invalid api key"}"#)
            .create_async()
            .await;

        let provider = RemoteEmbeddingProvider::new(
            server.url(),
            "bad-key".to_string(),
            "text-embedding-3-small".to_string(),
            3,
            30,
            CircuitBreakerRegistry::new(),
        );

        let result = provider.embed("test").await;
        let err = result.unwrap_err().to_string();
        assert!(err.contains("401"));
    }

    #[tokio::test]
    async fn empty_batch_returns_empty() {
        let provider = RemoteEmbeddingProvider::new(
            "http://unused".to_string(),
            "key".to_string(),
            "model".to_string(),
            3,
            30,
            CircuitBreakerRegistry::new(),
        );

        let result = provider.embed_batch(&[]).await.unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn model_id_and_dimensions() {
        let provider = RemoteEmbeddingProvider::new(
            "http://example.com".to_string(),
            "key".to_string(),
            "text-embedding-3-small".to_string(),
            1536,
            30,
            CircuitBreakerRegistry::new(),
        );

        assert_eq!(provider.model_id(), "text-embedding-3-small");
        assert_eq!(provider.dimensions(), 1536);
    }

    /// iter-67 regression guards for iter-56a semantic HTTP status mapping.
    /// Each test asserts the typed CoreError variant for a specific status.
    async fn run_status_mapping_test(status: u16) -> CoreError {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(status as usize)
            .with_body(format!(r#"{{"error": "http {status}"}}"#))
            .create_async()
            .await;
        let provider = RemoteEmbeddingProvider::new(
            server.url(),
            "key".to_string(),
            "model".to_string(),
            3,
            30,
            CircuitBreakerRegistry::new(),
        );
        provider.embed("test").await.unwrap_err()
    }

    #[tokio::test]
    async fn status_403_maps_to_auth() {
        let err = run_status_mapping_test(403).await;
        assert!(
            matches!(err, CoreError::Auth { .. }),
            "403 → Auth, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn status_408_maps_to_timeout() {
        let err = run_status_mapping_test(408).await;
        assert!(
            matches!(err, CoreError::RequestTimeout { .. }),
            "408 → RequestTimeout, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn status_429_maps_to_rate_limit() {
        let err = run_status_mapping_test(429).await;
        assert!(
            matches!(err, CoreError::RateLimit { .. }),
            "429 → RateLimit, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn status_502_maps_to_service_unavailable() {
        let err = run_status_mapping_test(502).await;
        assert!(
            matches!(err, CoreError::ServiceUnavailable { .. }),
            "502 → ServiceUnavailable, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn status_504_maps_to_timeout() {
        let err = run_status_mapping_test(504).await;
        assert!(
            matches!(err, CoreError::RequestTimeout { .. }),
            "504 → RequestTimeout, got: {err:?}"
        );
    }

    /// iter-78: domain fallback. Unmapped statuses stay as CoreError::Network.
    #[tokio::test]
    async fn status_500_falls_back_to_network() {
        let err = run_status_mapping_test(500).await;
        assert!(
            matches!(err, CoreError::Network { .. }),
            "500 should fall back to Network, got: {err:?}"
        );
    }

    // ── D7 Circuit breaker behavior ───────────────────────────────────────

    /// Helper: construct a provider with fast-cooldown breaker config for
    /// deterministic sub-second tests.
    fn make_provider_with_fast_breaker(
        server_url: String,
        registry: Arc<CircuitBreakerRegistry>,
    ) -> RemoteEmbeddingProvider {
        // Pre-seed the breaker for this endpoint with fast-cooldown config so
        // subsequent `get()` from the constructor returns the same Arc.
        let key = endpoint_authority(&server_url).unwrap();
        let _ = registry.get_with_config(
            &key,
            crate::circuit_breaker::CircuitBreakerConfig {
                failure_threshold: 3,
                initial_cooldown: std::time::Duration::from_millis(50),
                max_cooldown: std::time::Duration::from_millis(200),
                half_open_probes: 1,
            },
        );
        RemoteEmbeddingProvider::new(
            server_url,
            "test-key".to_string(),
            "text-embedding-3-small".to_string(),
            3,
            30,
            registry,
        )
    }

    #[tokio::test]
    async fn breaker_closed_passthrough() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "data": [{"embedding": [0.1, 0.2, 0.3]}]
                })
                .to_string(),
            )
            .create_async()
            .await;

        let registry = CircuitBreakerRegistry::new();
        let provider = make_provider_with_fast_breaker(server.url(), registry);
        let embedding = provider
            .embed("hello")
            .await
            .expect("closed breaker should pass through: embed must succeed on 200");
        // Mock returns {"data": [{"embedding": [0.1, 0.2, 0.3]}]}; verify the parsed values.
        assert_eq!(
            embedding,
            vec![0.1_f32, 0.2, 0.3],
            "embedding values must match the mock response body"
        );
    }

    #[tokio::test]
    async fn breaker_open_fast_fails_with_circuit_open_code() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(503)
            .with_body("down")
            .expect_at_most(3) // after 3 failures breaker trips; should NOT hit server again
            .create_async()
            .await;

        let registry = CircuitBreakerRegistry::new();
        let provider = make_provider_with_fast_breaker(server.url(), registry);
        // Trip the breaker with 3 failures.
        for _ in 0..3 {
            let _ = provider.embed("x").await;
        }
        // Next call must fast-fail with service.circuit_open WITHOUT hitting the server.
        let result = provider.embed("y").await;
        match result {
            Err(CoreError::ServiceUnavailable { code, .. }) => {
                assert_eq!(
                    code,
                    maekon_core::error_codes::ServiceCode::CircuitOpen,
                    "expected CircuitOpen code after trip"
                );
            }
            other => panic!("expected ServiceUnavailable with CircuitOpen code, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn breaker_half_open_success_closes() {
        let mut server = mockito::Server::new_async().await;
        // First 3 requests fail (503), then flip to success.
        let _fail = server
            .mock("POST", "/")
            .with_status(503)
            .with_body("down")
            .expect(3)
            .create_async()
            .await;

        let registry = CircuitBreakerRegistry::new();
        let provider = make_provider_with_fast_breaker(server.url(), registry.clone());
        // 3 failures → Open.
        for _ in 0..3 {
            let _ = provider.embed("x").await;
        }

        // Wait past cooldown (50ms).
        tokio::time::sleep(std::time::Duration::from_millis(70)).await;

        // Now the server returns success for the probe.
        let _success = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!({"data": [{"embedding": [1.0, 2.0, 3.0]}]}).to_string())
            .create_async()
            .await;

        let probe_embedding = provider
            .embed("probe")
            .await
            .expect("half-open probe success should transition → Closed: embed must return Ok");
        // Probe response is [1.0, 2.0, 3.0]; verify the parsed values confirm the probe succeeded.
        assert_eq!(
            probe_embedding,
            vec![1.0_f32, 2.0, 3.0],
            "probe embedding values must match the mock response body"
        );

        // Subsequent call passes through as Closed; embedding values must match
        // the mock response body ([1.0, 2.0, 3.0]).
        let embedding2 = provider
            .embed("follow-up")
            .await
            .expect("follow-up call must succeed once breaker has transitioned to Closed");
        assert_eq!(
            embedding2,
            vec![1.0_f32, 2.0, 3.0],
            "embedding values must match the mock response"
        );
    }

    #[tokio::test]
    async fn breaker_half_open_failure_doubles_cooldown() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(503)
            .with_body("down")
            .create_async()
            .await;

        let registry = CircuitBreakerRegistry::new();
        let provider = make_provider_with_fast_breaker(server.url(), registry.clone());

        // 3 failures → Open with 50ms cooldown.
        for _ in 0..3 {
            let _ = provider.embed("x").await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(70)).await;
        // Half-open probe fails → back to Open with doubled (100ms) cooldown.
        let _ = provider.embed("probe").await;

        let key = endpoint_authority(&server.url()).unwrap();
        let breaker = registry.get(&key);
        assert_eq!(
            breaker.stats().current_cooldown,
            std::time::Duration::from_millis(100),
            "probe failure should double the cooldown"
        );
    }

    // ── #5736 CredentialSource migration tests ──────────────────────────────

    /// Minimal in-memory SecretStore double for tests.
    struct FixedSecretStore(String);

    #[async_trait::async_trait]
    impl maekon_core::ports::secret_store::SecretStore for FixedSecretStore {
        async fn store(
            &self,
            _namespace: &str,
            _key: &str,
            _value: &str,
        ) -> Result<(), maekon_core::error::CoreError> {
            Ok(())
        }
        async fn retrieve(
            &self,
            _namespace: &str,
            _key: &str,
        ) -> Result<Option<String>, maekon_core::error::CoreError> {
            Ok(Some(self.0.clone()))
        }
        async fn delete(
            &self,
            _namespace: &str,
            _key: &str,
        ) -> Result<(), maekon_core::error::CoreError> {
            Ok(())
        }
        async fn delete_namespace(
            &self,
            _namespace: &str,
        ) -> Result<(), maekon_core::error::CoreError> {
            Ok(())
        }
    }

    /// StoredSecret resolved key appears in Authorization header.
    ///
    /// Asserts that `new_with_credential(StoredSecret)` resolves the key from
    /// the store at request time and sends `Authorization: Bearer <stored-key>`.
    #[tokio::test]
    async fn stored_secret_credential_sent_in_authorization_header() {
        const STORED_KEY: &str = "sk-embed-secret-from-keychain-99";

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .match_header("authorization", format!("Bearer {STORED_KEY}").as_str())
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!({ "data": [{"embedding": [0.1_f32]}] }).to_string())
            .create_async()
            .await;

        let credential = CredentialSource::StoredSecret {
            namespace: "provider/openai/default".to_string(),
            key: "api_key".to_string(),
            secret_store: Arc::new(FixedSecretStore(STORED_KEY.to_string())),
        };
        let provider = RemoteEmbeddingProvider::new_with_credential(
            server.url(),
            credential,
            "text-embedding-3-small".to_string(),
            1,
            30,
            CircuitBreakerRegistry::new(),
        )
        .with_pii_sanitizer(Arc::new(MarkerSanitizer), PiiFilterLevel::Standard);

        let result = provider.embed("test").await;
        result.expect("StoredSecret credential must reach the server");
        _mock.assert_async().await;
    }

    // ── MRL truncation + L2 re-normalisation ─────────────────────────────────

    #[test]
    fn truncate_exact_target_returns_unchanged() {
        let v = vec![1.0_f32, 0.0, 0.0];
        let result = truncate_and_renormalize(v.clone(), 3);
        assert_eq!(result, v);
    }

    #[test]
    fn truncate_and_renorm_produces_unit_vector() {
        // [3.0, 4.0, 0.0, 0.0] → truncate to 2 → [3.0, 4.0] → norm 5.0 → [0.6, 0.8]
        let v = vec![3.0_f32, 4.0, 0.0, 0.0];
        let result = truncate_and_renormalize(v, 2);
        assert_eq!(result.len(), 2);
        let norm: f32 = result.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0_f32).abs() < 1e-6,
            "norm should be ≈1.0, got {norm}"
        );
        assert!((result[0] - 0.6_f32).abs() < 1e-6);
        assert!((result[1] - 0.8_f32).abs() < 1e-6);
    }

    #[test]
    fn truncate_response_shorter_than_target_returns_as_is() {
        // response shorter than target_dims — no panic, vector returned unchanged
        let v = vec![0.5_f32, 0.5];
        let result = truncate_and_renormalize(v.clone(), 10);
        assert_eq!(result, v);
    }

    #[test]
    fn truncate_zero_vector_does_not_produce_nan() {
        let v = vec![0.0_f32, 0.0, 0.0, 0.0];
        let result = truncate_and_renormalize(v, 2);
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|x| x.is_finite()));
    }

    /// NoAuth credential sends no Authorization header to the server.
    #[tokio::test]
    async fn no_auth_credential_sends_no_authorization_header() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .match_header("authorization", mockito::Matcher::Missing)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!({ "data": [{"embedding": [0.1_f32, 0.2]}] }).to_string())
            .create_async()
            .await;

        let provider = RemoteEmbeddingProvider::new_with_credential(
            server.url(),
            CredentialSource::NoAuth,
            "embeddinggemma".to_string(),
            2,
            30,
            CircuitBreakerRegistry::new(),
        )
        .with_pii_sanitizer(Arc::new(MarkerSanitizer), PiiFilterLevel::Standard);

        let result = provider.embed("loopback test").await;
        result.expect("NoAuth provider should succeed");
        _mock.assert_async().await;
    }

    /// Inline ApiKey regression: RemoteEmbeddingProvider::new still sends the key as Bearer.
    ///
    /// Verifies that the CredentialSource migration did not regress the existing
    /// inline api_key path — `new()` wraps the key in ApiKey and the Authorization
    /// header contains that key.
    #[tokio::test]
    async fn inline_api_key_sent_in_authorization_header_regression() {
        const INLINE_KEY: &str = "sk-embed-inline-abcdef-99";

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .match_header("authorization", format!("Bearer {INLINE_KEY}").as_str())
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!({ "data": [{"embedding": [0.2_f32]}] }).to_string())
            .create_async()
            .await;

        let provider = RemoteEmbeddingProvider::new(
            server.url(),
            INLINE_KEY.to_string(),
            "text-embedding-3-small".to_string(),
            1,
            30,
            CircuitBreakerRegistry::new(),
        )
        .with_pii_sanitizer(Arc::new(MarkerSanitizer), PiiFilterLevel::Standard);

        let result = provider.embed("test").await;
        result.expect("Inline ApiKey must reach the server successfully");
        _mock.assert_async().await;
    }
}
