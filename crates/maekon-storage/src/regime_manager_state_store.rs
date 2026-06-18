//! SqliteRegimeManagerStateStore — RegimeStoragePort over SQLite.

use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::models::tiered_memory::Regime;
use maekon_core::ports::regime_storage::RegimeStoragePort;
use rusqlite::OptionalExtension;
use std::sync::Arc;

use crate::sqlite::GuardedConnection;

/// Accesses the connection only through the shared [`GuardedConnection`] (returned by
/// `SqliteStorage::connection_arc()`) — a barrier-free handle cannot be obtained (#4928).
pub struct SqliteRegimeManagerStateStore {
    conn: Arc<GuardedConnection>,
}

impl SqliteRegimeManagerStateStore {
    pub fn new(conn: Arc<GuardedConnection>) -> Self {
        Self { conn }
    }
}

#[async_trait]
impl RegimeStoragePort for SqliteRegimeManagerStateStore {
    async fn load_all(&self) -> Result<Vec<Regime>, CoreError> {
        // Read — read_lock (independent of deletion_flag). Only the SELECT runs under
        // read_lock; on a parse failure the quarantine UPDATE is an ALL_TABLES mutation,
        // so it is issued through the write_lock funnel after releasing read_lock (#4928
        // round-3 FIX A — no smuggling mutations through the read path). parking_lot
        // mutexes are not reentrant, so the read guard must be dropped before acquiring
        // write_lock.
        let payload: Option<String> = {
            let read = self.conn.read_lock();
            read.conn()
                .query_row(
                    "SELECT payload FROM regime_manager_state WHERE id = 0",
                    [],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| CoreError::Storage {
                    code: maekon_core::error_codes::StorageCode::Failed,
                    message: e.to_string(),
                })?
        };

        match payload {
            Some(json) => match serde_json::from_str::<Vec<Regime>>(&json) {
                Ok(regimes) => Ok(regimes),
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        "regime_manager_state payload failed to parse; quarantining to payload_backup and starting fresh. Recover via manual inspection of the backup column."
                    );
                    // Write — write_lock (skipped when deletion_flag is set).
                    // Quarantining a row that is about to be wiped is pointless, so
                    // skip-on-flag is the correct behavior.
                    let quarantine: Result<(), rusqlite::Error> =
                        self.conn.write_lock().run((), |conn| {
                            conn.execute(
                                "UPDATE regime_manager_state
                            SET payload_backup = payload,
                                payload_backup_at = datetime('now'),
                                payload = '[]',
                                updated_at = datetime('now')
                          WHERE id = 0",
                                [],
                            )?;
                            Ok(())
                        });
                    if let Err(qe) = quarantine {
                        // Second log line — DO NOT swallow. If quarantine
                        // itself fails (disk full, WAL corruption), the only
                        // user-visible signal that their curated state is
                        // unrecoverable is this line. ADR-018 explicitly
                        // rejects silent data loss.
                        tracing::error!(
                            error = %qe,
                            "regime_manager_state quarantine UPDATE failed — corrupt payload may be lost"
                        );
                    }
                    Ok(Vec::new())
                }
            },
            None => Ok(Vec::new()),
        }
    }

    async fn save_all(&self, regimes: &[Regime]) -> Result<(), CoreError> {
        let json = serde_json::to_string(regimes).map_err(|e| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: e.to_string(),
        })?;
        // Derive a dominant_category string from RegimeFeatures one-hot fields.
        // Intentionally inlined here (not imported from maekon-analysis) so that
        // the storage crate does not gain an analysis dependency — violating the
        // hexagonal adapter dependency rule.  The logic mirrors generate_auto_label
        // in maekon-analysis::regime_detector; if that function ever changes,
        // update here too.
        fn dominant_category(r: &Regime) -> &'static str {
            let c = &r.centroid;
            if c.category_coding > 0.5 {
                "coding"
            } else if c.category_communication > 0.5 {
                "communication"
            } else if c.category_browser > 0.5 {
                "browser"
            } else {
                "mixed"
            }
        }

        // Clone everything needed inside the write_lock closure (not Send/Sync
        // friendly otherwise for the parking_lot guard).
        let regimes_snapshot: Vec<_> = regimes
            .iter()
            .map(|r| {
                let id = r.regime_id.clone();
                let label = r.name.clone().unwrap_or_else(|| r.auto_label.clone());
                let detected_at = r.first_seen.to_rfc3339();
                let last_seen_at = r.last_seen.to_rfc3339();
                let occurrence_count = r.sample_count as i64;
                let avg_density = r.centroid.avg_event_rate as f64;
                let avg_importance = r.centroid.avg_importance as f64;
                let dom_cat = dominant_category(r).to_string();
                let is_active = match r.status {
                    maekon_core::models::tiered_memory::RegimeStatus::Active => 1i64,
                    _ => 0i64,
                };
                (
                    id,
                    label,
                    detected_at,
                    last_seen_at,
                    occurrence_count,
                    avg_density,
                    avg_importance,
                    dom_cat,
                    is_active,
                )
            })
            .collect();

        // Single write_lock span: persist the blob state AND upsert the regimes
        // relational table so that activity_segments.regime_id dangling refs are
        // resolved on standalone installs (#5665).
        //
        // Upsert semantics:
        //   INSERT OR IGNORE — new regime_id gets a row with hlc=(0,0,'') so any
        //   peer with a real HLC always wins LWW in sync_merger.  detected_at is
        //   set once and never changed by this path (first_seen is monotonically
        //   increasing from the moment of discovery).
        //
        //   UPDATE — label, occurrence_count, avg_density, avg_importance,
        //   dominant_category, is_active reflect local latest.  last_seen_at
        //   advances only if the stored value is older (max-preserve guard mirrors
        //   the sync_merger comment: "never go backward in time").
        //
        //   hlc_wall_ms / hlc_counter / origin_device_id are left at their current
        //   values if the row already existed (the UPDATE omits them) so that a
        //   real peer-sync HLC stamp is not silently clobbered by a local save.
        self.conn.write_lock().run((), |conn| {
            // Blob snapshot — identical to the pre-existing implementation.
            conn.execute(
                "INSERT OR REPLACE INTO regime_manager_state
                (id, payload, payload_backup, payload_backup_at, updated_at)
             VALUES (
                0, ?1,
                (SELECT payload_backup FROM regime_manager_state WHERE id = 0),
                (SELECT payload_backup_at FROM regime_manager_state WHERE id = 0),
                datetime('now')
             )",
                rusqlite::params![json],
            )
            .map_err(|e| CoreError::Storage {
                code: maekon_core::error_codes::StorageCode::Failed,
                message: e.to_string(),
            })?;

            // Relational upsert: fill the regimes table so regime_id FK refs
            // are resolvable on standalone installs.
            for (id, label, detected_at, last_seen_at, occ, avg_d, avg_i, dom, active) in
                &regimes_snapshot
            {
                // INSERT OR IGNORE establishes the row with a zero HLC (local-only
                // origin) if it does not exist yet.  PRAGMA foreign_keys is OFF on
                // this connection so the insert never blocks on params_snapshot_id.
                conn.execute(
                    "INSERT OR IGNORE INTO regimes \
                     (id, label, detected_at, last_seen_at, occurrence_count, \
                      avg_density, avg_importance, dominant_category, is_active, \
                      hlc_wall_ms, hlc_counter, origin_device_id) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, 0, '')",
                    rusqlite::params![
                        id,
                        label,
                        detected_at,
                        last_seen_at,
                        occ,
                        avg_d,
                        avg_i,
                        dom,
                        active
                    ],
                )
                .map_err(|e| CoreError::Storage {
                    code: maekon_core::error_codes::StorageCode::Failed,
                    message: format!("regimes upsert (insert): {e}"),
                })?;

                // UPDATE: refresh mutable columns; advance last_seen_at only if
                // newer (max-preserve — never go backward).  hlc columns are
                // untouched so an existing peer-sync stamp is preserved.
                conn.execute(
                    "UPDATE regimes SET \
                       label = ?2, \
                       last_seen_at = CASE WHEN last_seen_at < ?3 THEN ?3 ELSE last_seen_at END, \
                       occurrence_count = ?4, \
                       avg_density = ?5, \
                       avg_importance = ?6, \
                       dominant_category = ?7, \
                       is_active = ?8 \
                     WHERE id = ?1",
                    rusqlite::params![id, label, last_seen_at, occ, avg_d, avg_i, dom, active],
                )
                .map_err(|e| CoreError::Storage {
                    code: maekon_core::error_codes::StorageCode::Failed,
                    message: format!("regimes upsert (update): {e}"),
                })?;
            }

            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use maekon_core::models::tiered_memory::{Regime, RegimeFeatures, RegimeStatus, TriggerParams};
    use rusqlite::Connection;
    use tempfile::TempDir;

    fn open_db() -> (TempDir, Arc<GuardedConnection>) {
        let dir = tempfile::tempdir().unwrap();
        let conn = Connection::open(dir.path().join("t.db")).unwrap();
        crate::migration::run_migrations(&conn).unwrap();
        (dir, Arc::new(GuardedConnection::new_unflagged(conn)))
    }

    fn sample_regime(id: &str) -> Regime {
        Regime {
            regime_id: id.into(),
            name: None,
            auto_label: format!("label-{id}"),
            centroid: RegimeFeatures::default(),
            optimal_params: TriggerParams::default(),
            sample_count: 0,
            first_seen: Utc::now(),
            last_seen: Utc::now(),
            status: RegimeStatus::Active,
        }
    }

    /// T-C3c-1 — empty on first load.
    #[tokio::test]
    async fn empty_on_first_load() {
        let (_d, conn) = open_db();
        let store = SqliteRegimeManagerStateStore::new(conn);
        assert_eq!(store.load_all().await.unwrap().len(), 0);
    }

    /// T-C3c-2 — save then load roundtrip.
    #[tokio::test]
    async fn save_then_load_roundtrip() {
        let (_d, conn) = open_db();
        let store = SqliteRegimeManagerStateStore::new(conn);
        let regimes = vec![sample_regime("a"), sample_regime("b"), sample_regime("c")];
        store.save_all(&regimes).await.unwrap();
        let loaded = store.load_all().await.unwrap();
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[0].regime_id, "a");
        assert_eq!(loaded[2].regime_id, "c");
    }

    /// T-C3c-3 — save replaces previous.
    #[tokio::test]
    async fn save_replaces_previous() {
        let (_d, conn) = open_db();
        let store = SqliteRegimeManagerStateStore::new(conn);
        store
            .save_all(&[sample_regime("a"), sample_regime("b"), sample_regime("c")])
            .await
            .unwrap();
        store.save_all(&[sample_regime("just_one")]).await.unwrap();
        let loaded = store.load_all().await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].regime_id, "just_one");
    }

    /// T-C3c-4 — malformed payload quarantines, starts fresh.
    #[tokio::test]
    async fn malformed_payload_quarantines_and_starts_fresh() {
        let (_d, conn) = open_db();
        {
            let c = conn.test_lock();
            c.execute(
                "INSERT OR REPLACE INTO regime_manager_state (id, payload, updated_at) VALUES (0, '{not:valid json', datetime('now'))",
                [],
            )
            .unwrap();
        }
        let store = SqliteRegimeManagerStateStore::new(conn.clone());
        let result = store.load_all().await;
        // Line 194: quarantine must return Ok (no Err propagated to caller).
        // Line 195 (was result.unwrap()): destructure the Ok value immediately.
        let states = result.expect("quarantine must not return Err");
        assert_eq!(states.len(), 0, "fresh start expected");

        let c = conn.test_lock();
        let (backup, backup_at): (Option<String>, Option<String>) = c
            .query_row(
                "SELECT payload_backup, payload_backup_at FROM regime_manager_state WHERE id = 0",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(backup.unwrap(), "{not:valid json");
        assert!(backup_at.is_some(), "backup timestamp must be set");
    }

    /// #4928 round-3 (FIX A): the quarantine UPDATE goes through the write_lock funnel,
    /// so it self-skips when deletion_flag is set — a corrupt row that is about to be
    /// wiped is not quarantined. load_all itself must still return Ok(empty Vec) (the
    /// read / fresh-start path succeeds normally).
    #[tokio::test]
    async fn quarantine_self_skips_when_deletion_flag_set() {
        use std::sync::atomic::Ordering;

        let (_d, conn) = open_db();
        {
            let c = conn.test_lock();
            c.execute(
                "INSERT OR REPLACE INTO regime_manager_state (id, payload, updated_at) VALUES (0, '{not:valid json', datetime('now'))",
                [],
            )
            .unwrap();
        }
        // deletion_flag set → the quarantine write_lock must be skipped.
        conn.deletion_flag().store(true, Ordering::Release);

        let store = SqliteRegimeManagerStateStore::new(conn.clone());
        let result = store.load_all().await;
        // Line 230: even with deletion_flag set, load_all must return Ok — the
        // quarantine UPDATE is skipped but the read itself succeeds.
        // Immediately destructure to validate the empty Vec (line 234 assertion).
        let states = result.expect("load_all must be Ok even when quarantine is skipped");
        assert_eq!(states.len(), 0, "fresh start expected");

        // payload_backup must not be set (the quarantine UPDATE was skipped).
        let c = conn.test_lock();
        let backup: Option<String> = c
            .query_row(
                "SELECT payload_backup FROM regime_manager_state WHERE id = 0",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            backup.is_none(),
            "when deletion_flag is set the quarantine UPDATE is skipped, so payload_backup must be empty"
        );
    }

    /// T-C3c-6 — survives-restart roundtrip via two sequential store
    /// constructions on the same SQLite file. Simulates the full
    /// "save on shutdown → re-open at startup → hydrate" flow.
    #[tokio::test]
    async fn survives_restart_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("roundtrip.db");

        // Session 1: save.
        {
            let conn = Connection::open(&db_path).unwrap();
            crate::migration::run_migrations(&conn).unwrap();
            let s = SqliteRegimeManagerStateStore::new(Arc::new(GuardedConnection::new_unflagged(
                conn,
            )));
            s.save_all(&[sample_regime("a"), sample_regime("b")])
                .await
                .unwrap();
        }

        // Session 2: reload. run_migrations is idempotent (checks
        // schema_version and short-circuits), so this documents intent
        // — and protects against a future PRAGMA-on-open helper that
        // would otherwise silently skip.
        {
            let conn = Connection::open(&db_path).unwrap();
            crate::migration::run_migrations(&conn).unwrap();
            let s = SqliteRegimeManagerStateStore::new(Arc::new(GuardedConnection::new_unflagged(
                conn,
            )));
            let loaded = s.load_all().await.unwrap();
            assert_eq!(loaded.len(), 2);
            assert_eq!(loaded[0].regime_id, "a");
            assert_eq!(loaded[1].regime_id, "b");
        }
    }

    /// T-C3c-7 — slow save under tokio::time::timeout unblocks within
    /// the watchdog budget without panic. Documents the contract the
    /// shutdown guard in main.rs depends on.
    #[tokio::test]
    async fn save_slower_than_deadline_times_out_gracefully() {
        use std::time::Duration;

        struct SlowStore;
        #[async_trait]
        impl RegimeStoragePort for SlowStore {
            async fn load_all(&self) -> Result<Vec<Regime>, CoreError> {
                Ok(vec![])
            }
            async fn save_all(&self, _: &[Regime]) -> Result<(), CoreError> {
                tokio::time::sleep(Duration::from_secs(10)).await;
                Ok(())
            }
        }

        let start = std::time::Instant::now();
        let outcome = tokio::time::timeout(Duration::from_secs(4), SlowStore.save_all(&[])).await;
        let elapsed = start.elapsed();
        let _ = outcome.unwrap_err(); // tokio::time::error::Elapsed — timeout fired, no payload beyond unit type
        assert!(
            elapsed < Duration::from_secs(5),
            "must unblock within budget + margin"
        );
    }

    // -----------------------------------------------------------------------
    // AC tests for #5665 — regimes table local producer
    // -----------------------------------------------------------------------

    /// AC-1: save_all → regimes row exists with correct fields.
    #[tokio::test]
    async fn save_all_upserts_regimes_table_row() {
        let (_d, conn) = open_db();
        let store = SqliteRegimeManagerStateStore::new(conn.clone());

        let mut r = sample_regime("r-ac1");
        r.auto_label = "Deep Focus (varied)".to_string();
        r.sample_count = 42;
        r.centroid = maekon_core::models::tiered_memory::RegimeFeatures {
            category_coding: 0.9,
            category_communication: 0.0,
            category_browser: 0.0,
            avg_event_rate: 0.7,
            avg_importance: 0.8,
            context_activity_signal: 0.1,
            communication_ratio: 0.05,
        };
        r.status = maekon_core::models::tiered_memory::RegimeStatus::Active;

        store.save_all(std::slice::from_ref(&r)).await.unwrap();

        let c = conn.test_lock();
        let (label, dom_cat, occ, is_active): (String, String, i64, i64) = c
            .query_row(
                "SELECT label, dominant_category, occurrence_count, is_active \
                 FROM regimes WHERE id = 'r-ac1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("regimes row must exist after save_all");

        assert_eq!(label, r.auto_label, "label matches auto_label");
        assert_eq!(dom_cat, "coding", "dominant_category derived from centroid");
        assert_eq!(occ, 42, "occurrence_count matches sample_count");
        assert_eq!(is_active, 1, "Active regime → is_active=1");
    }

    /// AC-2: activity_segments JOIN regimes on regime_id resolves the label.
    #[tokio::test]
    async fn activity_segments_join_resolves_regime_label() {
        let (_d, conn) = open_db();
        let store = SqliteRegimeManagerStateStore::new(conn.clone());

        let r = sample_regime("r-ac2");
        store.save_all(std::slice::from_ref(&r)).await.unwrap();

        // Insert a segment that references the now-populated regime.
        {
            let c = conn.test_lock();
            c.execute(
                "INSERT INTO activity_segments \
                 (id, start_time, end_time, duration_secs, trigger_reason, \
                  dominant_category, regime_id, hlc_wall_ms, hlc_counter, origin_device_id) \
                 VALUES ('seg-ac2', '2026-01-01T00:00:00Z', '2026-01-01T01:00:00Z', \
                         3600, 'timer', 'coding', 'r-ac2', 0, 0, '')",
                [],
            )
            .unwrap();
        }

        // JOIN: segment can resolve its regime's label.
        let c = conn.test_lock();
        let resolved_label: String = c
            .query_row(
                "SELECT r.label FROM activity_segments s \
                 JOIN regimes r ON s.regime_id = r.id \
                 WHERE s.id = 'seg-ac2'",
                [],
                |row| row.get(0),
            )
            .expect("JOIN must resolve regime label for the inserted segment");
        assert_eq!(
            resolved_label,
            format!("label-{}", "r-ac2"),
            "resolved label must match the regime stored by save_all"
        );
    }

    /// AC-3: repeated save_all advances last_seen_at, never goes backward.
    #[tokio::test]
    async fn save_all_last_seen_at_advances_never_rewinds() {
        use chrono::Duration as ChronoDuration;

        let (_d, conn) = open_db();
        let store = SqliteRegimeManagerStateStore::new(conn.clone());

        let base_time = chrono::Utc::now();
        let mut r = sample_regime("r-ac3");
        r.last_seen = base_time;
        store.save_all(std::slice::from_ref(&r)).await.unwrap();

        let read_last_seen = |c: &rusqlite::Connection| -> String {
            c.query_row(
                "SELECT last_seen_at FROM regimes WHERE id = 'r-ac3'",
                [],
                |row| row.get(0),
            )
            .unwrap()
        };

        let first_stored = {
            let c = conn.test_lock();
            read_last_seen(&c)
        };

        // Save with a later last_seen — must advance.
        let mut r_later = r.clone();
        r_later.last_seen = base_time + ChronoDuration::seconds(60);
        store.save_all(&[r_later]).await.unwrap();

        let after_advance = {
            let c = conn.test_lock();
            read_last_seen(&c)
        };
        assert!(
            after_advance > first_stored,
            "last_seen_at must advance when a newer timestamp is saved"
        );

        // Save with the original (older) last_seen — must NOT rewind.
        let mut r_older = r.clone();
        r_older.last_seen = base_time;
        store.save_all(&[r_older]).await.unwrap();

        let after_rewind_attempt = {
            let c = conn.test_lock();
            read_last_seen(&c)
        };
        assert_eq!(
            after_rewind_attempt, after_advance,
            "last_seen_at must not rewind when an older timestamp is saved"
        );
    }

    /// AC-4: sync_merger's existing regimes path is unaffected (no new conflicts).
    /// Inserts a regime via sync path and verifies save_all UPDATE does not
    /// clobber its hlc columns or override a newer last_seen_at from the peer.
    #[tokio::test]
    async fn save_all_does_not_clobber_peer_synced_regime_hlc() {
        let (_d, conn) = open_db();

        // Seed a regime via the sync path (has a real HLC + peer origin).
        {
            let c = conn.test_lock();
            c.execute(
                "INSERT INTO regimes \
                 (id, label, detected_at, last_seen_at, occurrence_count, avg_density, \
                  avg_importance, dominant_category, is_active, \
                  hlc_wall_ms, hlc_counter, origin_device_id) \
                 VALUES ('r-sync', 'Peer Label', '2025-01-01T00:00:00Z', \
                         '2026-06-01T12:00:00Z', 100, 0.5, 0.7, 'coding', 1, \
                         9999, 5, 'peer-device')",
                [],
            )
            .unwrap();
        }

        let store = SqliteRegimeManagerStateStore::new(conn.clone());
        // Local in-memory view has a slightly older last_seen.
        let mut r = sample_regime("r-sync");
        r.last_seen = chrono::DateTime::parse_from_rfc3339("2026-05-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        store.save_all(&[r]).await.unwrap();

        let c = conn.test_lock();
        let (hlc_wall, hlc_counter, origin, last_seen): (i64, i64, String, String) = c
            .query_row(
                "SELECT hlc_wall_ms, hlc_counter, origin_device_id, last_seen_at \
                 FROM regimes WHERE id = 'r-sync'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();

        // HLC columns must be untouched (UPDATE path does not write them).
        assert_eq!(hlc_wall, 9999, "peer hlc_wall_ms must not be clobbered");
        assert_eq!(hlc_counter, 5, "peer hlc_counter must not be clobbered");
        assert_eq!(
            origin, "peer-device",
            "peer origin_device_id must not be clobbered"
        );
        // last_seen_at must not rewind to the older local value.
        assert_eq!(
            last_seen, "2026-06-01T12:00:00Z",
            "peer's newer last_seen_at must be preserved"
        );
    }
}
