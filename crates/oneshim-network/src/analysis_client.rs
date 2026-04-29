use async_trait::async_trait;
use oneshim_core::config::{AiProviderType, ExternalApiEndpoint, PiiFilterLevel};
use oneshim_core::error::CoreError;
use oneshim_core::models::suggestion::{Priority, Suggestion, SuggestionSource, SuggestionType};
use oneshim_core::ports::analysis_provider::AnalysisProvider;
use oneshim_core::ports::pii_sanitizer::PiiSanitizer;
use serde::Deserialize;
use std::sync::Arc;
use tracing::{debug, warn};

use crate::circuit_breaker::{CircuitBreaker, CircuitBreakerRegistry, CircuitState};
use crate::error::NetworkError;
use crate::resilience::{classify_for_breaker, endpoint_authority, BreakerSignal};

/// Adapter implementing `AnalysisProvider` by calling a remote LLM API.
/// Reuses the same multi-provider HTTP pattern as `RemoteLlmProvider`.
///
/// D7: Guarded by a per-endpoint `CircuitBreaker` shared across both
/// funnels (`analyze` + `summarize_text`) — a persistent outage at the
/// analysis endpoint fast-fails either call in microseconds.
pub struct AnalysisClient {
    http_client: reqwest::Client,
    endpoint: String,
    api_key: String,
    model: String,
    provider_type: AiProviderType,
    timeout_secs: u64,
    breaker: Arc<CircuitBreaker>,
    /// D5 iter-5: sanitize LLM-returned suggestion text before it leaves this
    /// client. LLMs can echo back user-context PII (e.g., "your email
    /// user@example.com..."). Apply at the candidate_to_suggestion exit.
    pii_sanitizer: Option<Arc<dyn PiiSanitizer>>,
    pii_level: PiiFilterLevel,
}

/// Private struct for parsing LLM suggestion candidates from JSON.
#[derive(Debug, Deserialize)]
struct SuggestionCandidate {
    #[serde(rename = "type")]
    suggestion_type: String,
    content: String,
    confidence: f64,
    #[serde(default)]
    reasoning: Option<String>,
}

impl AnalysisClient {
    pub fn new(
        config: &ExternalApiEndpoint,
        breaker_registry: Arc<CircuitBreakerRegistry>,
    ) -> Self {
        if !matches!(config.provider_type, AiProviderType::Ollama) && config.api_key.is_empty() {
            warn!(
                "AnalysisClient: empty API key for {:?} provider",
                config.provider_type
            );
        }

        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()
            .unwrap_or_default();

        // D7: resolve per-endpoint breaker.
        let breaker_key = endpoint_authority(&config.endpoint)
            .unwrap_or_else(|_| format!("malformed::{}", config.endpoint));
        let breaker = breaker_registry.get(&breaker_key);

        Self {
            http_client,
            endpoint: config.endpoint.clone(),
            api_key: config.api_key.clone(),
            model: config.model.clone().unwrap_or_else(|| {
                crate::default_model_for_provider(&config.provider_type).to_string()
            }),
            provider_type: config.provider_type,
            timeout_secs: config.timeout_secs,
            breaker,
            pii_sanitizer: None,
            pii_level: PiiFilterLevel::Standard,
        }
    }

    /// D5 iter-5: attach a PII sanitizer for suggestion-text sanitization.
    pub fn with_pii_sanitizer(
        mut self,
        sanitizer: Arc<dyn PiiSanitizer>,
        level: PiiFilterLevel,
    ) -> Self {
        self.pii_sanitizer = Some(sanitizer);
        self.pii_level = level;
        self
    }

    /// D5 iter-5: helper to sanitize a text fragment via the injected port.
    fn sanitize(&self, text: &str) -> String {
        self.pii_sanitizer
            .as_ref()
            .map(|s| s.sanitize_text(text, self.pii_level))
            .unwrap_or_else(|| text.to_string())
    }

    /// D7 helper: returns Err if breaker is Open, allowing the caller to
    /// short-circuit before constructing a request.
    fn check_breaker(&self) -> Result<(), CoreError> {
        if matches!(self.breaker.check(), CircuitState::Open { .. }) {
            Err(CoreError::ServiceUnavailable {
                code: oneshim_core::error_codes::ServiceCode::CircuitOpen,
                message: format!("circuit open for {}", self.endpoint),
            })
        } else {
            Ok(())
        }
    }

    /// D7 helper: record the breaker outcome based on initial HTTP send result.
    /// Called immediately after `.send().await`, before error-mapping, so the
    /// breaker sees the raw transport + status signal.
    fn record_breaker_outcome(&self, send_result: &Result<reqwest::Response, reqwest::Error>) {
        let signal = match send_result {
            Ok(resp) => classify_for_breaker(Some(resp.status().as_u16()), false),
            Err(_) => classify_for_breaker(None, true),
        };
        match signal {
            BreakerSignal::Success => self.breaker.record_success(),
            BreakerSignal::Failure => self.breaker.record_failure(),
            BreakerSignal::Neutral => {}
        }
    }

    /// Build provider-specific request body.
    fn build_request_body(&self, context_json: &str, system_prompt: &str) -> serde_json::Value {
        match self.provider_type {
            AiProviderType::Anthropic => {
                serde_json::json!({
                    "model": self.model,
                    "max_tokens": 1024,
                    "system": system_prompt,
                    "messages": [{"role": "user", "content": context_json}]
                })
            }
            AiProviderType::OpenAi
            | AiProviderType::Google
            | AiProviderType::Ollama
            | AiProviderType::Bedrock
            | AiProviderType::Copilot
            | AiProviderType::Generic => {
                serde_json::json!({
                    "model": self.model,
                    "messages": [
                        {"role": "system", "content": system_prompt},
                        {"role": "user", "content": context_json}
                    ],
                    "max_tokens": 1024,
                    "temperature": 0.3
                })
            }
        }
    }

    /// Extract text content from provider-specific response format.
    fn extract_text(&self, body: &serde_json::Value) -> Result<String, NetworkError> {
        match self.provider_type {
            AiProviderType::Anthropic => body
                .get("content")
                .and_then(|c| c.as_array())
                .and_then(|arr| arr.first())
                .and_then(|block| block.get("text"))
                .and_then(|t| t.as_str())
                .map(|s| s.to_string())
                .ok_or_else(|| NetworkError::Analysis("No text in Anthropic response".to_string())),
            _ => {
                // OpenAI / Generic / Ollama: choices[0].message.content
                body.get("choices")
                    .and_then(|c| c.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|choice| choice.get("message"))
                    .and_then(|msg| msg.get("content"))
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string())
                    .ok_or_else(|| {
                        NetworkError::Analysis("No text in OpenAI/Generic response".to_string())
                    })
            }
        }
    }

    /// Parse candidates from JSON text extracted from LLM response.
    fn parse_candidates(text: &str) -> Result<Vec<SuggestionCandidate>, NetworkError> {
        // Strip markdown fences if present
        let trimmed = text.trim();
        let json_str = if trimmed.starts_with("```") {
            let inner = trimmed
                .strip_prefix("```json")
                .or_else(|| trimmed.strip_prefix("```"))
                .unwrap_or(trimmed);
            inner.strip_suffix("```").unwrap_or(inner).trim()
        } else {
            trimmed
        };

        // Find the JSON array in the text
        let start = json_str.find('[').ok_or_else(|| {
            NetworkError::Analysis(format!(
                "No JSON array found in LLM response: {}",
                json_str.chars().take(200).collect::<String>()
            ))
        })?;
        let end = json_str.rfind(']').ok_or_else(|| {
            NetworkError::Analysis("No closing bracket in LLM response".to_string())
        })?;

        let array_str = &json_str[start..=end];
        serde_json::from_str(array_str).map_err(|e| {
            NetworkError::Analysis(format!(
                "Failed to parse suggestion candidates: {} (raw: {})",
                e,
                array_str.chars().take(200).collect::<String>()
            ))
        })
    }

    /// Convert a parsed candidate into a domain `Suggestion`.
    /// Stateless associated fn — kept associated so existing tests can call
    /// `AnalysisClient::candidate_to_suggestion(c)` without constructing an instance.
    /// D5 iter-5: sanitization happens in the caller (`candidate_to_suggestion_sanitized`)
    /// because the sanitizer is instance state.
    fn candidate_to_suggestion(candidate: SuggestionCandidate) -> Suggestion {
        let suggestion_type = match candidate.suggestion_type.as_str() {
            "ProductivityTip" => SuggestionType::ProductivityTip,
            "WorkflowOptimization" => SuggestionType::WorkflowOptimization,
            "ContextBased" => SuggestionType::ContextBased,
            "WorkGuidance" => SuggestionType::WorkGuidance,
            _ => SuggestionType::ContextBased,
        };

        let priority = if candidate.confidence >= 0.9 {
            Priority::High
        } else if candidate.confidence >= 0.7 {
            Priority::Medium
        } else {
            Priority::Low
        };

        Suggestion {
            suggestion_id: uuid::Uuid::new_v4().to_string(),
            suggestion_type,
            content: candidate.content,
            priority,
            confidence_score: candidate.confidence,
            relevance_score: candidate.confidence,
            is_actionable: true,
            created_at: chrono::Utc::now(),
            expires_at: None,
            source: SuggestionSource::LlmLocal,
            reasoning: candidate.reasoning,
        }
    }

    /// D5 iter-5: sanitized variant. Apply `PiiSanitizer` to `content` and
    /// `reasoning` fields before returning the Suggestion.
    fn candidate_to_suggestion_sanitized(&self, candidate: SuggestionCandidate) -> Suggestion {
        let mut s = Self::candidate_to_suggestion(candidate);
        s.content = self.sanitize(&s.content);
        s.reasoning = s.reasoning.map(|r| self.sanitize(&r));
        s
    }
}

#[async_trait]
impl AnalysisProvider for AnalysisClient {
    async fn analyze(
        &self,
        context_json: &str,
        system_prompt: &str,
    ) -> Result<Vec<Suggestion>, CoreError> {
        // D7: pre-flight breaker check.
        self.check_breaker()?;

        debug!(
            endpoint = %self.endpoint,
            model = %self.model,
            provider = ?self.provider_type,
            timeout = self.timeout_secs,
            "Calling analysis LLM API"
        );

        // ADR-019 §3: Bedrock is intentionally unsupported. Reject before attempting
        // to build/send a request with incompatible format + auth.
        if matches!(self.provider_type, AiProviderType::Bedrock) {
            return Err(CoreError::Config {
                code: oneshim_core::error_codes::ConfigCode::UnsupportedProviderBedrock,
                message: "AWS Bedrock is intentionally unsupported in this build".into(),
            });
        }

        let body = self.build_request_body(context_json, system_prompt);

        let mut builder = self
            .http_client
            .post(&self.endpoint)
            .header("Content-Type", "application/json")
            .json(&body);

        // Set auth headers based on provider type
        match self.provider_type {
            AiProviderType::Anthropic => {
                builder = builder
                    .header("x-api-key", &self.api_key)
                    .header("anthropic-version", crate::ANTHROPIC_API_VERSION);
            }
            _ => {
                if !self.api_key.is_empty() {
                    builder = builder.header("Authorization", format!("Bearer {}", self.api_key));
                }
            }
        }

        let send_result = builder.send().await;
        // D7: record breaker outcome based on initial send result.
        self.record_breaker_outcome(&send_result);
        let response = send_result.map_err(|e| {
            // Iter-90: route timeouts through NetworkError::Timeout so wire
            // code is network.timeout, not provider.analysis_failed.
            if e.is_timeout() {
                NetworkError::Timeout {
                    timeout_ms: self.timeout_secs * 1000,
                }
            } else {
                NetworkError::Analysis(format!("Analysis API request failed: {}", e))
            }
        })?;

        let status = response.status();
        let response_text = response.text().await.map_err(|e| {
            if e.is_timeout() {
                NetworkError::Timeout {
                    timeout_ms: self.timeout_secs * 1000,
                }
            } else {
                NetworkError::Analysis(format!("Failed to read analysis response: {}", e))
            }
        })?;

        if !status.is_success() {
            warn!(status = %status, "Analysis API error response");
            let message = format!(
                "Analysis API error ({}): {}",
                status,
                response_text.chars().take(200).collect::<String>()
            );
            // Semantic HTTP status mapping per iter-54..59 pattern via
            // NetworkError's existing typed variants.
            let net_err = match status.as_u16() {
                401 | 403 => NetworkError::Auth(message),
                408 | 504 => NetworkError::Timeout { timeout_ms: 0 },
                429 => NetworkError::RateLimited {
                    retry_after_secs: 60,
                },
                502 | 503 => NetworkError::ServiceUnavailable(message),
                _ => NetworkError::Analysis(message),
            };
            return Err(net_err.into());
        }

        let response_json: serde_json::Value = serde_json::from_str(&response_text)
            .map_err(|e| NetworkError::Analysis(format!("Invalid JSON response: {}", e)))?;

        let text = self.extract_text(&response_json)?;
        let candidates = Self::parse_candidates(&text)?;

        debug!(
            candidate_count = candidates.len(),
            "Parsed suggestion candidates"
        );

        let suggestions: Vec<Suggestion> = candidates
            .into_iter()
            .map(|c| self.candidate_to_suggestion_sanitized(c))
            .collect();

        Ok(suggestions)
    }

    /// Efficient single-completion call that returns raw text without JSON parsing.
    async fn summarize_text(
        &self,
        context_json: &str,
        system_prompt: &str,
    ) -> Result<String, CoreError> {
        // D7: pre-flight breaker check.
        self.check_breaker()?;

        debug!(
            endpoint = %self.endpoint,
            model = %self.model,
            provider = ?self.provider_type,
            "Calling summarize_text LLM API"
        );

        let body = self.build_request_body(context_json, system_prompt);

        let mut builder = self
            .http_client
            .post(&self.endpoint)
            .header("Content-Type", "application/json")
            .json(&body);

        match self.provider_type {
            AiProviderType::Anthropic => {
                builder = builder
                    .header("x-api-key", &self.api_key)
                    .header("anthropic-version", crate::ANTHROPIC_API_VERSION);
            }
            _ => {
                if !self.api_key.is_empty() {
                    builder = builder.header("Authorization", format!("Bearer {}", self.api_key));
                }
            }
        }

        let send_result = builder.send().await;
        // D7: record breaker outcome based on initial send result.
        self.record_breaker_outcome(&send_result);
        let response = send_result.map_err(|e| {
            // Iter-90: route timeouts through NetworkError::Timeout so wire
            // code is network.timeout, not provider.analysis_failed.
            if e.is_timeout() {
                NetworkError::Timeout {
                    timeout_ms: self.timeout_secs * 1000,
                }
            } else {
                NetworkError::Analysis(format!("Summarize API request failed: {}", e))
            }
        })?;

        let status = response.status();
        let response_text = response.text().await.map_err(|e| {
            if e.is_timeout() {
                NetworkError::Timeout {
                    timeout_ms: self.timeout_secs * 1000,
                }
            } else {
                NetworkError::Analysis(format!("Failed to read summary response: {}", e))
            }
        })?;

        if !status.is_success() {
            warn!(status = %status, "Summarize API error response");
            let message = format!(
                "Summarize API error ({}): {}",
                status,
                response_text.chars().take(200).collect::<String>()
            );
            // Semantic HTTP status mapping per iter-54..59.
            let net_err = match status.as_u16() {
                401 | 403 => NetworkError::Auth(message),
                408 | 504 => NetworkError::Timeout { timeout_ms: 0 },
                429 => NetworkError::RateLimited {
                    retry_after_secs: 60,
                },
                502 | 503 => NetworkError::ServiceUnavailable(message),
                _ => NetworkError::Analysis(message),
            };
            return Err(net_err.into());
        }

        let response_json: serde_json::Value = serde_json::from_str(&response_text)
            .map_err(|e| NetworkError::Analysis(format!("Invalid JSON response: {}", e)))?;

        let text = self.extract_text(&response_json)?;

        if text.trim().is_empty() {
            return Err(NetworkError::Analysis("Empty summary response".into()).into());
        }

        Ok(text.trim().to_string())
    }

    fn provider_name(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_json_response() {
        let text = r#"[
            {"type": "ProductivityTip", "content": "Take a break", "confidence": 0.85, "reasoning": "Long session"},
            {"type": "WorkflowOptimization", "content": "Batch emails", "confidence": 0.72}
        ]"#;

        let candidates = AnalysisClient::parse_candidates(text).unwrap();
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].suggestion_type, "ProductivityTip");
        assert_eq!(candidates[0].content, "Take a break");
        assert!((candidates[0].confidence - 0.85).abs() < f64::EPSILON);
        assert_eq!(candidates[0].reasoning.as_deref(), Some("Long session"));
        assert!(candidates[1].reasoning.is_none());
    }

    #[test]
    fn parse_malformed_response_returns_error() {
        let text = "This is not JSON at all";
        let result = AnalysisClient::parse_candidates(text);
        assert!(result.is_err());
        match result.unwrap_err() {
            NetworkError::Analysis(msg) => {
                assert!(msg.contains("No JSON array found"));
            }
            other => panic!("Expected NetworkError::Analysis, got: {:?}", other),
        }
    }

    #[test]
    fn parse_empty_array_returns_empty_vec() {
        let text = "[]";
        let candidates = AnalysisClient::parse_candidates(text).unwrap();
        assert!(candidates.is_empty());
    }

    #[test]
    fn parse_json_with_markdown_fences() {
        let text = r#"```json
[{"type": "ContextBased", "content": "Focus on current task", "confidence": 0.9}]
```"#;
        let candidates = AnalysisClient::parse_candidates(text).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].suggestion_type, "ContextBased");
    }

    #[test]
    fn candidate_to_suggestion_maps_fields() {
        let candidate = SuggestionCandidate {
            suggestion_type: "ProductivityTip".to_string(),
            content: "Take a break after 90 minutes".to_string(),
            confidence: 0.85,
            reasoning: Some("Extended focus session detected".to_string()),
        };

        let suggestion = AnalysisClient::candidate_to_suggestion(candidate);
        assert_eq!(suggestion.suggestion_type, SuggestionType::ProductivityTip);
        assert_eq!(suggestion.content, "Take a break after 90 minutes");
        assert!((suggestion.confidence_score - 0.85).abs() < f64::EPSILON);
        assert_eq!(suggestion.source, SuggestionSource::LlmLocal);
        assert_eq!(suggestion.priority, Priority::Medium);
        assert!(suggestion.reasoning.is_some());
    }

    #[test]
    fn candidate_high_confidence_gets_high_priority() {
        let candidate = SuggestionCandidate {
            suggestion_type: "WorkGuidance".to_string(),
            content: "Critical".to_string(),
            confidence: 0.95,
            reasoning: None,
        };
        let suggestion = AnalysisClient::candidate_to_suggestion(candidate);
        assert_eq!(suggestion.priority, Priority::High);
    }

    #[test]
    fn candidate_low_confidence_gets_low_priority() {
        let candidate = SuggestionCandidate {
            suggestion_type: "ContextBased".to_string(),
            content: "Maybe try this".to_string(),
            confidence: 0.62,
            reasoning: None,
        };
        let suggestion = AnalysisClient::candidate_to_suggestion(candidate);
        assert_eq!(suggestion.priority, Priority::Low);
    }

    #[test]
    fn unknown_suggestion_type_defaults_to_context_based() {
        let candidate = SuggestionCandidate {
            suggestion_type: "UnknownType".to_string(),
            content: "test".to_string(),
            confidence: 0.7,
            reasoning: None,
        };
        let suggestion = AnalysisClient::candidate_to_suggestion(candidate);
        assert_eq!(suggestion.suggestion_type, SuggestionType::ContextBased);
    }

    #[test]
    fn build_anthropic_request_body() {
        let config = ExternalApiEndpoint {
            endpoint: "https://api.anthropic.com/v1/messages".to_string(),
            api_key: "test-key".to_string(),
            model: Some("claude-sonnet-4-20250514".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::Anthropic,
            surface_id: None,
            credential: None,
        };
        let client = AnalysisClient::new(&config, CircuitBreakerRegistry::new());
        let body = client.build_request_body("ctx", "sys");

        assert_eq!(body["model"], "claude-sonnet-4-20250514");
        assert_eq!(body["system"], "sys");
        assert_eq!(body["max_tokens"], 1024);
        assert!(body["messages"].is_array());
    }

    #[test]
    fn build_openai_request_body() {
        let config = ExternalApiEndpoint {
            endpoint: "https://api.openai.com/v1/chat/completions".to_string(),
            api_key: "test-key".to_string(),
            model: Some("gpt-5.4".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::OpenAi,
            surface_id: None,
            credential: None,
        };
        let client = AnalysisClient::new(&config, CircuitBreakerRegistry::new());
        let body = client.build_request_body("ctx", "sys");

        assert_eq!(body["model"], "gpt-5.4");
        assert_eq!(body["temperature"], 0.3);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "user");
    }

    #[test]
    fn extract_text_anthropic_format() {
        let config = ExternalApiEndpoint {
            endpoint: "https://api.anthropic.com/v1/messages".to_string(),
            api_key: "key".to_string(),
            model: None,
            timeout_secs: 30,
            provider_type: AiProviderType::Anthropic,
            surface_id: None,
            credential: None,
        };
        let client = AnalysisClient::new(&config, CircuitBreakerRegistry::new());
        let body = serde_json::json!({
            "content": [{"type": "text", "text": "[{\"type\": \"ProductivityTip\", \"content\": \"test\", \"confidence\": 0.8}]"}]
        });
        let text = client.extract_text(&body).unwrap();
        assert!(text.contains("ProductivityTip"));
    }

    #[test]
    fn extract_text_openai_format() {
        let config = ExternalApiEndpoint {
            endpoint: "https://api.openai.com/v1/chat/completions".to_string(),
            api_key: "key".to_string(),
            model: None,
            timeout_secs: 30,
            provider_type: AiProviderType::OpenAi,
            surface_id: None,
            credential: None,
        };
        let client = AnalysisClient::new(&config, CircuitBreakerRegistry::new());
        let body = serde_json::json!({
            "choices": [{"message": {"content": "[]"}}]
        });
        let text = client.extract_text(&body).unwrap();
        assert_eq!(text, "[]");
    }

    #[test]
    fn extract_text_missing_content_returns_error() {
        let config = ExternalApiEndpoint {
            endpoint: "https://api.openai.com/v1/chat/completions".to_string(),
            api_key: "key".to_string(),
            model: None,
            timeout_secs: 30,
            provider_type: AiProviderType::OpenAi,
            surface_id: None,
            credential: None,
        };
        let client = AnalysisClient::new(&config, CircuitBreakerRegistry::new());
        let body = serde_json::json!({"choices": []});
        assert!(client.extract_text(&body).is_err());
    }

    /// ADR-019 §3 regression guard: `analyze()` must reject Bedrock before
    /// attempting the HTTP call. A regression that removes the guard would
    /// silently send OpenAI-format payloads to whatever endpoint the user
    /// configured, rather than returning the typed UnsupportedProviderBedrock
    /// code that telemetry/i18n depend on.
    #[tokio::test]
    async fn analyze_rejects_bedrock_provider() {
        let config = ExternalApiEndpoint {
            endpoint: "https://bedrock-runtime.us-east-1.amazonaws.com".to_string(),
            api_key: String::new(),
            model: Some("anthropic.claude-3-5-sonnet".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::Bedrock,
            surface_id: None,
            credential: None,
        };
        let client = AnalysisClient::new(&config, CircuitBreakerRegistry::new());
        let result = client.analyze("{}", "you are a test").await;
        match result {
            Err(CoreError::Config { code, message }) => {
                assert_eq!(
                    code,
                    oneshim_core::error_codes::ConfigCode::UnsupportedProviderBedrock,
                    "expected UnsupportedProviderBedrock code, got {code:?}"
                );
                assert!(
                    message.contains("Bedrock"),
                    "expected Bedrock-mentioning message, got {message:?}"
                );
            }
            other => panic!(
                "expected CoreError::Config {{ UnsupportedProviderBedrock, .. }}, got {other:?}"
            ),
        }
    }

    // iter-71 regression guards for iter-59b semantic HTTP status mapping
    // in analysis_client.rs::analyze. Shared helper pattern matches
    // iter-67..70.
    async fn run_analyze_status_test(status: u16) -> CoreError {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(status as usize)
            .with_body(format!("http {status}"))
            .create_async()
            .await;
        let config = ExternalApiEndpoint {
            endpoint: server.url(),
            api_key: "test-key".to_string(),
            model: Some("gpt-5.4".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::OpenAi,
            surface_id: None,
            credential: None,
        };
        let client = AnalysisClient::new(&config, CircuitBreakerRegistry::new());
        client.analyze("{}", "sys").await.unwrap_err()
    }

    #[tokio::test]
    async fn analyze_403_maps_to_auth() {
        let err = run_analyze_status_test(403).await;
        assert!(
            matches!(err, CoreError::Auth { .. }),
            "403 → Auth, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn analyze_408_maps_to_timeout() {
        let err = run_analyze_status_test(408).await;
        assert!(
            matches!(err, CoreError::RequestTimeout { .. }),
            "408 → RequestTimeout, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn analyze_429_maps_to_rate_limit() {
        let err = run_analyze_status_test(429).await;
        assert!(
            matches!(err, CoreError::RateLimit { .. }),
            "429 → RateLimit, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn analyze_502_maps_to_service_unavailable() {
        let err = run_analyze_status_test(502).await;
        assert!(
            matches!(err, CoreError::ServiceUnavailable { .. }),
            "502 → ServiceUnavailable, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn analyze_504_maps_to_timeout() {
        let err = run_analyze_status_test(504).await;
        assert!(
            matches!(err, CoreError::RequestTimeout { .. }),
            "504 → RequestTimeout, got: {err:?}"
        );
    }

    // iter-74: regression guards for summarize_text sibling of analyze.
    // iter-59b applied the same semantic HTTP status mapping to both.
    async fn run_summarize_status_test(status: u16) -> CoreError {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(status as usize)
            .with_body(format!("http {status}"))
            .create_async()
            .await;
        let config = ExternalApiEndpoint {
            endpoint: server.url(),
            api_key: "test-key".to_string(),
            model: Some("gpt-5.4".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::OpenAi,
            surface_id: None,
            credential: None,
        };
        let client = AnalysisClient::new(&config, CircuitBreakerRegistry::new());
        client.summarize_text("{}", "sys").await.unwrap_err()
    }

    #[tokio::test]
    async fn summarize_403_maps_to_auth() {
        let err = run_summarize_status_test(403).await;
        assert!(matches!(err, CoreError::Auth { .. }));
    }

    #[tokio::test]
    async fn summarize_429_maps_to_rate_limit() {
        let err = run_summarize_status_test(429).await;
        assert!(matches!(err, CoreError::RateLimit { .. }));
    }

    #[tokio::test]
    async fn summarize_503_maps_to_service_unavailable() {
        let err = run_summarize_status_test(503).await;
        assert!(matches!(err, CoreError::ServiceUnavailable { .. }));
    }

    /// iter-77: domain fallback regression guard. analyze() falls back to
    /// CoreError::Analysis (via NetworkError::Analysis) for unmapped
    /// status codes, not to Network::Generic. Mirrors cloud_stt / OCR
    /// fallback tests (iter-72, iter-77).
    #[tokio::test]
    async fn analyze_500_falls_back_to_analysis_error() {
        let err = run_analyze_status_test(500).await;
        assert!(
            matches!(err, CoreError::Analysis { .. }),
            "500 should fall back to CoreError::Analysis (domain-specific), got: {err:?}"
        );
    }

    /// iter-79: matching fallback guard for summarize_text (iter-74 sibling).
    /// Same mapping as analyze, but dispatched from a different method —
    /// test separately so a regression in summarize's error flow is caught
    /// even if analyze's tests still pass.
    #[tokio::test]
    async fn summarize_500_falls_back_to_analysis_error() {
        let err = run_summarize_status_test(500).await;
        assert!(
            matches!(err, CoreError::Analysis { .. }),
            "500 should fall back to CoreError::Analysis, got: {err:?}"
        );
    }

    // ── D7 Circuit breaker behavior ───────────────────────────────────────

    fn breaker_registry_with_fast_config(server_url: &str) -> Arc<CircuitBreakerRegistry> {
        let registry = CircuitBreakerRegistry::new();
        let key = endpoint_authority(server_url).unwrap();
        let _ = registry.get_with_config(
            &key,
            crate::circuit_breaker::CircuitBreakerConfig {
                failure_threshold: 3,
                initial_cooldown: std::time::Duration::from_millis(50),
                max_cooldown: std::time::Duration::from_millis(200),
                half_open_probes: 1,
            },
        );
        registry
    }

    fn make_analysis_client(
        server_url: &str,
        registry: Arc<CircuitBreakerRegistry>,
    ) -> AnalysisClient {
        let config = ExternalApiEndpoint {
            endpoint: server_url.to_string(),
            api_key: "test-key".to_string(),
            model: Some("gpt-5.4".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::OpenAi,
            surface_id: None,
            credential: None,
        };
        AnalysisClient::new(&config, registry)
    }

    #[tokio::test]
    async fn breaker_closed_passthrough_analyze() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(
                serde_json::json!({
                    "choices": [{"message": {"content": "[]"}}]
                })
                .to_string(),
            )
            .create_async()
            .await;

        let registry = breaker_registry_with_fast_config(&server.url());
        let client = make_analysis_client(&server.url(), registry);
        let result = client.analyze("{}", "sys").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn breaker_open_fast_fails_analyze() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(503)
            .with_body("down")
            .expect_at_most(3) // after 3 failures breaker is open; should NOT hit server on 4th
            .create_async()
            .await;

        let registry = breaker_registry_with_fast_config(&server.url());
        let client = make_analysis_client(&server.url(), registry);
        for _ in 0..3 {
            let _ = client.analyze("{}", "sys").await;
        }
        let result = client.analyze("{}", "sys").await;
        match result {
            Err(CoreError::ServiceUnavailable { code, .. }) => {
                assert_eq!(code, oneshim_core::error_codes::ServiceCode::CircuitOpen);
            }
            other => panic!("expected ServiceUnavailable CircuitOpen, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn breaker_open_also_blocks_summarize() {
        // Verify the SAME breaker state shared by both funnels: if analyze()
        // trips the breaker, summarize_text() is also blocked.
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(503)
            .with_body("down")
            .expect_at_most(3)
            .create_async()
            .await;

        let registry = breaker_registry_with_fast_config(&server.url());
        let client = make_analysis_client(&server.url(), registry);
        // Trip via analyze().
        for _ in 0..3 {
            let _ = client.analyze("{}", "sys").await;
        }
        // summarize_text() on the same client sees Open immediately.
        let result = client.summarize_text("{}", "sys").await;
        match result {
            Err(CoreError::ServiceUnavailable { code, .. }) => {
                assert_eq!(code, oneshim_core::error_codes::ServiceCode::CircuitOpen);
            }
            other => panic!("expected cross-funnel CircuitOpen, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn breaker_half_open_failure_doubles_cooldown_analysis() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(503)
            .with_body("down")
            .create_async()
            .await;

        let registry = breaker_registry_with_fast_config(&server.url());
        let client = make_analysis_client(&server.url(), registry.clone());
        for _ in 0..3 {
            let _ = client.analyze("{}", "sys").await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(70)).await;
        let _ = client.analyze("{}", "sys").await;

        let key = endpoint_authority(&server.url()).unwrap();
        let breaker = registry.get(&key);
        assert_eq!(
            breaker.stats().current_cooldown,
            std::time::Duration::from_millis(100)
        );
    }
}
