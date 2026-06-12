//! Read-side sync port for extracting local changes into outbound changesets.

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::sync::ChangeSet;
use crate::sync::Hlc;

/// Read-side port: extracts local changes for outbound sync.
///
/// Implemented by maekon-storage (SQLite queries against syncable
/// tables). The SyncEngine calls this to build an outbound ChangeSet
/// containing all rows modified since the peer's last-known watermark.
///
/// # Errors
/// `CoreError::Storage` (wire: `storage.failed`) for SQLite prepare/
/// query failures across syncable tables (iter-47 mass fix pattern).
/// An empty changeset (no rows since the watermark) is `Ok(ChangeSet { .. })`
/// with empty vectors — callers check `is_empty()`, not an error variant.
/// `local_watermark` on a fresh install returns `Ok(Hlc::ZERO)`, never Err.
#[async_trait]
pub trait ChangeExtractor: Send + Sync {
    /// Get local changes since the given HLC watermark.
    ///
    /// Returns a ChangeSet containing all rows where
    /// `(hlc_wall_ms, hlc_counter, origin_device_id) > since`.
    async fn get_changes_since(&self, since: &Hlc) -> Result<ChangeSet, CoreError>;

    /// Get the current device's high-watermark HLC.
    ///
    /// This is the maximum HLC across all syncable tables on this device.
    async fn local_watermark(&self) -> Result<Hlc, CoreError>;

    /// The persisted GDPR Art.17 erasure HLC anchor (`app_meta["sync.erasure_hlc"]`),
    /// written by the erase producer (#5179) and retained across the local wipe.
    ///
    /// The SyncEngine stamps the device-wide `DeletionEvent` watermark with this so a
    /// receiving peer can BOUND the delete to data that existed at erasure time —
    /// any post-re-grant data (HLC > anchor) is spared (#5181). Returns `None` when no
    /// erase has run on this device (pre-#5179 installs / tests); the caller then falls
    /// back to `Hlc::now`, which is effectively unbounded. The default impl returns
    /// `Ok(None)` so non-SQLite/mocks need not implement it.
    async fn persisted_erasure_hlc(&self) -> Result<Option<Hlc>, CoreError> {
        Ok(None)
    }
}
