//! V56: durable local-first WBS XLSX output receipt spool (#10358).

use rusqlite::Connection;

pub(super) fn migrate_v56(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS wbs_xlsx_output_receipts (
            receipt_id      TEXT PRIMARY KEY NOT NULL,
            organization_id TEXT NOT NULL,
            mapping_id      TEXT NOT NULL,
            assignment_id   TEXT NOT NULL,
            receipt_json    TEXT NOT NULL,
            produced_at     TEXT NOT NULL,
            upload_state    TEXT NOT NULL CHECK (upload_state IN ('pending', 'uploaded')),
            uploaded_at     TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_wbs_xlsx_output_receipts_pending
            ON wbs_xlsx_output_receipts(upload_state, produced_at, receipt_id);
        INSERT OR IGNORE INTO schema_version (version) VALUES (56);",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_append_only_receipt_spool_shape() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE schema_version (version INTEGER PRIMARY KEY);")
            .unwrap();
        migrate_v56(&conn).unwrap();
        let columns: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('wbs_xlsx_output_receipts')")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(columns.len(), 8);
        assert_eq!(columns[0], "receipt_id");
        assert_eq!(columns[4], "receipt_json");
    }
}
