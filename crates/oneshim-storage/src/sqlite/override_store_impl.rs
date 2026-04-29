//! SQLite implementation of the `OverrideStore` port.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use oneshim_core::error::CoreError;
use oneshim_core::models::recalibration::{RegimeOverride, UserOverrideAction};
use oneshim_core::ports::override_store::OverrideStore;
use rusqlite::params;

use super::SqliteStorage;
use crate::error::StorageError;

/// Serialize `UserOverrideAction` into (action_type, action_data) for storage.
fn serialize_action(action: &UserOverrideAction) -> (String, Option<String>) {
    match action {
        UserOverrideAction::MarkAsNoise => ("MARK_AS_NOISE".to_string(), None),
        UserOverrideAction::ReassignRegime { target_regime_id } => (
            "REASSIGN_REGIME".to_string(),
            Some(serde_json::json!({ "target_regime_id": target_regime_id }).to_string()),
        ),
        UserOverrideAction::MarkAsPersonalTime { from, to } => (
            "MARK_AS_PERSONAL_TIME".to_string(),
            Some(
                serde_json::json!({
                    "from": from.to_rfc3339(),
                    "to": to.to_rfc3339(),
                })
                .to_string(),
            ),
        ),
    }
}

/// Deserialize (action_type, action_data) back into `UserOverrideAction`.
fn deserialize_action(
    action_type: &str,
    action_data: Option<&str>,
) -> Result<UserOverrideAction, StorageError> {
    match action_type {
        "MARK_AS_NOISE" => Ok(UserOverrideAction::MarkAsNoise),
        "REASSIGN_REGIME" => {
            let data = action_data.ok_or_else(|| {
                StorageError::Internal("Missing action_data for REASSIGN_REGIME".to_string())
            })?;
            let parsed: serde_json::Value = serde_json::from_str(data).map_err(|e| {
                StorageError::Internal(format!("Failed to parse REASSIGN_REGIME action_data: {e}"))
            })?;
            let target = parsed["target_regime_id"]
                .as_str()
                .ok_or_else(|| {
                    StorageError::Internal("Missing target_regime_id in action_data".to_string())
                })?
                .to_string();
            Ok(UserOverrideAction::ReassignRegime {
                target_regime_id: target,
            })
        }
        "MARK_AS_PERSONAL_TIME" => {
            let data = action_data.ok_or_else(|| {
                StorageError::Internal("Missing action_data for MARK_AS_PERSONAL_TIME".to_string())
            })?;
            let parsed: serde_json::Value = serde_json::from_str(data).map_err(|e| {
                StorageError::Internal(format!(
                    "Failed to parse MARK_AS_PERSONAL_TIME action_data: {e}"
                ))
            })?;
            let from_str = parsed["from"].as_str().ok_or_else(|| {
                StorageError::Internal("Missing 'from' in action_data".to_string())
            })?;
            let to_str = parsed["to"]
                .as_str()
                .ok_or_else(|| StorageError::Internal("Missing 'to' in action_data".to_string()))?;
            let from = DateTime::parse_from_rfc3339(from_str)
                .map_err(|e| StorageError::Internal(format!("Invalid 'from' datetime: {e}")))?
                .with_timezone(&Utc);
            let to = DateTime::parse_from_rfc3339(to_str)
                .map_err(|e| StorageError::Internal(format!("Invalid 'to' datetime: {e}")))?
                .with_timezone(&Utc);
            Ok(UserOverrideAction::MarkAsPersonalTime { from, to })
        }
        other => Err(StorageError::Internal(format!(
            "Unknown action_type: {other}"
        ))),
    }
}

#[async_trait]
impl OverrideStore for SqliteStorage {
    async fn save_override(&self, entry: &RegimeOverride) -> Result<(), CoreError> {
        let override_id = entry.override_id.clone();
        let segment_id = entry.segment_id.clone();
        let original_regime_id = entry.original_regime_id.clone();
        let (action_type, action_data) = serialize_action(&entry.user_action);
        let created_at = entry.created_at.to_rfc3339();

        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO regime_overrides (override_id, segment_id, original_regime_id, action_type, action_data, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![override_id, segment_id, original_regime_id, action_type, action_data, created_at],
            )
            .map_err(|e| StorageError::Internal(format!("Failed to save override: {e}")))?;
            Ok(())
        })
        .await
        .map_err(Into::into)
    }

    async fn list_overrides(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<RegimeOverride>, CoreError> {
        let from_str = from.to_rfc3339();
        let to_str = to.to_rfc3339();

        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT override_id, segment_id, original_regime_id, action_type, action_data, created_at
                     FROM regime_overrides
                     WHERE created_at >= ?1 AND created_at <= ?2
                     ORDER BY created_at ASC",
                )
                .map_err(|e| StorageError::Internal(format!("Failed to prepare list query: {e}")))?;

            let rows = stmt
                .query_map(params![from_str, to_str], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                })
                .map_err(|e| StorageError::Internal(format!("Failed to query overrides: {e}")))?;

            let mut overrides = Vec::new();
            for row in rows {
                let (override_id, segment_id, original_regime_id, action_type, action_data, created_at_str) =
                    row.map_err(|e| StorageError::Internal(format!("Row read error: {e}")))?;

                let user_action =
                    deserialize_action(&action_type, action_data.as_deref())?;

                let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                    .map_err(|e| StorageError::Internal(format!("Invalid created_at: {e}")))?
                    .with_timezone(&Utc);

                overrides.push(RegimeOverride {
                    override_id,
                    segment_id,
                    original_regime_id,
                    user_action,
                    created_at,
                });
            }

            Ok(overrides)
        })
        .await
        .map_err(Into::into)
    }

    async fn delete_override(&self, override_id: &str) -> Result<(), CoreError> {
        let id = override_id.to_string();

        self.with_conn(move |conn| {
            conn.execute(
                "DELETE FROM regime_overrides WHERE override_id = ?1",
                params![id],
            )
            .map_err(|e| StorageError::Internal(format!("Failed to delete override: {e}")))?;

            if conn.changes() == 0 {
                return Err(StorageError::NotFound {
                    resource_type: "RegimeOverride".to_string(),
                    id,
                });
            }
            Ok(())
        })
        .await
        .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[tokio::test]
    async fn save_and_list_roundtrip() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let now = Utc::now();

        let entry = RegimeOverride {
            override_id: "ovr-001".to_string(),
            segment_id: "seg-001".to_string(),
            original_regime_id: Some("regime-0".to_string()),
            user_action: UserOverrideAction::MarkAsNoise,
            created_at: now,
        };

        storage.save_override(&entry).await.unwrap();

        let from = now - Duration::hours(1);
        let to = now + Duration::hours(1);
        let overrides = storage.list_overrides(from, to).await.unwrap();

        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].override_id, "ovr-001");
        assert_eq!(overrides[0].segment_id, "seg-001");
        assert!(matches!(
            overrides[0].user_action,
            UserOverrideAction::MarkAsNoise
        ));
    }

    #[tokio::test]
    async fn save_reassign_regime_roundtrip() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let now = Utc::now();

        let entry = RegimeOverride {
            override_id: "ovr-002".to_string(),
            segment_id: "seg-002".to_string(),
            original_regime_id: None,
            user_action: UserOverrideAction::ReassignRegime {
                target_regime_id: "regime-3".to_string(),
            },
            created_at: now,
        };

        storage.save_override(&entry).await.unwrap();

        let overrides = storage
            .list_overrides(now - Duration::hours(1), now + Duration::hours(1))
            .await
            .unwrap();

        assert_eq!(overrides.len(), 1);
        match &overrides[0].user_action {
            UserOverrideAction::ReassignRegime { target_regime_id } => {
                assert_eq!(target_regime_id, "regime-3");
            }
            other => panic!("Expected ReassignRegime, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn save_personal_time_roundtrip() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let now = Utc::now();
        let from_time = now - Duration::hours(2);
        let to_time = now - Duration::hours(1);

        let entry = RegimeOverride {
            override_id: "ovr-003".to_string(),
            segment_id: "seg-003".to_string(),
            original_regime_id: Some("regime-1".to_string()),
            user_action: UserOverrideAction::MarkAsPersonalTime {
                from: from_time,
                to: to_time,
            },
            created_at: now,
        };

        storage.save_override(&entry).await.unwrap();

        let overrides = storage
            .list_overrides(now - Duration::hours(1), now + Duration::hours(1))
            .await
            .unwrap();

        assert_eq!(overrides.len(), 1);
        match &overrides[0].user_action {
            UserOverrideAction::MarkAsPersonalTime { from, to } => {
                // Compare seconds-level precision (rfc3339 roundtrip)
                assert_eq!(from.timestamp(), from_time.timestamp());
                assert_eq!(to.timestamp(), to_time.timestamp());
            }
            other => panic!("Expected MarkAsPersonalTime, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn delete_override_removes_entry() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let now = Utc::now();

        let entry = RegimeOverride {
            override_id: "ovr-del".to_string(),
            segment_id: "seg-del".to_string(),
            original_regime_id: None,
            user_action: UserOverrideAction::MarkAsNoise,
            created_at: now,
        };

        storage.save_override(&entry).await.unwrap();
        storage.delete_override("ovr-del").await.unwrap();

        let overrides = storage
            .list_overrides(now - Duration::hours(1), now + Duration::hours(1))
            .await
            .unwrap();

        assert!(overrides.is_empty());
    }

    #[tokio::test]
    async fn delete_nonexistent_override_returns_not_found() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        let result = storage.delete_override("nonexistent-id").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, CoreError::NotFound { .. }),
            "expected NotFound, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn list_overrides_respects_date_range() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let now = Utc::now();

        // Create an override with "old" created_at
        let old_entry = RegimeOverride {
            override_id: "ovr-old".to_string(),
            segment_id: "seg-old".to_string(),
            original_regime_id: None,
            user_action: UserOverrideAction::MarkAsNoise,
            created_at: now - Duration::days(10),
        };
        storage.save_override(&old_entry).await.unwrap();

        let new_entry = RegimeOverride {
            override_id: "ovr-new".to_string(),
            segment_id: "seg-new".to_string(),
            original_regime_id: None,
            user_action: UserOverrideAction::MarkAsNoise,
            created_at: now,
        };
        storage.save_override(&new_entry).await.unwrap();

        // Query only recent range
        let overrides = storage
            .list_overrides(now - Duration::hours(1), now + Duration::hours(1))
            .await
            .unwrap();

        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].override_id, "ovr-new");
    }
}
