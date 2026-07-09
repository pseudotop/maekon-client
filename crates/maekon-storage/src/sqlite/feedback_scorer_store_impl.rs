//! `FeedbackScorerStore` over SQLite (#7913 T2.1c).
//!
//! Persists `FeedbackScorer`'s per-(suggestion_type, source) tallies to the
//! `feedback_scorer_tallies` table (V44). `last_updated` round-trips as RFC3339
//! so the 12h self-decay stays wall-clock-anchored across restarts.

use chrono::{DateTime, Utc};
use maekon_core::error::CoreError;
use maekon_core::error_codes::StorageCode;
use maekon_core::models::suggestion::{FeedbackTallyRecord, SuggestionSource, SuggestionType};
use maekon_core::ports::feedback_scorer_store::FeedbackScorerStore;

use super::SqliteStorage;

fn storage_err(message: impl Into<String>) -> CoreError {
    CoreError::Storage {
        code: StorageCode::Failed,
        message: message.into(),
    }
}

impl FeedbackScorerStore for SqliteStorage {
    fn upsert_feedback_tally(&self, record: &FeedbackTallyRecord) -> Result<(), CoreError> {
        let type_str = record.suggestion_type.as_sql_str();
        let source_str = record.source.as_sql_str();
        let last_updated = record.last_updated.to_rfc3339();
        // Write — write_lock (skipped when deletion_flag is set;
        // feedback_scorer_tallies ∈ ALL_TABLES). Idempotent upsert keyed by the
        // table's PRIMARY KEY(suggestion_type, source).
        self.conn.write_lock().run((), |conn| {
            conn.execute(
                "INSERT INTO feedback_scorer_tallies
                    (suggestion_type, source, accepted, rejected, deferred, last_updated)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(suggestion_type, source) DO UPDATE SET
                    accepted = ?3,
                    rejected = ?4,
                    deferred = ?5,
                    last_updated = ?6",
                rusqlite::params![
                    type_str,
                    source_str,
                    record.accepted,
                    record.rejected,
                    record.deferred,
                    last_updated,
                ],
            )
            .map_err(|e| storage_err(format!("upsert_feedback_tally: {e}")))?;
            Ok(())
        })
    }

    fn load_feedback_tallies(&self) -> Result<Vec<FeedbackTallyRecord>, CoreError> {
        // Read — read_lock (independent of deletion_flag).
        let read = self.conn.read_lock();
        let conn = read.conn();
        let mut stmt = conn
            .prepare(
                "SELECT suggestion_type, source, accepted, rejected, deferred, last_updated
                 FROM feedback_scorer_tallies",
            )
            .map_err(|e| storage_err(format!("prepare load_feedback_tallies: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u32>(2)?,
                    row.get::<_, u32>(3)?,
                    row.get::<_, u32>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .map_err(|e| storage_err(format!("load_feedback_tallies: {e}")))?;

        let mut out = Vec::new();
        for row in rows {
            let (type_str, source_str, accepted, rejected, deferred, last_updated_str) =
                row.map_err(|e| storage_err(format!("row read: {e}")))?;
            // Skip (don't fail) a row with an unrecognized enum or unparsable
            // timestamp — advisory learning state must never block startup on a
            // corrupt row (T2.1c fail-safe). Warn with the offending value.
            let Some(suggestion_type) = SuggestionType::from_sql_str(&type_str) else {
                tracing::warn!(value = %type_str, "skipping feedback tally with unknown suggestion_type");
                continue;
            };
            let Some(source) = SuggestionSource::from_sql_str(&source_str) else {
                tracing::warn!(value = %source_str, "skipping feedback tally with unknown source");
                continue;
            };
            let last_updated = match DateTime::parse_from_rfc3339(&last_updated_str) {
                Ok(dt) => dt.with_timezone(&Utc),
                Err(e) => {
                    tracing::warn!(value = %last_updated_str, error = %e, "skipping feedback tally with unparsable last_updated");
                    continue;
                }
            };
            out.push(FeedbackTallyRecord {
                suggestion_type,
                source,
                accepted,
                rejected,
                deferred,
                last_updated,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(
        t: SuggestionType,
        s: SuggestionSource,
        acc: u32,
        rej: u32,
        last_updated: DateTime<Utc>,
    ) -> FeedbackTallyRecord {
        FeedbackTallyRecord {
            suggestion_type: t,
            source: s,
            accepted: acc,
            rejected: rej,
            deferred: 0,
            last_updated,
        }
    }

    #[test]
    fn empty_on_first_load() {
        let storage = SqliteStorage::open_in_memory(44).unwrap();
        assert!(storage.load_feedback_tallies().unwrap().is_empty());
    }

    #[test]
    fn upsert_then_load_roundtrip_preserves_timestamp() {
        let storage = SqliteStorage::open_in_memory(44).unwrap();
        let ts = DateTime::parse_from_rfc3339("2026-07-08T12:34:56Z")
            .unwrap()
            .with_timezone(&Utc);
        storage
            .upsert_feedback_tally(&rec(
                SuggestionType::WorkGuidance,
                SuggestionSource::RuleBased,
                7,
                3,
                ts,
            ))
            .unwrap();

        let loaded = storage.load_feedback_tallies().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].suggestion_type, SuggestionType::WorkGuidance);
        assert_eq!(loaded[0].source, SuggestionSource::RuleBased);
        assert_eq!(loaded[0].accepted, 7);
        assert_eq!(loaded[0].rejected, 3);
        // Wall-clock anchor preserved exactly — the whole point of T2.1c decay.
        assert_eq!(loaded[0].last_updated, ts);
    }

    #[test]
    fn upsert_is_idempotent_per_key() {
        let storage = SqliteStorage::open_in_memory(44).unwrap();
        let ts = Utc::now();
        storage
            .upsert_feedback_tally(&rec(
                SuggestionType::EmailDraft,
                SuggestionSource::LlmLocal,
                1,
                0,
                ts,
            ))
            .unwrap();
        storage
            .upsert_feedback_tally(&rec(
                SuggestionType::EmailDraft,
                SuggestionSource::LlmLocal,
                9,
                4,
                ts,
            ))
            .unwrap();
        let loaded = storage.load_feedback_tallies().unwrap();
        assert_eq!(
            loaded.len(),
            1,
            "same (type, source) must upsert, not duplicate"
        );
        assert_eq!(loaded[0].accepted, 9);
        assert_eq!(loaded[0].rejected, 4);
    }

    #[test]
    fn survives_restart_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("scorer.db");
        let ts = Utc::now();
        {
            let storage = SqliteStorage::open(&db_path, 44, None).unwrap();
            storage
                .upsert_feedback_tally(&rec(
                    SuggestionType::ProductivityTip,
                    SuggestionSource::LlmServer,
                    2,
                    5,
                    ts,
                ))
                .unwrap();
        }
        {
            let storage = SqliteStorage::open(&db_path, 44, None).unwrap();
            let loaded = storage.load_feedback_tallies().unwrap();
            assert_eq!(loaded.len(), 1);
            assert_eq!(loaded[0].suggestion_type, SuggestionType::ProductivityTip);
            assert_eq!(loaded[0].rejected, 5);
        }
    }
}
