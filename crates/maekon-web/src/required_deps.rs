//! `WebServer` required (VERIFIED-unconditional) dependencies (#7738).
//!
//! Split counterpart to [`crate::runtime_bindings::WebServerRuntimeBindings`],
//! which carries only the genuinely optional/conditional fields. This struct
//! carries the fields that are unconditionally wired on EVERY production
//! deployment shape (default / `server` / `analysis` / `grpc-dashboard-external`)
//! — never behind a runtime feature flag such as `config.automation.enabled`.
//!
//! Each field here was individually re-verified against the wiring site in
//! `src-tauri/src/web_server_runtime.rs::build_and_spawn` at the time of the
//! split: either an unconditional `Some(self.storage.clone() as _)` cast (the
//! four storage-derived ports) or an unconditional concrete value with no
//! `if let` / `match` / cfg / config-gate anywhere upstream. Fields that are
//! runtime-conditional by design — e.g. `automation.controller` /
//! `automation.ai_runtime_status` (`None` when `config.automation.enabled =
//! false`, a live user toggle), `core.frame_storage` (`None` when
//! `SharedCaptureServices::build()` fails), `session.manager` (structurally
//! `Option`) — stay in `WebServerRuntimeBindings`. Promoting any of those
//! would break correct disabled/degraded deployments (Rust's type system
//! cannot express "present iff config.enabled").
//!
//! Deliberately: **no `Default` derive, no `..Default::default()` escape
//! hatch**. Every field must be named at the ONE construction site
//! (`web_server_runtime.rs::build_and_spawn`), so removing a field from that
//! literal — or forgetting to wire one when a new field is added — is a
//! compile error, not a silently-forgotten `None` (the #7678/#7600 incident
//! class this split exists to remove).
use std::path::PathBuf;
use std::sync::Arc;

use maekon_core::config_manager::ConfigManager;
use maekon_core::ports::audit_chain_verifier::AuditChainVerifierPort;
use maekon_core::ports::audit_log::AuditLogPort;
use maekon_core::ports::egress_ledger_reader::EgressLedgerReaderPort;
use maekon_core::ports::memory_graph_port::MemoryGraphPort;
use maekon_core::ports::memory_graph_projection::MemoryGraphProjectionPort;
use maekon_core::ports::memory_vault_writer::MemoryVaultWriterPort;
use maekon_core::ports::pii_sanitizer::PiiSanitizer;
use maekon_core::ports::regime_storage::RegimeStoragePort;
use maekon_core::ports::runtime_log_provider::RuntimeLogProvider;
use maekon_core::ports::system_info_provider::SystemInfoProvider;
use maekon_core::ports::text_search::TextSearchProvider;

use crate::services::provider_cli_diagnostics::ProviderCliDiagnosticsProvider;
use crate::update_control::UpdateControl;

/// The 16-field VERIFIED-unconditional `WebServer` dependency set. See the
/// module doc for the promotion criteria and the fields that deliberately
/// stay `Option` in [`crate::runtime_bindings::WebServerRuntimeBindings`].
pub struct WebServerRequiredDeps {
    /// ADR-023: local memory-graph store (the same `SqliteStorage` as
    /// `storage`, as a `MemoryGraphPort`). Lets the digest export render
    /// accumulated claims.
    pub memory_graph: Arc<dyn MemoryGraphPort>,
    /// ADR-032 Mode A: the bounded, fail-closed memory-graph projection
    /// helper. Unconditional because the implementation is always
    /// constructible and internally fail-closed (disabled config, denied or
    /// absent consent ⇒ empty projection ⇒ ranking unchanged) — the OFF state
    /// lives inside the port, not in an `Option` here.
    pub memory_graph_projection: Arc<dyn MemoryGraphProjectionPort>,
    /// ADR-033 §4: vault mirror writer. Unconditional (always constructible;
    /// internally fail-closed for cycles) — the web erase orchestrator MUST
    /// hold it so Art.17 Phase-3 can never be silently skipped.
    pub memory_vault_writer: Arc<dyn MemoryVaultWriterPort>,
    /// #7600: durable audit-log hash-chain verifier (the same `SqliteStorage`
    /// as `storage`, as an `AuditChainVerifierPort`). Lets `GET /audit/verify`
    /// reach the real ADR-072 verification.
    pub audit_chain_verifier: Arc<dyn AuditChainVerifierPort>,
    /// #7910: read-only egress-ledger reader (the same `SqliteStorage` as
    /// `storage`, cast to an `EgressLedgerReaderPort`). Lets
    /// `GET /api/privacy/egress-ledger` render the egress transparency browser
    /// ("what left this device") from the erase-retained #4803 ledger.
    pub egress_ledger_reader: Arc<dyn EgressLedgerReaderPort>,
    /// #7678 D2: regime storage (the same `SqliteStorage`-backed store used by
    /// the scheduler, as a `RegimeStoragePort`). Lets the dashboard digest
    /// endpoint resolve human-readable regime labels instead of leaking the
    /// opaque positional `regime_id` ("regime-N") into the timeline.
    pub regime_storage: Arc<dyn RegimeStoragePort>,
    /// #6279: keyword/FTS text-search provider (the same `SqliteStorage` as
    /// `storage`). Without this `/api/semantic-search` is permanently inert.
    pub text_search: Arc<dyn TextSearchProvider>,
    /// Server-lifetime settings-policy + automation audit log port.
    pub audit_logger: Arc<dyn AuditLogPort>,
    pub config_manager: ConfigManager,
    pub update_control: UpdateControl,
    /// E20-41 (#4833): the ephemeral per-session local-API auth token
    /// `require_local_auth` validates on every `/api` request.
    pub local_auth_token: Arc<str>,
    pub frames_dir: PathBuf,
    pub pii_sanitizer: Arc<dyn PiiSanitizer>,
    pub runtime_log_provider: Arc<dyn RuntimeLogProvider>,
    pub system_info_provider: Arc<dyn SystemInfoProvider>,
    pub provider_cli_diagnostics: Arc<dyn ProviderCliDiagnosticsProvider>,
}
