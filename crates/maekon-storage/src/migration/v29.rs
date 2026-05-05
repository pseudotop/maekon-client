use rusqlite::Connection;

/// V29: Create `automation_presets` table for persistent custom preset storage.
///
/// Stores user-created automation presets in SQLite so they survive across
/// restarts without relying solely on the JSON config file.
pub(super) fn migrate_v29(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS automation_presets (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT DEFAULT '',
            category TEXT DEFAULT 'Custom',
            steps_json TEXT NOT NULL,
            builtin INTEGER DEFAULT 0,
            platform TEXT,
            ai_profile_id TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        INSERT OR IGNORE INTO schema_version (version) VALUES (29);",
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
             INSERT INTO schema_version VALUES (28);",
        )
        .unwrap();
    }

    #[test]
    fn migrate_v29_creates_automation_presets_table() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v29(&conn).unwrap();

        conn.execute(
            "INSERT INTO automation_presets
             (id, name, description, category, steps_json, builtin, platform, ai_profile_id, created_at, updated_at)
             VALUES ('test-1', 'Test Preset', 'A test preset', 'Custom', '[]', 0, NULL, NULL,
                     '2026-04-06T00:00:00Z', '2026-04-06T00:00:00Z')",
            [],
        )
        .unwrap();

        let name: String = conn
            .query_row(
                "SELECT name FROM automation_presets WHERE id = 'test-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "Test Preset");
    }

    #[test]
    fn migrate_v29_records_version() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v29(&conn).unwrap();

        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 29);
    }

    #[test]
    fn migrate_v29_idempotent_with_if_not_exists() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v29(&conn).unwrap();
        // Running again should not fail due to IF NOT EXISTS
        migrate_v29(&conn).unwrap();
    }
}
