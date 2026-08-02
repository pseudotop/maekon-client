//! ADR-033 §1.4: persisted change-detection state for the vault mirror.
//!
//! Backed by the `vault_mirror_state` SQLite table (path-keyed rows holding
//! the last-rendered content hash). Keys are **vault-relative file names**
//! (`daily/2026-07-29.md`, `claims.md`, `README.md`) — never absolute
//! filesystem paths, so no OS-username-bearing path is ever persisted. The
//! table is a member of the erasure `ALL_TABLES` sweep: hash state must
//! never outlive the files it describes (an erase-surviving hash row would
//! silently suppress regeneration).
//!
//! The same table also carries a small number of **reserved** rows whose key is
//! not a file name (a `::` prefix cannot collide with a vault-relative name):
//! the last generated root, and the §6.4 last-cycle summary this port exposes
//! through typed accessors. Reserved rows ride the same `ALL_TABLES` sweep, so
//! nothing about the mirror's persisted state can survive an Art.17 erase.

use std::collections::HashMap;

use crate::error::CoreError;
use crate::models::memory_vault::VaultLastCycleSummary;

/// Hash-state store for ADR-033 §1.4 staleness decisions.
///
/// A missing row means "never rendered" and always triggers a write; the
/// writer additionally re-checks file existence on disk each cycle, so a
/// stored hash can never suppress recreating a deleted file.
#[async_trait::async_trait]
pub trait VaultMirrorStatePort: Send + Sync {
    /// All stored (vault-relative file name → content hash) rows.
    async fn vault_hashes(&self) -> Result<HashMap<String, String>, CoreError>;

    /// Insert or replace the hash for one generated file.
    async fn upsert_vault_hash(
        &self,
        file_name: &str,
        content_hash: &str,
        updated_at: i64,
    ) -> Result<(), CoreError>;

    /// Remove the hash row for one generated file (expiry sweep / erase).
    async fn delete_vault_hash(&self, file_name: &str) -> Result<(), CoreError>;

    /// The persisted summary of the last mirror cycle that ran, or `None` when
    /// none has (fresh install, or post-Art.17 wipe). ADR-033 §6.4 requires the
    /// settings/status UI to surface marker conflicts, and a **scheduled**
    /// cycle's stats are otherwise dropped on the floor (#9522).
    async fn last_cycle_summary(&self) -> Result<Option<VaultLastCycleSummary>, CoreError>;

    /// Replace the persisted last-cycle summary. Called by the writer at the
    /// end of every cycle that actually ran — scheduled and manual alike.
    async fn put_last_cycle_summary(
        &self,
        summary: &VaultLastCycleSummary,
    ) -> Result<(), CoreError>;
}
