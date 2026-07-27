//! Query port for assembling AI chat session system context from stored suggestions.

use crate::error::CoreError;
use crate::models::storage_records::SuggestionRecord;
use crate::ports::storage::StorageService;

/// Query subset needed to assemble AI session system context.
///
/// # Errors
/// `CoreError::Storage` (wire: `storage.failed`) for SQLite prepare/query
/// operations (iter-47 mass fix pattern). Empty result is `Ok(Vec::new())`,
/// not an Err variant — callers treat absence of suggestions as a valid
/// empty context.
pub trait SessionContextStorePort: StorageService + Send + Sync {
    /// Load up to `limit` stored suggestions for assembling AI session system context.
    ///
    /// # Async-context warning
    ///
    /// This method is intentionally **synchronous** while its supertrait
    /// (`StorageService`) is async — the underlying SQLite work blocks the calling
    /// thread. It MUST NOT be called directly from an async (tokio) task: doing so
    /// blocks a tokio worker thread for the duration of the query and can stall the
    /// runtime. Callers on an async path must wrap it in `tokio::task::spawn_blocking`
    /// (see `src-tauri/src/session_context.rs`, which does exactly this). Converging
    /// this method onto the async storage funnel is tracked as an ADR-026 follow-up.
    fn list_suggestions(&self, limit: usize) -> Result<Vec<SuggestionRecord>, CoreError>;
}
