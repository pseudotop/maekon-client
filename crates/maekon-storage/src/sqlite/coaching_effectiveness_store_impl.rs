//! `CoachingEffectivenessStore` over SQLite (#7913 T2.1b).
//!
//! Writes the DML for the pre-existing `coaching_effectiveness` table (V17),
//! which had a schema but ZERO reads/writes until now — the restart-surviving
//! backing for `FeedbackTracker`'s per-`(profile, trigger)` effectiveness scores.

use maekon_core::error::CoreError;
use maekon_core::error_codes::StorageCode;
use maekon_core::models::coaching::CoachingEffectivenessRecord;
use maekon_core::ports::coaching_effectiveness_store::CoachingEffectivenessStore;

use super::SqliteStorage;

fn storage_err(message: impl Into<String>) -> CoreError {
    CoreError::Storage {
        code: StorageCode::Failed,
        message: message.into(),
    }
}

impl CoachingEffectivenessStore for SqliteStorage {
    fn upsert_coaching_effectiveness(
        &self,
        records: &[CoachingEffectivenessRecord],
    ) -> Result<(), CoreError> {
        if records.is_empty() {
            return Ok(());
        }
        // Write — write_lock (skipped when deletion_flag is set;
        // coaching_effectiveness ∈ ALL_TABLES, so an erasure-in-progress
        // correctly drops these advisory learning rows). Idempotent upsert keyed
        // by the table's UNIQUE(profile_name, trigger_type) so a full-snapshot
        // write-through converges (last write wins per key).
        self.conn.write_lock().run((), |conn| {
            for rec in records {
                conn.execute(
                    "INSERT INTO coaching_effectiveness
                        (profile_name, trigger_type, total_shown, positive_feedback,
                         negative_feedback, neutral_count, behavior_change_count, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))
                     ON CONFLICT(profile_name, trigger_type) DO UPDATE SET
                        total_shown = ?3,
                        positive_feedback = ?4,
                        negative_feedback = ?5,
                        neutral_count = ?6,
                        behavior_change_count = ?7,
                        updated_at = datetime('now')",
                    rusqlite::params![
                        rec.profile_name,
                        rec.trigger_type,
                        rec.total_shown,
                        rec.positive_feedback,
                        rec.negative_feedback,
                        rec.neutral_count,
                        rec.behavior_change_count,
                    ],
                )
                .map_err(|e| storage_err(format!("upsert_coaching_effectiveness: {e}")))?;
            }
            Ok(())
        })
    }

    fn load_coaching_effectiveness(&self) -> Result<Vec<CoachingEffectivenessRecord>, CoreError> {
        // Read — read_lock (independent of deletion_flag).
        let read = self.conn.read_lock();
        let conn = read.conn();
        let mut stmt = conn
            .prepare(
                "SELECT profile_name, trigger_type, total_shown, positive_feedback,
                        negative_feedback, neutral_count, behavior_change_count
                 FROM coaching_effectiveness",
            )
            .map_err(|e| storage_err(format!("prepare load_coaching_effectiveness: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(CoachingEffectivenessRecord {
                    profile_name: row.get(0)?,
                    trigger_type: row.get(1)?,
                    total_shown: row.get(2)?,
                    positive_feedback: row.get(3)?,
                    negative_feedback: row.get(4)?,
                    neutral_count: row.get(5)?,
                    behavior_change_count: row.get(6)?,
                })
            })
            .map_err(|e| storage_err(format!("load_coaching_effectiveness: {e}")))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| storage_err(format!("row read: {e}")))?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(
        profile: &str,
        trigger: &str,
        shown: u32,
        pos: f32,
        neg: f32,
    ) -> CoachingEffectivenessRecord {
        CoachingEffectivenessRecord {
            profile_name: profile.to_string(),
            trigger_type: trigger.to_string(),
            total_shown: shown,
            positive_feedback: pos,
            negative_feedback: neg,
            neutral_count: 0,
            behavior_change_count: 0,
        }
    }

    #[test]
    fn empty_on_first_load() {
        let storage = SqliteStorage::open_in_memory(43).unwrap();
        assert!(storage.load_coaching_effectiveness().unwrap().is_empty());
    }

    #[test]
    fn upsert_then_load_roundtrip() {
        let storage = SqliteStorage::open_in_memory(43).unwrap();
        let rows = vec![
            rec("FocusGuard", "RegimeTransition", 5, 3.0, 1.0),
            rec("TimeAware", "RegimeOverstay", 8, 2.0, 5.0),
        ];
        storage.upsert_coaching_effectiveness(&rows).unwrap();

        let mut loaded = storage.load_coaching_effectiveness().unwrap();
        loaded.sort_by(|a, b| a.profile_name.cmp(&b.profile_name));
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].profile_name, "FocusGuard");
        assert_eq!(loaded[0].total_shown, 5);
        assert!((loaded[0].positive_feedback - 3.0).abs() < f32::EPSILON);
        assert_eq!(loaded[1].profile_name, "TimeAware");
        assert!((loaded[1].negative_feedback - 5.0).abs() < f32::EPSILON);
    }

    #[test]
    fn upsert_is_idempotent_per_key() {
        let storage = SqliteStorage::open_in_memory(43).unwrap();
        storage
            .upsert_coaching_effectiveness(&[rec("FocusGuard", "RegimeTransition", 5, 3.0, 1.0)])
            .unwrap();
        // Second write to the same key REPLACES (does not duplicate) the row.
        storage
            .upsert_coaching_effectiveness(&[rec("FocusGuard", "RegimeTransition", 9, 7.0, 2.0)])
            .unwrap();

        let loaded = storage.load_coaching_effectiveness().unwrap();
        assert_eq!(
            loaded.len(),
            1,
            "same (profile, trigger) must upsert, not duplicate"
        );
        assert_eq!(loaded[0].total_shown, 9);
        assert!((loaded[0].positive_feedback - 7.0).abs() < f32::EPSILON);
    }

    /// Simulates the full restart round-trip: write via one storage handle, then
    /// open a SEPARATE handle on the same file and read it back.
    #[test]
    fn survives_restart_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("coaching.db");

        {
            let storage = SqliteStorage::open(&db_path, 43, None).unwrap();
            storage
                .upsert_coaching_effectiveness(&[rec(
                    "FocusGuard",
                    "RegimeTransition",
                    4,
                    4.0,
                    0.0,
                )])
                .unwrap();
        }
        {
            let storage = SqliteStorage::open(&db_path, 43, None).unwrap();
            let loaded = storage.load_coaching_effectiveness().unwrap();
            assert_eq!(loaded.len(), 1);
            assert_eq!(loaded[0].profile_name, "FocusGuard");
            assert_eq!(loaded[0].total_shown, 4);
        }
    }
}
