//! Migration V38: `sync_tombstones` table — retention outbox for cross-device
//! delete convergence.
//!
//! On GDPR Art. 17 consent-revoke, retains only the *skeleton* (id + HLC +
//! deleted_at) of the sync rows that are hard-wiped locally, so that a peer that
//! was offline can connect later, receive the tombstones over the regular sync
//! stream, and converge (hard-delete + resurrection suppression). Public
//! companion: `docs/guides/sync-conflict-resolution.md`.
//!
//! **No PII**: holds only `table_name`/`row_id`/`origin_device_id`/HLC/`deleted_at`.
//! Body columns (llm_summary, original_text, etc.) are never stored, so no deleted
//! user content remains on disk or on the wire. This retention basis is the same
//! GDPR Art. 17(3) processing-record basis as `egress_ledger` (V36) and `audit_log`.
//!
//! **Intentionally NOT included** in `ALL_TABLES` of `delete_all_data_inner`
//! (retained). The actual tombstone creation (capture id on erase → INSERT) joins
//! the `delete_all_data_inner` transaction in S2 (#5179) — this migration only
//! creates the schema (behavior-neutral).
//!
//! Retention period: not permanent. After a re-grant, if a higher-HLC row
//! supersedes a tombstone, S3 may remove that tombstone early (#5180); otherwise
//! tombstone GC (`max(retention,90)d`, S5/#5182) enforces an upper bound —
//! metadata minimization (GDPR Art. 5(1)(e)).
//!
//! Index: `(hlc_wall_ms, hlc_counter)` — for the ordering/filtering of the path
//! where the extractor pulls outbox rows via `HLC > watermark`.

use rusqlite::Connection;

pub(super) fn migrate_v38(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sync_tombstones (
             table_name       TEXT    NOT NULL,
             row_id           TEXT    NOT NULL,
             origin_device_id TEXT    NOT NULL,
             hlc_wall_ms      INTEGER NOT NULL,
             hlc_counter      INTEGER NOT NULL,
             deleted_at       TEXT    NOT NULL,
             PRIMARY KEY (table_name, row_id)
         );
         CREATE INDEX IF NOT EXISTS idx_sync_tombstones_hlc
             ON sync_tombstones(hlc_wall_ms, hlc_counter);
         INSERT OR IGNORE INTO schema_version (version) VALUES (38);",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// Run the migration from the minimal v37-equivalent schema (schema_version only).
    fn setup_v37(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version VALUES (37);",
        )
        .unwrap();
    }

    #[test]
    fn migrate_v38_creates_table() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v37(&conn);
        migrate_v38(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type='table' AND name='sync_tombstones'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "sync_tombstones table should be created");

        let idx: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type='index' AND name='idx_sync_tombstones_hlc'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx, 1, "HLC index should exist");
    }

    #[test]
    fn migrate_v38_composite_pk_dedups() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v37(&conn);
        migrate_v38(&conn).unwrap();

        // row_id is just the live table's synthetic key (ULID/UUID/autoincrement),
        // not user content — we use a UUID-format string to convey its opaque-key nature.
        let rid = "01920e1f-0000-7000-8000-000000000001";
        conn.execute(
            "INSERT INTO sync_tombstones \
             (table_name, row_id, origin_device_id, hlc_wall_ms, hlc_counter, deleted_at) \
             VALUES ('activity_segments',?1,'dev-a',100,0,'2026-06-05T00:00:00Z')",
            [rid],
        )
        .unwrap();
        // Re-inserting the same (table_name, row_id) is a PK conflict — this only
        // verifies that SQLite supports composite-PK upsert (the actual producer's
        // upsert API choice is decided in S2/S3).
        conn.execute(
            "INSERT OR REPLACE INTO sync_tombstones \
             (table_name, row_id, origin_device_id, hlc_wall_ms, hlc_counter, deleted_at) \
             VALUES ('activity_segments',?1,'dev-a',200,1,'2026-06-05T01:00:00Z')",
            [rid],
        )
        .unwrap();

        let (rows, wall): (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), MAX(hlc_wall_ms) FROM sync_tombstones \
                 WHERE table_name='activity_segments' AND row_id=?1",
                [rid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(rows, 1, "composite PK should yield 1 row per (table, row)");
        assert_eq!(wall, 200, "REPLACE should retain the higher HLC");
    }

    #[test]
    fn migrate_v38_records_version() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v37(&conn);
        migrate_v38(&conn).unwrap();

        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 38);
    }

    #[test]
    fn migrate_v38_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v37(&conn);
        migrate_v38(&conn).unwrap();
        migrate_v38(&conn).unwrap();
    }
}
