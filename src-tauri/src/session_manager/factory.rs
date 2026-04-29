//! Provider routing — create HttpApi, Subprocess, and LocalLlm sessions.

use std::sync::Arc;
use std::time::Instant;

use tracing::info;

use oneshim_api_contracts::provider_specs::{self, ProviderTransportKind, SurfaceCapabilityKind};
use oneshim_core::error::CoreError;
use oneshim_core::models::ai_session::{SessionConfig, SessionState, SessionTransport};
use oneshim_core::ports::conversation_session::ConversationSession;
use oneshim_core::ports::credential_source::CredentialSource;

use crate::auditing_session::AuditingSession;
use crate::session_adapters::claude_session::ClaudeSubprocessSession;
use crate::session_adapters::subprocess_session::GenericSubprocessSession;
use crate::subprocess_provider::{probe_known_cli_surfaces, runtime_ready_for_surface};

use oneshim_network::http_api_session::{HttpApiSession, HttpApiSessionInit};
use oneshim_network::local_llm_session::LocalLlmSession;

use super::{ManagedSession, SessionManagerImpl};

/// Tool definitions extracted from the context assembler (if any).
pub(super) type DefaultTools = Option<Vec<oneshim_core::models::ai_session::ToolDefinition>>;

impl SessionManagerImpl {
    /// Create a session by dispatching on transport type.
    /// Called from the `SessionManager::create_session` trait impl.
    pub(super) async fn create_session_impl(
        &self,
        mut config: SessionConfig,
    ) -> Result<Arc<dyn ConversationSession>, CoreError> {
        // Auto-generate system prompt from context if not provided and lift any
        // context-assembler tool definitions into session defaults when enabled.
        let mut default_tools: DefaultTools = None;
        if let Some(ref assembler) = self.context_assembler {
            if config.system_prompt.is_none() || config.tools_enabled {
                let message = assembler.build_system_message().await;
                if config.system_prompt.is_none() {
                    config.system_prompt = Some(message.content);
                }
                if config.tools_enabled {
                    default_tools = message.tools.filter(|tools| !tools.is_empty());
                }
            }
        }

        match config.transport {
            SessionTransport::Subprocess => {
                self.create_subprocess_session(&config, &default_tools)
                    .await
            }
            SessionTransport::HttpApi => {
                self.create_http_api_session(&config, &default_tools).await
            }
            SessionTransport::LocalLlm => {
                self.create_local_llm_session(&config, &default_tools).await
            }
        }
    }

    async fn create_subprocess_session(
        &self,
        config: &SessionConfig,
        default_tools: &DefaultTools,
    ) -> Result<Arc<dyn ConversationSession>, CoreError> {
        let probed_surfaces = probe_known_cli_surfaces();
        let surface = if let Some(requested_surface_id) = config.surface_id.as_deref() {
            probed_surfaces
                .iter()
                .find(|surface| {
                    surface
                        .detected
                        .surface_id
                        .eq_ignore_ascii_case(requested_surface_id)
                })
                .map(|surface| surface.detected.clone())
                .ok_or_else(|| {
                    // Iter-94: NotFound semantically (the requested subprocess
                    // CLI tool is not installed / not detected); wire code
                    // `not_found.resource_missing` helps telemetry distinguish
                    // missing-tool from internal runtime failure.
                    CoreError::NotFound {
                        code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                        resource_type: "subprocess_cli_surface".to_string(),
                        id: requested_surface_id.to_string(),
                    }
                })?
        } else {
            probed_surfaces
                .iter()
                .find(|surface| {
                    runtime_ready_for_surface(&surface.detected.surface_id, surface.auth_status)
                })
                .map(|surface| surface.detected.clone())
                .or_else(|| {
                    probed_surfaces
                        .first()
                        .map(|surface| surface.detected.clone())
                })
                .ok_or_else(|| {
                    // Iter-94: no subprocess CLI detected — surface-level
                    // NotFound. Distinguishes "user hasn't installed any
                    // supported CLI" from generic runtime failure in logs.
                    CoreError::NotFound {
                        code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                        resource_type: "subprocess_cli_surface".to_string(),
                        id: "any".to_string(),
                    }
                })?
        };

        let inner: Arc<dyn ConversationSession> = if surface
            .surface_id
            .eq_ignore_ascii_case("provider_surface.anthropic.subprocess_cli")
        {
            Arc::new(ClaudeSubprocessSession::new(
                surface,
                config,
                self.config.clone(),
                default_tools.clone(),
            ))
        } else {
            Arc::new(GenericSubprocessSession::new(
                surface,
                config,
                self.config.clone(),
                default_tools.clone(),
            ))
        };

        let wrapped: Arc<dyn ConversationSession> =
            Arc::new(AuditingSession::new(inner, self.audit.clone()));

        let session_id = wrapped.session_id().to_string();
        info!(session_id = %session_id, "created subprocess session");

        let managed = ManagedSession {
            session: wrapped.clone(),
            state: SessionState::Active,
            created_at: Instant::now(),
            last_active: Instant::now(),
            retry_count: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
        };
        self.admit_session(session_id, managed).await?;

        Ok(wrapped)
    }

    async fn create_http_api_session(
        &self,
        config: &SessionConfig,
        default_tools: &DefaultTools,
    ) -> Result<Arc<dyn ConversationSession>, CoreError> {
        let surface_id =
            config
                .surface_id
                .as_deref()
                .ok_or_else(|| CoreError::InvalidArguments {
                    code: oneshim_core::error_codes::ValidationCode::InvalidArguments,
                    message: "surface_id is required for HttpApi sessions".to_string(),
                })?;

        // Resolve surface spec from the provider catalog.
        // Iter-94: catalog lookup miss = NotFound (the surface id references
        // an entry that doesn't exist in the catalog); not an internal error.
        let surface_spec = provider_specs::provider_surface_spec(surface_id).map_err(|msg| {
            CoreError::NotFound {
                code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                resource_type: "provider_surface".to_string(),
                id: format!("{surface_id}: {msg}"),
            }
        })?;
        // Iter-94: unknown provider_type = invalid config (the vendor_id
        // doesn't map to any known provider type).
        let provider_type = oneshim_core::provider_surface::provider_type_from_vendor_id(
            &surface_spec.provider_type,
        )
        .ok_or_else(|| CoreError::Config {
            code: oneshim_core::error_codes::ConfigCode::Invalid,
            message: format!(
                "unknown provider_type for vendor '{}'",
                surface_spec.vendor_id
            ),
        })?;

        // Resolve the LLM transport endpoint from the catalog.
        // Iter-94: transport catalog miss = NotFound.
        let transport_spec = provider_specs::resolved_transport_spec(
            provider_type,
            Some(surface_id),
            ProviderTransportKind::Llm,
        )
        .map_err(|msg| CoreError::NotFound {
            code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
            resource_type: "provider_transport".to_string(),
            id: format!("{surface_id}/llm: {msg}"),
        })?;

        // Model: explicit > catalog default > error.
        let model = config
            .model
            .clone()
            .or_else(|| {
                provider_specs::resolved_default_model(
                    provider_type,
                    Some(surface_id),
                    SurfaceCapabilityKind::Llm,
                )
                .ok()
                .flatten()
            })
            .ok_or_else(|| CoreError::InvalidArguments {
                code: oneshim_core::error_codes::ValidationCode::InvalidArguments,
                message: format!(
                    "no model specified and surface '{surface_id}' has no default LLM model"
                ),
            })?;

        // Build credential source from the secret store.
        let credential = CredentialSource::from_api_key_endpoint(
            &oneshim_core::config::ExternalApiEndpoint {
                endpoint: transport_spec.url.clone(),
                api_key: String::new(),
                model: Some(model.clone()),
                timeout_secs: 30,
                provider_type,
                surface_id: Some(surface_id.to_string()),
                credential: None,
            },
            self.secret_store.clone(),
        )
        .or_else(|_| {
            // Fallback: no-auth surfaces (e.g. Ollama local_http).
            if oneshim_core::provider_surface::provider_surface_uses_no_auth(surface_id) {
                Ok(CredentialSource::NoAuth)
            } else {
                Err(CoreError::Auth {
                    code: oneshim_core::error_codes::AuthCode::Failed,
                    message: format!("no credential available for surface '{surface_id}'"),
                })
            }
        })?;

        let inner: Arc<dyn ConversationSession> =
            Arc::new(HttpApiSession::new(HttpApiSessionInit {
                surface_id: surface_id.to_string(),
                model,
                endpoint: transport_spec.url.clone(),
                credential,
                provider_type,
                system_prompt: config.system_prompt.clone(),
                config: self.config.clone(),
                default_tools: default_tools.clone(),
                // D7: per-session registry; iter-011 consolidates to a
                // workspace-wide shared registry.
                breaker_registry: oneshim_network::CircuitBreakerRegistry::new(),
            }));

        let wrapped: Arc<dyn ConversationSession> =
            Arc::new(AuditingSession::new(inner, self.audit.clone()));

        let session_id = wrapped.session_id().to_string();
        info!(session_id = %session_id, surface_id = %surface_id, "created HttpApi session");

        let managed = ManagedSession {
            session: wrapped.clone(),
            state: SessionState::Active,
            created_at: Instant::now(),
            last_active: Instant::now(),
            retry_count: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
        };
        self.admit_session(session_id, managed).await?;

        Ok(wrapped)
    }

    async fn create_local_llm_session(
        &self,
        config: &SessionConfig,
        default_tools: &DefaultTools,
    ) -> Result<Arc<dyn ConversationSession>, CoreError> {
        let _ = default_tools; // LocalLlm does not use tool definitions yet.

        // Resolve Ollama base URL: use the Ollama surface spec probe URL,
        // stripping the path to get the base.
        let base_url = "http://localhost:11434".to_string();

        let model = config.model.clone().unwrap_or_else(|| "llama3".to_string());
        let session_id = uuid::Uuid::new_v4().to_string();

        let inner: Arc<dyn ConversationSession> = Arc::new(LocalLlmSession::new(
            session_id.clone(),
            model,
            base_url,
            config.system_prompt.clone(),
            self.config.clone(),
        ));

        let wrapped: Arc<dyn ConversationSession> =
            Arc::new(AuditingSession::new(inner, self.audit.clone()));

        let session_id = wrapped.session_id().to_string();
        info!(session_id = %session_id, "created LocalLlm (Ollama) session");

        let managed = ManagedSession {
            session: wrapped.clone(),
            state: SessionState::Active,
            created_at: Instant::now(),
            last_active: Instant::now(),
            retry_count: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
        };
        self.admit_session(session_id, managed).await?;

        Ok(wrapped)
    }
}
