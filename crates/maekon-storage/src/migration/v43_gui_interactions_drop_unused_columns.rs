//! Migration V43: drop unused `gui_interactions` columns (#7678 D3).
//!
//! `gui_interactions` (V13) carried `segment_id`/`element_text`/`element_type`/
//! `bbox_json` columns, but the only production writer
//! (`src-tauri/src/scheduler/loops/monitor.rs`) has always populated
//! `segment_id`/`element_text`/`bbox_json` with `None` and `element_type` with
//! the constant `Some("Click")` (duplicating `interaction_type`). The table
//! therefore never carried the richer per-element data its schema advertised:
//!
//! - Per-segment reads (`list_gui_interactions_for_segment`, keyed by
//!   `segment_id`) could never match a row, since `segment_id` is always
//!   `NULL` — dead-in-practice (removed alongside this migration).
//! - The FTS enrichment join in `sync_segment_enriched` (`collect_gui_element_text`,
//!   also keyed by `segment_id`) was equally dead-in-practice for the same
//!   reason (removed alongside this migration).
//!
//! The real GUI Activity Intelligence data (element text/type/bbox from OCR +
//! input correlation) flows through the separate `GuiActivitySummary`
//! aggregation pipeline (`src-tauri/src/scheduler/gui_pipeline/`), embedded
//! into `ContentActivity.gui_summary` — not through this table. Wiring the
//! real per-click element into this table's writer would require threading
//! the per-click `GuiElement` (not just the aggregated `GuiActivitySummary`)
//! out of `run_gui_tick`, a non-trivial cross-module refactor for a feature
//! that has not shipped. Dropping the unused columns instead so the schema no
//! longer advertises data it never carries.
//!
//! Remaining columns (`id`, `event_id`, `timestamp`, `interaction_type`,
//! `app_name`, `created_at`, `type_confidence`) back the one real consumer,
//! `query_gui_interaction_density` (hourly click-density chart), which never
//! read the dropped columns.

use rusqlite::Connection;

pub(super) fn migrate_v43(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "DROP INDEX IF EXISTS idx_gui_segment;
         ALTER TABLE gui_interactions DROP COLUMN segment_id;
         ALTER TABLE gui_interactions DROP COLUMN element_text;
         ALTER TABLE gui_interactions DROP COLUMN element_type;
         ALTER TABLE gui_interactions DROP COLUMN bbox_json;
         INSERT OR IGNORE INTO schema_version (version) VALUES (43);",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_schema(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version VALUES (42);
             CREATE TABLE gui_interactions (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 event_id TEXT NOT NULL,
                 segment_id TEXT,
                 timestamp TEXT NOT NULL,
                 element_text TEXT,
                 element_type TEXT,
                 interaction_type TEXT NOT NULL,
                 bbox_json TEXT,
                 app_name TEXT NOT NULL,
                 created_at TEXT NOT NULL DEFAULT (datetime('now')),
                 type_confidence REAL DEFAULT 1.0
             );
             CREATE INDEX idx_gui_segment ON gui_interactions(segment_id);
             CREATE INDEX idx_gui_timestamp ON gui_interactions(timestamp);",
        )
        .unwrap();
    }

    #[test]
    fn migrate_v43_drops_unused_columns() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);

        conn.execute(
            "INSERT INTO gui_interactions (event_id, segment_id, timestamp, element_text, element_type, interaction_type, bbox_json, app_name)
             VALUES ('evt-1', 'seg-1', '2026-07-01T00:00:00Z', 'Submit', 'button', 'Click', '{}', 'TestApp')",
            [],
        )
        .unwrap();

        migrate_v43(&conn).unwrap();

        let columns: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(gui_interactions)").unwrap();
            stmt.query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };

        for dropped in ["segment_id", "element_text", "element_type", "bbox_json"] {
            assert!(
                !columns.contains(&dropped.to_string()),
                "{dropped} should be dropped, got columns: {columns:?}"
            );
        }
        for kept in [
            "id",
            "event_id",
            "timestamp",
            "interaction_type",
            "app_name",
            "created_at",
            "type_confidence",
        ] {
            assert!(
                columns.contains(&kept.to_string()),
                "{kept} should be kept, got columns: {columns:?}"
            );
        }
    }

    #[test]
    fn migrate_v43_preserves_existing_rows() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);

        conn.execute(
            "INSERT INTO gui_interactions (event_id, timestamp, interaction_type, app_name)
             VALUES ('evt-1', '2026-07-01T00:00:00Z', 'Click', 'TestApp')",
            [],
        )
        .unwrap();

        migrate_v43(&conn).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM gui_interactions", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1, "existing rows must survive the column drop");

        let app_name: String = conn
            .query_row(
                "SELECT app_name FROM gui_interactions WHERE event_id = 'evt-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(app_name, "TestApp");
    }

    #[test]
    fn migrate_v43_drops_segment_index() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);

        migrate_v43(&conn).unwrap();

        let idx_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='index' AND name='idx_gui_segment'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            !idx_exists,
            "idx_gui_segment must be dropped with segment_id"
        );

        let idx_timestamp_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='index' AND name='idx_gui_timestamp'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            idx_timestamp_exists,
            "idx_gui_timestamp must survive (timestamp column kept)"
        );
    }

    #[test]
    fn migrate_v43_records_version() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v43(&conn).unwrap();

        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 43);
    }
}
