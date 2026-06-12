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

            // #5097: N+1 제거 — row 당 get_tags_for_frame(쿼리 N회) 대신
            // frame→tag_id 배치 1회 + 전체 태그 1회 조회 후 메모리 조인한다.
            // 기존 get_tags_for_frame 의 `INNER JOIN tags ... ORDER BY t.name`
            // 의미를 collect_frame_tags 가 보존한다(고아 tag_id 제외 + 이름순).
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

/// frame 한 개의 태그를 메모리 조인으로 모은다 (#5097 N+1 제거 헬퍼).
///
/// `tag_id_map`(frame→tag_id 배치 조회 결과)과 `tag_lookup`(tag_id→TagInfo,
/// 전체 태그 1회 조회)로부터 해당 frame 의 TagInfo 목록을 만든다. 기존
/// `get_tags_for_frame` 의 `INNER JOIN tags ... ORDER BY t.name` 동작을 보존한다:
/// - 고아 tag_id(삭제된 태그를 가리키는 frame_tags 행)는 `tag_lookup` miss 로 제외.
/// - 결과는 태그 이름 오름차순 정렬(원본 ORDER BY t.name 복원).
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

    /// collect_frame_tags 가 frame 의 tag_id 들을 TagInfo 로 조인하되 이름
    /// 오름차순으로 정렬한다 — 원본 get_tags_for_frame 의 ORDER BY t.name 보존.
    /// (입력 tag_id 순서가 3,1 이어도 결과는 Alpha,Bravo 여야 한다.)
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
            "태그는 이름 오름차순이어야 한다"
        );
    }

    /// 고아 tag_id(전체 태그 lookup 에 없는 = 삭제된 태그)는 제외된다 —
    /// 원본 INNER JOIN 동작 보존(고아 frame_tags 행은 결과에서 빠진다).
    #[test]
    fn collect_frame_tags_excludes_orphans() {
        let mut tag_id_map = HashMap::new();
        tag_id_map.insert(1, vec![1, 99]); // 99 는 고아(lookup 부재)
        let mut lookup = HashMap::new();
        lookup.insert(1, tag(1, "Alpha"));

        let result = collect_frame_tags(1, &tag_id_map, &lookup);
        assert_eq!(result.len(), 1, "고아 tag_id 99 는 제외되어야 한다");
        assert_eq!(result[0].id, 1);
    }

    /// tag_id_map 에 frame 항목이 없으면 빈 벡터를 반환한다 (태그 없는 frame).
    #[test]
    fn collect_frame_tags_empty_for_untagged_frame() {
        let tag_id_map: HashMap<i64, Vec<i64>> = HashMap::new();
        let lookup: HashMap<i64, TagInfo> = HashMap::new();
        assert!(collect_frame_tags(42, &tag_id_map, &lookup).is_empty());
    }
}
