use rusqlite::Connection;

pub fn migrate_v20(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS session_audit_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp TEXT NOT NULL DEFAULT (datetime('now')),
            session_id TEXT NOT NULL,
            category TEXT NOT NULL,
            event_type TEXT NOT NULL,
            provider TEXT NOT NULL DEFAULT '',
            payload TEXT,
            duration_ms INTEGER,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_session_audit_session_id
            ON session_audit_log(session_id);
        CREATE INDEX IF NOT EXISTS idx_session_audit_category
            ON session_audit_log(category);
        CREATE INDEX IF NOT EXISTS idx_session_audit_timestamp
            ON session_audit_log(timestamp);

        INSERT OR IGNORE INTO schema_version (version) VALUES (20);",
    )?;
    Ok(())
}

pub fn migrate_v21(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS ai_sessions (
            session_id          TEXT PRIMARY KEY,
            provider            TEXT NOT NULL,
            model               TEXT NOT NULL DEFAULT '',
            transport           TEXT NOT NULL,
            state               TEXT NOT NULL DEFAULT 'active',
            system_prompt       TEXT,
            turn_count          INTEGER NOT NULL DEFAULT 0,
            total_input_tokens  INTEGER NOT NULL DEFAULT 0,
            total_output_tokens INTEGER NOT NULL DEFAULT 0,
            created_at          TEXT NOT NULL DEFAULT (datetime('now')),
            last_active         TEXT NOT NULL DEFAULT (datetime('now')),
            terminated_at       TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_ai_sessions_state
            ON ai_sessions(state);
        CREATE INDEX IF NOT EXISTS idx_ai_sessions_created
            ON ai_sessions(created_at);
        CREATE INDEX IF NOT EXISTS idx_ai_sessions_last_active
            ON ai_sessions(last_active);

        CREATE TABLE IF NOT EXISTS ai_conversation_messages (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id   TEXT NOT NULL REFERENCES ai_sessions(session_id) ON DELETE CASCADE,
            role         TEXT NOT NULL,
            content      TEXT NOT NULL DEFAULT '',
            thinking     TEXT,
            tool_use     TEXT,
            usage_input  INTEGER,
            usage_output INTEGER,
            created_at   TEXT NOT NULL DEFAULT (datetime('now')),
            seq          INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_ai_messages_session
            ON ai_conversation_messages(session_id, seq);

        INSERT OR IGNORE INTO schema_version (version) VALUES (21);",
    )?;
    Ok(())
}

pub fn migrate_v22(conn: &Connection) -> Result<(), rusqlite::Error> {
    // Check if column already exists (idempotent guard)
    let has_column = conn
        .prepare("PRAGMA table_info(gui_interactions)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .any(|name| name == "type_confidence");

    if !has_column {
        conn.execute_batch(
            "ALTER TABLE gui_interactions ADD COLUMN type_confidence REAL DEFAULT 1.0;",
        )?;
    }

    conn.execute_batch("INSERT OR IGNORE INTO schema_version (version) VALUES (22);")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// Create the prerequisite schema_version table (normally created by run_migrations).
    fn setup_schema_version(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
    }

    #[test]
    fn creates_session_audit_log_table() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema_version(&conn);
        migrate_v20(&conn).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM session_audit_log", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn inserts_and_queries_audit_entry() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema_version(&conn);
        migrate_v20(&conn).unwrap();

        conn.execute(
            "INSERT INTO session_audit_log (session_id, category, event_type, provider)
             VALUES (?1, ?2, ?3, ?4)",
            ["sess-1", "session", "create", "claude"],
        )
        .unwrap();

        let session_id: String = conn
            .query_row(
                "SELECT session_id FROM session_audit_log WHERE category = 'session'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(session_id, "sess-1");
    }

    #[test]
    fn migration_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema_version(&conn);
        migrate_v20(&conn).unwrap();
        migrate_v20(&conn).unwrap();
    }

    #[test]
    fn creates_ai_sessions_and_messages_tables() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema_version(&conn);
        migrate_v21(&conn).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM ai_sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM ai_conversation_messages", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn v21_migration_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema_version(&conn);
        migrate_v21(&conn).unwrap();
        migrate_v21(&conn).unwrap();
    }

    #[test]
    fn v21_cascade_deletes_messages() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema_version(&conn);
        migrate_v21(&conn).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

        conn.execute(
            "INSERT INTO ai_sessions (session_id, provider, transport) VALUES ('s1', 'claude', 'http_api')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO ai_conversation_messages (session_id, role, content, seq) VALUES ('s1', 'user', 'hello', 0)",
            [],
        ).unwrap();

        conn.execute("DELETE FROM ai_sessions WHERE session_id = 's1'", [])
            .unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM ai_conversation_messages WHERE session_id = 's1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "CASCADE should delete messages");
    }

    /// Helper: create the V13 gui_interactions table prerequisite for V22.
    fn setup_gui_interactions(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS gui_interactions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_id TEXT NOT NULL,
                segment_id TEXT,
                timestamp TEXT NOT NULL,
                element_text TEXT,
                element_type TEXT,
                interaction_type TEXT NOT NULL,
                bbox_json TEXT,
                app_name TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
    }

    #[test]
    fn v22_adds_type_confidence_column() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema_version(&conn);
        setup_gui_interactions(&conn);
        migrate_v22(&conn).unwrap();

        // Insert a row with the new column
        conn.execute(
            "INSERT INTO gui_interactions (event_id, timestamp, interaction_type, app_name, type_confidence)
             VALUES ('evt-1', '2026-04-04T00:00:00Z', 'Click', 'TestApp', 0.85)",
            [],
        )
        .unwrap();

        let confidence: f64 = conn
            .query_row(
                "SELECT type_confidence FROM gui_interactions WHERE event_id = 'evt-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!((confidence - 0.85).abs() < 0.001);
    }

    #[test]
    fn v22_existing_rows_get_default_confidence() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema_version(&conn);
        setup_gui_interactions(&conn);

        // Insert a row before migration (no type_confidence column)
        conn.execute(
            "INSERT INTO gui_interactions (event_id, timestamp, interaction_type, app_name)
             VALUES ('evt-old', '2026-04-04T00:00:00Z', 'Click', 'OldApp')",
            [],
        )
        .unwrap();

        migrate_v22(&conn).unwrap();

        // Existing rows should have DEFAULT 1.0
        let confidence: f64 = conn
            .query_row(
                "SELECT type_confidence FROM gui_interactions WHERE event_id = 'evt-old'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!((confidence - 1.0).abs() < 0.001);
    }

    #[test]
    fn v22_migration_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema_version(&conn);
        setup_gui_interactions(&conn);
        migrate_v22(&conn).unwrap();
        // Second run should succeed without error
        migrate_v22(&conn).unwrap();
    }
}
