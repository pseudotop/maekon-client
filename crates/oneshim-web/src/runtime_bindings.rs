use std::path::PathBuf;
use std::sync::Arc;

use oneshim_api_contracts::integration::IntegrationOutboundRuntimeStatus;
use oneshim_core::config::CredentialBackendKind;
use oneshim_core::config_manager::ConfigManager;
use oneshim_core::ports::audit_log::AuditLogPort;
use oneshim_core::ports::automation::AutomationPort;
use oneshim_core::ports::coaching::CoachingPort;
use oneshim_core::ports::conversation_session::SessionManager;
use oneshim_core::ports::frame_storage::FrameStoragePort;
use oneshim_core::ports::integration::{
    IntegrationAuditPort, IntegrationAuthPort, IntegrationInboxPort, IntegrationInboxStorePort,
    IntegrationOutboxPort, IntegrationRuntimeTelemetryPort, IntegrationSessionPort,
};
use oneshim_core::ports::secret_store::{SecretStore, SecretStoreSet};
use tokio::sync::broadcast;

use crate::update_control::UpdateControl;
use crate::{AiRuntimeStatus, RealtimeEvent};

#[derive(Clone, Default)]
pub struct CoreRuntimeBindings {
    pub event_tx: Option<broadcast::Sender<RealtimeEvent>>,
    pub frames_dir: Option<PathBuf>,
    pub frame_storage: Option<Arc<dyn FrameStoragePort>>,
    pub config_manager: Option<ConfigManager>,
    pub update_control: Option<UpdateControl>,
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
    pub override_store: Option<Arc<dyn oneshim_core::ports::override_store::OverrideStore>>,
    pub recluster_requested: Option<Arc<std::sync::atomic::AtomicBool>>,
    pub coaching_engine: Option<Arc<dyn CoachingPort>>,
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
