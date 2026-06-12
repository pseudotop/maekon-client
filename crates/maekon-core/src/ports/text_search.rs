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

    /// Execute a phrase search: all CJK bigram tokens of `query` are joined into
    /// a single quoted FTS5 phrase, requiring them to appear contiguously in the
    /// shadow index. Returns only documents where the full phrase matches.
    ///
    /// The default implementation returns an empty vec, meaning tier-0 phrase
    /// boosting is simply absent for providers that do not implement this method.
    /// This preserves full binary compatibility — existing implementations
    /// (`SqliteStorage`, test mocks) require no change.
    ///
    /// # Fail-open design
    /// Phrase search is a supplementary recall signal. An Err here would suppress
    /// tier-0 ranking without improving accuracy; fail-open (empty vec) is correct.
    async fn search_fts_phrase(
        &self,
        _query: &str,
        _limit: usize,
    ) -> Result<Vec<TextSearchResult>, CoreError> {
        Ok(vec![])
    }
}
