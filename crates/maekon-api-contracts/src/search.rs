use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Clone)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TagInfo {
    pub id: i64,
    pub name: String,
    pub color: String,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SearchQuery {
    #[serde(default)]
    pub q: String,
    #[serde(default = "default_search_type")]
    pub search_type: String,
    pub tag_ids: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

fn default_search_type() -> String {
    "all".to_string()
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SearchResult {
    pub result_type: String,
    pub id: String,
    pub timestamp: String,
    pub app_name: Option<String>,
    pub window_title: Option<String>,
    pub matched_text: Option<String>,
    pub image_url: Option<String>,
    pub importance: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<TagInfo>>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SearchResponse {
    pub query: String,
    pub total: u64,
    pub offset: usize,
    pub limit: usize,
    pub results: Vec<SearchResult>,
}

/// Query parameters for semantic (vector) search.
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SemanticSearchQuery {
    /// Natural language query text.
    pub q: String,
    /// Maximum number of results (default: 10).
    pub limit: Option<usize>,
    /// Search mode: "hybrid" (default), "semantic", or "keyword".
    pub mode: Option<String>,
}

/// Capability snapshot for `GET /api/semantic-search/capabilities` (#7600).
///
/// Lets the frontend honestly label the "Semantic" search mode toggle BEFORE
/// the user fires a search — `mode=semantic` degrades to nothing useful (the
/// server returns HTTP 501 `service.not_implemented`, see
/// `SearchExecutionError::SemanticNotConfigured`) when the vector/embedding
/// pipeline is not wired up (e.g. `maekon-embedding` compiled out, or no
/// `EmbeddingProvider`/`VectorStore`/`AdaptiveSearchPort` configured at
/// runtime). `keyword` mode is always available (FTS-only, no dependency on
/// embeddings).
#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SemanticSearchCapabilities {
    /// True when `mode=semantic` would actually run a vector search instead
    /// of returning HTTP 501. Requires both an `EmbeddingProvider` and a
    /// vector search backend (`AdaptiveSearchPort` or `VectorStore`).
    pub semantic_available: bool,
}

/// A single semantic search result, enriched with segment metadata.
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SemanticSearchResult {
    pub segment_id: String,
    pub content_type: String,
    pub content_label: Option<String>,
    pub original_text: String,
    pub score: f32,
    pub similarity: f32,
    pub time_decay: f32,
    pub timestamp: String,
    pub segment_start: Option<String>,
    pub segment_end: Option<String>,
    pub duration_secs: Option<u64>,
    pub llm_summary: Option<String>,
    pub dominant_category: Option<String>,
    pub regime_label: Option<String>,
}
