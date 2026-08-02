//! ADR-033 vault mirror writer construction (composition-root helper).
//!
//! The writer is stateless — every durable input (digest rows, claims, hash
//! state, config) lives behind the SAME shared `SqliteStorage` Arc +
//! `ConfigManager` — so the multiple instances this helper builds (AppState
//! erase path, scheduler cycle, web required-deps) are interchangeable
//! (Port Instance Sharing guardrail: separate instances with rationale).

use std::sync::Arc;

use maekon_analysis::memory_vault_writer::VaultMirrorWriter;
use maekon_core::config_manager::ConfigManager;
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_core::ports::memory_vault_writer::MemoryVaultWriterPort;
use maekon_storage::sqlite::SqliteStorage;

/// Build a vault mirror writer over the shared storage Arc. `consent = None`
/// is the ADR-033 "consent authority unavailable" state (permanent fail-closed
/// no-op for cycles; erase still runs).
pub(crate) fn build_vault_writer(
    storage: Arc<SqliteStorage>,
    consent: Option<Arc<dyn ConsentManagerPort>>,
    config_manager: ConfigManager,
) -> Arc<dyn MemoryVaultWriterPort> {
    Arc::new(VaultMirrorWriter::new(
        storage.clone() as Arc<dyn maekon_core::ports::web_storage::DigestStorage>,
        storage.clone() as Arc<dyn maekon_core::ports::memory_graph_port::MemoryGraphPort>,
        storage.clone() as Arc<dyn maekon_core::ports::vault_mirror_state::VaultMirrorStatePort>,
        Arc::new(maekon_vision::privacy::VisionPiiSanitizer)
            as Arc<dyn maekon_core::ports::pii_sanitizer::PiiSanitizer>,
        storage as Arc<dyn maekon_core::ports::egress_ledger::EgressLedgerSink>,
        consent,
        config_manager,
        ConfigManager::data_dir().map(|d| d.join("vault")).ok(),
    )) as Arc<dyn MemoryVaultWriterPort>
}
