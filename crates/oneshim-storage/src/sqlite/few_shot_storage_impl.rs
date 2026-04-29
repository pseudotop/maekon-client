use oneshim_core::error::CoreError;
use oneshim_core::models::suggestion::SuggestionHistoryEntry;
use oneshim_core::ports::few_shot_storage::FewShotStorage;

use super::SqliteStorage;

impl FewShotStorage for SqliteStorage {
    /// 피드백이 기록된 최근 suggestion을 조회한다.
    ///
    /// `local_suggestions` 테이블에서 `feedback_type IS NOT NULL`인 행을 내림차순으로
    /// 최대 `limit`개 반환한다. `confidence` 컬럼은 V28 마이그레이션에서 추가된다.
    fn get_suggestions_with_feedback(
        &self,
        limit: usize,
    ) -> Result<Vec<SuggestionHistoryEntry>, CoreError> {
        let conn = self.conn.lock().map_err(|e| CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("lock: {e}"),
        })?;

        let mut stmt = conn
            .prepare(
                "SELECT suggestion_id, suggestion_type, content, confidence,
                        feedback_type, regime_label, context_app, context_window, created_at
                 FROM local_suggestions
                 WHERE feedback_type IS NOT NULL
                 ORDER BY created_at DESC
                 LIMIT ?1",
            )
            .map_err(|e| CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
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
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("query: {e}"),
            })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("row: {e}"),
            })?);
        }
        Ok(result)
    }

    /// suggestion에 피드백을 기록한다.
    ///
    /// `suggestion_id`로 `local_suggestions` 행을 찾아 `feedback_type`, `feedback_at`,
    /// `context_app`, `context_window`, `regime_label`을 업데이트한다.
    fn record_suggestion_feedback(
        &self,
        suggestion_id: &str,
        feedback_type: &str,
        context_app: &str,
        context_window: &str,
        regime_label: Option<&str>,
    ) -> Result<(), CoreError> {
        let conn = self.conn.lock().map_err(|e| CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("lock: {e}"),
        })?;

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
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("update: {e}"),
        })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::ports::few_shot_storage::FewShotStorage;

    /// 피드백이 없는 빈 DB에서 빈 벡터를 반환한다.
    #[test]
    fn few_shot_storage_empty_returns_empty() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let result = FewShotStorage::get_suggestions_with_feedback(&storage, 10).unwrap();
        assert!(result.is_empty());
    }

    /// suggestion을 삽입하고 피드백을 기록한 뒤 올바르게 조회되는지 확인한다.
    #[test]
    fn few_shot_storage_record_and_retrieve() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        // local_suggestions에 직접 삽입 (V28 이후 컬럼 포함, payload는 NOT NULL이므로 빈 JSON 사용)
        {
            let conn = storage.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO local_suggestions
                 (suggestion_id, suggestion_type, content, confidence, payload, created_at)
                 VALUES ('sugg-001', 'WORK_GUIDANCE', 'Take a break', 0.85, '{}', '2026-04-06T10:00:00Z')",
                [],
            )
            .unwrap();
        }

        // 피드백 기록
        FewShotStorage::record_suggestion_feedback(
            &storage,
            "sugg-001",
            "ACCEPTED",
            "VSCode",
            "main.rs",
            Some("deep_work"),
        )
        .unwrap();

        // 조회 및 검증
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

    /// limit 파라미터가 반환 개수를 제한하는지 확인한다.
    #[test]
    fn few_shot_storage_limit_respected() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        // 3개의 suggestion 삽입 (payload NOT NULL 요건 충족을 위해 빈 JSON 사용)
        {
            let conn = storage.conn.lock().unwrap();
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

        // 모두 피드백 기록
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

        // limit=2로 조회 → 2개만 반환해야 한다
        let entries = FewShotStorage::get_suggestions_with_feedback(&storage, 2).unwrap();
        assert_eq!(entries.len(), 2);
    }

    /// 피드백 없는 suggestion은 조회 결과에 포함되지 않는다.
    #[test]
    fn few_shot_storage_only_feedbacked_entries_returned() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        {
            let conn = storage.conn.lock().unwrap();
            // 피드백 있는 항목 (payload NOT NULL 요건 충족)
            conn.execute(
                "INSERT INTO local_suggestions
                 (suggestion_id, suggestion_type, content, confidence, payload, created_at,
                  feedback_type, context_app, context_window)
                 VALUES ('with-feedback', 'WORK_GUIDANCE', 'Do standup', 0.9, '{}',
                         '2026-04-06T09:00:00Z', 'ACCEPTED', 'Slack', '#standup')",
                [],
            )
            .unwrap();
            // 피드백 없는 항목
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
}
