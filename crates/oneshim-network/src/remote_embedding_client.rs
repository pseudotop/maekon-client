use async_trait::async_trait;
use oneshim_core::error::CoreError;
use oneshim_core::ports::embedding_provider::EmbeddingProvider;
use serde::Deserialize;
use std::sync::Arc;
use tracing::{debug, warn};

use crate::circuit_breaker::{CircuitBreaker, CircuitBreakerRegistry, CircuitState};
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
    api_key: String,
    model: String,
    dimensions: usize,
    breaker: Arc<CircuitBreaker>,
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
    pub fn new(
        endpoint: String,
        api_key: String,
        model: String,
        dimensions: usize,
        timeout_secs: u64,
        breaker_registry: Arc<CircuitBreakerRegistry>,
    ) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .unwrap_or_default();

        if api_key.is_empty() {
            warn!("RemoteEmbeddingProvider: empty API key for endpoint {endpoint}");
        }

        // D7: resolve per-endpoint breaker; malformed endpoint falls back to
        // a "none" key so at least the construction succeeds and runtime
        // errors surface via request-time URL parsing instead.
        let breaker_key =
            endpoint_authority(&endpoint).unwrap_or_else(|_| format!("malformed::{endpoint}"));
        let breaker = breaker_registry.get(&breaker_key);

        Self {
            http_client,
            endpoint,
            api_key,
            model,
            dimensions,
            breaker,
        }
    }

    async fn request_embeddings(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, CoreError> {
        // D7: pre-flight circuit breaker check.
        if matches!(self.breaker.check(), CircuitState::Open { .. }) {
            return Err(CoreError::ServiceUnavailable {
                code: oneshim_core::error_codes::ServiceCode::CircuitOpen,
                message: format!("circuit open for {}", self.endpoint),
            });
        }

        let body = serde_json::json!({
            "model": self.model,
            "input": texts,
        });

        debug!(
            "RemoteEmbeddingProvider: requesting {} embeddings from {}",
            texts.len(),
            self.endpoint
        );

        let send_result = self
            .http_client
            .post(&self.endpoint)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await;

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
                    code: oneshim_core::error_codes::NetworkCode::Timeout,
                    timeout_ms: 0, // sentinel; client-level timeout is in reqwest builder
                }
            } else {
                CoreError::Network {
                    code: oneshim_core::error_codes::NetworkCode::Generic,
                    message: format!("Embedding API request failed: {e}"),
                }
            }
        })?;

        let status = response.status();
        if !status.is_success() {
            let error_body = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            let message = format!("Embedding API returned {status}: {error_body}");
            // Semantic HTTP status mapping per iter-54/55 pattern.
            return Err(match status.as_u16() {
                401 | 403 => CoreError::Auth {
                    code: oneshim_core::error_codes::AuthCode::Failed,
                    message,
                },
                408 | 504 => CoreError::RequestTimeout {
                    code: oneshim_core::error_codes::NetworkCode::Timeout,
                    timeout_ms: 0,
                },
                429 => CoreError::RateLimit {
                    code: oneshim_core::error_codes::NetworkCode::RateLimit,
                    retry_after_secs: 60,
                },
                502 | 503 => CoreError::ServiceUnavailable {
                    code: oneshim_core::error_codes::ServiceCode::Unavailable,
                    message,
                },
                _ => CoreError::Network {
                    code: oneshim_core::error_codes::NetworkCode::Generic,
                    message,
                },
            });
        }

        let parsed: EmbeddingResponse = response.json().await.map_err(|e| CoreError::Network {
            code: oneshim_core::error_codes::NetworkCode::Generic,
            message: format!("Failed to parse embedding response: {e}"),
        })?;

        Ok(parsed.data.into_iter().map(|d| d.embedding).collect())
    }
}

#[async_trait]
impl EmbeddingProvider for RemoteEmbeddingProvider {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, CoreError> {
        let texts = vec![text.to_string()];
        let mut results = self.request_embeddings(&texts).await?;
        results.pop().ok_or_else(|| CoreError::Network {
            code: oneshim_core::error_codes::NetworkCode::Generic,
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
        assert!(result.is_err());
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
        let result = provider.embed("hello").await;
        assert!(result.is_ok(), "closed breaker should pass through");
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
                    oneshim_core::error_codes::ServiceCode::CircuitOpen,
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

        let result = provider.embed("probe").await;
        assert!(
            result.is_ok(),
            "half-open probe success should transition → Closed"
        );

        // Subsequent call passes through as Closed.
        let result2 = provider.embed("follow-up").await;
        assert!(result2.is_ok());
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
}
