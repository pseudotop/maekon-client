//! HTTP chat construction and origin-bound credential resolution.

use std::sync::Arc;
use std::time::Instant;

use tracing::info;

use maekon_api_contracts::provider_specs::{self, ProviderTransportKind, SurfaceCapabilityKind};
use maekon_core::error::CoreError;
use maekon_core::models::ai_session::{SessionConfig, SessionState};
use maekon_core::ports::conversation_session::ConversationSession;
use maekon_core::ports::credential_source::CredentialSource;
use maekon_network::http_api_session::{HttpApiSession, HttpApiSessionInit};

use super::factory::DefaultTools;
use super::{ManagedSession, SessionManagerImpl};

impl SessionManagerImpl {
    pub(super) async fn create_http_api_session(
        &self,
        config: &SessionConfig,
        default_tools: &DefaultTools,
    ) -> Result<Arc<dyn ConversationSession>, CoreError> {
        let surface_id =
            config
                .surface_id
                .as_deref()
                .ok_or_else(|| CoreError::InvalidArguments {
                    code: maekon_core::error_codes::ValidationCode::InvalidArguments,
                    message: "surface_id is required for HttpApi sessions".to_string(),
                })?;

        // Resolve surface spec from the provider catalog.
        // Iter-94: catalog lookup miss = NotFound (the surface id references
        // an entry that doesn't exist in the catalog); not an internal error.
        let surface_spec = provider_specs::provider_surface_spec(surface_id).map_err(|msg| {
            CoreError::NotFound {
                code: maekon_core::error_codes::NotFoundCode::ResourceMissing,
                resource_type: "provider_surface".to_string(),
                id: format!("{surface_id}: {msg}"),
            }
        })?;
        // Iter-94: unknown provider_type = invalid config (the vendor_id
        // doesn't map to any known provider type).
        let provider_type = maekon_core::provider_surface::provider_type_from_vendor_id(
            &surface_spec.provider_type,
        )
        .ok_or_else(|| CoreError::Config {
            code: maekon_core::error_codes::ConfigCode::Invalid,
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
            code: maekon_core::error_codes::NotFoundCode::ResourceMissing,
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
                code: maekon_core::error_codes::ValidationCode::InvalidArguments,
                message: format!(
                    "no model specified and surface '{surface_id}' has no default LLM model"
                ),
            })?;

        let configured = self
            .context_assembler
            .as_ref()
            .and_then(|context| context.ai_provider().llm_api.as_ref());
        let endpoint =
            CredentialSource::http_session_endpoint(configured, surface_id, &transport_spec.url);
        let credential = self.http_session_credential(surface_id, endpoint)?;

        let inner: Arc<dyn ConversationSession> =
            Arc::new(HttpApiSession::new(HttpApiSessionInit {
                surface_id: surface_id.to_string(),
                model,
                endpoint: endpoint.to_string(),
                credential,
                provider_type,
                system_prompt: config.system_prompt.clone(),
                config: self.config.clone(),
                default_tools: default_tools.clone(),
                // D7 (#4812 / E20-20): shared workspace-wide registry from the
                // composition root (iter-011 consolidation done) — same Arc as the
                // other network adapters targeting the same endpoint.
                breaker_registry: self.breaker_registry.clone(),
            }));

        let wrapped: Arc<dyn ConversationSession> = self.decorate_session(inner)?;

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

    /// Credential an HttpApi session on `surface_id` sends to `session_url` (#12541):
    /// the `llm_api` key of the boot-time config the context assembler carries, on
    /// its own surface and host only. Other surfaces, like the Chat page default,
    /// stay not configured (`CredentialSource::for_http_session`).
    pub(crate) fn http_session_credential(
        &self,
        surface_id: &str,
        session_url: &str,
    ) -> Result<CredentialSource, CoreError> {
        let ai = self
            .context_assembler
            .as_ref()
            .map(|context| context.ai_provider());
        let configured = ai.and_then(|ai| ai.llm_api.as_ref());
        // #12563: the store the binding names, as the LLM resolver reads it.
        let binding = configured.and_then(|endpoint| endpoint.credential.as_ref());
        CredentialSource::for_http_session(
            configured,
            ai.map(|ai| ai.active_secret_profile_id_or("llm")),
            surface_id,
            session_url,
            self.secret_stores.for_binding(binding),
        )
        .map_err(|_| CoreError::Auth {
            code: maekon_core::error_codes::AuthCode::Failed,
            message: format!("no credential available for surface '{surface_id}'"),
        })
    }
}
