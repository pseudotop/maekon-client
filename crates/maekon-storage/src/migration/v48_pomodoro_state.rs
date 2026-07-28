//! V48: durable singleton state for the local Pomodoro timer (#8218).

use rusqlite::Connection;

pub(super) fn migrate_v48(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS pomodoro_state (
            slot         INTEGER PRIMARY KEY CHECK (slot = 1),
            session_json TEXT NOT NULL,
            started_at   TEXT NOT NULL,
            completed_at TEXT,
            updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
        );

        INSERT OR IGNORE INTO schema_version (version) VALUES (48);",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_creates_singleton_table_and_records_version() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version VALUES (47);",
        )
        .unwrap();

        migrate_v48(&conn).unwrap();
        conn.execute(
            "INSERT INTO pomodoro_state (slot, session_json, started_at)
             VALUES (1, '{}', '2026-07-12T00:00:00Z')",
            [],
        )
        .unwrap();
        let err = conn
            .execute(
                "INSERT INTO pomodoro_state (slot, session_json, started_at)
                 VALUES (2, '{}', '2026-07-12T00:00:00Z')",
                [],
            )
            .unwrap_err();
        assert!(
            err.to_string().contains("CHECK constraint failed"),
            "second slot must be rejected by the slot = 1 singleton CHECK, got: {err}"
        );
        assert_eq!(
            conn.query_row("SELECT MAX(version) FROM schema_version", [], |r| r
                .get::<_, u32>(0))
                .unwrap(),
            48
        );
    }
}
