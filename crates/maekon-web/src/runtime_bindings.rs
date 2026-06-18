use std::path::PathBuf;
use std::sync::Arc;

use maekon_api_contracts::integration::IntegrationOutboundRuntimeStatus;
use maekon_core::config::CredentialBackendKind;
use maekon_core::config_manager::ConfigManager;
use maekon_core::ports::audit_log::AuditLogPort;
use maekon_core::ports::automation::AutomationPort;
use maekon_core::ports::coaching::CoachingPort;
use maekon_core::ports::conversation_session::SessionManager;
use maekon_core::ports::frame_storage::FrameStoragePort;
use maekon_core::ports::integration::{
    IntegrationAuditPort, IntegrationAuthPort, IntegrationInboxPort, IntegrationInboxStorePort,
    IntegrationOutboxPort, IntegrationRuntimeTelemetryPort, IntegrationSessionPort,
};
use maekon_core::ports::memory_graph_port::MemoryGraphPort;
use maekon_core::ports::provider_model_catalog::ProviderModelCatalogPort;
use maekon_core::ports::secret_store::{SecretStore, SecretStoreSet};
use tokio::sync::broadcast;

use crate::update_control::UpdateControl;
use crate::{AiRuntimeStatus, RealtimeEvent};

// #5734: per-call LLM health handle forwarded from the composition root so the
// live GET /api/automation/status can read the true last-call outcome.
// Imported from maekon-core (the canonical definition site) to avoid a
// forbidden adapter-to-adapter dependency on maekon-network.
pub use maekon_core::ports::llm_provider::LlmCallHealth;

#[derive(Clone, Default)]
pub struct CoreRuntimeBindings {
    pub event_tx: Option<broadcast::Sender<RealtimeEvent>>,
    pub frames_dir: Option<PathBuf>,
    pub frame_storage: Option<Arc<dyn FrameStoragePort>>,
    pub config_manager: Option<ConfigManager>,
    pub update_control: Option<UpdateControl>,
    /// ADR-023: local memory-graph store, so the digest export endpoint can
    /// render accumulated claims (`DigestExporter::to_markdown_with_claims`).
    pub memory_graph: Option<Arc<dyn MemoryGraphPort>>,
    /// #4478 G3: one-shot signal the "Delete all data" endpoint sets so the
    /// SyncEngine propagates a device-wide erasure to LAN peers.
    pub erasure_requested: Option<Arc<std::sync::atomic::AtomicBool>>,
}

#[derive(Clone, Default)]
pub struct SecretRuntimeBindings {
    pub default_secret_backend_kind: Option<CredentialBackendKind>,
    pub secret_store: Option<Arc<dyn SecretStore>>,
    pub secret_stores: Option<SecretStoreSet>,
}

#[derive(Clone, Default)]
pub struct AutomationRuntimeBindings {
    pub audit_logger: Option<Arc<dyn AuditLogPort>>,
    pub automation_controller: Option<Arc<dyn AutomationPort>>,
    pub ai_runtime_status: Option<AiRuntimeStatus>,
    /// #5734: per-call LLM health handle. `None` means health tracking is not
    /// wired (standalone web server, tests, CLI arm). Cloned into `AutomationState`
    /// and surfaced as `AutomationStatusDto.llm_healthy` at request time.
    pub llm_call_health: Option<Arc<LlmCallHealth>>,
}

#[derive(Clone, Default)]
pub struct IntegrationRuntimeBindings {
    pub integration_runtime_status: Option<IntegrationOutboundRuntimeStatus>,
    pub integration_auth: Option<Arc<dyn IntegrationAuthPort>>,
    pub integration_session: Option<Arc<dyn IntegrationSessionPort>>,
    pub integration_outbox: Option<Arc<dyn IntegrationOutboxPort>>,
    pub integration_inbox: Option<Arc<dyn IntegrationInboxPort>>,
    pub integration_inbox_store: Option<Arc<dyn IntegrationInboxStorePort>>,
    pub integration_audit: Option<Arc<dyn IntegrationAuditPort>>,
    pub integration_runtime_telemetry: Option<Arc<dyn IntegrationRuntimeTelemetryPort>>,
}

#[derive(Clone, Default)]
pub struct AnalysisRuntimeBindings {
    pub override_store: Option<Arc<dyn maekon_core::ports::override_store::OverrideStore>>,
    pub recluster_requested: Option<Arc<std::sync::atomic::AtomicBool>>,
    pub coaching_engine: Option<Arc<dyn CoachingPort>>,
    pub model_catalog_client: Option<Arc<dyn ProviderModelCatalogPort>>,
    /// #6279: keyword/FTS text-search provider. Without this the
    /// `/api/semantic-search` endpoint is permanently inert (every mode returns
    /// service.unavailable). Wiring it (SqliteStorage impls TextSearchProvider)
    /// makes Keyword + the Hybrid-degraded path work; the vector-only ports
    /// below remain optional (semantic-vector mode stays unavailable until the
    /// embedding pipeline is threaded).
    pub text_search: Option<Arc<dyn maekon_core::ports::text_search::TextSearchProvider>>,
}

#[derive(Clone, Default)]
pub struct SessionRuntimeBindings {
    pub session_manager: Option<Arc<dyn SessionManager>>,
}

#[derive(Clone, Default)]
pub struct WebServerRuntimeBindings {
    pub core: CoreRuntimeBindings,
    pub secrets: SecretRuntimeBindings,
    pub automation: AutomationRuntimeBindings,
    pub integration: IntegrationRuntimeBindings,
    pub analysis: AnalysisRuntimeBindings,
    pub session: SessionRuntimeBindings,
}
