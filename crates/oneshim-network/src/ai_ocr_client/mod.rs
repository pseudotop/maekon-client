use async_trait::async_trait;
use oneshim_api_contracts::ai_providers::ProviderModelSupportStatus;
use oneshim_api_contracts::provider_specs::{
    self, ProviderAuthScheme, ProviderRequestShape, ProviderTransportKind,
    SurfaceModelCapabilityKind,
};
use oneshim_core::ai_model_lifecycle_policy::{self, ModelLifecycleDecision};
use oneshim_core::config::{AiProviderType, ExternalApiEndpoint};
use oneshim_core::error::CoreError;
use oneshim_core::ports::credential_source::CredentialSource;
use oneshim_core::ports::ocr_provider::{OcrProvider, OcrResult};
use std::sync::Arc;
use tracing::{debug, warn};

use crate::circuit_breaker::{CircuitBreaker, CircuitBreakerRegistry, CircuitState};
use crate::resilience::{classify_for_breaker, endpoint_authority, BreakerSignal};

mod ollama;
mod parsers;
mod strategy;
#[cfg(test)]
mod tests;
pub use strategy::OcrProviderStrategy;
/// - Claude Vision (Anthropic): `POST /v1/messages` + image content block
/// - Google Cloud Vision: `POST /v1/images:annotate` + TEXT_DETECTION
///
/// D7: Guarded by a per-endpoint `CircuitBreaker` resolved from a shared
/// `CircuitBreakerRegistry`. The Ollama model-capability probe in
/// `ensure_runtime_ocr_model_ready` is intentionally NOT wrapped — it's a
/// sidecar that runs once per call; if the probe fails transiently the
/// subsequent main OCR send drives breaker state.
pub struct RemoteOcrProvider {
    http_client: reqwest::Client,
    endpoint: String,
    credential: CredentialSource,
    model: Option<String>,
    provider_type: AiProviderType,
    surface_id: Option<String>,
    #[allow(dead_code)]
    timeout_secs: u64,
    breaker: Arc<CircuitBreaker>,
}
impl std::fmt::Debug for RemoteOcrProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteOcrProvider")
            .field("endpoint", &self.endpoint)
            .field("credential", &self.credential)
            .field("model", &self.model)
            .field("provider_type", &self.provider_type)
            .field("surface_id", &self.surface_id)
            .finish()
    }
}
fn apply_auth_headers(
    auth_scheme: ProviderAuthScheme,
    builder: reqwest::RequestBuilder,
    api_key: &str,
) -> Result<reqwest::RequestBuilder, CoreError> {
    match auth_scheme {
        ProviderAuthScheme::None => Ok(builder),
        ProviderAuthScheme::XApiKey => Ok(builder
            .header("x-api-key", api_key)
            .header("anthropic-version", crate::ANTHROPIC_API_VERSION)),
        ProviderAuthScheme::XGoogApiKey => Ok(builder.header("x-goog-api-key", api_key)),
        ProviderAuthScheme::Bearer => {
            Ok(builder.header("Authorization", format!("Bearer {api_key}")))
        }
        ProviderAuthScheme::AwsSignatureV4 => Err(CoreError::Config {
            code: oneshim_core::error_codes::ConfigCode::UnsupportedProviderBedrock,
            message: "AWS Bedrock is intentionally unsupported in this build".into(),
        }),
    }
}
impl RemoteOcrProvider {
    fn ocr_request_shape(&self) -> Result<ProviderRequestShape, CoreError> {
        provider_specs::resolved_request_shape(
            self.provider_type,
            self.surface_id.as_deref(),
            ProviderTransportKind::Ocr,
        )
        .map_err(|msg| CoreError::Config {
            code: oneshim_core::error_codes::ConfigCode::Invalid,
            message: msg,
        })
    }
    fn ocr_auth_scheme(&self) -> Result<ProviderAuthScheme, CoreError> {
        provider_specs::resolved_auth_scheme(
            self.provider_type,
            self.surface_id.as_deref(),
            ProviderTransportKind::Ocr,
        )
        .map_err(|msg| CoreError::Config {
            code: oneshim_core::error_codes::ConfigCode::Invalid,
            message: msg,
        })
    }
    fn ensure_ocr_parameters_supported(&self, parameters: &[&str]) -> Result<(), CoreError> {
        provider_specs::validate_supported_parameters(
            self.provider_type,
            self.surface_id.as_deref(),
            provider_specs::SurfaceCapabilityKind::Ocr,
            parameters,
        )
        .map_err(|msg| CoreError::Config {
            code: oneshim_core::error_codes::ConfigCode::Invalid,
            message: msg,
        })
    }
    async fn ensure_runtime_ocr_model_ready(&self, model: &str) -> Result<(), CoreError> {
        if model.trim().is_empty()
            || !self.surface_id.as_deref().is_some_and(|surface_id| {
                surface_id.eq_ignore_ascii_case("provider_surface.ollama.local_http")
            })
        {
            return Ok(());
        }
        match ollama::probe_ollama_model_supports_ocr(&self.http_client, &self.endpoint, model).await { Ok(Some(true)) | Ok(None) => Ok(()), Ok(Some(false)) => Err(CoreError::Config { code: oneshim_core::error_codes::ConfigCode::Invalid, message: format!("Selected Ollama model '{model}' does not advertise image support. Choose a multimodal model such as 'qwen3-vl:8b' or 'gemma3:4b'.") }), Err(error) => { warn!(endpoint = %self.endpoint, model = %model, error = %error, "Failed to verify Ollama OCR model capability; proceeding with request."); Ok(()) } }
    }
    pub fn new(
        config: &ExternalApiEndpoint,
        breaker_registry: Arc<CircuitBreakerRegistry>,
    ) -> Result<Self, crate::error::NetworkError> {
        use crate::error::NetworkError;
        let auth_scheme = provider_specs::resolved_auth_scheme(
            config.provider_type,
            config.surface_id.as_deref(),
            ProviderTransportKind::Ocr,
        )
        .map_err(NetworkError::Internal)?;
        if !matches!(auth_scheme, ProviderAuthScheme::None) && config.api_key.is_empty() {
            // Iter-99: "not configured" is Missing semantics. Route via
            // NetworkError::Core to emit ConfigCode::Missing wire code.
            return Err(NetworkError::Core(oneshim_core::error::CoreError::Config {
                code: oneshim_core::error_codes::ConfigCode::Missing,
                message: "AI OCR API key is not configured. Set it in Settings.".into(),
            }));
        }
        let credential = if matches!(auth_scheme, ProviderAuthScheme::None) {
            CredentialSource::NoAuth
        } else {
            CredentialSource::ApiKey(config.api_key.clone())
        };
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(|e| NetworkError::Http(format!("HTTP client create failure: {}", e)))?;
        let supports_model = provider_specs::resolved_surface_supports_model_selection(
            config.provider_type,
            config.surface_id.as_deref(),
            provider_specs::SurfaceCapabilityKind::Ocr,
        )
        .map_err(NetworkError::Internal)?;
        let resolved_model = config.model.clone().or_else(|| {
            provider_specs::resolved_default_model(
                config.provider_type,
                config.surface_id.as_deref(),
                provider_specs::SurfaceCapabilityKind::Ocr,
            )
            .ok()
            .flatten()
        });
        if resolved_model
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .is_none()
            && supports_model
        {
            // Iter-99 follow-up: required model selection = Missing.
            return Err(NetworkError::Core(oneshim_core::error::CoreError::Config {
                code: oneshim_core::error_codes::ConfigCode::Missing,
                message: "The selected OCR provider surface requires an explicit model selection."
                    .to_string(),
            }));
        }
        if let Some(model) = resolved_model
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            if !supports_model {
                return Err(NetworkError::Config("The selected OCR provider surface does not support configurable model selection.".to_string()));
            }
            match ai_model_lifecycle_policy::evaluate_model_lifecycle_now_for_surface(
                config.provider_type,
                config.surface_id.as_deref(),
                model,
            )
            .map_err(NetworkError::Core)?
            {
                ModelLifecycleDecision::Allowed => {}
                ModelLifecycleDecision::Warn {
                    message,
                    replacement,
                } => {
                    warn!(provider = ?config.provider_type, model = %model, replacement = ?replacement, "{}", message);
                }
                ModelLifecycleDecision::Block { message, .. } => {
                    return Err(NetworkError::PolicyDenied(message));
                }
            }
            if let Some(message) = provider_specs::known_model_capability_warning(
                config.provider_type,
                config.surface_id.as_deref(),
                provider_specs::SurfaceCapabilityKind::Ocr,
                model,
            )
            .map_err(NetworkError::Internal)?
            {
                warn!(provider = ?config.provider_type, surface_id = ?config.surface_id, model = %model, "{message}");
            }
            provider_specs::validate_known_model_capability(
                config.provider_type,
                config.surface_id.as_deref(),
                provider_specs::SurfaceCapabilityKind::Ocr,
                model,
            )
            .map_err(NetworkError::Config)?;
            if provider_specs::resolved_ocr_requires_structured_output_model(
                config.provider_type,
                config.surface_id.as_deref(),
            )
            .map_err(NetworkError::Internal)?
            {
                match provider_specs::resolved_model_capability_status(
                    config.provider_type,
                    config.surface_id.as_deref(),
                    SurfaceModelCapabilityKind::StructuredOutput,
                    model,
                )
                .map_err(NetworkError::Internal)?
                {
                    ProviderModelSupportStatus::Supported => {}
                    ProviderModelSupportStatus::Unsupported => {
                        return Err(NetworkError::Config(format!("Selected OCR model '{model}' is not marked as supporting structured JSON output required by this provider surface.")));
                    }
                    ProviderModelSupportStatus::Unknown => {
                        warn!(provider = ?config.provider_type, surface_id = ?config.surface_id, model = %model, "OCR surface requires structured output, but the selected model's support is unknown.");
                    }
                }
            }
        }
        debug!(endpoint = %config.endpoint, model = ?config.model, timeout = config.timeout_secs, "RemoteOcrProvider initialize");
        // D7: resolve per-endpoint breaker.
        let breaker_key = endpoint_authority(&config.endpoint)
            .unwrap_or_else(|_| format!("malformed::{}", config.endpoint));
        let breaker = breaker_registry.get(&breaker_key);
        Ok(Self {
            http_client,
            endpoint: config.endpoint.clone(),
            credential,
            model: resolved_model,
            provider_type: config.provider_type,
            surface_id: config.surface_id.clone(),
            timeout_secs: config.timeout_secs,
            breaker,
        })
    }
    pub fn new_with_credential(
        config: &ExternalApiEndpoint,
        credential: CredentialSource,
        breaker_registry: Arc<CircuitBreakerRegistry>,
    ) -> Result<Self, crate::error::NetworkError> {
        use crate::error::NetworkError;
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(|e| NetworkError::Http(format!("HTTP client create failure: {}", e)))?;
        let supports_model = provider_specs::resolved_surface_supports_model_selection(
            config.provider_type,
            config.surface_id.as_deref(),
            provider_specs::SurfaceCapabilityKind::Ocr,
        )
        .map_err(NetworkError::Internal)?;
        let resolved_model = config.model.clone().or_else(|| {
            provider_specs::resolved_default_model(
                config.provider_type,
                config.surface_id.as_deref(),
                provider_specs::SurfaceCapabilityKind::Ocr,
            )
            .ok()
            .flatten()
        });
        if resolved_model
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .is_none()
            && supports_model
        {
            // Iter-99 follow-up: required model selection = Missing.
            return Err(NetworkError::Core(oneshim_core::error::CoreError::Config {
                code: oneshim_core::error_codes::ConfigCode::Missing,
                message: "The selected OCR provider surface requires an explicit model selection."
                    .to_string(),
            }));
        }
        if let Some(model) = resolved_model
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            if !supports_model {
                return Err(NetworkError::Config("The selected OCR provider surface does not support configurable model selection.".to_string()));
            }
            match ai_model_lifecycle_policy::evaluate_model_lifecycle_now_for_surface(
                config.provider_type,
                config.surface_id.as_deref(),
                model,
            )
            .map_err(NetworkError::Core)?
            {
                ModelLifecycleDecision::Allowed => {}
                ModelLifecycleDecision::Warn {
                    message,
                    replacement,
                } => {
                    warn!(provider = ?config.provider_type, model = %model, replacement = ?replacement, "{}", message);
                }
                ModelLifecycleDecision::Block { message, .. } => {
                    return Err(NetworkError::PolicyDenied(message));
                }
            }
            provider_specs::validate_known_model_capability(
                config.provider_type,
                config.surface_id.as_deref(),
                provider_specs::SurfaceCapabilityKind::Ocr,
                model,
            )
            .map_err(NetworkError::Config)?;
            if provider_specs::resolved_ocr_requires_structured_output_model(
                config.provider_type,
                config.surface_id.as_deref(),
            )
            .map_err(NetworkError::Internal)?
            {
                match provider_specs::resolved_model_capability_status(
                    config.provider_type,
                    config.surface_id.as_deref(),
                    SurfaceModelCapabilityKind::StructuredOutput,
                    model,
                )
                .map_err(NetworkError::Internal)?
                {
                    ProviderModelSupportStatus::Supported => {}
                    ProviderModelSupportStatus::Unsupported => {
                        return Err(NetworkError::Config(format!("Selected OCR model '{model}' is not marked as supporting structured JSON output required by this provider surface.")));
                    }
                    ProviderModelSupportStatus::Unknown => {
                        warn!(provider = ?config.provider_type, surface_id = ?config.surface_id, model = %model, "OCR surface requires structured output, but the selected model's support is unknown.");
                    }
                }
            }
        }
        let endpoint = credential
            .api_base_url()
            .map(String::from)
            .unwrap_or_else(|| config.endpoint.clone());
        // D7: resolve per-endpoint breaker.
        let breaker_key =
            endpoint_authority(&endpoint).unwrap_or_else(|_| format!("malformed::{endpoint}"));
        let breaker = breaker_registry.get(&breaker_key);
        Ok(Self {
            http_client,
            endpoint,
            credential,
            model: resolved_model,
            provider_type: config.provider_type,
            surface_id: config.surface_id.clone(),
            timeout_secs: config.timeout_secs,
            breaker,
        })
    }
}
#[async_trait]
impl OcrProvider for RemoteOcrProvider {
    async fn extract_elements(
        &self,
        image: &[u8],
        image_format: &str,
    ) -> Result<Vec<OcrResult>, CoreError> {
        // D7: pre-flight breaker check.
        if matches!(self.breaker.check(), CircuitState::Open { .. }) {
            return Err(CoreError::ServiceUnavailable {
                code: oneshim_core::error_codes::ServiceCode::CircuitOpen,
                message: format!("circuit open for {}", self.endpoint),
            });
        }
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(image);
        let media_type = match image_format {
            "png" => "image/png",
            "jpeg" | "jpg" => "image/jpeg",
            "webp" => "image/webp",
            _ => "image/png",
        };
        let model = self.model.as_deref().unwrap_or("");
        self.ensure_runtime_ocr_model_ready(model).await?;
        let request_shape = self.ocr_request_shape()?;
        match request_shape {
            ProviderRequestShape::AnthropicMessages
            | ProviderRequestShape::AnthropicVisionMessages => {
                self.ensure_ocr_parameters_supported(&["model", "max_tokens", "messages"])?;
            }
            ProviderRequestShape::OpenAiChatCompletions
            | ProviderRequestShape::OpenAiVisionChatCompletions
            | ProviderRequestShape::OpenAiResponses => {
                self.ensure_ocr_parameters_supported(&[
                    "model",
                    "max_tokens",
                    "response_format",
                    "messages",
                ])?;
            }
            ProviderRequestShape::GoogleGenerateContent
            | ProviderRequestShape::GoogleVisionAnnotate => {
                self.ensure_ocr_parameters_supported(&[
                    "requests",
                    "TEXT_DETECTION",
                    "maxResults",
                ])?;
            }
            ProviderRequestShape::BedrockConverse => {
                return Err(CoreError::Config {
                    code: oneshim_core::error_codes::ConfigCode::UnsupportedProviderBedrock,
                    message: "AWS Bedrock is intentionally unsupported in this build".into(),
                });
            }
        }
        let strategy = OcrProviderStrategy::try_from(request_shape)?;
        let request_body = strategy.build_request_body(&encoded, media_type, model);
        debug!(endpoint = %self.endpoint, model = model, image_size = image.len(), "Calling external OCR API");
        let mut builder = self
            .http_client
            .post(&self.endpoint)
            .header("Content-Type", "application/json")
            .json(&request_body);
        let auth_scheme = self.ocr_auth_scheme()?;
        if matches!(auth_scheme, ProviderAuthScheme::None) {
            builder = apply_auth_headers(auth_scheme, builder, "")?;
        } else {
            let bearer_token = self.credential.resolve_bearer_token().await?;
            builder = apply_auth_headers(auth_scheme, builder, &bearer_token)?;
        }
        if self.credential.is_managed() && matches!(auth_scheme, ProviderAuthScheme::Bearer) {
            builder = builder.header("version", env!("CARGO_PKG_VERSION"));
        }
        let send_result = builder.send().await;
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
            // Iter-90: split timeout vs generic (canonical pattern).
            if e.is_timeout() {
                CoreError::RequestTimeout {
                    code: oneshim_core::error_codes::NetworkCode::Timeout,
                    timeout_ms: self.timeout_secs * 1000,
                }
            } else {
                CoreError::Network {
                    code: oneshim_core::error_codes::NetworkCode::Generic,
                    message: format!("OCR API request failed: {}", e),
                }
            }
        })?;
        let status = response.status();
        let body = response.text().await.map_err(|e| {
            if e.is_timeout() {
                CoreError::RequestTimeout {
                    code: oneshim_core::error_codes::NetworkCode::Timeout,
                    timeout_ms: self.timeout_secs * 1000,
                }
            } else {
                CoreError::Network {
                    code: oneshim_core::error_codes::NetworkCode::Generic,
                    message: format!("OCR API response read failure: {}", e),
                }
            }
        })?;
        if !status.is_success() {
            warn!(status = %status, "OCR API error response");
            let message = format!(
                "OCR API error ({}): {}",
                status,
                body.chars().take(200).collect::<String>()
            );
            // Semantic HTTP status mapping per iter-54/55/56/58 — even OCR
            // domain errors benefit from differentiating auth/timeout/rate-limit
            // from generic OCR failures.
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
                _ => CoreError::OcrError {
                    code: oneshim_core::error_codes::ProviderCode::OcrFailed,
                    message,
                },
            });
        }
        let results = strategy.parse_response(&body)?;
        debug!(count = results.len(), "OCR received");
        Ok(results)
    }
    fn provider_name(&self) -> &str {
        "remote-ocr"
    }
    fn is_external(&self) -> bool {
        true
    }
}
