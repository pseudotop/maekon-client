//! Migration V32: add partial index on `audit_log.command_id` for D25
//! `entries_by_command_id` queries.
//!
//! The `WHERE command_id IS NOT NULL` predicate keeps the index compact —
//! rows with a NULL `command_id` (none expected in practice, but schema
//! allows it) are excluded, matching the query's equality predicate.

use rusqlite::Connection;

pub(super) fn migrate_v32(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_audit_log_command_id
         ON audit_log (command_id) WHERE command_id IS NOT NULL;
         INSERT OR IGNORE INTO schema_version (version) VALUES (32);",
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
             INSERT INTO schema_version VALUES (31);
             CREATE TABLE audit_log (
                 entry_id TEXT PRIMARY KEY,
                 timestamp TEXT NOT NULL,
                 session_id TEXT NOT NULL,
                 command_id TEXT,
                 action_type TEXT NOT NULL,
                 status TEXT NOT NULL,
                 details TEXT,
                 execution_time_ms INTEGER
             );",
        )
        .unwrap();
    }

    #[test]
    fn migrate_v32_creates_index() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v32(&conn).unwrap();

        // Index existence can be verified via sqlite_master.
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_audit_log_command_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn migrate_v32_idempotent_with_if_not_exists() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v32(&conn).unwrap();
        // Second call must not error (CREATE INDEX IF NOT EXISTS).
        migrate_v32(&conn).unwrap();
    }

    #[test]
    fn migrate_v32_records_version() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v32(&conn).unwrap();

        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 32);
    }
}
