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
    fn list_suggestions(&self, limit: usize) -> Result<Vec<SuggestionRecord>, CoreError>;
}
