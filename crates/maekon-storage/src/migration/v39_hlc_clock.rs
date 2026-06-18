//! Migration V39: `hlc_clock` singleton — durable monotonic HLC clock for local writes.
//!
//! For cross-device sync to actually propagate local data, every local
//! synced-table write must be stamped with a monotonically increasing HLC (Public
//! companion: `docs/guides/sync-conflict-resolution.md`, F0/#5186). This table is
//! the singleton (`id=0`) that holds that clock's **durable floor**. The actual
//! stamping (write-site wiring) is a follow-up PR; this migration only creates the
//! table (behavior-neutral).
//!
//! **erase policy:** `hlc_clock` is activity-timing metadata (wall_ms), so it is a
//! GDPR Art. 17 erasure target — it is included in `ALL_TABLES` of
//! `delete_all_data_inner` (not retained). After erasure, on the next restart
//! `post_migration_setup` re-seeds it based on the retained
//! `app_meta["sync.erasure_hlc"]`. (By contrast, the single `erasure_hlc` value is
//! retained `app_meta`.)

use rusqlite::Connection;

pub(super) fn migrate_v39(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS hlc_clock (
             id           INTEGER PRIMARY KEY CHECK (id = 0),
             last_wall_ms INTEGER NOT NULL DEFAULT 0,
             last_counter INTEGER NOT NULL DEFAULT 0
         );
         INSERT OR IGNORE INTO hlc_clock (id, last_wall_ms, last_counter) VALUES (0, 0, 0);
         INSERT OR IGNORE INTO schema_version (version) VALUES (39);",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn setup_v38(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version VALUES (38);",
        )
        .unwrap();
    }

    #[test]
    fn migrate_v39_creates_singleton() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v38(&conn);
        migrate_v39(&conn).unwrap();

        let (rows, wall, counter): (i64, i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), COALESCE(MAX(last_wall_ms),0), COALESCE(MAX(last_counter),0) \
                 FROM hlc_clock",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(rows, 1, "exactly one singleton row should be seeded");
        assert_eq!((wall, counter), (0, 0), "initial floor is (0,0)");
    }

    #[test]
    fn migrate_v39_singleton_constraint() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v38(&conn);
        migrate_v39(&conn).unwrap();
        // id != 0 violates the CHECK constraint.
        let insert_err = conn
            .execute(
                "INSERT INTO hlc_clock (id, last_wall_ms, last_counter) VALUES (1, 5, 0)",
                [],
            )
            .unwrap_err();
        let msg = insert_err.to_string().to_uppercase();
        assert!(
            msg.contains("CHECK"),
            "inserting id=1 should violate CHECK(id=0); got: {insert_err}"
        );
    }

    #[test]
    fn migrate_v39_records_version() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v38(&conn);
        migrate_v39(&conn).unwrap();
        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 39);
    }

    #[test]
    fn migrate_v39_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v38(&conn);
        migrate_v39(&conn).unwrap();
        migrate_v39(&conn).unwrap();
    }
}
