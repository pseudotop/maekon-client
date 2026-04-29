use axum::extract::{Query, State};
use axum::Json;
use oneshim_api_contracts::search::{SearchQuery, SearchResponse};
#[cfg(test)]
use oneshim_api_contracts::search::{SearchResult, TagInfo};

use crate::error::ApiError;
use crate::services::search_service::SearchQueryService;
use crate::services::web_contexts::StorageWebContext;

pub async fn search(
    State(context): State<StorageWebContext>,
    Query(params): Query<SearchQuery>,
) -> Result<Json<SearchResponse>, ApiError> {
    Ok(Json(
        SearchQueryService::new(context).search(&params).await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::search_service::build_frame_queries;

    #[test]
    fn search_query_defaults() {
        let json = r#"{"q": "test"}"#;
        let query: SearchQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.q, "test");
        assert_eq!(query.search_type, "all");
        assert!(query.limit.is_none());
        assert!(query.tag_ids.is_none());
    }

    #[test]
    fn search_query_with_tags() {
        let json = r#"{"q": "test", "tag_ids": "1,2,3"}"#;
        let query: SearchQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.q, "test");
        assert_eq!(query.tag_ids, Some("1,2,3".to_string()));
    }

    #[test]
    fn search_query_empty_q_allowed() {
        let json = r#"{"tag_ids": "1,2"}"#;
        let query: SearchQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.q, "");
        assert_eq!(query.tag_ids, Some("1,2".to_string()));
    }

    #[test]
    fn search_result_serializes() {
        let result = SearchResult {
            result_type: "frame".to_string(),
            id: "123".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            app_name: Some("VS Code".to_string()),
            window_title: Some("main.rs".to_string()),
            matched_text: Some("fn main()".to_string()),
            image_url: Some("/api/frames/123/image".to_string()),
            importance: Some(0.85),
            tags: None,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("VS Code"));
        assert!(json.contains("frame"));
        assert!(!json.contains("tags"));
    }

    #[test]
    fn search_result_with_tags_serializes() {
        let result = SearchResult {
            result_type: "frame".to_string(),
            id: "123".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            app_name: Some("VS Code".to_string()),
            window_title: None,
            matched_text: None,
            image_url: None,
            importance: None,
            tags: Some(vec![TagInfo {
                id: 1,
                name: "work".to_string(),
                color: "#3b82f6".to_string(),
            }]),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("tags"));
        assert!(json.contains("work"));
        assert!(json.contains("#3b82f6"));
    }

    #[test]
    fn search_response_serializes() {
        let response = SearchResponse {
            query: "test".to_string(),
            total: 10,
            offset: 0,
            limit: 50,
            results: vec![],
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"total\":10"));
    }

    #[test]
    fn build_frame_queries_text_only() {
        let (count, select) = build_frame_queries(true, false, &[]);
        assert!(count.contains("app_name LIKE"));
        assert!(select.contains("LIMIT ?2 OFFSET ?3"));
        assert!(!count.contains("frame_tags"));
    }

    #[test]
    fn build_frame_queries_tags_only() {
        let (count, select) = build_frame_queries(false, true, &[1, 2]);
        assert!(count.contains("frame_tags"));
        assert!(count.contains("tag_id IN (1,2)"));
        assert!(select.contains("LIMIT ?1 OFFSET ?2"));
    }

    #[test]
    fn build_frame_queries_both() {
        let (count, select) = build_frame_queries(true, true, &[5]);
        assert!(count.contains("app_name LIKE"));
        assert!(count.contains("frame_tags"));
        assert!(count.contains("tag_id IN (5)"));
        assert!(select.contains("LIMIT ?2 OFFSET ?3"));
    }
}
