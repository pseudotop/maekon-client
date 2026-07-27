use async_trait::async_trait;
use maekon_api_contracts::provider_specs::{
    self, ProviderAuthScheme, ProviderRequestShape, ProviderTransportKind,
};
use maekon_core::ai_model_lifecycle_policy::{self, ModelLifecycleDecision};
use maekon_core::config::{AiProviderType, ExternalApiEndpoint};
use maekon_core::error::CoreError;
use maekon_core::ports::credential_source::CredentialSource;
use maekon_core::ports::llm_provider::{
    InterpretedAction, LlmCallHealth, LlmProvider, ScreenContext, SkillContext,
};
use std::sync::Arc;
use tracing::{debug, warn};

use crate::circuit_breaker::{CircuitBreaker, CircuitBreakerRegistry};
use crate::resilience::endpoint_authority;

mod parsers;
mod request;
#[cfg(test)]
mod tests;
/// - Claude (Anthropic): `POST /v1/messages`
///
/// D7: Guarded by a per-endpoint `CircuitBreaker` resolved at construction
/// time. The `send_and_parse` funnel in `request.rs` uses `self.breaker` to
/// pre-flight check + record outcome.
pub struct RemoteLlmProvider {
    http_client: reqwest::Client,
    endpoint: String,
    credential: CredentialSource,
    model: String,
    provider_type: AiProviderType,
    surface_id: Option<String>,
    // Cached from config; the actual timeout enforcement is baked into
    // `http_client` at construction (`.timeout(...)` below), so this is not
    // read back today — kept for a future diagnostics/status surface.
    #[allow(dead_code)]
    timeout_secs: u64,
    /// Token budget for the `max_output_tokens` field of Responses API bodies.
    /// Default 512 (all existing providers); raised to 2048 for Ollama intent
    /// paths (Decision 4: qwen3 thinking tokens consume the 512 budget).
    max_output_tokens: u32,
    /// Per-call health handle: tri-state UNKNOWN/OK/FAILED.
    /// Read by the automation status endpoint at request time.
    /// `None` when no caller has wired a handle.
    call_health: Option<Arc<LlmCallHealth>>,
    /// D7: per-endpoint circuit breaker (shared via registry across adapter
    /// instances targeting the same endpoint).
    pub(super) breaker: Arc<CircuitBreaker>,
}
impl std::fmt::Debug for RemoteLlmProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteLlmProvider")
            .field("endpoint", &self.endpoint)
            .field("credential", &self.credential)
            .field("model", &self.model)
            .field("provider_type", &self.provider_type)
            .field("surface_id", &self.surface_id)
            .finish()
    }
}
impl RemoteLlmProvider {
    fn fallback_llm_model(provider_type: AiProviderType) -> &'static str {
        crate::default_model_for_provider(&provider_type)
    }
    fn resolved_runtime_endpoint(
        config: &ExternalApiEndpoint,
        model: &str,
    ) -> Result<String, crate::error::NetworkError> {
        let shape = provider_specs::resolved_request_shape(
            config.provider_type,
            config.surface_id.as_deref(),
            ProviderTransportKind::Llm,
        )
        .map_err(crate::error::NetworkError::Internal)?;
        Ok(match shape {
            ProviderRequestShape::GoogleGenerateContent => {
                Self::rewrite_google_generate_content_endpoint(&config.endpoint, model)
            }
            _ => config.endpoint.clone(),
        })
    }
    fn rewrite_google_generate_content_endpoint(endpoint: &str, model: &str) -> String {
        let model = model.trim();
        if model.is_empty() {
            return endpoint.to_string();
        }
        let Some(models_idx) = endpoint.find("/models/") else {
            return endpoint.to_string();
        };
        let prefix_end = models_idx + "/models/".len();
        let rest = &endpoint[prefix_end..];
        let Some(action_idx) = rest.find(':') else {
            return endpoint.to_string();
        };
        let prefix = &endpoint[..prefix_end];
        let suffix = &rest[action_idx..];
        format!("{prefix}{model}{suffix}")
    }
    pub fn new(
        config: &ExternalApiEndpoint,
        breaker_registry: Arc<CircuitBreakerRegistry>,
    ) -> Result<Self, crate::error::NetworkError> {
        use crate::error::NetworkError;
        let auth_scheme = provider_specs::resolved_auth_scheme(
            config.provider_type,
            config.surface_id.as_deref(),
            ProviderTransportKind::Llm,
        )
        .map_err(NetworkError::Internal)?;
        if !matches!(auth_scheme, ProviderAuthScheme::None) && config.api_key.is_empty() {
            // Iter-99: "not configured" is Missing semantics, not Invalid.
            // Route via NetworkError::Core wrapper to emit the correct
            // ConfigCode::Missing wire code (config.missing).
            return Err(NetworkError::Core(CoreError::Config {
                code: maekon_core::error_codes::ConfigCode::Missing,
                message: "AI LLM API key is not configured. Set it in Settings.".into(),
            }));
        }
        let credential = if matches!(auth_scheme, ProviderAuthScheme::None) {
            CredentialSource::NoAuth
        } else {
            CredentialSource::ApiKey(config.api_key.clone())
        };
        // #6892: redirect=none — prevents a provider 30x redirect from leaking the api-key header + prompt body.
        // #8045 C3: https_only backstop derived from the target endpoint (loopback
        // local LLM like Ollama keeps cleartext; remote providers are HTTPS-only).
        let http_client = crate::outbound::hardened_client_builder(
            crate::outbound::TransportPolicy::for_endpoint(&config.endpoint),
        )
        .timeout(std::time::Duration::from_secs(config.timeout_secs))
        .build()
        .map_err(|e| NetworkError::Http(format!("HTTP client create failure: {}", e)))?;
        let supports_model = provider_specs::resolved_surface_supports_model_selection(
            config.provider_type,
            config.surface_id.as_deref(),
            provider_specs::SurfaceCapabilityKind::Llm,
        )
        .map_err(NetworkError::Internal)?;
        let model = config
            .model
            .clone()
            .or_else(|| {
                provider_specs::resolved_default_model(
                    config.provider_type,
                    config.surface_id.as_deref(),
                    provider_specs::SurfaceCapabilityKind::Llm,
                )
                .ok()
                .flatten()
            })
            .or_else(|| {
                if supports_model {
                    None
                } else {
                    Some(Self::fallback_llm_model(config.provider_type).to_string())
                }
            })
            .ok_or_else(|| {
                // Iter-99: "requires an explicit model" is Missing semantics.
                NetworkError::Core(CoreError::Config {
                    code: maekon_core::error_codes::ConfigCode::Missing,
                    message:
                        "The selected LLM provider surface requires an explicit model selection."
                            .to_string(),
                })
            })?;
        if !supports_model {
            return Err(NetworkError::Config(
                "The selected LLM provider surface does not support configurable model selection."
                    .to_string(),
            ));
        }
        match ai_model_lifecycle_policy::evaluate_model_lifecycle_now_for_surface(
            config.provider_type,
            config.surface_id.as_deref(),
            &model,
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
            provider_specs::SurfaceCapabilityKind::Llm,
            &model,
        )
        .map_err(NetworkError::Internal)?
        {
            warn!(provider = ?config.provider_type, surface_id = ?config.surface_id, model = %model, "{message}");
        }
        provider_specs::validate_known_model_capability(
            config.provider_type,
            config.surface_id.as_deref(),
            provider_specs::SurfaceCapabilityKind::Llm,
            &model,
        )
        .map_err(NetworkError::Config)?;
        debug!(endpoint = %config.endpoint, model = %model, timeout = config.timeout_secs, "RemoteLlmProvider initialize");
        let endpoint = Self::resolved_runtime_endpoint(config, &model)?;
        // D7: resolve per-endpoint breaker.
        let breaker_key =
            endpoint_authority(&endpoint).unwrap_or_else(|_| format!("malformed::{endpoint}"));
        let breaker = breaker_registry.get(&breaker_key);
        Ok(Self {
            http_client,
            endpoint,
            credential,
            model,
            provider_type: config.provider_type,
            surface_id: config.surface_id.clone(),
            timeout_secs: config.timeout_secs,
            max_output_tokens: 512,
            call_health: None,
            breaker,
        })
    }
    /// Attach a shared per-call health handle that records UNKNOWN/OK/FAILED
    /// after each `interpret_intent` call.  The automation status endpoint reads
    /// `call_health.as_option_bool()` at request time to expose `llm_healthy`.
    pub fn with_call_health(mut self, handle: Arc<LlmCallHealth>) -> Self {
        self.call_health = Some(handle);
        self
    }
    /// Override the `max_output_tokens` cap for this provider (Decision 4).
    ///
    /// Default is 512 (all provider paths). Set to 2048 for Ollama intent
    /// planning where qwen3 thinking tokens can exhaust the 512 budget and
    /// return an empty parse, silently falling back to the rule matcher.
    pub fn with_token_budget(mut self, tokens: u32) -> Self {
        self.max_output_tokens = tokens;
        self
    }
    /// Create a provider with a managed credential source (e.g., OAuth).
    pub fn new_with_credential(
        config: &ExternalApiEndpoint,
        credential: CredentialSource,
        breaker_registry: Arc<CircuitBreakerRegistry>,
    ) -> Result<Self, crate::error::NetworkError> {
        use crate::error::NetworkError;
        // #6892: redirect=none — prevents a provider 30x redirect from leaking the api-key header + prompt body.
        // #8045 C3: https_only backstop derived from the target endpoint (loopback
        // local LLM like Ollama keeps cleartext; remote providers are HTTPS-only).
        let http_client = crate::outbound::hardened_client_builder(
            crate::outbound::TransportPolicy::for_endpoint(&config.endpoint),
        )
        .timeout(std::time::Duration::from_secs(config.timeout_secs))
        .build()
        .map_err(|e| NetworkError::Http(format!("HTTP client create failure: {}", e)))?;
        let supports_model = provider_specs::resolved_surface_supports_model_selection(
            config.provider_type,
            config.surface_id.as_deref(),
            provider_specs::SurfaceCapabilityKind::Llm,
        )
        .map_err(NetworkError::Internal)?;
        let model = config
            .model
            .clone()
            .or_else(|| {
                provider_specs::resolved_default_model(
                    config.provider_type,
                    config.surface_id.as_deref(),
                    provider_specs::SurfaceCapabilityKind::Llm,
                )
                .ok()
                .flatten()
            })
            .or_else(|| {
                if supports_model {
                    None
                } else {
                    Some(Self::fallback_llm_model(config.provider_type).to_string())
                }
            })
            .ok_or_else(|| {
                // Iter-99: "requires an explicit model" is Missing semantics.
                NetworkError::Core(CoreError::Config {
                    code: maekon_core::error_codes::ConfigCode::Missing,
                    message:
                        "The selected LLM provider surface requires an explicit model selection."
                            .to_string(),
                })
            })?;
        if !supports_model {
            return Err(NetworkError::Config(
                "The selected LLM provider surface does not support configurable model selection."
                    .to_string(),
            ));
        }
        match ai_model_lifecycle_policy::evaluate_model_lifecycle_now_for_surface(
            config.provider_type,
            config.surface_id.as_deref(),
            &model,
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
            provider_specs::SurfaceCapabilityKind::Llm,
            &model,
        )
        .map_err(NetworkError::Config)?;
        let endpoint = credential
            .api_base_url()
            .map(String::from)
            .unwrap_or_else(|| {
                Self::resolved_runtime_endpoint(config, &model)
                    .unwrap_or_else(|_| config.endpoint.clone())
            });
        // D7: resolve per-endpoint breaker.
        let breaker_key =
            endpoint_authority(&endpoint).unwrap_or_else(|_| format!("malformed::{endpoint}"));
        let breaker = breaker_registry.get(&breaker_key);
        Ok(Self {
            http_client,
            endpoint,
            credential,
            model,
            provider_type: config.provider_type,
            surface_id: config.surface_id.clone(),
            timeout_secs: config.timeout_secs,
            max_output_tokens: 512,
            call_health: None,
            breaker,
        })
    }
    fn llm_request_shape(&self) -> Result<ProviderRequestShape, CoreError> {
        provider_specs::resolved_request_shape(
            self.provider_type,
            self.surface_id.as_deref(),
            ProviderTransportKind::Llm,
        )
        .map_err(|msg| CoreError::Config {
            code: maekon_core::error_codes::ConfigCode::Invalid,
            message: msg,
        })
    }
    fn uses_responses_api(&self) -> bool {
        matches!(
            self.llm_request_shape(),
            Ok(ProviderRequestShape::OpenAiResponses)
        )
    }
    fn llm_auth_scheme(&self) -> Result<ProviderAuthScheme, CoreError> {
        provider_specs::resolved_auth_scheme(
            self.provider_type,
            self.surface_id.as_deref(),
            ProviderTransportKind::Llm,
        )
        .map_err(|msg| CoreError::Config {
            code: maekon_core::error_codes::ConfigCode::Invalid,
            message: msg,
        })
    }
    fn ensure_llm_parameters_supported(&self, parameters: &[&str]) -> Result<(), CoreError> {
        provider_specs::validate_supported_parameters(
            self.provider_type,
            self.surface_id.as_deref(),
            provider_specs::SurfaceCapabilityKind::Llm,
            parameters,
        )
        .map_err(|msg| CoreError::Config {
            code: maekon_core::error_codes::ConfigCode::Invalid,
            message: msg,
        })
    }
}
#[async_trait]
impl LlmProvider for RemoteLlmProvider {
    async fn interpret_intent(
        &self,
        screen_context: &ScreenContext,
        intent_hint: &str,
    ) -> Result<InterpretedAction, CoreError> {
        // No skill context on this path — the skill region stays empty.
        let prompts = request::build_prompts(&SkillContext::default(), screen_context, intent_hint);
        debug!(endpoint = %self.endpoint, model = %self.model, hint = %intent_hint, "Calling external LLM API");
        // Argument order is load-bearing: `prompts.system` (trusted region) maps
        // to the provider's system/instruction slot and `prompts.user` (fenced
        // untrusted data) to the user slot. Swapping them would put untrusted
        // screen/intent text into instruction position (#8588).
        let request_body = self.build_chat_body(&prompts.system, &prompts.user)?;
        self.send_and_parse(&request_body).await
    }
    async fn interpret_intent_with_skills(
        &self,
        screen_context: &ScreenContext,
        intent_hint: &str,
        skill_ctx: &SkillContext,
    ) -> Result<InterpretedAction, CoreError> {
        let prompts = request::build_prompts(skill_ctx, screen_context, intent_hint);
        debug!(endpoint = %self.endpoint, model = %self.model, hint = %intent_hint, skills = skill_ctx.available_skills.len(), has_active_skill = skill_ctx.active_skill.is_some(), responses_api = self.uses_responses_api(), "Calling external LLM API (with skills)");
        // Argument order is load-bearing (see `interpret_intent`): trusted
        // `prompts.system` to the instruction slot, fenced `prompts.user` to the
        // data slot (#8588).
        let request_body = self.build_chat_body(&prompts.system, &prompts.user)?;
        self.send_and_parse(&request_body).await
    }
    fn provider_name(&self) -> &str {
        &self.model
    }
    fn is_external(&self) -> bool {
        true
    }
    fn egress_endpoint_urls(&self) -> Vec<&str> {
        vec![&self.endpoint]
    }
}
