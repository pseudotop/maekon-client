//! Domain-scoped AppState sub-structs.
//!
//! AppState fields are grouped by domain concern. Sub-structs with `Default`
//! impls mean adding a new field never requires updating test construction sites.

use std::path::PathBuf;
use std::sync::Arc;

use oneshim_api_contracts::bug_report::BugReportBundleDto;
use oneshim_api_contracts::integration::IntegrationOutboundRuntimeStatus;
use oneshim_core::config::CredentialBackendKind;
use oneshim_core::config_manager::ConfigManager;
use oneshim_core::ports::adaptive_search::AdaptiveSearchPort;
use oneshim_core::ports::audit_log::AuditLogPort;
use oneshim_core::ports::automation::AutomationPort;
use oneshim_core::ports::coaching::CoachingPort;
use oneshim_core::ports::conversation_session::SessionManager;
use oneshim_core::ports::embedding_provider::EmbeddingProvider;
use oneshim_core::ports::frame_storage::FrameStoragePort;
use oneshim_core::ports::integration::{
    IntegrationAuditPort, IntegrationAuthPort, IntegrationInboxPort, IntegrationInboxStorePort,
    IntegrationOutboxPort, IntegrationRuntimeTelemetryPort, IntegrationSessionPort,
};
use oneshim_core::ports::override_store::OverrideStore;
use oneshim_core::ports::pii_sanitizer::PiiSanitizer;
use oneshim_core::ports::runtime_log_provider::RuntimeLogProvider;
use oneshim_core::ports::secret_store::{SecretStore, SecretStoreSet};
use oneshim_core::ports::system_info_provider::SystemInfoProvider;
use oneshim_core::ports::text_search::TextSearchProvider;
use oneshim_core::ports::vector_store::VectorStore;
use tokio::sync::broadcast;

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
    pub override_store: Option<Arc<dyn OverrideStore>>,
    pub recluster_requested: Option<Arc<std::sync::atomic::AtomicBool>>,
    pub coaching_engine: Option<Arc<dyn CoachingPort>>,
}

/// Session management — conversation sessions + pomodoro.
#[derive(Clone)]
pub struct SessionState {
    pub manager: Option<Arc<dyn SessionManager>>,
    pub pomodoro: Arc<std::sync::Mutex<Option<oneshim_core::models::pomodoro::PomodoroSession>>>,
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
            },
            secrets: Default::default(),
            automation: Default::default(),
            integration: Default::default(),
            analysis: Default::default(),
            session: Default::default(),
            diagnostics: Default::default(),
        }
    }
}
