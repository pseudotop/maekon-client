use crate::error::CoreError;
use crate::models::intent::WorkflowPreset;

/// Synchronous storage port for automation presets (follows FewShotStorage pattern).
///
/// Implementations persist user-created custom presets to durable storage.
/// Built-in presets are not stored — they are returned by `builtin_presets()`.
///
/// # Errors
/// `CoreError::Storage` (wire: `storage.failed`) for SQLite operations
/// (iter-47 mass fix pattern: query/execute/JSON column parse).
/// `get_preset` expresses not-found as `Ok(None)`; `delete_preset` returns
/// `Ok(false)` for a missing ID rather than erroring, so callers can
/// distinguish "no-op" from a transport failure.
pub trait PresetStorage: Send + Sync {
    fn list_presets(&self) -> Result<Vec<WorkflowPreset>, CoreError>;
    fn get_preset(&self, id: &str) -> Result<Option<WorkflowPreset>, CoreError>;
    fn save_preset(&self, preset: &WorkflowPreset) -> Result<(), CoreError>;
    fn delete_preset(&self, id: &str) -> Result<bool, CoreError>;
}
