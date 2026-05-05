use rusqlite::Connection;

/// V30: Create `frame_annotations` table for user-created highlights, memos,
/// and arrows attached to captured frames.
pub(super) fn migrate_v30(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS frame_annotations (
            annotation_id TEXT PRIMARY KEY,
            frame_id INTEGER NOT NULL,
            annotation_type TEXT NOT NULL,
            x REAL NOT NULL,
            y REAL NOT NULL,
            width REAL DEFAULT 0,
            height REAL DEFAULT 0,
            color TEXT,
            text TEXT,
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_annotations_frame ON frame_annotations(frame_id);
        INSERT OR IGNORE INTO schema_version (version) VALUES (30);",
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
             INSERT INTO schema_version VALUES (29);",
        )
        .unwrap();
    }

    #[test]
    fn migrate_v30_creates_frame_annotations_table() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v30(&conn).unwrap();

        conn.execute(
            "INSERT INTO frame_annotations
             (annotation_id, frame_id, annotation_type, x, y, width, height, color, text, created_at)
             VALUES ('ann-1', 42, 'Highlight', 10.0, 20.0, 100.0, 50.0, '#ff0000', 'test', '2026-04-06T00:00:00Z')",
            [],
        )
        .unwrap();

        let annotation_type: String = conn
            .query_row(
                "SELECT annotation_type FROM frame_annotations WHERE annotation_id = 'ann-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(annotation_type, "Highlight");
    }

    #[test]
    fn migrate_v30_records_version() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v30(&conn).unwrap();

        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 30);
    }

    #[test]
    fn migrate_v30_idempotent_with_if_not_exists() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v30(&conn).unwrap();
        // Running again should not fail due to IF NOT EXISTS
        migrate_v30(&conn).unwrap();
    }

    #[test]
    fn migrate_v30_index_on_frame_id() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v30(&conn).unwrap();

        // Verify index exists by querying sqlite_master
        let idx_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_annotations_frame'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx_count, 1);
    }
}
