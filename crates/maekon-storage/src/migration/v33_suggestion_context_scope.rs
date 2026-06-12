//! Migration V33: persist suggestion app/window/target context scope.
//!
//! These columns keep restored suggestions scoped to the GUI entity that
//! produced them, so duplicate text in different apps does not collapse after
//! restart.

use rusqlite::Connection;

pub(super) fn migrate_v33(conn: &Connection) -> rusqlite::Result<()> {
    let existing_cols: Vec<String> = conn
        .prepare("PRAGMA table_info(suggestions)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(Result::ok)
        .collect();

    if !existing_cols.iter().any(|name| name == "context_app") {
        conn.execute_batch("ALTER TABLE suggestions ADD COLUMN context_app TEXT DEFAULT '';")?;
    }
    if !existing_cols.iter().any(|name| name == "context_window") {
        conn.execute_batch("ALTER TABLE suggestions ADD COLUMN context_window TEXT DEFAULT '';")?;
    }
    if !existing_cols.iter().any(|name| name == "context_target_id") {
        conn.execute_batch(
            "ALTER TABLE suggestions ADD COLUMN context_target_id TEXT DEFAULT '';",
        )?;
    }

    conn.execute_batch("INSERT OR IGNORE INTO schema_version (version) VALUES (33);")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn setup_schema(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version VALUES (32);
             CREATE TABLE suggestions (
                 id INTEGER PRIMARY KEY,
                 suggestion_id TEXT NOT NULL UNIQUE,
                 suggestion_type TEXT NOT NULL,
                 source TEXT NOT NULL DEFAULT 'RULE_BASED',
                 content TEXT NOT NULL,
                 priority TEXT NOT NULL DEFAULT 'MEDIUM',
                 confidence_score REAL NOT NULL DEFAULT 0.0,
                 relevance_score REAL NOT NULL DEFAULT 0.0,
                 is_actionable INTEGER NOT NULL DEFAULT 1,
                 reasoning TEXT,
                 shown_at TEXT,
                 dismissed_at TEXT,
                 acted_at TEXT,
                 created_at TEXT NOT NULL DEFAULT (datetime('now')),
                 expires_at TEXT,
                 state TEXT NOT NULL DEFAULT 'pending',
                 resurface_at TEXT
             );",
        )
        .unwrap();
    }

    fn column_count(conn: &Connection, name: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('suggestions') WHERE name = ?1",
            [name],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn migrate_v33_adds_context_scope_columns() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v33(&conn).unwrap();

        assert_eq!(column_count(&conn, "context_app"), 1);
        assert_eq!(column_count(&conn, "context_window"), 1);
        assert_eq!(column_count(&conn, "context_target_id"), 1);
    }

    #[test]
    fn migrate_v33_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v33(&conn).unwrap();
        migrate_v33(&conn).unwrap();

        assert_eq!(column_count(&conn, "context_app"), 1);
        assert_eq!(column_count(&conn, "context_window"), 1);
        assert_eq!(column_count(&conn, "context_target_id"), 1);
    }

    #[test]
    fn migrate_v33_records_version() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v33(&conn).unwrap();

        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 33);
    }
}
