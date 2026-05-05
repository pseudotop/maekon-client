use rusqlite::Connection;

/// V25: Add `audit_log` table for durable persistence of automation audit entries.
pub(crate) fn migrate_v25(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS audit_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            entry_id TEXT NOT NULL,
            timestamp TEXT NOT NULL,
            session_id TEXT NOT NULL,
            command_id TEXT NOT NULL,
            action_type TEXT NOT NULL,
            status TEXT NOT NULL,
            details TEXT,
            execution_time_ms INTEGER,
            UNIQUE(entry_id)
        );
        CREATE INDEX IF NOT EXISTS idx_audit_log_session_id ON audit_log(session_id);
        CREATE INDEX IF NOT EXISTS idx_audit_log_timestamp ON audit_log(timestamp);
        CREATE INDEX IF NOT EXISTS idx_audit_log_action_type ON audit_log(action_type);
        INSERT OR IGNORE INTO schema_version (version) VALUES (25);",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn migrate_v25_creates_table() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version VALUES (24);",
        )
        .unwrap();
        migrate_v25(&conn).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='audit_log'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn migrate_v25_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version VALUES (24);",
        )
        .unwrap();
        migrate_v25(&conn).unwrap();
        migrate_v25(&conn).unwrap(); // second call should not fail
    }

    #[test]
    fn migrate_v25_insert_and_query() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version VALUES (24);",
        )
        .unwrap();
        migrate_v25(&conn).unwrap();

        conn.execute(
            "INSERT INTO audit_log (entry_id, timestamp, session_id, command_id, action_type, status, details, execution_time_ms)
             VALUES ('e-001', '2026-04-05T00:00:00Z', 'sess-1', 'cmd-1', 'MouseClick', 'Completed', 'Success', 42)",
            [],
        )
        .unwrap();

        let (entry_id, status): (String, String) = conn
            .query_row(
                "SELECT entry_id, status FROM audit_log WHERE entry_id = 'e-001'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(entry_id, "e-001");
        assert_eq!(status, "Completed");
    }

    #[test]
    fn migrate_v25_indexes_created() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version VALUES (24);",
        )
        .unwrap();
        migrate_v25(&conn).unwrap();

        for idx_name in [
            "idx_audit_log_session_id",
            "idx_audit_log_timestamp",
            "idx_audit_log_action_type",
        ] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
                    [idx_name],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "index {idx_name} should exist");
        }
    }
}
