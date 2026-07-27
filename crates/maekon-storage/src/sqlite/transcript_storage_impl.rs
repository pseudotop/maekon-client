//! SQLite implementation of `TranscriptStoragePort` (#8059).
//!
//! Persists PII-masked speech-to-text output into the local `transcripts` table
//! (V47) and indexes the text into `search_fts` (`content_type = 'transcript'`)
//! through the SAME `upsert_fts` writer the segment path uses, so a saved
//! transcript is immediately reachable from keyword/hybrid search. LOCAL-ONLY:
//! no HLC/sync columns, no sync-tombstone participation.

use async_trait::async_trait;

use maekon_core::error::CoreError;
use maekon_core::models::audio::TranscriptRecord;
use maekon_core::ports::transcript_storage::TranscriptStoragePort;
use maekon_core::types::TimeWindow;

use super::SqliteStorage;
use crate::error::StorageError;

/// Distinct FTS `content_type` for transcript rows. Lets keyword/hybrid search
/// surface them (the query does not filter on `content_type`, so this is purely
/// a discriminator the result payload carries back to the UI) and lets the
/// erasure paths clean up ONLY transcript index rows.
pub(super) const TRANSCRIPT_CONTENT_TYPE: &str = "transcript";

#[async_trait]
impl TranscriptStoragePort for SqliteStorage {
    async fn save_transcript(&self, record: &TranscriptRecord) -> Result<(), CoreError> {
        let r = record.clone();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT OR REPLACE INTO transcripts
                 (id, timestamp, duration_secs, source, language, text, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    r.id,
                    r.timestamp,
                    r.duration_secs as f64,
                    r.source,
                    r.language,
                    r.text,
                    r.created_at,
                ],
            )
            .map_err(|e| StorageError::Internal(format!("transcript insert failed: {e}")))?;

            // Index the (already PII-masked) text for keyword search, keyed by the
            // transcript id so the erasure paths can clean the paired FTS row.
            Self::upsert_fts(conn, &r.id, TRANSCRIPT_CONTENT_TYPE, &r.text)?;
            Ok(())
        })
        .await
        .map_err(Into::into)
    }

    async fn query_transcripts_in_range(
        &self,
        window: &TimeWindow,
    ) -> Result<Vec<TranscriptRecord>, CoreError> {
        let (from, to) = window.to_sql_pair();
        self.with_conn_read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, timestamp, duration_secs, source, language, text, created_at
                     FROM transcripts
                     WHERE timestamp >= ?1 AND timestamp <= ?2
                     ORDER BY timestamp DESC",
                )
                .map_err(|e| StorageError::Internal(format!("transcript query prepare: {e}")))?;
            let rows = stmt
                .query_map(rusqlite::params![from, to], |row| {
                    Ok(TranscriptRecord {
                        id: row.get(0)?,
                        timestamp: row.get(1)?,
                        duration_secs: row.get::<_, f64>(2)? as f32,
                        source: row.get(3)?,
                        language: row.get(4)?,
                        text: row.get(5)?,
                        created_at: row.get(6)?,
                    })
                })
                .map_err(|e| StorageError::Internal(format!("transcript query failed: {e}")))?
                .filter_map(Result::ok)
                .collect();
            Ok(rows)
        })
        .await
        .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, timestamp: &str, text: &str) -> TranscriptRecord {
        TranscriptRecord {
            id: id.to_string(),
            timestamp: timestamp.to_string(),
            duration_secs: 3.5,
            source: "whisper".to_string(),
            language: Some("en".to_string()),
            text: text.to_string(),
            created_at: timestamp.to_string(),
        }
    }

    #[tokio::test]
    async fn save_and_query_roundtrip() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        storage
            .save_transcript(&record(
                "t-1",
                "2026-07-11T10:00:00+00:00",
                "quarterly planning sync notes",
            ))
            .await
            .unwrap();

        let window =
            TimeWindow::from_rfc3339_pair("2026-07-11T00:00:00+00:00", "2026-07-11T23:59:59+00:00")
                .unwrap();
        let rows = storage.query_transcripts_in_range(&window).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "t-1");
        assert_eq!(rows[0].text, "quarterly planning sync notes");
        assert_eq!(rows[0].source, "whisper");
        assert_eq!(rows[0].language.as_deref(), Some("en"));
    }

    #[tokio::test]
    async fn query_filters_by_range_and_orders_desc() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        storage
            .save_transcript(&record("t-early", "2026-07-10T09:00:00+00:00", "early"))
            .await
            .unwrap();
        storage
            .save_transcript(&record("t-late", "2026-07-11T09:00:00+00:00", "late"))
            .await
            .unwrap();
        storage
            .save_transcript(&record(
                "t-out",
                "2026-08-01T09:00:00+00:00",
                "out of range",
            ))
            .await
            .unwrap();

        let window =
            TimeWindow::from_rfc3339_pair("2026-07-01T00:00:00+00:00", "2026-07-31T23:59:59+00:00")
                .unwrap();
        let rows = storage.query_transcripts_in_range(&window).await.unwrap();
        assert_eq!(
            rows.len(),
            2,
            "the out-of-range transcript must be excluded"
        );
        // Most-recent-first ordering.
        assert_eq!(rows[0].id, "t-late");
        assert_eq!(rows[1].id, "t-early");
    }

    #[tokio::test]
    async fn saved_transcript_is_keyword_searchable() {
        use maekon_core::ports::text_search::TextSearchProvider;

        let storage = SqliteStorage::open_in_memory(30).unwrap();
        storage
            .save_transcript(&record(
                "t-search",
                "2026-07-11T10:00:00+00:00",
                "discuss the migration rollout plan",
            ))
            .await
            .unwrap();

        // A persisted transcript must surface via the shared FTS keyword path,
        // carrying content_type='transcript' as the result discriminator.
        let hits = storage.search_fts("migration", 10).await.unwrap();
        assert_eq!(hits.len(), 1, "transcript must be found by keyword search");
        assert_eq!(hits[0].segment_id, "t-search");
        assert_eq!(hits[0].content_type, TRANSCRIPT_CONTENT_TYPE);
        assert!(hits[0].matched_text.contains("migration"));
    }

    #[tokio::test]
    async fn korean_transcript_is_keyword_searchable() {
        use maekon_core::ports::text_search::TextSearchProvider;

        // CJK bigram-shadow smoke: a Korean transcript must be reachable via the
        // Korean keyword query (mirrors the segment-path CJK tests).
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        storage
            .save_transcript(&record(
                "t-ko",
                "2026-07-11T10:00:00+00:00",
                "월별 급여 지급 회의록",
            ))
            .await
            .unwrap();

        let hits = storage.search_fts("급여", 10).await.unwrap();
        assert_eq!(hits.len(), 1, "Korean transcript must match Korean query");
        assert_eq!(hits[0].segment_id, "t-ko");
    }
}
