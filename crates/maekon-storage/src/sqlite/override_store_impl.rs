//! SQLite implementation of the `OverrideStore` port.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use maekon_core::error::CoreError;
use maekon_core::models::recalibration::{RegimeOverride, UserOverrideAction};
use maekon_core::ports::override_store::OverrideStore;
use rusqlite::{params, OptionalExtension};

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

        let clock = self.clock.clone();
        self.with_conn(move |conn| {
            // F0/#5186: stamp a monotonic HLC so this override propagates via sync.
            let h = clock
                .next(conn)
                .map_err(|e| StorageError::Internal(format!("hlc stamp: {e}")))?;
            conn.execute(
                "INSERT INTO regime_overrides (override_id, segment_id, original_regime_id, action_type, action_data, created_at, hlc_wall_ms, hlc_counter, origin_device_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![override_id, segment_id, original_regime_id, action_type, action_data, created_at, h.wall_ms, h.counter, h.device_id],
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

        // Pure SELECT: route through the READ funnel so it always executes.
        // The WRITE funnel (`with_conn`) is skipped (returns `T::default()`,
        // i.e. an empty Vec here) while `deletion_flag || erasing` is set, which
        // would silently return no overrides during an erase window. Reads must
        // never be blocked by the #4928 erase barrier, matching the other
        // SELECT-only paths (annotation/habit/work_sessions/...).
        self.with_conn_read(move |conn| {
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
        let clock = self.clock.clone();

        // #8086: `regime_overrides` is a synced table, so a bare local DELETE
        // left the synchronized copies behind — peers kept the row and the next
        // pull could resurrect it locally (no suppression entry). Match the
        // #8043/#8068 durable-erasure discipline: record a `sync_tombstones`
        // suppression row (stamped at a FRESH deletion HLC so it orders above
        // the row and every peer copy, but carrying the row's ORIGINAL
        // `origin_device_id` — the receiving merger deletes by
        // `pk AND origin_device_id`) and hard-delete the local row in one
        // transaction.
        self.with_conn(move |conn| {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| StorageError::Internal(format!("delete override tx: {e}")))?;

            let row_origin: Option<String> = tx
                .query_row(
                    "SELECT origin_device_id FROM regime_overrides WHERE override_id = ?1",
                    params![id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| StorageError::Internal(format!("Failed to read override: {e}")))?;

            let Some(row_origin) = row_origin else {
                return Err(StorageError::NotFound {
                    resource_type: "RegimeOverride".to_string(),
                    id,
                });
            };

            let h = clock
                .next(&tx)
                .map_err(|e| StorageError::Internal(format!("hlc stamp: {e}")))?;
            tx.execute(
                "INSERT INTO sync_tombstones \
                   (table_name, row_id, origin_device_id, hlc_wall_ms, hlc_counter, deleted_at) \
                 VALUES ('regime_overrides', ?1, ?2, ?3, ?4, datetime('now')) \
                 ON CONFLICT(table_name, row_id) DO UPDATE SET \
                   origin_device_id = excluded.origin_device_id, \
                   hlc_wall_ms = excluded.hlc_wall_ms, \
                   hlc_counter = excluded.hlc_counter, \
                   deleted_at  = excluded.deleted_at \
                 WHERE excluded.hlc_wall_ms > sync_tombstones.hlc_wall_ms \
                    OR (excluded.hlc_wall_ms = sync_tombstones.hlc_wall_ms \
                        AND excluded.hlc_counter > sync_tombstones.hlc_counter)",
                params![id, row_origin, h.wall_ms, h.counter],
            )
            .map_err(|e| StorageError::Internal(format!("record override tombstone: {e}")))?;

            tx.execute(
                "DELETE FROM regime_overrides WHERE override_id = ?1",
                params![id],
            )
            .map_err(|e| StorageError::Internal(format!("Failed to delete override: {e}")))?;

            tx.commit()
                .map_err(|e| StorageError::Internal(format!("delete override commit: {e}")))?;
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
    async fn save_override_stamps_monotonic_hlc() {
        // F0/#5186: regime_overrides INSERT must carry a monotonic HLC + device id so it
        // propagates via cross-device sync (previously hlc=0 → never propagated).
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        {
            let c = storage.conn.test_lock();
            c.execute(
                "INSERT INTO device_identity (id, device_id, device_name) VALUES (1, 'dev-z', 'Z')",
                [],
            )
            .unwrap();
        }
        let entry = RegimeOverride {
            override_id: "ovr-hlc".to_string(),
            segment_id: "seg-hlc".to_string(),
            original_regime_id: None,
            user_action: UserOverrideAction::MarkAsNoise,
            created_at: Utc::now(),
        };
        storage.save_override(&entry).await.unwrap();

        let (wall, dev): (i64, String) = {
            let c = storage.conn.test_lock();
            c.query_row(
                "SELECT hlc_wall_ms, origin_device_id FROM regime_overrides \
                 WHERE override_id = 'ovr-hlc'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
        };
        assert!(wall > 0, "regime_override must be HLC-stamped");
        assert_eq!(dev, "dev-z", "origin_device_id must be stamped");
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

    /// #8086: a local delete of a synced override must record a suppression
    /// tombstone (carrying the row's ORIGINAL origin_device_id, stamped at a
    /// FRESH deletion HLC above the row's own) so peers hard-delete their
    /// copies and cannot resurrect the row on the next pull.
    #[tokio::test]
    async fn delete_override_records_suppression_tombstone() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let entry = RegimeOverride {
            override_id: "ovr-ts".to_string(),
            segment_id: "seg-ts".to_string(),
            original_regime_id: None,
            user_action: UserOverrideAction::MarkAsNoise,
            created_at: Utc::now(),
        };
        storage.save_override(&entry).await.unwrap();

        let (row_origin, row_wall): (String, i64) = {
            let conn = storage.connection_arc();
            let guard = conn.test_lock();
            guard
                .query_row(
                    "SELECT origin_device_id, hlc_wall_ms FROM regime_overrides \
                     WHERE override_id='ovr-ts'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap()
        };

        storage.delete_override("ovr-ts").await.unwrap();

        let conn = storage.connection_arc();
        let guard = conn.test_lock();
        let (ts_origin, ts_wall, ts_deleted_at): (String, i64, String) = guard
            .query_row(
                "SELECT origin_device_id, hlc_wall_ms, deleted_at FROM sync_tombstones \
                 WHERE table_name='regime_overrides' AND row_id='ovr-ts'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("delete_override must record a sync_tombstones suppression row");
        assert_eq!(
            ts_origin, row_origin,
            "tombstone must carry the ROW's origin_device_id (merger deletes by pk AND origin)"
        );
        assert!(
            ts_wall >= row_wall,
            "deletion HLC must order at/above the row's own HLC"
        );
        assert!(!ts_deleted_at.is_empty());

        let remaining: i64 = guard
            .query_row(
                "SELECT COUNT(*) FROM regime_overrides WHERE override_id='ovr-ts'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0, "local row must be hard-deleted");
    }

    #[tokio::test]
    async fn delete_nonexistent_override_returns_not_found() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        let err = storage.delete_override("nonexistent-id").await.unwrap_err();
        assert!(
            matches!(err, CoreError::NotFound { .. }),
            "expected NotFound, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn list_overrides_survives_erase_barrier() {
        // Regression: `list_overrides` is a pure SELECT and must route through the
        // READ funnel (`with_conn_read`), not the WRITE funnel (`with_conn`).
        // The write funnel is skipped while `deletion_flag || erasing` is set,
        // which previously made this read return an empty Vec during an erase
        // window (the #4928 erase barrier wrongly blocking a read).
        use std::sync::atomic::Ordering;

        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let now = Utc::now();

        let entry = RegimeOverride {
            override_id: "ovr-erase".to_string(),
            segment_id: "seg-erase".to_string(),
            original_regime_id: None,
            user_action: UserOverrideAction::MarkAsNoise,
            created_at: now,
        };
        storage.save_override(&entry).await.unwrap();

        // Enter an erase window: writes must now be skipped, reads must not.
        storage.deletion_flag().store(true, Ordering::Release);

        // Control: a write IS skipped while the barrier is set (no error, no-op),
        // proving the barrier is genuinely active for this connection.
        let skipped = RegimeOverride {
            override_id: "ovr-skipped".to_string(),
            segment_id: "seg-skipped".to_string(),
            original_regime_id: None,
            user_action: UserOverrideAction::MarkAsNoise,
            created_at: now,
        };
        storage.save_override(&skipped).await.unwrap();

        // The read MUST still return the row persisted before the barrier.
        let overrides = storage
            .list_overrides(now - Duration::hours(1), now + Duration::hours(1))
            .await
            .unwrap();

        assert_eq!(
            overrides.len(),
            1,
            "pure SELECT must execute through the read funnel during an erase window"
        );
        assert_eq!(overrides[0].override_id, "ovr-erase");
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
