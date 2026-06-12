use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::FromRef;
use maekon_api_contracts::bug_report::BugReportBundleDto;
use maekon_api_contracts::stream::RealtimeEvent;
use maekon_core::config::CredentialBackendKind;
use maekon_core::config_manager::ConfigManager;
use maekon_core::ports::audit_log::AuditLogPort;
use maekon_core::ports::automation::AutomationPort;
use maekon_core::ports::coaching::CoachingPort;
use maekon_core::ports::frame_storage::FrameStoragePort;
use maekon_core::ports::pii_sanitizer::PiiSanitizer;
use maekon_core::ports::provider_model_catalog::ProviderModelCatalogPort;
use maekon_core::ports::runtime_log_provider::RuntimeLogProvider;
use maekon_core::ports::secret_store::{SecretStore, SecretStoreSet};
use maekon_core::ports::system_info_provider::SystemInfoProvider;

use maekon_core::ports::conversation_session::SessionManager;

use crate::services::integration_assembler::IntegrationStatusConfigSnapshot;
use crate::services::provider_cli_diagnostics::ProviderCliDiagnosticsProvider;
use crate::storage_port::WebStorage;
use crate::update_control::UpdateControl;
use crate::AiRuntimeStatus;
use crate::AppState;

#[derive(Clone)]
pub struct StorageWebContext {
    pub storage: Arc<dyn WebStorage>,
    pub frames_dir: Option<PathBuf>,
    pub frame_storage: Option<Arc<dyn FrameStoragePort>>,
    /// D5 iter-15: PII sanitizer for export-handler belt-and-suspenders
    /// sanitization. Cloned from AppState.diagnostics.pii_sanitizer at
    /// context construction time.
    pub pii_sanitizer: Option<Arc<dyn PiiSanitizer>>,
    /// #4478 G3: one-shot erasure-propagation signal the `delete_all_data`
    /// service sets so a local GDPR erasure reaches LAN sync peers.
    pub erasure_requested: Option<Arc<std::sync::atomic::AtomicBool>>,
}

impl StorageWebContext {
    pub fn from_state(state: &AppState) -> Self {
        Self {
            storage: state.core.storage.clone(),
            frames_dir: state.core.frames_dir.clone(),
            frame_storage: state.core.frame_storage.clone(),
            pii_sanitizer: state.diagnostics.pii_sanitizer.clone(),
            erasure_requested: state.core.erasure_requested.clone(),
        }
    }
}

impl FromRef<AppState> for StorageWebContext {
    fn from_ref(state: &AppState) -> Self {
        Self::from_state(state)
    }
}

#[derive(Clone)]
pub struct UpdateWebContext {
    pub update_control: Option<UpdateControl>,
}

impl UpdateWebContext {
    pub fn from_state(state: &AppState) -> Self {
        Self {
            update_control: state.core.update_control.clone(),
        }
    }
}

impl FromRef<AppState> for UpdateWebContext {
    fn from_ref(state: &AppState) -> Self {
        Self::from_state(state)
    }
}

#[derive(Clone)]
pub struct ConfigWebContext {
    pub config_manager: Option<ConfigManager>,
}

impl ConfigWebContext {
    pub fn from_state(state: &AppState) -> Self {
        Self {
            config_manager: state.core.config_manager.clone(),
        }
    }
}

impl FromRef<AppState> for ConfigWebContext {
    fn from_ref(state: &AppState) -> Self {
        Self::from_state(state)
    }
}

#[derive(Clone)]
pub struct BackupWebContext {
    pub storage: Arc<dyn WebStorage>,
    pub config_manager: Option<ConfigManager>,
}

impl BackupWebContext {
    pub fn from_state(state: &AppState) -> Self {
        Self {
            storage: state.core.storage.clone(),
            config_manager: state.core.config_manager.clone(),
        }
    }
}

impl FromRef<AppState> for BackupWebContext {
    fn from_ref(state: &AppState) -> Self {
        Self::from_state(state)
    }
}

#[derive(Clone)]
pub struct SettingsWebContext {
    pub(crate) storage: Arc<dyn WebStorage>,
    pub(crate) frames_dir: Option<PathBuf>,
    pub(crate) config_manager: Option<ConfigManager>,
    pub(crate) default_secret_backend_kind: CredentialBackendKind,
    pub(crate) secret_store: Option<Arc<dyn SecretStore>>,
    pub(crate) secret_stores: Option<SecretStoreSet>,
    pub(crate) audit_logger: Option<Arc<dyn AuditLogPort>>,
    /// #5707: 설정 저장 후 coaching 엔진에 hot-reload 신호를 보내기 위한 핸들.
    /// `None`이면 엔진이 연결되지 않은 환경(단독 웹 서버, 테스트)이므로 skip.
    pub(crate) coaching_engine: Option<Arc<dyn CoachingPort>>,
}

impl SettingsWebContext {
    pub fn from_state(state: &AppState) -> Self {
        Self {
            storage: state.core.storage.clone(),
            frames_dir: state.core.frames_dir.clone(),
            config_manager: state.core.config_manager.clone(),
            default_secret_backend_kind: state.secrets.default_backend_kind,
            secret_store: state.secrets.store.clone(),
            secret_stores: state.secrets.stores.clone(),
            audit_logger: state.automation.audit_logger.clone(),
            coaching_engine: state.analysis.coaching_engine.clone(),
        }
    }
}

impl FromRef<AppState> for SettingsWebContext {
    fn from_ref(state: &AppState) -> Self {
        Self::from_state(state)
    }
}

#[derive(Clone)]
pub struct AiModelCatalogWebContext {
    pub(crate) config_manager: Option<ConfigManager>,
    pub(crate) secret_store: Option<Arc<dyn SecretStore>>,
    pub(crate) secret_stores: Option<SecretStoreSet>,
    pub(crate) model_catalog_client: Option<Arc<dyn ProviderModelCatalogPort>>,
}

impl AiModelCatalogWebContext {
    pub fn from_state(state: &AppState) -> Self {
        Self {
            config_manager: state.core.config_manager.clone(),
            secret_store: state.secrets.store.clone(),
            secret_stores: state.secrets.stores.clone(),
            model_catalog_client: state.analysis.model_catalog_client.clone(),
        }
    }
}

impl FromRef<AppState> for AiModelCatalogWebContext {
    fn from_ref(state: &AppState) -> Self {
        Self::from_state(state)
    }
}

#[derive(Clone)]
pub struct SupportDiagnosticsContext {
    pub settings: SettingsWebContext,
    pub frames_dir: Option<PathBuf>,
    pub config_manager_configured: bool,
    pub automation_controller_configured: bool,
    pub update_control_configured: bool,
    pub audit_logger: Option<Arc<dyn AuditLogPort>>,
    pub provider_cli_diagnostics: Option<Arc<dyn ProviderCliDiagnosticsProvider>>,
}

impl SupportDiagnosticsContext {
    pub fn from_state(state: &AppState) -> Self {
        Self {
            settings: SettingsWebContext::from_state(state),
            frames_dir: state.core.frames_dir.clone(),
            config_manager_configured: state.core.config_manager.is_some(),
            automation_controller_configured: state.automation.controller.is_some(),
            update_control_configured: state.core.update_control.is_some(),
            audit_logger: state.automation.audit_logger.clone(),
            provider_cli_diagnostics: state.diagnostics.provider_cli_diagnostics.clone(),
        }
    }
}

impl FromRef<AppState> for SupportDiagnosticsContext {
    fn from_ref(state: &AppState) -> Self {
        Self::from_state(state)
    }
}

#[derive(Clone)]
pub struct AutomationWebContext {
    pub storage: Arc<dyn WebStorage>,
    pub frames_dir: Option<PathBuf>,
    pub config_manager: Option<ConfigManager>,
    pub audit_logger: Option<Arc<dyn AuditLogPort>>,
    pub automation_controller: Option<Arc<dyn AutomationPort>>,
    pub ai_runtime_status: Option<AiRuntimeStatus>,
    /// #5734: per-call LLM health handle. `None` when health tracking is not wired.
    /// Read via `as_option_bool()` at request time to produce the live
    /// `AutomationStatusDto.llm_healthy` value (not a build-time snapshot).
    pub llm_call_health: Option<std::sync::Arc<maekon_core::ports::llm_provider::LlmCallHealth>>,
}

impl AutomationWebContext {
    pub fn from_state(state: &AppState) -> Self {
        Self {
            storage: state.core.storage.clone(),
            frames_dir: state.core.frames_dir.clone(),
            config_manager: state.core.config_manager.clone(),
            audit_logger: state.automation.audit_logger.clone(),
            automation_controller: state.automation.controller.clone(),
            ai_runtime_status: state.automation.ai_runtime_status.clone(),
            llm_call_health: state.automation.llm_call_health.clone(),
        }
    }
}

impl FromRef<AppState> for AutomationWebContext {
    fn from_ref(state: &AppState) -> Self {
        Self::from_state(state)
    }
}

#[derive(Clone)]
pub struct AutomationGuiWebContext {
    pub(crate) automation_controller: Option<Arc<dyn AutomationPort>>,
}

impl AutomationGuiWebContext {
    pub fn from_state(state: &AppState) -> Self {
        Self {
            automation_controller: state.automation.controller.clone(),
        }
    }
}

impl FromRef<AppState> for AutomationGuiWebContext {
    fn from_ref(state: &AppState) -> Self {
        Self::from_state(state)
    }
}

#[derive(Clone)]
pub struct RealtimeStreamWebContext {
    pub ai_runtime_status: Option<AiRuntimeStatus>,
    pub event_tx: tokio::sync::broadcast::Sender<RealtimeEvent>,
}

impl RealtimeStreamWebContext {
    pub fn from_state(state: &AppState) -> Self {
        Self {
            ai_runtime_status: state.automation.ai_runtime_status.clone(),
            event_tx: state.core.event_tx.clone(),
        }
    }
}

impl FromRef<AppState> for RealtimeStreamWebContext {
    fn from_ref(state: &AppState) -> Self {
        Self::from_state(state)
    }
}

#[derive(Clone)]
pub struct IntegrationWebContext {
    pub(crate) config: IntegrationStatusConfigSnapshot,
    pub(crate) automation_controller_configured: bool,
    pub(crate) ai_runtime_status: Option<AiRuntimeStatus>,
    pub(crate) runtime_status_seed:
        maekon_api_contracts::integration::IntegrationOutboundRuntimeStatus,
    pub(crate) auth: Option<Arc<dyn maekon_core::ports::integration::IntegrationAuthPort>>,
    pub(crate) session: Option<Arc<dyn maekon_core::ports::integration::IntegrationSessionPort>>,
    pub(crate) outbox: Option<Arc<dyn maekon_core::ports::integration::IntegrationOutboxPort>>,
    pub(crate) inbox: Option<Arc<dyn maekon_core::ports::integration::IntegrationInboxPort>>,
    pub(crate) inbox_store:
        Option<Arc<dyn maekon_core::ports::integration::IntegrationInboxStorePort>>,
    pub(crate) audit: Option<Arc<dyn maekon_core::ports::integration::IntegrationAuditPort>>,
    pub(crate) telemetry:
        Option<Arc<dyn maekon_core::ports::integration::IntegrationRuntimeTelemetryPort>>,
}

impl IntegrationWebContext {
    pub fn from_state(state: &AppState) -> Self {
        Self {
            config: IntegrationStatusConfigSnapshot::from_state(state),
            automation_controller_configured: state.automation.controller.is_some(),
            ai_runtime_status: state.automation.ai_runtime_status.clone(),
            runtime_status_seed: state.integration.runtime_status.clone().unwrap_or_default(),
            auth: state.integration.auth.clone(),
            session: state.integration.session.clone(),
            outbox: state.integration.outbox.clone(),
            inbox: state.integration.inbox.clone(),
            inbox_store: state.integration.inbox_store.clone(),
            audit: state.integration.audit.clone(),
            telemetry: state.integration.runtime_telemetry.clone(),
        }
    }
}

impl FromRef<AppState> for IntegrationWebContext {
    fn from_ref(state: &AppState) -> Self {
        Self::from_state(state)
    }
}

#[derive(Clone)]
pub struct AiSessionWebContext {
    pub session_manager: Option<Arc<dyn SessionManager>>,
}

impl AiSessionWebContext {
    pub fn from_state(state: &AppState) -> Self {
        Self {
            session_manager: state.session.manager.clone(),
        }
    }
}

impl FromRef<AppState> for AiSessionWebContext {
    fn from_ref(state: &AppState) -> Self {
        Self::from_state(state)
    }
}

#[derive(Clone)]
pub struct BugReportContext {
    pub support: SupportDiagnosticsContext,
    pub pii_sanitizer: Option<Arc<dyn PiiSanitizer>>,
    pub runtime_log_provider: Option<Arc<dyn RuntimeLogProvider>>,
    pub system_info_provider: Option<Arc<dyn SystemInfoProvider>>,
    pub latest: Arc<parking_lot::RwLock<Option<BugReportBundleDto>>>,
}

impl FromRef<AppState> for BugReportContext {
    fn from_ref(state: &AppState) -> Self {
        Self {
            support: SupportDiagnosticsContext::from_ref(state),
            pii_sanitizer: state.diagnostics.pii_sanitizer.clone(),
            runtime_log_provider: state.diagnostics.runtime_log_provider.clone(),
            system_info_provider: state.diagnostics.system_info_provider.clone(),
            latest: state.diagnostics.latest_bug_report.clone(),
        }
    }
}
