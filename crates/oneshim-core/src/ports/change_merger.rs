//! Write-side sync port for merging inbound changesets with LWW conflict resolution.

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::sync::{ChangeSet, SyncResult};

/// Write-side port: applies inbound changesets with LWW conflict resolution.
///
/// Implemented by oneshim-storage (SQLite queries with HLC comparison).
/// The SyncEngine calls this after pulling a remote ChangeSet from the
/// transport to merge it into the local database.
///
/// Conflict resolution rules:
/// - Append-only tables (segments, overrides, param_snapshots): insert if PK absent.
/// - LWW tables (regimes, suggestions, embeddings): compare HLC, higher wins.
/// - Tombstoned rows: propagate soft-delete via is_deleted + deleted_at.
/// - DeletionEvent changeset: hard-delete all rows from the originating device.
///
/// # Errors
/// `CoreError::Storage` (wire: `storage.failed`) for SQLite transaction
/// / constraint / commit failures during the merge (iter-47 mass fix
/// pattern). Conflict resolution is not an error — LWW skips and
/// tombstone propagations are recorded in `SyncResult`. A malformed
/// inbound changeset (e.g., missing required FK) also routes through
/// Storage because SQLite enforces the integrity constraint.
#[async_trait]
pub trait ChangeMerger: Send + Sync {
    /// Apply a remote changeset, resolving conflicts via HLC.
    ///
    /// Returns statistics on applied/skipped/tombstoned rows.
    async fn apply_changes(&self, changes: ChangeSet) -> Result<SyncResult, CoreError>;
}
