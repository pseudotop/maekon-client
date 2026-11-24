use rusqlite::Connection;

/// V58: Keep private upload evidence outside public Event JSON.
///
/// Legacy/import/batch writers leave this nullable column unknown. Evidence has
/// exactly its parent event's lifetime, including retention and consent erasure.
pub(super) fn migrate_v58(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "ALTER TABLE events ADD COLUMN upload_provenance TEXT;
         INSERT OR IGNORE INTO schema_version (version) VALUES (58);",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone, Utc};
    use maekon_core::models::event::{
        ContextEvent, Event, SystemEvent, SystemEventType, WindowInfo, WindowLayoutEvent,
        WindowLayoutEventType,
    };
    use maekon_core::ports::batch_sink::UploadProvenance;
    use maekon_core::ports::storage::StorageService;
    use maekon_core::types::TimeWindow;
    use rusqlite::types::Value;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use uuid::Uuid;

    use crate::sqlite::{storage_event_id, SqliteStorage};

    type EventRow = (String, String, String, String, i64);

    fn timestamp() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 25, 12, 0, 0)
            .single()
            .unwrap()
    }

    fn context(title: &str) -> Event {
        Event::Context(ContextEvent {
            app_name: "Code".to_string(),
            window_title: title.to_string(),
            prev_app_name: None,
            timestamp: timestamp(),
            input_activity_level: 0.25,
        })
    }

    fn proof(event: &Event) -> UploadProvenance {
        UploadProvenance::new(&storage_event_id(event), event, [171; 32], true, true).unwrap()
    }

    fn event_rows(storage: &SqliteStorage) -> Vec<EventRow> {
        let conn = storage.conn.test_lock();
        let mut stmt = conn
            .prepare("SELECT event_id, event_type, timestamp, data, is_sent FROM events ORDER BY event_id")
            .unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .unwrap();
        rows.collect::<Result<Vec<_>, _>>().unwrap()
    }

    fn proof_value(storage: &SqliteStorage, id: &str) -> Value {
        storage
            .conn
            .test_lock()
            .query_row(
                "SELECT upload_provenance FROM events WHERE event_id = ?1",
                [id],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn nullable_private_column_preserves_legacy_event_bytes() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version (version) VALUES (57);
             CREATE TABLE events (event_id TEXT PRIMARY KEY, data TEXT, is_sent INTEGER);
             INSERT INTO events (event_id, data, is_sent)
             VALUES ('legacy', '{\"type\":\"Context\",\"window_title\":\"masked\"}', 1);",
        )
        .unwrap();
        let before: (String, String, i64) = conn
            .query_row("SELECT event_id, data, is_sent FROM events", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .unwrap();
        migrate_v58(&conn).unwrap();
        let after: (String, String, i64, Option<String>) = conn
            .query_row(
                "SELECT event_id, data, is_sent, upload_provenance FROM events",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!((after.0, after.1, after.2), before);
        assert_eq!(after.3, None);
        let (column_type, not_null, default): (String, i64, Option<String>) = conn
            .query_row(
                "SELECT type, \"notnull\", dflt_value FROM pragma_table_info('events')
                 WHERE name = 'upload_provenance'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!((column_type.as_str(), not_null, default), ("TEXT", 0, None));
    }

    #[tokio::test]
    async fn real_v57_file_upgrade_and_reopen_keep_legacy_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v57-events.db");
        let pending = context("legacy pending masked title");
        let sent = context("legacy sent masked title");
        let legacy =
            SqliteStorage::open_with_max_schema_version(&path, 30, None, Some(57)).unwrap();
        assert_eq!(
            crate::migration::get_version(&legacy.conn.test_lock()).unwrap(),
            57
        );
        legacy.save_event(&pending).await.unwrap();
        legacy.save_event(&sent).await.unwrap();
        legacy
            .mark_as_sent(&[storage_event_id(&sent)])
            .await
            .unwrap();
        let before = event_rows(&legacy);
        drop(legacy);

        let upgraded = SqliteStorage::open(&path, 30, None).unwrap();
        assert_eq!(
            crate::migration::get_version(&upgraded.conn.test_lock()).unwrap(),
            58
        );
        assert_eq!(event_rows(&upgraded), before);
        for row in &before {
            assert_eq!(proof_value(&upgraded, &row.0), Value::Null);
        }
        let rows = upgraded
            .get_pending_events_with_upload_provenance(10)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].storage_id.as_deref(),
            Some(storage_event_id(&pending).as_str())
        );
        assert!(rows[0].provenance.is_none());
        // An ignored later write must never backfill legacy evidence.
        assert!(!upgraded
            .save_event_with_upload_provenance(&pending, Some(&proof(&pending)))
            .await
            .unwrap());
        assert_eq!(
            proof_value(&upgraded, &storage_event_id(&pending)),
            Value::Null
        );
        drop(upgraded);

        let reopened = SqliteStorage::open(&path, 30, None).unwrap();
        assert_eq!(
            crate::migration::get_version(&reopened.conn.test_lock()).unwrap(),
            58
        );
        assert_eq!(event_rows(&reopened), before);
        let rows = reopened
            .get_pending_events_with_upload_provenance(10)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].provenance.is_none());
        let conn = reopened.conn.test_lock();
        let columns: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('events') WHERE name='upload_provenance'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            columns, 1,
            "reopening reapplies no ALTER or legacy backfill"
        );
    }

    #[tokio::test]
    async fn known_context_and_window_reopen_without_public_proof_leakage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known-events.db");
        let events = [
            context("public-mask-positive"),
            Event::Window(WindowLayoutEvent {
                timestamp: timestamp(),
                event_type: WindowLayoutEventType::Focus,
                window: WindowInfo {
                    app_name: "Code".to_string(),
                    window_title: "window-mask-positive".to_string(),
                    position: (0, 0),
                    size: (800, 600),
                    screen_ratio: 0.5,
                    is_fullscreen: false,
                    z_order: 1,
                },
                screen_resolution: (1600, 1200),
                monitor_index: 0,
            }),
        ];
        let storage = SqliteStorage::open(&path, 30, None).unwrap();
        for event in &events {
            assert!(storage
                .save_event_with_upload_provenance(event, Some(&proof(event)))
                .await
                .unwrap());
        }
        let before = event_rows(&storage);
        for event in &events {
            let expected = serde_json::to_string(event).unwrap();
            assert!(
                before
                    .iter()
                    .any(|candidate| candidate.0 == storage_event_id(event)
                        && candidate.3 == expected)
            );
        }
        assert!(before
            .iter()
            .all(|row| !row.3.contains("event_fingerprint")));
        drop(storage);

        let reopened = SqliteStorage::open(&path, 30, None).unwrap();
        let pending = reopened
            .get_pending_events_with_upload_provenance(10)
            .await
            .unwrap();
        assert_eq!(pending.len(), 2);
        for item in &pending {
            assert!(item
                .provenance
                .as_ref()
                .unwrap()
                .is_bound_to(item.storage_id.as_deref().unwrap(), &item.event));
            assert_eq!(item.provenance.as_ref(), Some(&proof(&item.event)));
        }
        assert_eq!(event_rows(&reopened), before);
        for marker in [
            "upload_provenance",
            "event_fingerprint",
            "title_policy_fingerprint",
        ] {
            let pattern = format!("%{marker}%");
            assert_eq!(reopened.count_search_events(&pattern).unwrap(), 0);
            assert!(reopened.search_events(&pattern, 10, 0).unwrap().is_empty());
        }
        assert_eq!(
            reopened
                .count_search_events("%public-mask-positive%")
                .unwrap(),
            1
        );
        let public = reopened.search_events("%positive%", 10, 0).unwrap();
        assert_eq!(public.len(), 2);
        assert!(public.iter().all(|row| row
            .data
            .as_deref()
            .is_some_and(|data| !data.contains("event_fingerprint"))));
        let legacy = reopened.get_pending_events(10).await.unwrap();
        assert_eq!(legacy.len(), 2);
        assert!(!serde_json::to_string(&legacy)
            .unwrap()
            .contains("title_policy_fingerprint"));
        let exports = reopened
            .list_event_exports_async(
                &(timestamp() - Duration::hours(1)).to_rfc3339(),
                &(timestamp() + Duration::hours(1)).to_rfc3339(),
            )
            .await
            .unwrap();
        assert_eq!(exports.len(), 2);
        assert!(exports
            .iter()
            .all(|row| row.event_id == storage_event_id(&events[0])
                || row.event_id == storage_event_id(&events[1])));
    }

    #[tokio::test]
    async fn duplicate_context_id_keeps_known_or_legacy_winner() {
        for known in [false, true] {
            let storage = SqliteStorage::open_in_memory(30).unwrap();
            let winner = context(&format!("{}-winner", "x".repeat(20)));
            let loser = context(&format!("{}-loser", "x".repeat(20)));
            assert_eq!(storage_event_id(&winner), storage_event_id(&loser));
            if known {
                assert!(storage
                    .save_event_with_upload_provenance(&winner, Some(&proof(&winner)))
                    .await
                    .unwrap());
            } else {
                storage.save_event(&winner).await.unwrap();
            }
            let before = event_rows(&storage);
            let old_proof = proof_value(&storage, &storage_event_id(&winner));
            assert!(!storage
                .save_event_with_upload_provenance(&loser, Some(&proof(&loser)))
                .await
                .unwrap());
            assert_eq!(event_rows(&storage), before);
            assert_eq!(proof_value(&storage, &storage_event_id(&winner)), old_proof);
            let rows = storage
                .get_pending_events_with_upload_provenance(10)
                .await
                .unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(
                serde_json::to_value(&rows[0].event).unwrap(),
                serde_json::to_value(&winner).unwrap()
            );
            assert_eq!(rows[0].provenance, known.then(|| proof(&winner)));
        }
    }

    #[tokio::test]
    async fn unbound_new_proof_and_failed_insert_write_no_partial_row() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let event = context("masked target");
        let other = context("different masked event");
        let invalid = proof(&other);
        assert!(matches!(
            storage
                .save_event_with_upload_provenance(&event, Some(&invalid))
                .await,
            Err(maekon_core::error::CoreError::Validation { .. })
        ));
        let mut unsupported = proof(&event);
        unsupported.version = 99;
        let error = storage
            .save_event_with_upload_provenance(&event, Some(&unsupported))
            .await
            .unwrap_err();
        match error {
            maekon_core::error::CoreError::Validation {
                code,
                field,
                message,
            } => {
                assert_eq!(code, maekon_core::error_codes::ValidationCode::InvalidField);
                assert_eq!(field, "upload_provenance");
                assert_eq!(
                    message,
                    "Upload provenance is not bound to the stored event"
                );
            }
            other => panic!("unsupported proof must fail validation, got {other:?}"),
        }
        assert!(event_rows(&storage).is_empty());
        storage
            .conn
            .test_lock()
            .execute_batch(
                "CREATE TRIGGER fail_upload_insert BEFORE INSERT ON events
             BEGIN SELECT RAISE(ABORT, 'synthetic insertion failure'); END;",
            )
            .unwrap();
        let error = storage
            .save_event_with_upload_provenance(&event, Some(&proof(&event)))
            .await
            .unwrap_err();
        match error {
            maekon_core::error::CoreError::Storage { code, message } => {
                assert_eq!(code, maekon_core::error_codes::StorageCode::Failed);
                assert_eq!(message, "event save failure: synthetic insertion failure");
            }
            other => panic!("failed insert must report a storage error, got {other:?}"),
        }
        assert!(
            event_rows(&storage).is_empty(),
            "event and proof share one failing INSERT"
        );
    }

    #[tokio::test]
    async fn invalid_private_rows_remain_unknown_without_losing_event_or_healing() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let event = context("masked row");
        storage.save_event(&event).await.unwrap();
        let id = storage_event_id(&event);
        let valid = proof(&event);
        let mut unsupported = valid.clone();
        unsupported.version = 99;
        let mut wrong_hash = valid.clone();
        wrong_hash.event_fingerprint[0] ^= 1;
        let other = proof(&context("wrong event"));
        for bad in [
            Value::Null,
            Value::Text("{".to_string()),
            Value::Text("123".to_string()),
            Value::Blob(vec![0xff, 0x00]),
            Value::Text(serde_json::to_string(&unsupported).unwrap()),
            Value::Text(serde_json::to_string(&wrong_hash).unwrap()),
            Value::Text(serde_json::to_string(&other).unwrap()),
            Value::Text(format!(
                "{},\"unexpected\":true}}",
                serde_json::to_string(&valid).unwrap().trim_end_matches('}')
            )),
        ] {
            storage
                .conn
                .test_lock()
                .execute(
                    "UPDATE events SET upload_provenance = ?1 WHERE event_id = ?2",
                    rusqlite::params![bad, id],
                )
                .unwrap();
            let before = proof_value(&storage, &id);
            for _ in 0..2 {
                let rows = storage
                    .get_pending_events_with_upload_provenance(10)
                    .await
                    .unwrap();
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].storage_id.as_deref(), Some(id.as_str()));
                assert!(rows[0].provenance.is_none());
            }
            assert_eq!(
                proof_value(&storage, &id),
                before,
                "reads never mint or backfill proof"
            );
        }
    }

    #[tokio::test]
    async fn erasure_skip_returns_false_and_parent_erasure_removes_proof() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let event = context("stored masked event");
        assert!(storage
            .save_event_with_upload_provenance(&event, Some(&proof(&event)))
            .await
            .unwrap());
        let flag = Arc::new(AtomicBool::new(true));
        storage.set_deletion_flag(flag.clone());
        let blocked = context("must not be inserted");
        assert!(!storage
            .save_event_with_upload_provenance(&blocked, Some(&proof(&blocked)))
            .await
            .unwrap());
        assert_eq!(event_rows(&storage).len(), 1);
        assert!(storage
            .get_pending_events_with_upload_provenance(10)
            .await
            .unwrap()
            .is_empty());
        flag.store(false, Ordering::Release);
        storage.set_erasing(Arc::new(AtomicBool::new(true)));
        assert!(!storage
            .save_event_with_upload_provenance(&blocked, Some(&proof(&blocked)))
            .await
            .unwrap());
        assert!(storage
            .get_pending_events_with_upload_provenance(10)
            .await
            .unwrap()
            .is_empty());
        storage.delete_all_data().unwrap();
        assert!(event_rows(&storage).is_empty());
    }

    #[tokio::test]
    async fn range_deletion_removes_private_proof_with_parent_row() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let event = context("range deletion masked event");
        assert!(storage
            .save_event_with_upload_provenance(&event, Some(&proof(&event)))
            .await
            .unwrap());
        let window = TimeWindow::from_rfc3339_pair(
            &(timestamp() - Duration::hours(1)).to_rfc3339(),
            &(timestamp() + Duration::hours(1)).to_rfc3339(),
        )
        .unwrap();
        let removed = storage
            .delete_data_in_range(&window, true, false, false, false, false)
            .unwrap();
        assert_eq!(removed.events_deleted, 1);
        assert!(event_rows(&storage).is_empty());
    }

    #[tokio::test]
    async fn nested_object_key_order_survives_reopen_but_value_or_array_tamper_does_not() {
        let dir = tempfile::tempdir().unwrap();
        for change_array in [false, true] {
            let path = dir.path().join(format!("nested-{change_array}.db"));
            let uuid = Uuid::new_v4();
            let event = Event::System(SystemEvent {
                event_id: uuid,
                event_type: SystemEventType::MetricsUpdate,
                timestamp: timestamp(),
                data: serde_json::json!({"alpha": [{"a":1,"z":3},4], "omega":{"left":1,"right":2}}),
            });
            let storage = SqliteStorage::open(&path, 30, None).unwrap();
            let original = proof(&event);
            assert!(storage
                .save_event_with_upload_provenance(&event, Some(&original))
                .await
                .unwrap());
            let id = storage_event_id(&event);
            let reordered = format!(
                "{{\"data\":{{\"omega\":{{\"right\":2,\"left\":1}},\"alpha\":[{{\"z\":3,\"a\":1}},4]}},\"timestamp\":{},\"event_type\":\"METRICS_UPDATE\",\"event_id\":{},\"type\":\"System\"}}",
                serde_json::to_string(&timestamp()).unwrap(), serde_json::to_string(&uuid).unwrap(),
            );
            assert_ne!(event_rows(&storage)[0].3, reordered);
            let equivalent: Event = serde_json::from_str(&reordered).unwrap();
            assert!(original.is_bound_to(&id, &equivalent));
            storage
                .conn
                .test_lock()
                .execute(
                    "UPDATE events SET data = ?1 WHERE event_id = ?2",
                    rusqlite::params![reordered, id],
                )
                .unwrap();
            drop(storage);
            let reopened = SqliteStorage::open(&path, 30, None).unwrap();
            let rows = reopened
                .get_pending_events_with_upload_provenance(10)
                .await
                .unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].provenance.as_ref(), Some(&original));
            let mut changed: serde_json::Value = serde_json::from_str(&reordered).unwrap();
            if change_array {
                changed["data"]["alpha"].as_array_mut().unwrap().reverse();
            } else {
                changed["data"]["omega"]["right"] = serde_json::json!(99);
            }
            let old_proof = proof_value(&reopened, &id);
            reopened
                .conn
                .test_lock()
                .execute(
                    "UPDATE events SET data = ?1 WHERE event_id = ?2",
                    rusqlite::params![serde_json::to_string(&changed).unwrap(), id],
                )
                .unwrap();
            let changed_rows = reopened
                .get_pending_events_with_upload_provenance(10)
                .await
                .unwrap();
            assert_eq!(changed_rows.len(), 1);
            assert!(changed_rows[0].provenance.is_none());
            assert_eq!(proof_value(&reopened, &id), old_proof);
        }
    }
}
