use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::models::storage_records::LocalSuggestionRecord;
use maekon_core::ports::integration::LocalSuggestionQueryPort;

use super::edge_intelligence::map_local_suggestion_row;
use super::SqliteStorage;

#[async_trait]
impl LocalSuggestionQueryPort for SqliteStorage {
    async fn list_local_suggestions_after(
        &self,
        after_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<LocalSuggestionRecord>, CoreError> {
        let storage = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            // Read — read_lock (independent of deletion_flag).
            let read = storage.read_lock();
            let guard = read.conn();

            let sql = if after_id.is_some() {
                "SELECT id, suggestion_type, payload, created_at, shown_at, dismissed_at, acted_at
                 FROM local_suggestions
                 WHERE id > ?1
                 ORDER BY id ASC
                 LIMIT ?2"
            } else {
                "SELECT id, suggestion_type, payload, created_at, shown_at, dismissed_at, acted_at
                 FROM local_suggestions
                 ORDER BY id ASC
                 LIMIT ?1"
            };

            let mut stmt = guard.prepare(sql).map_err(|err| CoreError::Storage {
                code: maekon_core::error_codes::StorageCode::Failed,
                message: format!("Failed to prepare query: {err}"),
            })?;

            let rows = if let Some(after_id) = after_id {
                stmt.query_map(
                    rusqlite::params![after_id, limit as i64],
                    map_local_suggestion_row,
                )
            } else {
                stmt.query_map(rusqlite::params![limit as i64], map_local_suggestion_row)
            }
            .map_err(|err| CoreError::Storage {
                code: maekon_core::error_codes::StorageCode::Failed,
                message: format!("Failed to execute query: {err}"),
            })?;

            let mut records = Vec::new();
            for row in rows {
                records.push(row.map_err(|err| CoreError::Storage {
                    code: maekon_core::error_codes::StorageCode::Failed,
                    message: format!("Failed to read row: {err}"),
                })?);
            }
            Ok(records)
        })
        .await
        .map_err(|err| CoreError::Storage {
            code: maekon_core::error_codes::StorageCode::Failed,
            message: format!("spawn_blocking join error: {err}"),
        })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #7733: fixture rows are inserted via raw SQL (the deprecated `LocalSuggestion`
    /// enum writer `save_local_suggestion` was dead code and has been deleted); the
    /// `local_suggestions` table + `LocalSuggestionQueryPort` read path stay live
    /// (consumed by `LocalSuggestionIntegrationSource` in `src-tauri`).
    #[tokio::test]
    async fn list_local_suggestions_after_returns_ascending_rows() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        let (first, second): (i64, i64) = {
            let conn = storage.conn.test_lock();
            conn.execute(
                "INSERT INTO local_suggestions (suggestion_type, payload) VALUES (?1, ?2)",
                rusqlite::params![
                    "TakeBreak",
                    serde_json::json!({ "continuous_work_mins": 90 }).to_string()
                ],
            )
            .unwrap();
            let first = conn.last_insert_rowid();

            conn.execute(
                "INSERT INTO local_suggestions (suggestion_type, payload) VALUES (?1, ?2)",
                rusqlite::params![
                    "NeedFocusTime",
                    serde_json::json!({
                        "communication_ratio": 0.6,
                        "suggested_focus_mins": 45,
                    })
                    .to_string()
                ],
            )
            .unwrap();
            let second = conn.last_insert_rowid();

            (first, second)
        };

        let rows = storage
            .list_local_suggestions_after(Some(first), 10)
            .await
            .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, second);
        assert_eq!(rows[0].suggestion_type, "NeedFocusTime");
    }
}
