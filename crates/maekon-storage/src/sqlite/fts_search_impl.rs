use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::ports::text_search::{TextSearchProvider, TextSearchResult};
use std::sync::atomic::Ordering;
use tracing::{debug, warn};

use super::cjk_shadow::{build_fts_match_query, build_fts_phrase_query, cjk_bigram_shadow};
use super::{SqliteStorage, FTS_AVAILABLE, GUI_INTERACTIONS_AVAILABLE};
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
}

// ── Enriched FTS indexing (storage-level, not a port method) ───────
impl SqliteStorage {
    /// Index a segment with enriched content from multiple sources.
    ///
    /// In addition to the base `searchable_text` (llm_summary + dominant_category),
    /// this method gathers window titles from `events`, `element_text` from
    /// `gui_interactions`, and suggestion `content` from `suggestions` that fall
    /// within the segment's time range and concatenates them into the FTS index.
    pub async fn sync_segment_enriched(
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

            // Gather GUI interaction element_text from gui_interactions table
            let gui_text = Self::collect_gui_element_text(conn, &segment_id);
            if !gui_text.is_empty() {
                parts.push(gui_text);
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

    /// Insert or replace a row in the FTS5 search_fts table.
    fn upsert_fts(
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

    /// Collect GUI interaction element_text for the given segment.
    fn collect_gui_element_text(conn: &rusqlite::Connection, segment_id: &str) -> String {
        if !GUI_INTERACTIONS_AVAILABLE.load(Ordering::Relaxed) {
            return String::new();
        }
        let result: Result<Vec<String>, rusqlite::Error> = (|| {
            let mut stmt = conn.prepare_cached(
                "SELECT DISTINCT element_text FROM gui_interactions
                 WHERE segment_id = ?1
                   AND element_text IS NOT NULL AND element_text != ''
                 LIMIT 100",
            )?;
            let rows =
                stmt.query_map(rusqlite::params![segment_id], |row| row.get::<_, String>(0))?;
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
    async fn enriched_sync_includes_gui_interactions() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        // Insert a gui_interaction for the segment (V13 schema: event_id, segment_id,
        // timestamp, element_text, element_type, interaction_type, app_name)
        {
            let conn = storage.conn.test_lock();
            conn.execute(
                "INSERT INTO gui_interactions (event_id, segment_id, timestamp, element_type, element_text, interaction_type, app_name)
                 VALUES ('gui-evt-1', 'seg-enriched-3', '2026-03-01T10:00:00Z', 'button', 'Submit Pull Request', 'click', 'GitHub')",
                [],
            )
            .unwrap();
        }

        storage
            .sync_segment_enriched(
                "seg-enriched-3",
                "development work",
                "2026-03-01T09:00:00Z",
                "2026-03-01T11:00:00Z",
            )
            .await
            .unwrap();

        let results = storage.search_fts("Pull Request", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].segment_id, "seg-enriched-3");
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
}
