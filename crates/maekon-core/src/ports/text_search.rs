//! Full-text search port over activity segment content (backed by FTS5 or similar).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;

/// A single result from full-text search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextSearchResult {
    pub segment_id: String,
    pub content_type: String,
    pub matched_text: String,
    pub rank: f32,
}

/// Port for full-text search over activity segment content.
///
/// Implementations typically back this with SQLite FTS5 or similar.
///
/// # Errors
/// `CoreError::Storage` (wire: `storage.failed`) for SQLite-backed
/// FTS5 query/index operations (iter-47 mass fix pattern).
/// Malformed FTS query syntax is returned as Storage as well — FTS5
/// parser errors are surfaced as opaque SQL errors, not validation
/// failures; the caller sanitizes query input before invoking.
#[async_trait]
pub trait TextSearchProvider: Send + Sync {
    /// Execute a full-text search query and return ranked results.
    async fn search_fts(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<TextSearchResult>, CoreError>;

    /// Index (or update) the searchable text for a given segment.
    async fn sync_segment(&self, segment_id: &str, searchable_text: &str) -> Result<(), CoreError>;
}
