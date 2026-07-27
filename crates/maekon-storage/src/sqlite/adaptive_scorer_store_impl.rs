//! `AdaptiveScorerStore` over SQLite (#8058 P2-1).
//!
//! Persists the coaching `AdaptiveScorer`'s online-logistic-regression weights to
//! the singleton `adaptive_scorer_state` table (V46). The weight vector is stored
//! JSON-encoded so a future feature-count change is a data concern, not a schema
//! migration. This was the LAST feedback-learning component that reset on restart
//! — its siblings (`coaching_effectiveness`, `feedback_scorer_tallies`,
//! `regime_reaction_stats`) already persist.

use maekon_core::error::CoreError;
use maekon_core::error_codes::StorageCode;
use maekon_core::models::coaching::AdaptiveScorerState;
use maekon_core::ports::adaptive_scorer_store::AdaptiveScorerStore;

use super::SqliteStorage;

fn storage_err(message: impl Into<String>) -> CoreError {
    CoreError::Storage {
        code: StorageCode::Failed,
        message: message.into(),
    }
}

impl AdaptiveScorerStore for SqliteStorage {
    fn save_adaptive_scorer_state(&self, state: &AdaptiveScorerState) -> Result<(), CoreError> {
        let weights_json = serde_json::to_string(&state.weights)
            .map_err(|e| storage_err(format!("serialize adaptive scorer weights: {e}")))?;
        // Write — write_lock (skipped when deletion_flag is set;
        // adaptive_scorer_state ∈ ALL_TABLES, so an erasure-in-progress correctly
        // drops this advisory learning row). Idempotent upsert of the `id = 0`
        // singleton so a full-snapshot write-through converges (last write wins).
        self.conn.write_lock().run((), |conn| {
            conn.execute(
                "INSERT INTO adaptive_scorer_state
                    (id, weights_json, bias, train_count, updated_at)
                 VALUES (0, ?1, ?2, ?3, datetime('now'))
                 ON CONFLICT(id) DO UPDATE SET
                    weights_json = ?1,
                    bias = ?2,
                    train_count = ?3,
                    updated_at = datetime('now')",
                rusqlite::params![weights_json, state.bias, state.train_count],
            )
            .map_err(|e| storage_err(format!("save_adaptive_scorer_state: {e}")))?;
            Ok(())
        })
    }

    fn load_adaptive_scorer_state(&self) -> Result<Option<AdaptiveScorerState>, CoreError> {
        // Read — read_lock (independent of deletion_flag).
        let read = self.conn.read_lock();
        let conn = read.conn();
        let row = conn.query_row(
            "SELECT weights_json, bias, train_count
             FROM adaptive_scorer_state
             WHERE id = 0",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, f32>(1)?,
                    row.get::<_, u32>(2)?,
                ))
            },
        );
        let (weights_json, bias, train_count) = match row {
            Ok(v) => v,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(storage_err(format!("load_adaptive_scorer_state: {e}"))),
        };
        // A corrupt weights blob is treated as "no persisted state" (fail-safe):
        // the caller then starts from the neutral default model rather than
        // failing startup on advisory learning state. Warn with the offending
        // value so the corruption is observable.
        let weights: Vec<f32> = match serde_json::from_str(&weights_json) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "adaptive scorer weights unparsable; starting from default"
                );
                return Ok(None);
            }
        };
        Ok(Some(AdaptiveScorerState {
            weights,
            bias,
            train_count,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(train_count: u32) -> AdaptiveScorerState {
        AdaptiveScorerState {
            weights: vec![0.1, -0.2, 0.3, 0.0, 0.5, -0.1, 0.4, 0.2],
            bias: 0.05,
            train_count,
        }
    }

    #[test]
    fn none_on_first_load() {
        let storage = SqliteStorage::open_in_memory(46).unwrap();
        assert!(storage.load_adaptive_scorer_state().unwrap().is_none());
    }

    #[test]
    fn save_then_load_roundtrip() {
        let storage = SqliteStorage::open_in_memory(46).unwrap();
        storage.save_adaptive_scorer_state(&state(60)).unwrap();

        let loaded = storage.load_adaptive_scorer_state().unwrap().unwrap();
        assert_eq!(loaded.weights.len(), 8);
        assert_eq!(loaded.train_count, 60);
        assert!((loaded.bias - 0.05).abs() < f32::EPSILON);
        assert!((loaded.weights[2] - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn save_is_idempotent_singleton() {
        let storage = SqliteStorage::open_in_memory(46).unwrap();
        storage.save_adaptive_scorer_state(&state(10)).unwrap();
        storage.save_adaptive_scorer_state(&state(99)).unwrap();

        // Only ONE row (the singleton) — the second write replaces the first.
        let loaded = storage.load_adaptive_scorer_state().unwrap().unwrap();
        assert_eq!(
            loaded.train_count, 99,
            "singleton must upsert, not duplicate"
        );
    }

    /// Simulates the full restart round-trip: write via one storage handle, then
    /// open a SEPARATE handle on the same file and read it back.
    #[test]
    fn survives_restart_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("scorer_state.db");
        {
            let storage = SqliteStorage::open(&db_path, 46, None).unwrap();
            storage.save_adaptive_scorer_state(&state(75)).unwrap();
        }
        {
            let storage = SqliteStorage::open(&db_path, 46, None).unwrap();
            let loaded = storage.load_adaptive_scorer_state().unwrap().unwrap();
            assert_eq!(loaded.train_count, 75);
            assert_eq!(loaded.weights.len(), 8);
        }
    }

    #[test]
    fn corrupt_weights_blob_starts_fresh() {
        let storage = SqliteStorage::open_in_memory(46).unwrap();
        // Write a row with an unparsable weights blob directly.
        storage
            .connection_arc()
            .write_lock()
            .run((), |conn| {
                conn.execute(
                    "INSERT INTO adaptive_scorer_state (id, weights_json, bias, train_count)
                     VALUES (0, 'not-json', 0.0, 50)",
                    [],
                )
                .map_err(|e| storage_err(format!("seed corrupt row: {e}")))?;
                Ok::<(), CoreError>(())
            })
            .unwrap();

        // Fail-safe: the corrupt row reads back as None (start from default),
        // never an error that would block startup.
        assert!(storage.load_adaptive_scorer_state().unwrap().is_none());
    }
}
