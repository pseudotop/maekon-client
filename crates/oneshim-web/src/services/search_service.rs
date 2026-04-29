use oneshim_api_contracts::search::{SearchQuery, SearchResponse};

use crate::error::ApiError;
use crate::services::search_assembler::{
    assemble_event_search_result, assemble_frame_search_result, assemble_search_response,
    assemble_tag_info,
};
use crate::services::web_contexts::StorageWebContext;

#[derive(Clone)]
pub struct SearchQueryService {
    ctx: StorageWebContext,
}

impl SearchQueryService {
    pub fn new(ctx: StorageWebContext) -> Self {
        Self { ctx }
    }

    #[allow(clippy::unused_async)] // called from axum async handler
    pub async fn search(&self, params: &SearchQuery) -> Result<SearchResponse, ApiError> {
        let query = params.q.trim();
        let tag_ids = parse_tag_ids(params);

        if query.is_empty() && tag_ids.is_empty() {
            return Err(ApiError::BadRequest(
                "A search query or tag filter is required.".to_string(),
            ));
        }

        let limit = params.limit.unwrap_or(50).min(200);
        let offset = params.offset.unwrap_or(0);
        let search_type = params.search_type.as_str();
        let has_text_query = !query.is_empty();
        let has_tag_filter = !tag_ids.is_empty();

        let mut results = Vec::new();
        let mut total: u64 = 0;

        let pattern = format!("%{}%", query);

        if search_type == "all" || search_type == "frames" {
            let (count_sql, select_sql) =
                build_frame_queries(has_text_query, has_tag_filter, &tag_ids);

            let frame_count = self
                .ctx
                .storage
                .count_search_frames(
                    &count_sql,
                    if has_text_query { Some(&pattern) } else { None },
                )
                .unwrap_or(0);
            total += frame_count;

            let frame_rows = self
                .ctx
                .storage
                .search_frames_with_sql(
                    &select_sql,
                    if has_text_query { Some(&pattern) } else { None },
                    limit,
                    offset,
                )
                .map_err(|error| ApiError::Internal(error.to_string()))?;

            let mut frame_results = Vec::with_capacity(frame_rows.len());
            for row in frame_rows {
                let tags = self
                    .ctx
                    .storage
                    .get_tags_for_frame(row.id)
                    .map_err(|error| ApiError::Internal(error.to_string()))?
                    .into_iter()
                    .map(assemble_tag_info)
                    .collect();

                frame_results.push(assemble_frame_search_result(row, tags));
            }

            results.extend(frame_results);
        }

        if (search_type == "all" || search_type == "events") && has_text_query && !has_tag_filter {
            let event_count = self.ctx.storage.count_search_events(&pattern).unwrap_or(0);
            total += event_count;

            let remaining = limit.saturating_sub(results.len());
            if remaining > 0 {
                let event_offset = if search_type == "all" {
                    offset.saturating_sub(results.len())
                } else {
                    offset
                };

                let event_rows = self
                    .ctx
                    .storage
                    .search_events(&pattern, remaining, event_offset)
                    .map_err(|error| ApiError::Internal(error.to_string()))?;

                results.extend(event_rows.into_iter().map(assemble_event_search_result));
            }
        }

        Ok(assemble_search_response(
            query.to_string(),
            total,
            offset,
            limit,
            results,
        ))
    }
}

fn parse_tag_ids(params: &SearchQuery) -> Vec<i64> {
    params
        .tag_ids
        .as_ref()
        .map(|value| {
            value
                .split(',')
                .filter_map(|id| id.trim().parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn build_frame_queries(
    has_text: bool,
    has_tags: bool,
    tag_ids: &[i64],
) -> (String, String) {
    let tag_ids_str = tag_ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");

    let text_condition = if has_text {
        "(app_name LIKE ?1 OR window_title LIKE ?1 OR ocr_text LIKE ?1)"
    } else {
        "1=1"
    };

    let tag_condition = if has_tags {
        debug_assert!(
            tag_ids.iter().all(|id| id
                .to_string()
                .chars()
                .all(|c| c.is_ascii_digit() || c == '-')),
            "tag_ids contains unexpected characters"
        );
        format!(
            "EXISTS (SELECT 1 FROM frame_tags ft WHERE ft.frame_id = frames.id AND ft.tag_id IN ({}))",
            tag_ids_str
        )
    } else {
        "1=1".to_string()
    };

    let where_clause = format!("{} AND {}", text_condition, tag_condition);

    let count_sql = format!("SELECT COUNT(*) FROM frames WHERE {}", where_clause);

    let select_sql = if has_text {
        format!(
            "SELECT id, timestamp, app_name, window_title, ocr_text, importance, file_path
             FROM frames
             WHERE {}
             ORDER BY timestamp DESC
             LIMIT ?2 OFFSET ?3",
            where_clause
        )
    } else {
        format!(
            "SELECT id, timestamp, app_name, window_title, ocr_text, importance, file_path
             FROM frames
             WHERE {}
             ORDER BY timestamp DESC
             LIMIT ?1 OFFSET ?2",
            where_clause
        )
    };

    (count_sql, select_sql)
}
