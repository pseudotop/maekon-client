//! `RegimeReactionStore` over SQLite (#7913 T2.1c).
//!
//! Persists `RegimeClassifier`'s per-regime (and aggregate) user-reaction counts
//! to the `regime_reaction_stats` table (V44). The global aggregate is stored
//! under the reserved `regime_id = ''` sentinel (`RegimeReactionRecord::AGGREGATE_KEY`).

use maekon_core::error::CoreError;
use maekon_core::error_codes::StorageCode;
use maekon_core::models::tiered_memory::RegimeReactionRecord;
use maekon_core::ports::regime_reaction_store::RegimeReactionStore;

use super::SqliteStorage;

fn storage_err(message: impl Into<String>) -> CoreError {
    CoreError::Storage {
        code: StorageCode::Failed,
        message: message.into(),
    }
}

impl RegimeReactionStore for SqliteStorage {
    fn upsert_regime_reaction(&self, record: &RegimeReactionRecord) -> Result<(), CoreError> {
        // Write — write_lock (skipped when deletion_flag is set;
        // regime_reaction_stats ∈ ALL_TABLES). Idempotent upsert keyed by the
        // table's PRIMARY KEY(regime_id).
        self.conn.write_lock().run((), |conn| {
            conn.execute(
                "INSERT INTO regime_reaction_stats
                    (regime_id, total, accepted, rejected, deferred)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(regime_id) DO UPDATE SET
                    total = ?2,
                    accepted = ?3,
                    rejected = ?4,
                    deferred = ?5",
                rusqlite::params![
                    record.regime_id,
                    record.total,
                    record.accepted,
                    record.rejected,
                    record.deferred,
                ],
            )
            .map_err(|e| storage_err(format!("upsert_regime_reaction: {e}")))?;
            Ok(())
        })
    }

    fn load_regime_reactions(&self) -> Result<Vec<RegimeReactionRecord>, CoreError> {
        // Read — read_lock (independent of deletion_flag).
        let read = self.conn.read_lock();
        let conn = read.conn();
        let mut stmt = conn
            .prepare(
                "SELECT regime_id, total, accepted, rejected, deferred
                 FROM regime_reaction_stats",
            )
            .map_err(|e| storage_err(format!("prepare load_regime_reactions: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(RegimeReactionRecord {
                    regime_id: row.get(0)?,
                    total: row.get(1)?,
                    accepted: row.get(2)?,
                    rejected: row.get(3)?,
                    deferred: row.get(4)?,
                })
            })
            .map_err(|e| storage_err(format!("load_regime_reactions: {e}")))?;
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

    fn rec(id: &str, total: u64, acc: u64, rej: u64) -> RegimeReactionRecord {
        RegimeReactionRecord {
            regime_id: id.to_string(),
            total,
            accepted: acc,
            rejected: rej,
            deferred: 0,
        }
    }

    #[test]
    fn empty_on_first_load() {
        let storage = SqliteStorage::open_in_memory(44).unwrap();
        assert!(storage.load_regime_reactions().unwrap().is_empty());
    }

    #[test]
    fn upsert_then_load_roundtrip_including_aggregate() {
        let storage = SqliteStorage::open_in_memory(44).unwrap();
        storage
            .upsert_regime_reaction(&rec("regime-3", 5, 4, 1))
            .unwrap();
        // Aggregate under the "" sentinel.
        storage
            .upsert_regime_reaction(&rec(RegimeReactionRecord::AGGREGATE_KEY, 8, 6, 2))
            .unwrap();

        let mut loaded = storage.load_regime_reactions().unwrap();
        loaded.sort_by(|a, b| a.regime_id.cmp(&b.regime_id));
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].regime_id, ""); // aggregate sorts first
        assert_eq!(loaded[0].total, 8);
        assert_eq!(loaded[1].regime_id, "regime-3");
        assert_eq!(loaded[1].accepted, 4);
    }

    #[test]
    fn upsert_is_idempotent_per_regime() {
        let storage = SqliteStorage::open_in_memory(44).unwrap();
        storage
            .upsert_regime_reaction(&rec("regime-x", 1, 1, 0))
            .unwrap();
        storage
            .upsert_regime_reaction(&rec("regime-x", 9, 5, 4))
            .unwrap();
        let loaded = storage.load_regime_reactions().unwrap();
        assert_eq!(loaded.len(), 1, "same regime_id must upsert, not duplicate");
        assert_eq!(loaded[0].total, 9);
    }

    #[test]
    fn survives_restart_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("regime.db");
        {
            let storage = SqliteStorage::open(&db_path, 44, None).unwrap();
            storage
                .upsert_regime_reaction(&rec("regime-7", 10, 7, 3))
                .unwrap();
        }
        {
            let storage = SqliteStorage::open(&db_path, 44, None).unwrap();
            let loaded = storage.load_regime_reactions().unwrap();
            assert_eq!(loaded.len(), 1);
            assert_eq!(loaded[0].regime_id, "regime-7");
            assert_eq!(loaded[0].accepted, 7);
        }
    }
}
