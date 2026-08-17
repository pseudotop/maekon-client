//! V55: non-authoritative effective mapping revalidation cache (#10358).
//!
//! The row contains a complete server response and its last live-validation
//! instant, but it is never an offline write lease. The adapter exposes it only
//! as `CachedEffectiveMappingCandidate`; a fresh server gate remains mandatory
//! before every workbook write.

use rusqlite::Connection;

pub(super) fn migrate_v55(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS effective_mapping_cache (
            organization_id        TEXT NOT NULL,
            mapping_id             TEXT NOT NULL,
            assignment_id          TEXT NOT NULL,
            version_id             TEXT NOT NULL,
            version_seq            INTEGER NOT NULL,
            content_hash           TEXT NOT NULL,
            content                TEXT NOT NULL,
            approval_seq           INTEGER NOT NULL,
            approved_at            TEXT NOT NULL,
            approved_by_user_id    TEXT NOT NULL,
            approved_template_hash TEXT NOT NULL,
            assignment_hash        TEXT NOT NULL,
            source_snapshot_hash   TEXT NOT NULL,
            server_validated_at    TEXT NOT NULL,
            PRIMARY KEY (organization_id, mapping_id, assignment_id)
        );
        INSERT OR IGNORE INTO schema_version (version) VALUES (55);",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_cache_with_anchor_scoped_primary_key() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE schema_version (version INTEGER PRIMARY KEY);")
            .unwrap();
        migrate_v55(&conn).unwrap();

        let columns: Vec<(String, i64)> = conn
            .prepare("SELECT name, pk FROM pragma_table_info('effective_mapping_cache')")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(columns.len(), 14);
        assert_eq!(columns[0], ("organization_id".into(), 1));
        assert_eq!(columns[1], ("mapping_id".into(), 2));
        assert_eq!(columns[2], ("assignment_id".into(), 3));
    }
}
