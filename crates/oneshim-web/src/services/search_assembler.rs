use oneshim_api_contracts::search::{SearchResponse, SearchResult, TagInfo};
use oneshim_core::models::storage_records::{SearchEventRow, SearchFrameRow, TagRecord};

pub(crate) fn assemble_tag_info(tag: TagRecord) -> TagInfo {
    TagInfo {
        id: tag.id,
        name: tag.name,
        color: tag.color,
    }
}

pub(crate) fn assemble_frame_search_result(
    row: SearchFrameRow,
    tags: Vec<TagInfo>,
) -> SearchResult {
    let image_url = row
        .file_path
        .as_ref()
        .map(|_| format!("/api/frames/{}/image", row.id));

    SearchResult {
        result_type: "frame".to_string(),
        id: row.id.to_string(),
        timestamp: row.timestamp,
        app_name: row.app_name,
        window_title: row.window_title,
        matched_text: row.matched_text,
        image_url,
        importance: row.importance,
        tags: Some(tags),
    }
}

pub(crate) fn assemble_event_search_result(row: SearchEventRow) -> SearchResult {
    SearchResult {
        result_type: "event".to_string(),
        id: row.event_id,
        timestamp: row.timestamp,
        app_name: row.app_name,
        window_title: row.window_title,
        matched_text: row.data,
        image_url: None,
        importance: None,
        tags: None,
    }
}

pub(crate) fn assemble_search_response(
    query: String,
    total: u64,
    offset: usize,
    limit: usize,
    results: Vec<SearchResult>,
) -> SearchResponse {
    SearchResponse {
        query,
        total,
        offset,
        limit,
        results,
    }
}
