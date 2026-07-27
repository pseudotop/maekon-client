//! Domain-scoped AppState sub-structs.
//!
//! AppState fields are grouped by domain concern. Sub-structs with `Default`
//! impls mean adding a new field never requires updating test construction sites.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use maekon_api_contracts::bug_report::BugReportBundleDto;
use maekon_api_contracts::integration::IntegrationOutboundRuntimeStatus;
use maekon_core::config::CredentialBackendKind;
use maekon_core::config_manager::ConfigManager;
use maekon_core::ports::adaptive_search::AdaptiveSearchPort;
use maekon_core::ports::audit_chain_verifier::AuditChainVerifierPort;
use maekon_core::ports::audit_log::AuditLogPort;
use maekon_core::ports::automation::AutomationPort;
use maekon_core::ports::coaching::CoachingPort;
use maekon_core::ports::conversation_session::SessionManager;
use maekon_core::ports::egress_ledger_reader::EgressLedgerReaderPort;
use maekon_core::ports::embedding_provider::EmbeddingProvider;
use maekon_core::ports::frame_storage::FrameStoragePort;
use maekon_core::ports::integration::{
    IntegrationAuditPort, IntegrationAuthPort, IntegrationInboxPort, IntegrationInboxStorePort,
    IntegrationOutboxPort, IntegrationRuntimeTelemetryPort, IntegrationSessionPort,
};
use maekon_core::ports::memory_graph_port::MemoryGraphPort;
use maekon_core::ports::override_store::OverrideStore;
use maekon_core::ports::pii_sanitizer::PiiSanitizer;
use maekon_core::ports::pomodoro_store::PomodoroStorePort;
use maekon_core::ports::provider_model_catalog::ProviderModelCatalogPort;
use maekon_core::ports::regime_storage::RegimeStoragePort;
use maekon_core::ports::runtime_log_provider::RuntimeLogProvider;
use maekon_core::ports::secret_store::{SecretStore, SecretStoreSet};
use maekon_core::ports::system_info_provider::SystemInfoProvider;
use maekon_core::ports::text_search::TextSearchProvider;
use maekon_core::ports::vector_store::VectorStore;
use tokio::sync::{broadcast, Mutex};
use tokio::time::Instant;

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
    /// #7600: durable audit-log hash-chain verifier (the same `SqliteStorage`
    /// as `storage`, as an `AuditChainVerifierPort`). Lets `GET /audit/verify`
    /// reach the real ADR-072 verification instead of the compliance
    /// capability being reachable only via the desktop `verify_audit_log` IPC
    /// command.
    pub audit_chain_verifier: Option<Arc<dyn AuditChainVerifierPort>>,
    /// #7910: read-only egress-ledger reader (the same `SqliteStorage` as
    /// `storage`, cast to an `EgressLedgerReaderPort`). Lets
    /// `GET /api/privacy/egress-ledger` render the egress transparency browser
    /// ("what left this device") from the erase-retained #4803 ledger. Read
    /// only — no mutation surface, since the ledger is compliance evidence.
    pub egress_ledger_reader: Option<Arc<dyn EgressLedgerReaderPort>>,
    /// #7678 D2: regime storage (the same `SqliteStorage`-backed store used by
    /// the scheduler, as a `RegimeStoragePort`). Lets the dashboard digest
    /// endpoint resolve human-readable regime labels (name > auto_label)
    /// instead of leaking the opaque positional `regime_id` ("regime-N") into
    /// the timeline (mirrors the #7480 coaching-path fix).
    pub regime_storage: Option<Arc<dyn RegimeStoragePort>>,
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
    pub(crate) integration_auth_rate_limiter: IntegrationAuthRateLimiter,
    /// Capture-history viewing re-authentication gate (#8044). A "view" gate
    /// distinct from `local_auth_token` (the session token) — it blocks a
    /// physical accessor who opens the captured screenshot timeline on an
    /// already-authenticated session. Defaults to a **disabled** gate
    /// (viewing allowed); the composition root injects an enabled gate from
    /// `config.privacy.reauth`. Shares the **same `Arc`** as the Tauri
    /// re-auth command, so this middleware immediately sees a gate the
    /// command opened via `record_success()`.
    pub reauth_gate: Arc<maekon_core::reauth::CaptureReauthGate>,
}

pub(crate) const INTEGRATION_AUTH_FAILURE_LIMIT: u32 = 5;
const INTEGRATION_AUTH_FAILURE_WINDOW: Duration = Duration::from_secs(60);
const INTEGRATION_AUTH_LOCKOUT_DURATION: Duration = Duration::from_secs(60);

#[derive(Clone, Default)]
pub(crate) struct IntegrationAuthRateLimiter {
    attempts_by_ip: Arc<Mutex<HashMap<IpAddr, IntegrationAuthAttempt>>>,
}

#[derive(Debug, Clone)]
struct IntegrationAuthAttempt {
    failures: u32,
    window_started_at: Instant,
    locked_until: Option<Instant>,
}

impl IntegrationAuthRateLimiter {
    pub(crate) async fn is_locked_out(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let mut attempts_by_ip = self.attempts_by_ip.lock().await;
        let Some(attempt) = attempts_by_ip.get(&ip) else {
            return false;
        };
        let Some(locked_until) = attempt.locked_until else {
            return false;
        };
        if locked_until > now {
            return true;
        }
        attempts_by_ip.remove(&ip);
        false
    }

    pub(crate) async fn record_failure(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let mut attempts_by_ip = self.attempts_by_ip.lock().await;
        let locked = {
            let attempt = attempts_by_ip.entry(ip).or_insert(IntegrationAuthAttempt {
                failures: 0,
                window_started_at: now,
                locked_until: None,
            });

            if attempt
                .locked_until
                .is_some_and(|locked_until| locked_until > now)
            {
                true
            } else {
                if attempt.locked_until.is_some() {
                    attempt.failures = 0;
                    attempt.window_started_at = now;
                    attempt.locked_until = None;
                }

                if now.duration_since(attempt.window_started_at) > INTEGRATION_AUTH_FAILURE_WINDOW {
                    attempt.failures = 0;
                    attempt.window_started_at = now;
                }

                attempt.failures = attempt.failures.saturating_add(1);
                if attempt.failures >= INTEGRATION_AUTH_FAILURE_LIMIT {
                    attempt.locked_until = Some(now + INTEGRATION_AUTH_LOCKOUT_DURATION);
                    true
                } else {
                    false
                }
            }
        };
        drop(attempts_by_ip);

        locked
    }

    pub(crate) async fn record_success(&self, ip: IpAddr) {
        self.attempts_by_ip.lock().await.remove(&ip);
    }
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
    /// #6117: single, server-lifetime settings-policy audit writer.
    ///
    /// Built ONCE in `WebServer::build_router` (from `audit_logger`) and shared
    /// by reference through every per-request `SettingsWebContext` clone, so the
    /// bounded-channel drain task lives for the whole server lifetime instead of
    /// being spawned and aborted per `POST /api/settings`. The prior per-request
    /// writer was dropped at request end, aborting its drain task before the
    /// fire-and-forget audit event was flushed, so security-policy audit events
    /// were lost on every settings save.
    ///
    /// `pub(crate)` because `PolicyAuditWriter` is a crate-internal type.
    pub(crate) policy_audit_writer:
        Option<Arc<crate::services::settings_policy_service::PolicyAuditWriter>>,
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
    pub pomodoro: Arc<Mutex<PomodoroRuntimeState>>,
    pub pomodoro_store: Option<Arc<dyn PomodoroStorePort>>,
}

#[derive(Default)]
pub struct PomodoroRuntimeState {
    pub session: Option<maekon_core::models::pomodoro::PomodoroSession>,
    pub hydrated: bool,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            manager: None,
            pomodoro: Arc::new(Mutex::new(PomodoroRuntimeState::default())),
            pomodoro_store: None,
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
                audit_chain_verifier: None,
                egress_ledger_reader: None,
                regime_storage: None,
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
