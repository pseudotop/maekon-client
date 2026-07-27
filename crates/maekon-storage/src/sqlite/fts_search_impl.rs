// lint:allow-non-english-comments — contains illustrative CJK/Korean example tokens
// documenting CJK bigram full-text search behavior; they must remain non-English.
use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::models::tiered_memory::{compose_searchable_content, ContentActivity};
use maekon_core::ports::text_search::{TextSearchProvider, TextSearchResult};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use tracing::{debug, warn};

use super::cjk_shadow::{build_fts_match_query, build_fts_phrase_query, cjk_bigram_shadow};
use super::{SqliteStorage, FTS_AVAILABLE};
use crate::error::StorageError;

#[async_trait]
impl TextSearchProvider for SqliteStorage {
    async fn search_fts(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<TextSearchResult>, CoreError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        if !FTS_AVAILABLE.load(Ordering::Relaxed) {
            warn!("search_fts table not available; returning empty results");
            return Ok(vec![]);
        }

        // Build the bigram-shadow MATCH expression. Returns None when the query
        // produces no usable tokens (e.g. query = "\"\""); return empty rather
        // than issuing a syntactically invalid MATCH.
        let match_expr = match build_fts_match_query(query) {
            Some(expr) => expr,
            None => return Ok(vec![]),
        };

        self.with_conn(move |conn| {
            // Query against the `shadow` column (CJK bigram index).
            // `searchable_text` is UNINDEXED since V41 and is returned as
            // `matched_text` for the caller — its value is unchanged.
            let mut stmt = conn
                .prepare_cached(
                    "SELECT segment_id, content_type, searchable_text, rank
                     FROM search_fts
                     WHERE shadow MATCH ?1
                     ORDER BY rank
                     LIMIT ?2",
                )
                .map_err(|e| StorageError::Internal(format!("FTS5 query prepare failed: {e}")))?;

            let results = stmt
                .query_map(rusqlite::params![match_expr, limit as i64], |row| {
                    Ok(TextSearchResult {
                        segment_id: row.get(0)?,
                        content_type: row.get(1)?,
                        matched_text: row.get(2)?,
                        rank: row.get(3)?,
                    })
                })
                .map_err(|e| StorageError::Internal(format!("FTS5 query failed: {e}")))?
                .filter_map(|r| r.ok())
                .collect();

            Ok(results)
        })
        .await
        .map_err(Into::into)
    }

    async fn sync_segment(&self, segment_id: &str, searchable_text: &str) -> Result<(), CoreError> {
        let segment_id = segment_id.to_string();
        let searchable_text = searchable_text.to_string();
        self.with_conn(move |conn| {
            Self::upsert_fts(conn, &segment_id, "segment", &searchable_text).map_err(Into::into)
        })
        .await
        .map_err(Into::into)
    }

    /// Phrase search: all CJK bigram tokens of `query` are joined into a single
    /// FTS5 quoted phrase requiring contiguous adjacency in the shadow column.
    ///
    /// This is the tier-0 signal for hybrid search — documents where the full
    /// phrase matches rank above documents that only satisfy the per-token OR
    /// query. Fail-open: FTS unavailable or OperationalError → empty vec +
    /// debug log (phrase is a supplementary signal; surfacing an Err would
    /// suppress tier-0 ranking without improving accuracy).
    async fn search_fts_phrase(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<TextSearchResult>, CoreError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        if !FTS_AVAILABLE.load(Ordering::Relaxed) {
            debug!("search_fts_phrase: FTS table not available, returning empty");
            return Ok(vec![]);
        }

        let phrase_expr = match build_fts_phrase_query(query) {
            Some(expr) => expr,
            None => return Ok(vec![]),
        };

        let result = self
            .with_conn(move |conn| {
                let mut stmt = conn
                    .prepare_cached(
                        "SELECT segment_id, content_type, searchable_text, rank
                         FROM search_fts
                         WHERE shadow MATCH ?1
                         ORDER BY rank
                         LIMIT ?2",
                    )
                    .map_err(|e| {
                        StorageError::Internal(format!("FTS5 phrase query prepare failed: {e}"))
                    })?;

                let results = stmt
                    .query_map(rusqlite::params![phrase_expr, limit as i64], |row| {
                        Ok(TextSearchResult {
                            segment_id: row.get(0)?,
                            content_type: row.get(1)?,
                            matched_text: row.get(2)?,
                            rank: row.get(3)?,
                        })
                    })
                    .map_err(|e| StorageError::Internal(format!("FTS5 phrase query failed: {e}")))?
                    .filter_map(|r| r.ok())
                    .collect();

                Ok(results)
            })
            .await;

        // Fail-open: phrase search is a supplementary signal.
        Ok(result.unwrap_or_else(|e| {
            debug!("search_fts_phrase storage error (fail-open): {e}");
            vec![]
        }))
    }

    /// Index a segment with enriched content from multiple sources.
    ///
    /// Overrides the `TextSearchProvider` default (which indexes only the base
    /// text): in addition to `searchable_text` (llm_summary + dominant_category
    /// + content labels + app names), this gathers window titles from `events`
    /// and suggestion `content` from `suggestions` that fall within the
    /// segment's `[start_time, end_time]` range and concatenates them into the
    /// FTS index.
    ///
    /// #7678 D3: previously also gathered `element_text` from
    /// `gui_interactions` keyed by `segment_id` — removed because the
    /// production writer never populates `segment_id` (always `NULL`), so this
    /// join could never match a row (dead-in-practice; the columns themselves
    /// were dropped in V43).
    async fn sync_segment_enriched(
        &self,
        segment_id: &str,
        searchable_text: &str,
        start_time: &str,
        end_time: &str,
    ) -> Result<(), CoreError> {
        let segment_id = segment_id.to_string();
        let searchable_text = searchable_text.to_string();
        let start_time = start_time.to_string();
        let end_time = end_time.to_string();
        self.with_conn(move |conn| {
            let mut parts: Vec<String> = vec![searchable_text];

            // Gather window titles from events table
            let titles = Self::collect_window_titles(conn, &start_time, &end_time);
            if !titles.is_empty() {
                parts.push(titles);
            }

            // Gather suggestion content from suggestions table
            let suggestion_text = Self::collect_suggestion_content(conn, &start_time, &end_time);
            if !suggestion_text.is_empty() {
                parts.push(suggestion_text);
            }

            let combined = parts.join(" ");
            Self::upsert_fts(conn, &segment_id, "segment", &combined).map_err(Into::into)
        })
        .await
        .map_err(Into::into)
    }
}

// ── FTS content backfill + helpers (storage-level, not port methods) ───────
impl SqliteStorage {
    /// Backfill FTS content for activity segments persisted before the live
    /// segment-close FTS indexing path existed (#8051).
    ///
    /// Historically the only `search_fts` content writers were test-only, so
    /// every previously-closed segment is absent from the index and keyword
    /// search returns nothing for it. This indexes up to `max_segments` of the
    /// most recent still-unindexed segments in a single blocking pass
    /// (offloaded via `with_conn` → `spawn_blocking`, so it never touches the
    /// async runtime hot path).
    ///
    /// The searchable text is rebuilt from already-persisted columns
    /// (`llm_summary`, `dominant_category`, content-activity labels, app names)
    /// using the same [`compose_searchable_content`] SSOT the live path uses,
    /// but WITHOUT the per-segment events/suggestions time-range joins — that
    /// keeps a large-backlog pass bounded in cost.
    ///
    /// Bounded + resumable: unindexed rows are selected with
    /// `id NOT IN (SELECT segment_id FROM search_fts)`, so each run makes
    /// forward progress and a subsequent run drains the next batch. A very
    /// large backlog is therefore indexed across restarts without any single
    /// run exceeding `max_segments`.
    ///
    /// Returns the number of segments indexed. Non-fatal by contract: the
    /// caller logs and continues.
    pub async fn backfill_unindexed_segments_fts(
        &self,
        max_segments: usize,
    ) -> Result<usize, CoreError> {
        if !FTS_AVAILABLE.load(Ordering::Relaxed) {
            debug!("search_fts table not available; skipping segment backfill");
            return Ok(0);
        }
        if max_segments == 0 {
            return Ok(0);
        }

        self.with_conn(move |conn| {
            // 1. Read a bounded batch of still-unindexed segments (most recent
            //    first — those are the ones a user is most likely to search).
            let rows: Vec<(String, String, String, String, Option<String>)> = {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, dominant_category, content_activities_json, \
                                app_breakdown, llm_summary \
                         FROM activity_segments \
                         WHERE id NOT IN (SELECT segment_id FROM search_fts) \
                         ORDER BY start_time DESC \
                         LIMIT ?1",
                    )
                    .map_err(|e| {
                        StorageError::Internal(format!("FTS backfill prepare failed: {e}"))
                    })?;
                let mapped = stmt
                    .query_map(rusqlite::params![max_segments as i64], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, Option<String>>(4)?,
                        ))
                    })
                    .map_err(|e| {
                        StorageError::Internal(format!("FTS backfill query failed: {e}"))
                    })?;
                mapped.filter_map(|r| r.ok()).collect()
            };

            // 2. Rebuild searchable text per row and upsert.
            let mut indexed = 0usize;
            for (id, dominant_category, content_json, app_breakdown_json, llm_summary) in &rows {
                let content_labels: Vec<String> =
                    serde_json::from_str::<Vec<ContentActivity>>(content_json)
                        .map(|acts| acts.into_iter().map(|a| a.content_label).collect())
                        .unwrap_or_default();
                let app_names: Vec<String> =
                    serde_json::from_str::<HashMap<String, u64>>(app_breakdown_json)
                        .map(|m| m.into_keys().collect())
                        .unwrap_or_default();

                let text = compose_searchable_content(
                    llm_summary.as_deref(),
                    dominant_category,
                    content_labels.iter().map(String::as_str),
                    app_names.iter().map(String::as_str),
                );
                if text.trim().is_empty() {
                    continue;
                }
                Self::upsert_fts(conn, id, "segment", &text)?;
                indexed += 1;
            }
            Ok(indexed)
        })
        .await
        .map_err(Into::into)
    }

    /// Insert or replace a row in the FTS5 search_fts table.
    ///
    /// `pub(super)` so the sibling `transcript_storage_impl` can index a
    /// persisted transcript through the SAME writer the segment path uses
    /// (single-source the CJK-shadow computation + delete-then-insert upsert).
    pub(super) fn upsert_fts(
        conn: &rusqlite::Connection,
        segment_id: &str,
        content_type: &str,
        searchable_text: &str,
    ) -> Result<(), CoreError> {
        if !FTS_AVAILABLE.load(Ordering::Relaxed) {
            warn!("search_fts table not available; skipping FTS upsert");
            return Ok(());
        }

        conn.execute(
            "DELETE FROM search_fts WHERE segment_id = ?1",
            rusqlite::params![segment_id],
        )
        .map_err(|e| CoreError::Storage {
            code: maekon_core::error_codes::StorageCode::Failed,
            message: format!("FTS5 delete failed: {e}"),
        })?;

        // Compute CJK bigram shadow for the indexed column (V41 schema).
        let shadow = cjk_bigram_shadow(searchable_text);

        conn.execute(
            "INSERT INTO search_fts (segment_id, content_type, searchable_text, shadow)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![segment_id, content_type, searchable_text, shadow],
        )
        .map_err(|e| CoreError::Storage {
            code: maekon_core::error_codes::StorageCode::Failed,
            message: format!("FTS5 insert failed: {e}"),
        })?;

        Ok(())
    }

    /// Collect distinct window titles from the events table within the given time range.
    /// The events table stores event data as JSON in the `data` column, so we
    /// extract `window_title` via `json_extract`.
    fn collect_window_titles(
        conn: &rusqlite::Connection,
        start_time: &str,
        end_time: &str,
    ) -> String {
        let result: Result<Vec<String>, rusqlite::Error> = (|| {
            let mut stmt = conn.prepare_cached(
                "SELECT DISTINCT json_extract(data, '$.window_title') AS wt FROM events
                 WHERE timestamp >= ?1 AND timestamp <= ?2
                   AND wt IS NOT NULL AND wt != ''
                 LIMIT 100",
            )?;
            let rows = stmt.query_map(rusqlite::params![start_time, end_time], |row| {
                row.get::<_, String>(0)
            })?;
            Ok(rows.filter_map(|r| r.ok()).collect())
        })();

        result.unwrap_or_default().join(" ")
    }

    /// Collect suggestion content from the suggestions table within the given time range.
    fn collect_suggestion_content(
        conn: &rusqlite::Connection,
        start_time: &str,
        end_time: &str,
    ) -> String {
        let result: Result<Vec<String>, rusqlite::Error> = (|| {
            let mut stmt = conn.prepare_cached(
                "SELECT DISTINCT content FROM suggestions
                 WHERE created_at >= ?1 AND created_at <= ?2
                   AND content IS NOT NULL AND content != ''
                 LIMIT 50",
            )?;
            let rows = stmt.query_map(rusqlite::params![start_time, end_time], |row| {
                row.get::<_, String>(0)
            })?;
            Ok(rows.filter_map(|r| r.ok()).collect())
        })();

        result.unwrap_or_default().join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sync_and_search_roundtrip() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        storage
            .sync_segment("seg-001", "deep work on authentication module")
            .await
            .unwrap();
        storage
            .sync_segment("seg-002", "slack communication with team")
            .await
            .unwrap();

        let results = storage.search_fts("authentication", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].segment_id, "seg-001");
        assert!(results[0].matched_text.contains("authentication"));
    }

    #[tokio::test]
    async fn search_empty_query_returns_empty() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        storage
            .sync_segment("seg-001", "some content")
            .await
            .unwrap();

        let results = storage.search_fts("", 10).await.unwrap();
        assert!(results.is_empty());

        let results = storage.search_fts("   ", 10).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn multiple_results_ordering() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        storage
            .sync_segment("seg-001", "rust programming language systems")
            .await
            .unwrap();
        storage
            .sync_segment("seg-002", "rust compiler optimization rust")
            .await
            .unwrap();
        storage
            .sync_segment("seg-003", "python web development")
            .await
            .unwrap();

        let results = storage.search_fts("rust", 10).await.unwrap();
        assert_eq!(results.len(), 2);
        // seg-002 mentions "rust" twice, so it should rank better (lower rank value in FTS5)
        assert_eq!(results[0].segment_id, "seg-002");
    }

    #[tokio::test]
    async fn sync_segment_updates_existing() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        storage
            .sync_segment("seg-001", "original content about rust")
            .await
            .unwrap();
        storage
            .sync_segment("seg-001", "updated content about python")
            .await
            .unwrap();

        let rust_results = storage.search_fts("rust", 10).await.unwrap();
        assert!(rust_results.is_empty());

        let python_results = storage.search_fts("python", 10).await.unwrap();
        assert_eq!(python_results.len(), 1);
        assert_eq!(python_results[0].segment_id, "seg-001");
    }

    #[tokio::test]
    async fn search_respects_limit() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        for i in 0..5 {
            storage
                .sync_segment(&format!("seg-{i:03}"), "common keyword content")
                .await
                .unwrap();
        }

        let results = storage.search_fts("keyword", 2).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn enriched_sync_includes_window_titles() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        // Insert events with window titles in the time range (data column stores JSON)
        {
            let conn = storage.conn.test_lock();
            conn.execute(
                "INSERT INTO events (event_id, event_type, timestamp, data)
                 VALUES ('evt-1', 'window_change', '2026-03-01T10:00:00Z',
                         '{\"app_name\":\"Code\",\"window_title\":\"authentication.rs - VS Code\"}')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO events (event_id, event_type, timestamp, data)
                 VALUES ('evt-2', 'window_change', '2026-03-01T10:30:00Z',
                         '{\"app_name\":\"Firefox\",\"window_title\":\"Rust Documentation\"}')",
                [],
            )
            .unwrap();
        }

        storage
            .sync_segment_enriched(
                "seg-enriched-1",
                "deep work session",
                "2026-03-01T09:00:00Z",
                "2026-03-01T11:00:00Z",
            )
            .await
            .unwrap();

        // Should find via window title text
        let results = storage.search_fts("authentication", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].segment_id, "seg-enriched-1");

        let results = storage.search_fts("Documentation", 10).await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn enriched_sync_includes_suggestion_content() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        // Insert a suggestion in the time range
        {
            let conn = storage.conn.test_lock();
            conn.execute(
                "INSERT INTO suggestions (suggestion_id, suggestion_type, content, priority, source, created_at)
                 VALUES ('sugg-001', 'focus', 'Take a break after debugging session', 'Medium', 'local', '2026-03-01T10:15:00Z')",
                [],
            )
            .unwrap();
        }

        storage
            .sync_segment_enriched(
                "seg-enriched-2",
                "coding session",
                "2026-03-01T09:00:00Z",
                "2026-03-01T11:00:00Z",
            )
            .await
            .unwrap();

        let results = storage.search_fts("debugging", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].segment_id, "seg-enriched-2");
    }

    #[tokio::test]
    async fn enriched_sync_no_extra_sources_works_like_basic() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        // No events/gui/suggestions in range -- should still index the base text
        storage
            .sync_segment_enriched(
                "seg-basic",
                "plain segment text",
                "2099-01-01T00:00:00Z",
                "2099-01-01T01:00:00Z",
            )
            .await
            .unwrap();

        let results = storage.search_fts("plain", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].segment_id, "seg-basic");
    }

    // ── Option F behaviour tests (CJK bigram shadow, #5758) ──────────────────
    //
    // These tests document the core value of the migration: before V41 all three
    // queries below returned 0 results; after V41 they match via the shadow column.

    #[tokio::test]
    async fn ja_document_matches_ja_query() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        // "月次売上レポートの集計" — a realistic Japanese segment summary.
        storage
            .sync_segment("seg-ja-001", "月次売上レポートの集計")
            .await
            .unwrap();

        // Query with a substring bigram pair that appears in the shadow index.
        // "売上のまとめ" → shadow = "売上 上の のま まと とめ" → OR query hits "売上".
        let results = storage.search_fts("売上のまとめ", 10).await.unwrap();
        assert_eq!(
            results.len(),
            1,
            "Japanese document must match Japanese bigram query"
        );
        assert_eq!(results[0].segment_id, "seg-ja-001");
    }

    #[tokio::test]
    async fn ko_document_matches_ko_query() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        // Multi-word Korean segment.
        storage
            .sync_segment("seg-ko-001", "월별 급여 지급 보고서")
            .await
            .unwrap();

        // Query with a single Korean eojeol.
        let results = storage.search_fts("급여", 10).await.unwrap();
        assert_eq!(
            results.len(),
            1,
            "Korean document must match Korean bigram query"
        );
        assert_eq!(results[0].segment_id, "seg-ko-001");
    }

    #[tokio::test]
    async fn quoted_query_does_not_cause_fts5_syntax_error() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        storage
            .sync_segment("seg-en-001", "deep work on authentication module")
            .await
            .unwrap();

        // A query containing literal double-quote characters must not panic or
        // return an Err — the query builder strips them before FTS5 quoting.
        let result = storage.search_fts("\"authentication\"", 10).await;
        let hits = result.expect("query with literal quotes must not produce FTS5 syntax error");
        // The result may be empty (FTS5 strips the outer quotes) — the key invariant
        // is no Err (no FTS5 syntax error surfaced to the caller).
        let _ = hits;
    }

    // ── search_fts_phrase (tier-0 phrase signal, #5766) ──────────────────────

    #[tokio::test]
    async fn phrase_full_ja_match_returns_one() {
        // A document containing "月次売上レポートの集計" should match the phrase
        // query "月次売上" because the shadow tokens "月次 次売 売上" appear
        // contiguously in the shadow column (the phrase query is "月次 次売 売上").
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        storage
            .sync_segment("seg-ja-phrase-001", "月次売上レポートの集計")
            .await
            .unwrap();
        storage
            .sync_segment("seg-ja-phrase-002", "売上目標の策定") // "売上" only, no "月次" before
            .await
            .unwrap();

        // Phrase query covers "月次売上" — only seg-ja-phrase-001 contains those
        // bigrams in that exact order adjacently.
        let results = storage.search_fts_phrase("月次売上", 10).await.unwrap();
        assert_eq!(
            results.len(),
            1,
            "Only the document containing the full phrase should match"
        );
        assert_eq!(results[0].segment_id, "seg-ja-phrase-001");
    }

    #[tokio::test]
    async fn phrase_partial_ja_no_match() {
        // A query whose phrase tokens do NOT appear adjacently in any document
        // must return 0 results, not fall back to the OR query.
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        // Document contains "月次" and "売上" but separated by other characters.
        storage
            .sync_segment("seg-ja-gap-001", "月次の業績と売上の分析")
            .await
            .unwrap();

        // Phrase "月次売上" → shadow phrase "月次 次売 売上" — "次売" does not
        // appear in the document's shadow because the run "月次" and "売上" are
        // separated by non-CJK characters.
        let results = storage.search_fts_phrase("月次売上", 10).await.unwrap();
        assert_eq!(
            results.len(),
            0,
            "Phrase query must not match document where tokens are not adjacent"
        );
    }

    #[tokio::test]
    async fn phrase_empty_query_returns_empty() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        storage
            .sync_segment("seg-001", "some content")
            .await
            .unwrap();

        let results = storage.search_fts_phrase("", 10).await.unwrap();
        assert!(results.is_empty());

        let results = storage.search_fts_phrase("   ", 10).await.unwrap();
        assert!(results.is_empty());
    }

    // ── FTS content backfill (#8051) ─────────────────────────────────────────
    //
    // Existing users have segments in `activity_segments` that predate the live
    // segment-close FTS wiring, so they are absent from `search_fts`. These
    // tests exercise the one-shot startup backfill that indexes them.

    use maekon_core::models::tiered_memory::{
        ContentType, EngagementMetrics, SegmentSummary, TriggerReason, WorkType,
    };
    use maekon_core::ports::storage::StorageService;

    fn content_activity(label: &str) -> ContentActivity {
        ContentActivity {
            content_label: label.to_string(),
            content_type: ContentType::File,
            start_time: chrono::Utc::now(),
            duration_secs: 300,
            confidence: 0.9,
            work_type: WorkType::ActiveCoding,
            engagement: EngagementMetrics::default(),
            gui_summary: None,
        }
    }

    /// Build a segment summary with distinctive searchable content but leave it
    /// out of the FTS index (as `save_activity_segment` does in production).
    fn make_summary(id: &str, dominant: &str, labels: &[&str], app: &str) -> SegmentSummary {
        let now = chrono::Utc::now();
        let mut app_breakdown = HashMap::new();
        app_breakdown.insert(app.to_string(), 300u64);
        SegmentSummary {
            segment_id: id.to_string(),
            start_time: now - chrono::Duration::minutes(10),
            end_time: now,
            duration_secs: 600,
            regime_id: None,
            trigger_reason: TriggerReason::ScoreLow,
            event_count: 0,
            app_breakdown,
            category_breakdown: HashMap::new(),
            context_switch_count: 0,
            dominant_category: dominant.to_string(),
            avg_importance: 0.0,
            patterns_detected: vec![],
            content_activities: labels.iter().map(|l| content_activity(l)).collect(),
            container: None,
            llm_summary: None,
        }
    }

    #[tokio::test]
    async fn backfill_indexes_previously_saved_segments() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        // Persist segments the way production does — this does NOT touch FTS.
        storage
            .save_activity_segment(&make_summary(
                "seg-bf-1",
                "Development",
                &["authentication.rs"],
                "VS Code",
            ))
            .await
            .unwrap();
        storage
            .save_activity_segment(&make_summary(
                "seg-bf-2",
                "Communication",
                &["team standup"],
                "Slack",
            ))
            .await
            .unwrap();

        // Pre-condition: keyword search is empty (the dead-writer symptom).
        assert!(storage
            .search_fts("authentication", 10)
            .await
            .unwrap()
            .is_empty());

        // Backfill indexes both.
        let indexed = storage.backfill_unindexed_segments_fts(100).await.unwrap();
        assert_eq!(indexed, 2);

        // Now keyword search returns the segments (content-label + app-name text).
        let hits = storage.search_fts("authentication", 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].segment_id, "seg-bf-1");

        let app_hits = storage.search_fts("Slack", 10).await.unwrap();
        assert_eq!(app_hits.len(), 1);
        assert_eq!(app_hits[0].segment_id, "seg-bf-2");
    }

    #[tokio::test]
    async fn backfill_is_idempotent_and_skips_indexed() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        storage
            .save_activity_segment(&make_summary(
                "seg-idem",
                "Development",
                &["main.rs"],
                "VS Code",
            ))
            .await
            .unwrap();

        // First run indexes the one unindexed segment.
        assert_eq!(
            storage.backfill_unindexed_segments_fts(100).await.unwrap(),
            1
        );
        // Second run finds nothing new (NOT IN search_fts already excludes it).
        assert_eq!(
            storage.backfill_unindexed_segments_fts(100).await.unwrap(),
            0
        );

        // A live sync_segment write is also treated as indexed by the backfill.
        storage
            .sync_segment("seg-live", "already indexed content")
            .await
            .unwrap();
        storage
            .save_activity_segment(&make_summary(
                "seg-live",
                "Development",
                &["live.rs"],
                "VS Code",
            ))
            .await
            .unwrap();
        assert_eq!(
            storage.backfill_unindexed_segments_fts(100).await.unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn backfill_respects_max_segments_cap() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        for i in 0..5 {
            storage
                .save_activity_segment(&make_summary(
                    &format!("seg-cap-{i}"),
                    "Development",
                    &["capkeyword.rs"],
                    "VS Code",
                ))
                .await
                .unwrap();
        }

        // Cap at 2 per run — the pass is bounded.
        assert_eq!(storage.backfill_unindexed_segments_fts(2).await.unwrap(), 2);
        // Remaining drain across subsequent runs (resumable).
        assert_eq!(storage.backfill_unindexed_segments_fts(2).await.unwrap(), 2);
        assert_eq!(storage.backfill_unindexed_segments_fts(2).await.unwrap(), 1);
        assert_eq!(storage.backfill_unindexed_segments_fts(2).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn backfill_korean_content_is_searchable() {
        // CJK smoke: a Korean content label backfilled from activity_segments
        // must be reachable via the bigram-shadow keyword query (mirrors the
        // `ko_document_matches_ko_query` sync-path test).
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        storage
            .save_activity_segment(&make_summary(
                "seg-ko-bf",
                "Documentation",
                &["월별 급여 지급 보고서"],
                "한글",
            ))
            .await
            .unwrap();

        assert_eq!(
            storage.backfill_unindexed_segments_fts(100).await.unwrap(),
            1
        );

        let hits = storage.search_fts("급여", 10).await.unwrap();
        assert_eq!(
            hits.len(),
            1,
            "Korean backfilled content must match Korean query"
        );
        assert_eq!(hits[0].segment_id, "seg-ko-bf");
    }
}
