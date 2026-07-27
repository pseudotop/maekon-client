//! Domain-scoped AppState sub-structs.
//!
//! AppState fields are grouped by domain concern. Sub-structs with `Default`
//! impls mean adding a new field never requires updating test construction sites.
//!
//! #7738 D-3: this struct now carries ONLY the optional/conditional/cfg
//! fields — the fields that are runtime-conditional by design (an automation
//! toggle, a capture-degrade fallback, a `server`/`analysis` cfg branch) and
//! therefore CANNOT be promoted to a required, always-`Some` field. The
//! VERIFIED-unconditional fields (wired on every deployment shape, never
//! behind a runtime flag) moved to [`crate::required_deps::WebServerRequiredDeps`].

use std::sync::Arc;

use maekon_api_contracts::integration::IntegrationOutboundRuntimeStatus;
use maekon_core::config::CredentialBackendKind;
use maekon_core::ports::automation::AutomationPort;
use maekon_core::ports::coaching::CoachingPort;
use maekon_core::ports::conversation_session::SessionManager;
use maekon_core::ports::frame_storage::FrameStoragePort;
use maekon_core::ports::integration::{
    IntegrationAuditPort, IntegrationAuthPort, IntegrationInboxPort, IntegrationInboxStorePort,
    IntegrationOutboxPort, IntegrationRuntimeTelemetryPort, IntegrationSessionPort,
};
use maekon_core::ports::provider_model_catalog::ProviderModelCatalogPort;
use maekon_core::ports::secret_store::{SecretStore, SecretStoreSet};

use crate::AiRuntimeStatus;

// #5734: per-call LLM health handle forwarded from the composition root so the
// live GET /api/automation/status can read the true last-call outcome.
// Imported from maekon-core (the canonical definition site) to avoid a
// forbidden adapter-to-adapter dependency on maekon-network.
pub use maekon_core::ports::llm_provider::LlmCallHealth;

/// #7738 D-3: `frame_storage` (`None` when `SharedCaptureServices::build()`
/// fails — capture_wiring.rs warn+degrade) and `erasure_requested` (a
/// oneshot signal set only by the "Delete all data" endpoint) are both
/// genuinely conditional; they stay here rather than in
/// `WebServerRequiredDeps`.
#[derive(Clone, Default)]
pub struct CoreRuntimeBindings {
    /// ADR-023 M3: local frame-image store. `None` when
    /// `SharedCaptureServices::build()` fails (capture degrades but the
    /// server keeps running) — `frames_service.rs` has a functional `None`
    /// fallback and `consent.rs` tests pin `None` as first-class.
    pub frame_storage: Option<Arc<dyn FrameStoragePort>>,
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

/// #7738 core correction: `automation_controller` + `ai_runtime_status` are
/// `None` when `config.automation.enabled = false` — a live user toggle, not
/// a forgettable wiring gap. They stay `Option` by design.
#[derive(Clone, Default)]
pub struct AutomationRuntimeBindings {
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

/// #7738 decision: `override_store` / `recluster_requested` / `coaching_engine`
/// flow through the upstream `WebServerRuntimeBuilder` `with_*` Option chain in
/// src-tauri (not materialized inside `web_server_runtime.rs`'s own literal),
/// so promoting them to `WebServerRequiredDeps` would need builder surgery out
/// of this issue's scope — they stay here, covered by the D-5 wiring-map log
/// instead. `model_catalog_client` is genuinely `analysis`-cfg-conditional.
///
/// #8059: `vector_store` / `embedding_provider` / `adaptive_search` are the
/// semantic-search query-side components. They are `None` unless the user
/// enabled `analysis.embedding.enabled` (and a real embedding provider could be
/// built) — the composition root taps the same `embedding_setup` source the
/// scheduler ingestion pipeline uses, so search reads the SAME
/// `embedding_vectors` table it writes. All three staying `None` is the honest
/// degrade: `/api/semantic-search/capabilities` then reports
/// `semantic_available = false` instead of letting `mode=semantic` 501 or
/// `mode=hybrid` silently fall back to keyword-only.
#[derive(Clone, Default)]
pub struct AnalysisRuntimeBindings {
    pub override_store: Option<Arc<dyn maekon_core::ports::override_store::OverrideStore>>,
    pub recluster_requested: Option<Arc<std::sync::atomic::AtomicBool>>,
    pub coaching_engine: Option<Arc<dyn CoachingPort>>,
    pub model_catalog_client: Option<Arc<dyn ProviderModelCatalogPort>>,
    /// Backing vector store for semantic/hybrid search (same SQLite-backed store
    /// the scheduler embedding pipeline writes into).
    pub vector_store: Option<Arc<dyn maekon_core::ports::vector_store::VectorStore>>,
    /// Query-side embedding provider, built from the same config + credential
    /// resolution as the ingestion pipeline so query and document embeddings match.
    pub embedding_provider:
        Option<Arc<dyn maekon_core::ports::embedding_provider::EmbeddingProvider>>,
    /// Adaptive search coordinator (IVF/brute-force auto-select) over the vector store.
    pub adaptive_search: Option<Arc<dyn maekon_core::ports::adaptive_search::AdaptiveSearchPort>>,
}

#[derive(Clone, Default)]
pub struct SessionRuntimeBindings {
    pub session_manager: Option<Arc<dyn SessionManager>>,
    pub pomodoro_store: Option<Arc<dyn maekon_core::ports::pomodoro_store::PomodoroStorePort>>,
    /// #8044: capture-history re-authentication gate. When the composition
    /// root injects the `Arc` it built from `config.privacy.reauth` here, the
    /// web middleware (`require_capture_reauth`) reads the **same instance**
    /// as the Tauri re-auth command. `None` leaves `AuthState`'s default
    /// (disabled) gate in place (no effect).
    pub reauth_gate: Option<Arc<maekon_core::reauth::CaptureReauthGate>>,
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
