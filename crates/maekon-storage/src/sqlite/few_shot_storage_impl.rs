use maekon_core::error::CoreError;
use maekon_core::models::suggestion::SuggestionHistoryEntry;
use maekon_core::ports::few_shot_storage::FewShotStorage;

use super::SqliteStorage;

impl FewShotStorage for SqliteStorage {
    /// Queries the most recent suggestions that have recorded feedback.
    ///
    /// Returns up to `limit` rows from the `local_suggestions` table where
    /// `feedback_type IS NOT NULL`, ordered descending. The `confidence` column
    /// is added by the V28 migration.
    fn get_suggestions_with_feedback(
        &self,
        limit: usize,
    ) -> Result<Vec<SuggestionHistoryEntry>, CoreError> {
        // Read — read_lock (independent of deletion_flag).
        let read = self.conn.read_lock();
        let conn = read.conn();

        let mut stmt = conn
            .prepare(
                "SELECT suggestion_id, suggestion_type, content, confidence,
                        feedback_type, regime_label, context_app, context_window, created_at
                 FROM (
                     SELECT suggestion_id, suggestion_type, content, confidence,
                            feedback_type, regime_label, context_app, context_window, created_at
                     FROM local_suggestions
                     WHERE feedback_type IS NOT NULL
                     UNION ALL
                     SELECT s.suggestion_id,
                            s.suggestion_type,
                            s.content,
                            s.confidence_score AS confidence,
                            CASE
                                WHEN s.acted_at IS NOT NULL THEN 'ACCEPTED'
                                WHEN s.dismissed_at IS NOT NULL THEN 'REJECTED'
                                WHEN s.state = 'deferred' THEN 'DEFERRED'
                            END AS feedback_type,
                            NULL AS regime_label,
                            '' AS context_app,
                            '' AS context_window,
                            s.created_at
                     FROM suggestions s
                     WHERE (s.acted_at IS NOT NULL
                            OR s.dismissed_at IS NOT NULL
                            OR s.state = 'deferred')
                       AND NOT EXISTS (
                           SELECT 1
                           FROM local_suggestions l
                           WHERE l.suggestion_id = s.suggestion_id
                             AND l.feedback_type IS NOT NULL
                       )
                 )
                 ORDER BY created_at DESC
                 LIMIT ?1",
            )
            .map_err(|e| CoreError::Storage {
                code: maekon_core::error_codes::StorageCode::Failed,
                message: format!("prepare: {e}"),
            })?;

        let rows = stmt
            .query_map([limit as i64], |row| {
                Ok(SuggestionHistoryEntry {
                    suggestion_id: row.get(0)?,
                    suggestion_type: row.get(1)?,
                    content: row.get(2)?,
                    confidence: row.get(3)?,
                    feedback_type: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    regime_label: row.get(5)?,
                    context_app: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
                    context_window: row.get::<_, Option<String>>(7)?.unwrap_or_default(),
                    created_at: row
                        .get::<_, String>(8)
                        .ok()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or_else(chrono::Utc::now),
                })
            })
            .map_err(|e| CoreError::Storage {
                code: maekon_core::error_codes::StorageCode::Failed,
                message: format!("query: {e}"),
            })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| CoreError::Storage {
                code: maekon_core::error_codes::StorageCode::Failed,
                message: format!("row: {e}"),
            })?);
        }
        Ok(result)
    }

    /// Records feedback for a suggestion.
    ///
    /// Finds the `local_suggestions` row by `suggestion_id` and updates
    /// `feedback_type`, `feedback_at`, `context_app`, `context_window`, and
    /// `regime_label`.
    fn record_suggestion_feedback(
        &self,
        suggestion_id: &str,
        feedback_type: &str,
        context_app: &str,
        context_window: &str,
        regime_label: Option<&str>,
    ) -> Result<(), CoreError> {
        // Write — write_lock (skipped when deletion_flag is set; local_suggestions ∈ ALL_TABLES).
        self.conn.write_lock().run((), |conn| {
            conn.execute(
                "UPDATE local_suggestions
                 SET feedback_type  = ?1,
                     feedback_at    = ?2,
                     context_app    = ?3,
                     context_window = ?4,
                     regime_label   = ?5
                 WHERE suggestion_id = ?6",
                rusqlite::params![
                    feedback_type,
                    chrono::Utc::now().to_rfc3339(),
                    context_app,
                    context_window,
                    regime_label,
                    suggestion_id,
                ],
            )
            .map_err(|e| CoreError::Storage {
                code: maekon_core::error_codes::StorageCode::Failed,
                message: format!("update: {e}"),
            })?;

            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::ports::few_shot_storage::FewShotStorage;

    /// Returns an empty vector for an empty DB with no feedback.
    #[test]
    fn few_shot_storage_empty_returns_empty() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let result = FewShotStorage::get_suggestions_with_feedback(&storage, 10).unwrap();
        assert!(result.is_empty());
    }

    /// Inserts a suggestion, records feedback, then verifies it is retrieved correctly.
    #[test]
    fn few_shot_storage_record_and_retrieve() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        // Insert directly into local_suggestions (includes post-V28 columns; payload is NOT NULL so use empty JSON)
        {
            let conn = storage.conn.test_lock();
            conn.execute(
                "INSERT INTO local_suggestions
                 (suggestion_id, suggestion_type, content, confidence, payload, created_at)
                 VALUES ('sugg-001', 'WORK_GUIDANCE', 'Take a break', 0.85, '{}', '2026-04-06T10:00:00Z')",
                [],
            )
            .unwrap();
        }

        // Record feedback
        FewShotStorage::record_suggestion_feedback(
            &storage,
            "sugg-001",
            "ACCEPTED",
            "VSCode",
            "main.rs",
            Some("deep_work"),
        )
        .unwrap();

        // Query and verify
        let entries = FewShotStorage::get_suggestions_with_feedback(&storage, 10).unwrap();
        assert_eq!(entries.len(), 1);

        let entry = &entries[0];
        assert_eq!(entry.suggestion_id, "sugg-001");
        assert_eq!(entry.suggestion_type, "WORK_GUIDANCE");
        assert_eq!(entry.content, "Take a break");
        assert!((entry.confidence - 0.85).abs() < f64::EPSILON);
        assert_eq!(entry.feedback_type, "ACCEPTED");
        assert_eq!(entry.context_app, "VSCode");
        assert_eq!(entry.context_window, "main.rs");
        assert_eq!(entry.regime_label, Some("deep_work".to_string()));
    }

    /// Verifies that the `limit` parameter caps the number of returned rows.
    #[test]
    fn few_shot_storage_limit_respected() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        // Insert 3 suggestions (use empty JSON to satisfy the payload NOT NULL constraint)
        {
            let conn = storage.conn.test_lock();
            for i in 1..=3u32 {
                conn.execute(
                    "INSERT INTO local_suggestions
                     (suggestion_id, suggestion_type, content, confidence, payload, created_at)
                     VALUES (?1, 'PRODUCTIVITY_TIP', ?2, 0.7, '{}', ?3)",
                    rusqlite::params![
                        format!("sugg-{i:03}"),
                        format!("Tip number {i}"),
                        format!("2026-04-06T10:0{i}:00Z"),
                    ],
                )
                .unwrap();
            }
        }

        // Record feedback for all of them
        for i in 1..=3u32 {
            FewShotStorage::record_suggestion_feedback(
                &storage,
                &format!("sugg-{i:03}"),
                "ACCEPTED",
                "Terminal",
                "",
                None,
            )
            .unwrap();
        }

        // Query with limit=2 → must return only 2
        let entries = FewShotStorage::get_suggestions_with_feedback(&storage, 2).unwrap();
        assert_eq!(entries.len(), 2);
    }

    /// Suggestions without feedback are excluded from the query result.
    #[test]
    fn few_shot_storage_only_feedbacked_entries_returned() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        {
            let conn = storage.conn.test_lock();
            // Item with feedback (satisfies the payload NOT NULL constraint)
            conn.execute(
                "INSERT INTO local_suggestions
                 (suggestion_id, suggestion_type, content, confidence, payload, created_at,
                  feedback_type, context_app, context_window)
                 VALUES ('with-feedback', 'WORK_GUIDANCE', 'Do standup', 0.9, '{}',
                         '2026-04-06T09:00:00Z', 'ACCEPTED', 'Slack', '#standup')",
                [],
            )
            .unwrap();
            // Item without feedback
            conn.execute(
                "INSERT INTO local_suggestions
                 (suggestion_id, suggestion_type, content, confidence, payload, created_at)
                 VALUES ('no-feedback', 'PRODUCTIVITY_TIP', 'Stay hydrated', 0.6, '{}',
                         '2026-04-06T09:30:00Z')",
                [],
            )
            .unwrap();
        }

        let entries = FewShotStorage::get_suggestions_with_feedback(&storage, 10).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].suggestion_id, "with-feedback");
    }

    #[test]
    fn few_shot_storage_reads_unified_accept_reject_and_deferred_feedback() {
        use maekon_core::models::suggestion::{
            Priority, Suggestion, SuggestionSource, SuggestionType,
        };

        fn suggestion(id: &str, content: &str) -> Suggestion {
            Suggestion {
                suggestion_id: id.to_string(),
                suggestion_type: SuggestionType::WorkGuidance,
                content: content.to_string(),
                priority: Priority::Medium,
                confidence_score: 0.8,
                relevance_score: 0.9,
                is_actionable: true,
                created_at: chrono::Utc::now(),
                expires_at: None,
                source: SuggestionSource::LlmLocal,
                reasoning: None,
                context_scope: None,
            }
        }

        let storage = SqliteStorage::open_in_memory(30).unwrap();
        storage
            .save_rule_suggestion_sync(&suggestion("unified-accepted", "Accepted example"))
            .unwrap();
        storage
            .save_rule_suggestion_sync(&suggestion("unified-rejected", "Rejected example"))
            .unwrap();
        let deferred = suggestion("unified-deferred", "Deferred example");
        storage.save_rule_suggestion_sync(&deferred).unwrap();

        storage
            .mark_unified_suggestion_acted("unified-accepted")
            .unwrap();
        storage
            .dismiss_unified_suggestion("unified-rejected")
            .unwrap();
        storage
            .save_suggestion_with_state(
                &deferred,
                "deferred",
                Some(&(chrono::Utc::now() + chrono::Duration::minutes(30)).to_rfc3339()),
            )
            .unwrap();

        let entries = FewShotStorage::get_suggestions_with_feedback(&storage, 10).unwrap();
        let feedback_by_id = entries
            .iter()
            .map(|entry| (entry.suggestion_id.as_str(), entry.feedback_type.as_str()))
            .collect::<std::collections::BTreeMap<_, _>>();

        assert_eq!(feedback_by_id.get("unified-accepted"), Some(&"ACCEPTED"));
        assert_eq!(feedback_by_id.get("unified-rejected"), Some(&"REJECTED"));
        assert_eq!(feedback_by_id.get("unified-deferred"), Some(&"DEFERRED"));
    }
}
