//! Port for persisting user regime overrides.
//!
//! Implementations reside in the storage adapter layer (e.g., SQLite).

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::CoreError;
use crate::models::recalibration::RegimeOverride;

/// Async port for CRUD operations on regime overrides.
///
/// # Errors
/// - `CoreError::Storage` (wire: `storage.failed`) for SQLite operations
///   (iter-47 mass fix pattern).
/// - `CoreError::NotFound` (wire: `resource.not_found`,
///   `resource_type = "RegimeOverride"`) from `delete_override` when the
///   `override_id` doesn't match any row (rowcount=0 after DELETE). This
///   diverges from session_storage/preset_storage, which treat rowcount=0
///   as a no-op — override deletion is a user-explicit action, so
///   unknown-ID is surfaced distinctly.
/// - Empty range results from `list_overrides` are `Ok(Vec::new())`.
#[async_trait]
pub trait OverrideStore: Send + Sync {
    /// Persist a new user override.
    async fn save_override(&self, entry: &RegimeOverride) -> Result<(), CoreError>;

    /// List overrides whose `created_at` falls within `[from, to]`.
    async fn list_overrides(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<RegimeOverride>, CoreError>;

    /// Delete an override by its ID.
    async fn delete_override(&self, override_id: &str) -> Result<(), CoreError>;
}
