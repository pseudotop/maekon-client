use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Type of content that was embedded into a vector.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EmbeddingContentType {
    SegmentSummary,
    ContentActivity,
}

/// Metadata stored alongside each embedding vector row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingMetadata {
    pub segment_id: String,
    pub content_type: EmbeddingContentType,
    pub content_label: Option<String>,
    pub timestamp: DateTime<Utc>,
    /// The actual text that was embedded into the vector.
    pub original_text: String,
    /// Embedding model identifier used to generate this vector.
    pub model_id: String,
}

/// A single search result from the vector store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub segment_id: String,
    pub content_type: EmbeddingContentType,
    pub content_label: Option<String>,
    pub score: f32,
    pub similarity: f32,
    pub time_decay: f32,
    pub timestamp: DateTime<Utc>,
    pub original_text: String,
}

/// Filters applied when searching the vector store.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SearchFilters {
    pub after: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
    pub content_types: Option<Vec<EmbeddingContentType>>,
    pub regime_id: Option<String>,
    /// Segment IDs to exclude from search results (e.g. segments whose
    /// originating suggestion was dismissed by the user).
    #[serde(default)]
    pub excluded_segment_ids: Vec<String>,
}

/// Enriched search result combining vector similarity with segment metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrichedSearchResult {
    pub score: f32,
    pub similarity: f32,
    pub matched_text: String,
    pub segment_id: String,
    pub segment_start: DateTime<Utc>,
    pub segment_end: DateTime<Utc>,
    pub duration_secs: u64,
    pub llm_summary: Option<String>,
    pub dominant_category: String,
    pub regime_label: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedding_content_type_serde_roundtrip() {
        let ct = EmbeddingContentType::SegmentSummary;
        let json = serde_json::to_string(&ct).unwrap();
        assert_eq!(json, "\"SEGMENT_SUMMARY\"");
        let back: EmbeddingContentType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, EmbeddingContentType::SegmentSummary);
    }

    #[test]
    fn embedding_content_type_content_activity_serde() {
        let ct = EmbeddingContentType::ContentActivity;
        let json = serde_json::to_string(&ct).unwrap();
        assert_eq!(json, "\"CONTENT_ACTIVITY\"");
        let back: EmbeddingContentType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, EmbeddingContentType::ContentActivity);
    }

    #[test]
    fn embedding_metadata_serde_roundtrip() {
        let meta = EmbeddingMetadata {
            segment_id: "seg-001".to_string(),
            content_type: EmbeddingContentType::SegmentSummary,
            content_label: Some("VSCode: main.rs".to_string()),
            timestamp: Utc::now(),
            original_text: "VSCode: main.rs".to_string(),
            model_id: "test-model".to_string(),
        };
        let json = serde_json::to_string(&meta).unwrap();
        let back: EmbeddingMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(back.segment_id, "seg-001");
        assert_eq!(back.content_type, EmbeddingContentType::SegmentSummary);
        assert_eq!(back.content_label, Some("VSCode: main.rs".to_string()));
    }

    #[test]
    fn search_filters_default_is_empty() {
        let filters = SearchFilters::default();
        assert!(filters.after.is_none());
        assert!(filters.before.is_none());
        assert!(filters.content_types.is_none());
        assert!(filters.regime_id.is_none());
        assert!(filters.excluded_segment_ids.is_empty());
    }

    #[test]
    fn search_filters_excluded_segment_ids_backward_compat() {
        // Old JSON without excluded_segment_ids should deserialize fine
        let json = r#"{"after":null,"before":null,"content_types":null,"regime_id":null}"#;
        let filters: SearchFilters = serde_json::from_str(json).unwrap();
        assert!(filters.excluded_segment_ids.is_empty());
    }

    #[test]
    fn search_filters_excluded_segment_ids_roundtrip() {
        let filters = SearchFilters {
            excluded_segment_ids: vec!["seg-x".to_string(), "seg-y".to_string()],
            ..Default::default()
        };
        let json = serde_json::to_string(&filters).unwrap();
        let back: SearchFilters = serde_json::from_str(&json).unwrap();
        assert_eq!(back.excluded_segment_ids.len(), 2);
        assert_eq!(back.excluded_segment_ids[0], "seg-x");
    }

    #[test]
    fn search_result_serde_roundtrip() {
        let result = SearchResult {
            segment_id: "seg-002".to_string(),
            content_type: EmbeddingContentType::ContentActivity,
            content_label: Some("Slack: #general".to_string()),
            score: 0.85,
            similarity: 0.92,
            time_decay: 0.95,
            timestamp: Utc::now(),
            original_text: "Team standup discussion".to_string(),
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: SearchResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.segment_id, "seg-002");
        assert!((back.score - 0.85).abs() < f32::EPSILON);
    }

    #[test]
    fn enriched_search_result_serde_roundtrip() {
        let result = EnrichedSearchResult {
            score: 0.9,
            similarity: 0.95,
            matched_text: "Deep coding session on auth module".to_string(),
            segment_id: "seg-003".to_string(),
            segment_start: Utc::now(),
            segment_end: Utc::now(),
            duration_secs: 1800,
            llm_summary: Some("Focused development session".to_string()),
            dominant_category: "Development".to_string(),
            regime_label: Some("Deep Focus".to_string()),
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: EnrichedSearchResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.segment_id, "seg-003");
        assert_eq!(back.duration_secs, 1800);
        assert!(back.llm_summary.is_some());
    }
}
