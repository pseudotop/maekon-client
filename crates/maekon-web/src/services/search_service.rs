use std::collections::HashMap;

use maekon_api_contracts::search::{SearchQuery, SearchResponse, TagInfo};

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

    /// ADR-026 PR-4/PR-5: the search SQLite calls (`count_search_frames`,
    /// `search_frames_with_sql`, `count_search_events`, `search_events`,
    /// `get_tag_ids_for_frames`, `get_all_tags`) are all async and route through
    /// the storage `with_conn_read` funnel, which offloads each query onto the
    /// `spawn_blocking` pool internally. The prior hand-rolled `spawn_blocking`
    /// wrapper is therefore removed and each call is awaited directly.
    ///
    /// #5097 (ADR-026 follow-up): the per-frame `get_tags_for_frame` call inside
    /// the result loop (1 query per row, up to `limit`=200) is replaced by a
    /// single `get_tag_ids_for_frames` batch + one `get_all_tags` lookup joined in
    /// memory (`collect_frame_tags`) — bounded constant queries instead of N+1.
    pub async fn search(&self, params: &SearchQuery) -> Result<SearchResponse, ApiError> {
        let query = params.q.trim().to_string();
        let tag_ids = parse_tag_ids(params);

        if query.is_empty() && tag_ids.is_empty() {
            return Err(ApiError::BadRequest(
                "A search query or tag filter is required.".to_string(),
            ));
        }

        let limit = params.limit.unwrap_or(50).min(200);
        let offset = params.offset.unwrap_or(0);
        let search_type = params.search_type.clone();
        let storage = self.ctx.storage.clone();
        let pii_sanitizer = self.ctx.pii_sanitizer.clone();

        let has_text_query = !query.is_empty();
        let has_tag_filter = !tag_ids.is_empty();
        let search_type = search_type.as_str();
        let pattern = format!("%{query}%");

        let mut results = Vec::new();
        let mut total: u64 = 0;

        if search_type == "all" || search_type == "frames" {
            let (count_sql, select_sql) =
                build_frame_queries(has_text_query, has_tag_filter, &tag_ids);

            let frame_count = storage
                .count_search_frames(
                    &count_sql,
                    if has_text_query { Some(&pattern) } else { None },
                )
                .await
                .unwrap_or(0);
            total += frame_count;

            let frame_rows = storage
                .search_frames_with_sql(
                    &select_sql,
                    if has_text_query { Some(&pattern) } else { None },
                    limit,
                    offset,
                )
                .await
                .map_err(|error| ApiError::Internal(error.to_string()))?;

            // #5097: eliminate N+1 — instead of get_tags_for_frame per row (N
            // queries), do one frame→tag_id batch + one full-tag lookup, then join
            // in memory. collect_frame_tags preserves the semantics of the old
            // get_tags_for_frame `INNER JOIN tags ... ORDER BY t.name` (orphan
            // tag_ids excluded + name order).
            let mut frame_results = Vec::with_capacity(frame_rows.len());
            if !frame_rows.is_empty() {
                let frame_ids: Vec<i64> = frame_rows.iter().map(|row| row.id).collect();
                let tag_id_map = storage
                    .get_tag_ids_for_frames(&frame_ids)
                    .await
                    .map_err(|error| ApiError::Internal(error.to_string()))?;
                let tag_lookup: HashMap<i64, TagInfo> = storage
                    .get_all_tags()
                    .await
                    .map_err(|error| ApiError::Internal(error.to_string()))?
                    .into_iter()
                    .map(|tag| (tag.id, assemble_tag_info(tag)))
                    .collect();

                for row in frame_rows {
                    let tags = collect_frame_tags(row.id, &tag_id_map, &tag_lookup);
                    frame_results.push(assemble_frame_search_result(row, tags, &pii_sanitizer)?);
                }
            }

            results.extend(frame_results);
        }

        if (search_type == "all" || search_type == "events") && has_text_query && !has_tag_filter {
            let event_count = storage.count_search_events(&pattern).await.unwrap_or(0);
            total += event_count;

            let remaining = limit.saturating_sub(results.len());
            if remaining > 0 {
                let event_offset = if search_type == "all" {
                    offset.saturating_sub(results.len())
                } else {
                    offset
                };

                let event_rows = storage
                    .search_events(&pattern, remaining, event_offset)
                    .await
                    .map_err(|error| ApiError::Internal(error.to_string()))?;

                results.extend(
                    event_rows
                        .into_iter()
                        .map(|row| assemble_event_search_result(row, &pii_sanitizer))
                        .collect::<Result<Vec<_>, _>>()?,
                );
            }
        }

        Ok(assemble_search_response(
            query, total, offset, limit, results,
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

/// Collect the tags for a single frame via an in-memory join (#5097 N+1-removal
/// helper).
///
/// Builds the TagInfo list for the given frame from `tag_id_map` (the frame→tag_id
/// batch-lookup result) and `tag_lookup` (tag_id→TagInfo, the single full-tag
/// lookup). Preserves the behaviour of the old `get_tags_for_frame`
/// `INNER JOIN tags ... ORDER BY t.name`:
/// - Orphan tag_ids (frame_tags rows pointing at a deleted tag) are excluded via a
///   `tag_lookup` miss.
/// - The result is sorted by tag name ascending (restoring the original
///   ORDER BY t.name).
fn collect_frame_tags(
    frame_id: i64,
    tag_id_map: &HashMap<i64, Vec<i64>>,
    tag_lookup: &HashMap<i64, TagInfo>,
) -> Vec<TagInfo> {
    let mut tags: Vec<TagInfo> = tag_id_map
        .get(&frame_id)
        .map(|ids| {
            ids.iter()
                .filter_map(|id| tag_lookup.get(id).cloned())
                .collect()
        })
        .unwrap_or_default();
    tags.sort_by(|a, b| a.name.cmp(&b.name));
    tags
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(id: i64, name: &str) -> TagInfo {
        TagInfo {
            id,
            name: name.to_string(),
            color: "#000000".to_string(),
        }
    }

    /// collect_frame_tags joins a frame's tag_ids into TagInfo but sorts by name
    /// ascending — preserving the original get_tags_for_frame ORDER BY t.name.
    /// (Even if the input tag_id order is 3,1 the result must be Alpha,Bravo.)
    #[test]
    fn collect_frame_tags_sorts_by_name() {
        let mut tag_id_map = HashMap::new();
        tag_id_map.insert(1, vec![3, 1]);
        let mut lookup = HashMap::new();
        lookup.insert(1, tag(1, "Bravo"));
        lookup.insert(3, tag(3, "Alpha"));

        let result = collect_frame_tags(1, &tag_id_map, &lookup);
        let names: Vec<&str> = result.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Alpha", "Bravo"],
            "tags must be in ascending name order"
        );
    }

    /// An orphan tag_id (absent from the full-tag lookup = deleted tag) is
    /// excluded — preserving the original INNER JOIN behaviour (orphan frame_tags
    /// rows drop out of the result).
    #[test]
    fn collect_frame_tags_excludes_orphans() {
        let mut tag_id_map = HashMap::new();
        tag_id_map.insert(1, vec![1, 99]); // 99 is an orphan (absent from lookup)
        let mut lookup = HashMap::new();
        lookup.insert(1, tag(1, "Alpha"));

        let result = collect_frame_tags(1, &tag_id_map, &lookup);
        assert_eq!(result.len(), 1, "orphan tag_id 99 must be excluded");
        assert_eq!(result[0].id, 1);
    }

    /// Returns an empty vector when tag_id_map has no entry for the frame (an
    /// untagged frame).
    #[test]
    fn collect_frame_tags_empty_for_untagged_frame() {
        let tag_id_map: HashMap<i64, Vec<i64>> = HashMap::new();
        let lookup: HashMap<i64, TagInfo> = HashMap::new();
        assert!(collect_frame_tags(42, &tag_id_map, &lookup).is_empty());
    }
}
