//! v31 — create `regime_manager_state` singleton table for RegimeManager
//! persistence (Phase 3 C3c/X6).
//!
//! The `payload_backup_at` column is intentionally nullable and lacks the
//! usual `NOT NULL DEFAULT (datetime('now'))` convention because it is set
//! only when `SqliteRegimeManagerStateStore::load_all` quarantines a corrupt
//! payload. Do NOT "fix" this to match the sibling migrations — the nullable
//! shape is load-bearing.

use rusqlite::Connection;

pub(super) fn migrate_v31(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS regime_manager_state (
            id INTEGER PRIMARY KEY CHECK (id = 0),
            payload TEXT NOT NULL,
            payload_backup TEXT,
            payload_backup_at TEXT,
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        INSERT OR IGNORE INTO schema_version (version) VALUES (31);",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn setup_schema(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version VALUES (30);",
        )
        .unwrap();
    }

    #[test]
    fn migrate_v31_creates_regime_manager_state_table() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v31(&conn).unwrap();

        conn.execute(
            "INSERT INTO regime_manager_state (id, payload) VALUES (0, '[]')",
            [],
        )
        .unwrap();

        let payload: String = conn
            .query_row(
                "SELECT payload FROM regime_manager_state WHERE id = 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(payload, "[]");
    }

    #[test]
    fn migrate_v31_enforces_singleton_via_check_constraint() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v31(&conn).unwrap();

        // id = 0 is allowed
        conn.execute(
            "INSERT INTO regime_manager_state (id, payload) VALUES (0, '[]')",
            [],
        )
        .unwrap();

        // id != 0 must fail the CHECK constraint
        let err = conn
            .execute(
                "INSERT INTO regime_manager_state (id, payload) VALUES (1, '[]')",
                [],
            )
            .unwrap_err();
        assert!(
            err.to_string().contains("CHECK"),
            "expected CHECK constraint failure, got: {err}"
        );
    }

    #[test]
    fn migrate_v31_records_version() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v31(&conn).unwrap();

        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 31);
    }

    #[test]
    fn migrate_v31_idempotent_with_if_not_exists() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v31(&conn).unwrap();
        migrate_v31(&conn).unwrap();
    }

    #[test]
    fn migrate_v31_payload_backup_columns_nullable() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v31(&conn).unwrap();

        // Insert without specifying payload_backup or payload_backup_at.
        conn.execute(
            "INSERT INTO regime_manager_state (id, payload) VALUES (0, '[]')",
            [],
        )
        .unwrap();

        let backup: Option<String> = conn
            .query_row(
                "SELECT payload_backup FROM regime_manager_state WHERE id = 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(backup.is_none());
    }
}
