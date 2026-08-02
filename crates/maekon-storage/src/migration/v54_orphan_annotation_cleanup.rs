//! V54: delete `frame_annotations` rows whose frame is already gone (#9736).
//!
//! Not a schema change — a one-time repair of data that earlier builds left
//! behind.
//!
//! `frame_annotations.frame_id` declares **no** foreign key (v30), so deleting a
//! frame never removed its annotations. #9721/#9731 added the explicit cleanup
//! to the two deletion paths, but only for deletions from that point on. Every
//! install that ran retention (default 30 days) or app-deletion before those
//! landed still holds the rows.
//!
//! Why that is not harmless: `frame_annotations` is on the GDPR Art. 20 export
//! allowlist (`maintenance/export.rs`), and the export dumps it with the default
//! `SELECT * FROM {table}` — no frame join, no filter. So the rows keep appearing
//! in the user's own data export, carrying the `text` column, which is the
//! highlight or memo they wrote. Personal content outliving the deletion meant to
//! remove it.
//!
//! Why deleting them loses nothing reachable: the only production read is
//! `AnnotationStorage::list_annotations`, which is scoped `WHERE frame_id = ?1`.
//! An orphan's `frame_id` names a frame that no longer exists, so no UI path can
//! ask for it. The export is the single surface that ever sees these rows, and
//! seeing them there is the defect.
//!
//! Deliberately a migration rather than a maintenance task: it must run exactly
//! once per install, without waiting for a retention cycle that may be days away
//! on a machine whose export could be requested tomorrow.

use rusqlite::Connection;

pub(super) fn migrate_v54(conn: &Connection) -> rusqlite::Result<()> {
    // `frames.id` is the primary key, so `NOT IN (SELECT id FROM frames)` is an
    // index scan rather than a per-row table scan.
    let removed = conn.execute(
        "DELETE FROM frame_annotations \
         WHERE frame_id NOT IN (SELECT id FROM frames)",
        [],
    )?;
    if removed > 0 {
        // Counted, not just done: the backlog size on real installs was
        // unmeasurable before this ran — the store is encrypted, so it cannot be
        // inspected from outside the app. This log is the only way that number
        // ever becomes known.
        tracing::info!(
            removed,
            "#9736: deleted frame_annotations rows whose frame no longer exists"
        );
    }
    conn.execute_batch("INSERT OR IGNORE INTO schema_version (version) VALUES (54);")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::SqliteStorage;

    /// Seed a frame plus one annotation on it and one pointing nowhere.
    fn seed(conn: &Connection) {
        conn.execute(
            "INSERT INTO frames \
             (id, timestamp, trigger_type, app_name, window_title, importance, \
              resolution_w, resolution_h, created_at) \
             VALUES (1, '2026-01-01T00:00:00Z', 'manual', 'Editor', 'notes', 0.5, \
                     1920, 1080, '2026-01-01T00:00:00Z')",
            [],
        )
        .expect("seed frame");
        for (id, frame_id, text) in [
            ("ann-live", 1_i64, "kept — its frame exists"),
            ("ann-orphan", 999_i64, "account number"),
        ] {
            conn.execute(
                "INSERT INTO frame_annotations \
                 (annotation_id, frame_id, annotation_type, x, y, text, created_at) \
                 VALUES (?1, ?2, 'memo', 0.1, 0.1, ?3, '2026-01-01T00:00:00Z')",
                rusqlite::params![id, frame_id, text],
            )
            .expect("seed annotation");
        }
    }

    fn annotation_ids(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT annotation_id FROM frame_annotations ORDER BY annotation_id")
            .expect("prepare");
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query");
        rows.filter_map(Result::ok).collect()
    }

    /// The orphan goes, the live annotation stays.
    ///
    /// Both halves matter: a migration that deleted everything would also make
    /// the export stop leaking, and would pass a test that only checked the
    /// orphan.
    #[test]
    fn removes_only_the_annotations_whose_frame_is_gone() {
        let storage = SqliteStorage::open_in_memory(30).expect("in-memory sqlite");
        let conn = storage.connection_arc();
        let guard = conn.test_lock();
        seed(&guard);

        // Re-run the step directly: `open_in_memory` already migrated past v54,
        // so the seeded orphan is inserted afterwards and needs the sweep again.
        migrate_v54(&guard).expect("migrate_v54");

        assert_eq!(
            annotation_ids(&guard),
            vec!["ann-live".to_string()],
            "the orphan must be deleted and the live annotation kept"
        );
    }

    /// Running it twice is not an error and does not delete more.
    #[test]
    fn is_idempotent() {
        let storage = SqliteStorage::open_in_memory(30).expect("in-memory sqlite");
        let conn = storage.connection_arc();
        let guard = conn.test_lock();
        seed(&guard);

        migrate_v54(&guard).expect("first run");
        let after_first = annotation_ids(&guard);
        migrate_v54(&guard).expect("second run");

        assert_eq!(annotation_ids(&guard), after_first);
    }

    /// An install with no annotations at all is untouched, not failed.
    #[test]
    fn tolerates_an_empty_table() {
        let storage = SqliteStorage::open_in_memory(30).expect("in-memory sqlite");
        let conn = storage.connection_arc();
        let guard = conn.test_lock();

        migrate_v54(&guard).expect("empty table must not fail");
        assert!(annotation_ids(&guard).is_empty());
    }
}
