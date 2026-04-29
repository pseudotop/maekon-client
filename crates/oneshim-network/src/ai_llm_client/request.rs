use std::sync::atomic::Ordering;

use super::parsers;
use super::RemoteLlmProvider;
use crate::circuit_breaker::CircuitState;
use crate::resilience::{classify_for_breaker, BreakerSignal};
use oneshim_api_contracts::provider_specs::ProviderAuthScheme;
use oneshim_api_contracts::provider_specs::ProviderRequestShape;
use oneshim_core::error::CoreError;
use oneshim_core::ports::llm_provider::{InterpretedAction, ScreenContext, SkillContext};
use tracing::{debug, warn};
pub(super) fn system_prompt() -> &'static str {
    r#"You are a UI automation agent.
Interpret the user's intent and return which UI element to act on as JSON.

Response schema:
{
  "target_text": "Text to target (or null)",
  "target_role": "button, input, link, menu, etc. (or null)",
  "action_type": "one of click, type, hotkey, wait, activate",
  "confidence": "confidence between 0.0 and 1.0"
}

Decide based on visible screen text and the user intent.
Return JSON only."#
}
pub(super) fn build_system_prompt(skill_ctx: &SkillContext) -> String {
    let mut prompt = String::from(system_prompt());
    if !skill_ctx.available_skills.is_empty() {
        prompt.push_str("\n\nAvailable skills:");
        for skill in &skill_ctx.available_skills {
            prompt.push_str(&format!("\n  - {}: {}", skill.name, skill.description));
        }
    }
    if let Some(ref body) = skill_ctx.active_skill_body {
        prompt.push_str("\n\n--- Active Skill ---\n");
        prompt.push_str(body);
        prompt.push_str("\n--- End Skill ---");
    }
    prompt
}
pub(super) fn build_user_prompt(screen_context: &ScreenContext, intent_hint: &str) -> String {
    let mut prompt = String::new();
    prompt.push_str(&format!("Active app: {}\n", screen_context.active_app));
    prompt.push_str(&format!(
        "Window title: {}\n",
        screen_context.active_window_title
    ));
    if !screen_context.visible_texts.is_empty() {
        prompt.push_str("Visible screen text:\n");
        for text in &screen_context.visible_texts {
            prompt.push_str(&format!("  - {}\n", text));
        }
    }
    if let Some(layout) = &screen_context.layout_description {
        prompt.push_str(&format!("Layout: {}\n", layout));
    }
    prompt.push_str(&format!("\nUser intent: {}\n", intent_hint));
    prompt
}
impl RemoteLlmProvider {
    pub(super) fn build_responses_api_body(
        &self,
        system_prompt: &str,
        user_prompt: &str,
    ) -> serde_json::Value {
        serde_json::json!({ "model": self.model, "instructions": system_prompt, "input": user_prompt, "max_output_tokens": 512 })
    }
    pub(super) fn build_chat_body(
        &self,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<serde_json::Value, CoreError> {
        let body = match self.llm_request_shape()? {
            ProviderRequestShape::AnthropicMessages
            | ProviderRequestShape::AnthropicVisionMessages => {
                self.ensure_llm_parameters_supported(&[
                    "model",
                    "max_tokens",
                    "system",
                    "messages",
                ])?;
                serde_json::json!({ "model": self.model, "max_tokens": 512, "system": system_prompt, "messages": [{"role": "user", "content": user_prompt}] })
            }
            ProviderRequestShape::GoogleGenerateContent => {
                self.ensure_llm_parameters_supported(&[
                    "contents",
                    "system_instruction",
                    "generationConfig.maxOutputTokens",
                ])?;
                serde_json::json!({ "contents": [{"role": "user", "parts": [{"text": user_prompt}]}], "system_instruction": {"parts": [{"text": system_prompt}]}, "generationConfig": {"maxOutputTokens": 512} })
            }
            ProviderRequestShape::OpenAiResponses => {
                self.ensure_llm_parameters_supported(&[
                    "model",
                    "instructions",
                    "input",
                    "max_output_tokens",
                ])?;
                self.build_responses_api_body(system_prompt, user_prompt)
            }
            ProviderRequestShape::OpenAiChatCompletions
            | ProviderRequestShape::OpenAiVisionChatCompletions => {
                self.ensure_llm_parameters_supported(&["model", "max_tokens", "messages"])?;
                serde_json::json!({ "model": self.model, "max_tokens": 512, "messages": [{"role": "system", "content": system_prompt}, {"role": "user", "content": user_prompt}] })
            }
            ProviderRequestShape::GoogleVisionAnnotate => {
                return Err(CoreError::Config {
                    code: oneshim_core::error_codes::ConfigCode::Invalid,
                    message: "LLM transport shape resolved to OCR-only Google Vision Annotate"
                        .to_string(),
                });
            }
            ProviderRequestShape::BedrockConverse => {
                return Err(CoreError::Config {
                    code: oneshim_core::error_codes::ConfigCode::UnsupportedProviderBedrock,
                    message: "AWS Bedrock is intentionally unsupported in this build".into(),
                });
            }
        };
        Ok(body)
    }
    pub(super) async fn send_and_parse(
        &self,
        request_body: &serde_json::Value,
    ) -> Result<InterpretedAction, CoreError> {
        // D7: pre-flight breaker check.
        if matches!(self.breaker.check(), CircuitState::Open { .. }) {
            return Err(CoreError::ServiceUnavailable {
                code: oneshim_core::error_codes::ServiceCode::CircuitOpen,
                message: format!("circuit open for {}", self.endpoint),
            });
        }
        let mut builder = self
            .http_client
            .post(&self.endpoint)
            .header("Content-Type", "application/json")
            .json(request_body);
        match self.llm_auth_scheme()? {
            ProviderAuthScheme::None => {}
            ProviderAuthScheme::XApiKey => {
                let bearer_token = self.credential.resolve_bearer_token().await?;
                builder = builder
                    .header("x-api-key", &bearer_token)
                    .header("anthropic-version", crate::ANTHROPIC_API_VERSION);
            }
            ProviderAuthScheme::XGoogApiKey => {
                let bearer_token = self.credential.resolve_bearer_token().await?;
                builder = builder.header("x-goog-api-key", &bearer_token);
            }
            ProviderAuthScheme::Bearer => {
                let bearer_token = self.credential.resolve_bearer_token().await?;
                builder = builder.header("Authorization", format!("Bearer {}", bearer_token));
                if self.credential.is_managed() {
                    builder = builder.header("version", env!("CARGO_PKG_VERSION"));
                }
            }
            ProviderAuthScheme::AwsSignatureV4 => {
                return Err(CoreError::Config {
                    code: oneshim_core::error_codes::ConfigCode::UnsupportedProviderBedrock,
                    message: "AWS Bedrock is intentionally unsupported in this build".into(),
                });
            }
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
            if let Some(ref flag) = self.last_request_ok {
                flag.store(false, Ordering::Relaxed);
            }
            // Iter-90: split timeout vs generic per canonical pattern
            // (cloud_stt.rs:107, http_client.rs map_reqwest_error).
            if e.is_timeout() {
                CoreError::RequestTimeout {
                    code: oneshim_core::error_codes::NetworkCode::Timeout,
                    timeout_ms: 0, // sentinel; configured timeout is in request-site logs
                }
            } else {
                CoreError::Network {
                    code: oneshim_core::error_codes::NetworkCode::Generic,
                    message: format!("LLM API request failed: {}", e),
                }
            }
        })?;
        let status = response.status();
        let body = response.text().await.map_err(|e| {
            // Iter-90: body-read timeout is also a timeout; keep the split
            // consistent with send()-time timeout handling above.
            if e.is_timeout() {
                CoreError::RequestTimeout {
                    code: oneshim_core::error_codes::NetworkCode::Timeout,
                    timeout_ms: 0,
                }
            } else {
                CoreError::Network {
                    code: oneshim_core::error_codes::NetworkCode::Generic,
                    message: format!("LLM API response read failure: {}", e),
                }
            }
        })?;
        if !status.is_success() {
            if let Some(ref flag) = self.last_request_ok {
                flag.store(false, Ordering::Relaxed);
            }
            warn!(status = %status, "LLM API error response");
            let message = format!(
                "LLM API error ({}): {}",
                status,
                body.chars().take(200).collect::<String>()
            );
            // Semantic status mapping per iter-55 / iter-56 pattern — give
            // upstream LLM failures specific wire codes so telemetry can
            // distinguish timeouts (transient, retryable) from auth (permanent)
            // from rate-limiting (backoff-based).
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
        let action = match self.llm_request_shape()? {
            ProviderRequestShape::AnthropicMessages
            | ProviderRequestShape::AnthropicVisionMessages => {
                parsers::parse_claude_response(&body)?
            }
            ProviderRequestShape::GoogleGenerateContent => parsers::parse_google_response(&body)?,
            ProviderRequestShape::OpenAiChatCompletions
            | ProviderRequestShape::OpenAiVisionChatCompletions
            | ProviderRequestShape::OpenAiResponses => parsers::parse_openai_response(&body)?,
            ProviderRequestShape::GoogleVisionAnnotate => {
                return Err(CoreError::Config {
                    code: oneshim_core::error_codes::ConfigCode::Invalid,
                    message: "LLM transport shape resolved to OCR-only Google Vision Annotate"
                        .to_string(),
                });
            }
            ProviderRequestShape::BedrockConverse => {
                return Err(CoreError::Config {
                    code: oneshim_core::error_codes::ConfigCode::UnsupportedProviderBedrock,
                    message: "AWS Bedrock is intentionally unsupported in this build".into(),
                });
            }
        };
        if let Some(ref flag) = self.last_request_ok {
            flag.store(true, Ordering::Relaxed);
        }
        debug!(action_type = %action.action_type, target = ?action.target_text, confidence = action.confidence, "LLM intent interpretation completed");
        Ok(action)
    }
}
