//! Domain-scoped AppState sub-structs.
//!
//! AppState fields are grouped by domain concern. Sub-structs with `Default`
//! impls mean adding a new field never requires updating test construction sites.

use std::path::PathBuf;
use std::sync::Arc;

use maekon_api_contracts::bug_report::BugReportBundleDto;
use maekon_api_contracts::integration::IntegrationOutboundRuntimeStatus;
use maekon_core::config::CredentialBackendKind;
use maekon_core::config_manager::ConfigManager;
use maekon_core::ports::adaptive_search::AdaptiveSearchPort;
use maekon_core::ports::audit_log::AuditLogPort;
use maekon_core::ports::automation::AutomationPort;
use maekon_core::ports::coaching::CoachingPort;
use maekon_core::ports::conversation_session::SessionManager;
use maekon_core::ports::embedding_provider::EmbeddingProvider;
use maekon_core::ports::frame_storage::FrameStoragePort;
use maekon_core::ports::integration::{
    IntegrationAuditPort, IntegrationAuthPort, IntegrationInboxPort, IntegrationInboxStorePort,
    IntegrationOutboxPort, IntegrationRuntimeTelemetryPort, IntegrationSessionPort,
};
use maekon_core::ports::memory_graph_port::MemoryGraphPort;
use maekon_core::ports::override_store::OverrideStore;
use maekon_core::ports::pii_sanitizer::PiiSanitizer;
use maekon_core::ports::provider_model_catalog::ProviderModelCatalogPort;
use maekon_core::ports::runtime_log_provider::RuntimeLogProvider;
use maekon_core::ports::secret_store::{SecretStore, SecretStoreSet};
use maekon_core::ports::system_info_provider::SystemInfoProvider;
use maekon_core::ports::text_search::TextSearchProvider;
use maekon_core::ports::vector_store::VectorStore;
use tokio::sync::broadcast;

use crate::services::provider_cli_diagnostics::ProviderCliDiagnosticsProvider;
use crate::update_control::UpdateControl;
use crate::{AiRuntimeStatus, RealtimeEvent, WebStorage};

// ---------------------------------------------------------------------------
// Sub-structs
// ---------------------------------------------------------------------------

/// Core infrastructure — storage, event bus, config. Contains required fields
/// (`storage`, `event_tx`) so does NOT implement `Default`.
#[derive(Clone)]
pub struct CoreState {
    pub storage: Arc<dyn WebStorage>,
    pub event_tx: broadcast::Sender<RealtimeEvent>,
    pub frames_dir: Option<PathBuf>,
    pub frame_storage: Option<Arc<dyn FrameStoragePort>>,
    pub config_manager: Option<ConfigManager>,
    pub update_control: Option<UpdateControl>,
    /// ADR-023: local memory-graph store (the same `SqliteStorage` as `storage`,
    /// as a `MemoryGraphPort`). Lets the digest export render accumulated claims.
    pub memory_graph: Option<Arc<dyn MemoryGraphPort>>,
    /// #4478 G3: one-shot erasure-propagation signal shared with the SyncEngine;
    /// the "Delete all data" endpoint sets it so a local erasure reaches LAN peers.
    pub erasure_requested: Option<Arc<std::sync::atomic::AtomicBool>>,
}

/// Per-session local-API authentication (E20-41 #4833).
///
/// `local_auth_token` is an ephemeral, process-lifetime random token generated
/// at app launch (NEVER persisted to config.json, NEVER exposed over HTTP). The
/// `require_local_auth` middleware requires it on every `/api` request so that on
/// a multi-user host (RDP/Citrix) a different local user — who reaches the same
/// loopback port but never transited the legit Tauri WebView — cannot read this
/// user's settings or audit export. `None` ⇒ the gate fails closed (401).
#[derive(Clone, Default)]
pub struct AuthState {
    pub local_auth_token: Option<Arc<str>>,
}

/// Secret management — credential backends and stores.
#[derive(Clone)]
pub struct SecretState {
    pub default_backend_kind: CredentialBackendKind,
    pub store: Option<Arc<dyn SecretStore>>,
    pub stores: Option<SecretStoreSet>,
}

impl Default for SecretState {
    fn default() -> Self {
        Self {
            default_backend_kind: CredentialBackendKind::Unavailable,
            store: None,
            stores: None,
        }
    }
}

/// Audit logging, automation control, AI runtime status.
#[derive(Clone, Default)]
pub struct AutomationState {
    pub audit_logger: Option<Arc<dyn AuditLogPort>>,
    pub controller: Option<Arc<dyn AutomationPort>>,
    pub ai_runtime_status: Option<AiRuntimeStatus>,
    /// #5734: per-call LLM health handle. `None` in tests / standalone web-server
    /// builds where no Ollama provider is wired. Surfaced as
    /// `AutomationStatusDto.llm_healthy` by reading `as_option_bool()` at request
    /// time (live value, not the build-time SSE snapshot).
    pub llm_call_health: Option<Arc<maekon_core::ports::llm_provider::LlmCallHealth>>,
}

/// External system integration — 8 port fields.
#[derive(Clone, Default)]
pub struct IntegrationState {
    pub runtime_status: Option<IntegrationOutboundRuntimeStatus>,
    pub auth: Option<Arc<dyn IntegrationAuthPort>>,
    pub session: Option<Arc<dyn IntegrationSessionPort>>,
    pub outbox: Option<Arc<dyn IntegrationOutboxPort>>,
    pub inbox: Option<Arc<dyn IntegrationInboxPort>>,
    pub inbox_store: Option<Arc<dyn IntegrationInboxStorePort>>,
    pub audit: Option<Arc<dyn IntegrationAuditPort>>,
    pub runtime_telemetry: Option<Arc<dyn IntegrationRuntimeTelemetryPort>>,
}

/// Analysis, search, coaching — vector/embedding/text search + coaching engine.
#[derive(Clone, Default)]
pub struct AnalysisState {
    pub vector_store: Option<Arc<dyn VectorStore>>,
    pub embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    pub text_search: Option<Arc<dyn TextSearchProvider>>,
    pub adaptive_search: Option<Arc<dyn AdaptiveSearchPort>>,
    pub model_catalog_client: Option<Arc<dyn ProviderModelCatalogPort>>,
    pub override_store: Option<Arc<dyn OverrideStore>>,
    pub recluster_requested: Option<Arc<std::sync::atomic::AtomicBool>>,
    pub coaching_engine: Option<Arc<dyn CoachingPort>>,
}

/// Session management — conversation sessions + pomodoro.
#[derive(Clone)]
pub struct SessionState {
    pub manager: Option<Arc<dyn SessionManager>>,
    pub pomodoro: Arc<std::sync::Mutex<Option<maekon_core::models::pomodoro::PomodoroSession>>>,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            manager: None,
            pomodoro: Arc::new(std::sync::Mutex::new(None)),
        }
    }
}

/// PII sanitization, bug reports, runtime logs, system info.
#[derive(Clone)]
pub struct DiagnosticsState {
    pub pii_sanitizer: Option<Arc<dyn PiiSanitizer>>,
    pub latest_bug_report: Arc<parking_lot::RwLock<Option<BugReportBundleDto>>>,
    pub runtime_log_provider: Option<Arc<dyn RuntimeLogProvider>>,
    pub system_info_provider: Option<Arc<dyn SystemInfoProvider>>,
    pub provider_cli_diagnostics: Option<Arc<dyn ProviderCliDiagnosticsProvider>>,

    // Task 7.1 — live-config REST endpoint (GET /api/external-grpc/live-config).
    // Populated from build_external_spawn_config return value when external gRPC is enabled.
    #[cfg(feature = "grpc-dashboard-external")]
    pub external_grpc_live: Option<Arc<crate::grpc::external::live_config::LiveExternalConfig>>,
    #[cfg(feature = "grpc-dashboard-external")]
    pub external_grpc_metrics: Option<Arc<crate::grpc::external::metrics::ExternalMetrics>>,
}

impl Default for DiagnosticsState {
    fn default() -> Self {
        Self {
            pii_sanitizer: None,
            latest_bug_report: Arc::new(parking_lot::RwLock::new(None)),
            runtime_log_provider: None,
            system_info_provider: None,
            provider_cli_diagnostics: None,
            #[cfg(feature = "grpc-dashboard-external")]
            external_grpc_live: None,
            #[cfg(feature = "grpc-dashboard-external")]
            external_grpc_metrics: None,
        }
    }
}

// ---------------------------------------------------------------------------
// AppState
// ---------------------------------------------------------------------------

/// Application-wide shared state, grouped by domain concern.
#[derive(Clone)]
pub struct AppState {
    pub core: CoreState,
    pub auth: AuthState,
    pub secrets: SecretState,
    pub automation: AutomationState,
    pub integration: IntegrationState,
    pub analysis: AnalysisState,
    pub session: SessionState,
    pub diagnostics: DiagnosticsState,
}

impl AppState {
    /// Create AppState with required core fields; all other sub-states default to empty.
    pub fn with_core(
        storage: Arc<dyn WebStorage>,
        event_tx: broadcast::Sender<RealtimeEvent>,
    ) -> Self {
        Self {
            core: CoreState {
                storage,
                event_tx,
                frames_dir: None,
                frame_storage: None,
                config_manager: None,
                update_control: None,
                memory_graph: None,
                erasure_requested: None,
            },
            auth: Default::default(),
            secrets: Default::default(),
            automation: Default::default(),
            integration: Default::default(),
            analysis: Default::default(),
            session: Default::default(),
            diagnostics: Default::default(),
        }
    }
}
