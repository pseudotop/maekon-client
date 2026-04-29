use rusqlite::Connection;

/// V28: Add feedback tracking columns to `local_suggestions` for few-shot prompt construction.
///
/// Also adds `suggestion_id`, `content`, and `confidence_score` columns so that the
/// `local_suggestions` table can be queried by `FewShotStorage` without joining
/// the `suggestions` (v8) table. These three columns are guarded by a PRAGMA
/// table_info check for idempotency (the v28 unit-test `setup_schema` pre-creates
/// them, so a plain `ALTER TABLE` would fail on the second run).
pub(super) fn migrate_v28(conn: &Connection) -> rusqlite::Result<()> {
    // --- idempotency guard for data columns (may already exist in test setups) ---
    let existing_cols: Vec<String> = conn
        .prepare("PRAGMA table_info(local_suggestions)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .collect();

    if !existing_cols.iter().any(|c| c == "suggestion_id") {
        conn.execute_batch(
            "ALTER TABLE local_suggestions ADD COLUMN suggestion_id TEXT NOT NULL DEFAULT '';",
        )?;
    }
    if !existing_cols.iter().any(|c| c == "content") {
        conn.execute_batch(
            "ALTER TABLE local_suggestions ADD COLUMN content TEXT NOT NULL DEFAULT '';",
        )?;
    }
    if !existing_cols.iter().any(|c| c == "confidence") {
        conn.execute_batch(
            "ALTER TABLE local_suggestions ADD COLUMN confidence REAL NOT NULL DEFAULT 0.0;",
        )?;
    }

    // --- feedback tracking columns (always new in v28) ---
    conn.execute_batch(
        "ALTER TABLE local_suggestions ADD COLUMN feedback_type TEXT;
         ALTER TABLE local_suggestions ADD COLUMN feedback_at TEXT;
         ALTER TABLE local_suggestions ADD COLUMN context_app TEXT DEFAULT '';
         ALTER TABLE local_suggestions ADD COLUMN context_window TEXT DEFAULT '';
         ALTER TABLE local_suggestions ADD COLUMN regime_label TEXT;
         CREATE INDEX IF NOT EXISTS idx_suggestions_feedback
           ON local_suggestions(feedback_type) WHERE feedback_type IS NOT NULL;
         INSERT OR IGNORE INTO schema_version (version) VALUES (28);",
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
             INSERT INTO schema_version VALUES (27);
             CREATE TABLE local_suggestions (
                 id INTEGER PRIMARY KEY,
                 suggestion_id TEXT NOT NULL,
                 suggestion_type TEXT NOT NULL,
                 content TEXT NOT NULL,
                 confidence REAL NOT NULL DEFAULT 0.0,
                 created_at TEXT NOT NULL
             );",
        )
        .unwrap();
    }

    #[test]
    fn migrate_v28_adds_feedback_columns() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v28(&conn).unwrap();

        conn.execute(
            "INSERT INTO local_suggestions
             (suggestion_id, suggestion_type, content, confidence, created_at,
              feedback_type, feedback_at, context_app, context_window, regime_label)
             VALUES ('s1', 'WORK_GUIDANCE', 'Take a break', 0.9, '2026-04-06T00:00:00Z',
                     'ACCEPTED', '2026-04-06T00:01:00Z', 'VSCode', 'main.rs', 'deep_work')",
            [],
        )
        .unwrap();

        let feedback: String = conn
            .query_row(
                "SELECT feedback_type FROM local_suggestions WHERE suggestion_id = 's1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(feedback, "ACCEPTED");
    }

    #[test]
    fn migrate_v28_feedback_columns_nullable() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v28(&conn).unwrap();

        conn.execute(
            "INSERT INTO local_suggestions
             (suggestion_id, suggestion_type, content, confidence, created_at)
             VALUES ('s2', 'PRODUCTIVITY_TIP', 'Stay hydrated', 0.7, '2026-04-06T00:00:00Z')",
            [],
        )
        .unwrap();

        let feedback: Option<String> = conn
            .query_row(
                "SELECT feedback_type FROM local_suggestions WHERE suggestion_id = 's2'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(feedback.is_none());
    }

    #[test]
    fn migrate_v28_records_version() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v28(&conn).unwrap();

        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 28);
    }

    #[test]
    fn migrate_v28_creates_feedback_index() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v28(&conn).unwrap();

        let count: u32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='index' AND name='idx_suggestions_feedback'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
}
