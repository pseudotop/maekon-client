use async_trait::async_trait;
use oneshim_core::error::CoreError;
use oneshim_core::models::storage_records::LocalSuggestionRecord;
use oneshim_core::ports::integration::LocalSuggestionQueryPort;

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
            let guard = storage.lock().map_err(|err| CoreError::Storage {
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("SQLite lock poisoned: {err}"),
            })?;

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
                code: oneshim_core::error_codes::StorageCode::Failed,
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
                code: oneshim_core::error_codes::StorageCode::Failed,
                message: format!("Failed to execute query: {err}"),
            })?;

            let mut records = Vec::new();
            for row in rows {
                records.push(row.map_err(|err| CoreError::Storage {
                    code: oneshim_core::error_codes::StorageCode::Failed,
                    message: format!("Failed to read row: {err}"),
                })?);
            }
            Ok(records)
        })
        .await
        .map_err(|err| CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: format!("spawn_blocking join error: {err}"),
        })?
    }
}

#[cfg(test)]
mod tests {
    #[allow(deprecated)]
    use oneshim_core::models::work_session::LocalSuggestion;

    use super::*;

    #[tokio::test]
    #[allow(deprecated)]
    async fn list_local_suggestions_after_returns_ascending_rows() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        let first = storage
            .save_local_suggestion(&LocalSuggestion::TakeBreak {
                continuous_work_mins: 90,
            })
            .unwrap();
        let second = storage
            .save_local_suggestion(&LocalSuggestion::NeedFocusTime {
                communication_ratio: 0.6,
                suggested_focus_mins: 45,
            })
            .unwrap();

        let rows = storage
            .list_local_suggestions_after(Some(first), 10)
            .await
            .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, second);
        assert_eq!(rows[0].suggestion_type, "NeedFocusTime");
    }
}
