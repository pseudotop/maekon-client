use rusqlite::Connection;

/// V23: Add `state` and `resurface_at` columns to `suggestions` table
/// for queue persistence (offline save/restore).
pub(crate) fn migrate_v23(conn: &Connection) -> Result<(), rusqlite::Error> {
    // Idempotency guard: check if columns already exist.
    let has_state = conn
        .prepare("PRAGMA table_info(suggestions)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .any(|name| name == "state");

    if !has_state {
        conn.execute_batch(
            "ALTER TABLE suggestions ADD COLUMN state TEXT NOT NULL DEFAULT 'pending';
             ALTER TABLE suggestions ADD COLUMN resurface_at TEXT;",
        )?;
    }

    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_suggestions_state ON suggestions(state);
         INSERT OR IGNORE INTO schema_version (version) VALUES (23);",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// Create prerequisite tables for V23 migration.
    fn setup_prerequisites(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS suggestions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                suggestion_id TEXT NOT NULL UNIQUE,
                suggestion_type TEXT NOT NULL,
                source TEXT NOT NULL DEFAULT 'RULE_BASED',
                content TEXT NOT NULL,
                priority TEXT NOT NULL DEFAULT 'MEDIUM',
                confidence_score REAL NOT NULL DEFAULT 0.0,
                relevance_score REAL NOT NULL DEFAULT 0.0,
                is_actionable INTEGER NOT NULL DEFAULT 0,
                reasoning TEXT,
                shown_at TEXT,
                dismissed_at TEXT,
                acted_at TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                expires_at TEXT
            );",
        )
        .unwrap();
    }

    #[test]
    fn v23_adds_state_and_resurface_columns() {
        let conn = Connection::open_in_memory().unwrap();
        setup_prerequisites(&conn);
        migrate_v23(&conn).unwrap();

        // Insert a row with the new columns
        conn.execute(
            "INSERT INTO suggestions (suggestion_id, suggestion_type, content, state, resurface_at)
             VALUES ('s-1', 'WORK_GUIDANCE', 'test', 'deferred', '2026-04-05T00:00:00Z')",
            [],
        )
        .unwrap();

        let state: String = conn
            .query_row(
                "SELECT state FROM suggestions WHERE suggestion_id = 's-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "deferred");
    }

    #[test]
    fn v23_existing_rows_get_default_pending_state() {
        let conn = Connection::open_in_memory().unwrap();
        setup_prerequisites(&conn);

        // Insert a row before migration
        conn.execute(
            "INSERT INTO suggestions (suggestion_id, suggestion_type, content)
             VALUES ('s-old', 'WORK_GUIDANCE', 'old content')",
            [],
        )
        .unwrap();

        migrate_v23(&conn).unwrap();

        let state: String = conn
            .query_row(
                "SELECT state FROM suggestions WHERE suggestion_id = 's-old'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "pending");
    }

    #[test]
    fn v23_migration_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        setup_prerequisites(&conn);
        migrate_v23(&conn).unwrap();
        // Second run should succeed without error
        migrate_v23(&conn).unwrap();
    }
}
