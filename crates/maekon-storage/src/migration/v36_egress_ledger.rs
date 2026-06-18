//! Migration V36: `egress_ledger` table — egress audit ledger.
//!
//! Records events that left the device (or were policy-blocked) as regulatory
//! compliance evidence (#4803, E20). Both the successful-upload path and the
//! blocked path (clipboard/file-access/excluded-app) are recorded, distinguished
//! by `disposition`. `record_id` is a caller-generated UUID; the UNIQUE constraint
//! deduplicates on re-execution (INSERT OR IGNORE).
//!
//! Indexes: `occurred_at` (time-series queries), `event_type` (per-type aggregation),
//! and `disposition` (upload/blocked filter), one each.

use rusqlite::Connection;

pub(super) fn migrate_v36(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS egress_ledger (
             id            INTEGER PRIMARY KEY AUTOINCREMENT,
             record_id     TEXT NOT NULL UNIQUE,
             event_type    TEXT NOT NULL,
             event_id      TEXT,
             byte_count    INTEGER NOT NULL,
             destination   TEXT NOT NULL,
             disposition   TEXT NOT NULL,
             consent_state TEXT NOT NULL,
             occurred_at   TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_egress_ledger_occurred_at
             ON egress_ledger(occurred_at);
         CREATE INDEX IF NOT EXISTS idx_egress_ledger_event_type
             ON egress_ledger(event_type);
         CREATE INDEX IF NOT EXISTS idx_egress_ledger_disposition
             ON egress_ledger(disposition);
         INSERT OR IGNORE INTO schema_version (version) VALUES (36);",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// Build the minimal v35-equivalent schema (schema_version only) and run the
    /// migration from a realistic immediately-prior state.
    fn setup_v35(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version VALUES (35);",
        )
        .unwrap();
    }

    #[test]
    fn migrate_v36_creates_table() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v35(&conn);
        migrate_v36(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type='table' AND name='egress_ledger'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "egress_ledger table should be created");

        // Verify all three indexes exist.
        for idx in [
            "idx_egress_ledger_occurred_at",
            "idx_egress_ledger_event_type",
            "idx_egress_ledger_disposition",
        ] {
            let c: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
                    [idx],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(c, 1, "{idx} index should exist");
        }
    }

    #[test]
    fn migrate_v36_records_version() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v35(&conn);
        migrate_v36(&conn).unwrap();

        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 36);
    }

    #[test]
    fn migrate_v36_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v35(&conn);
        migrate_v36(&conn).unwrap();
        // The second call should not error, thanks to CREATE TABLE/INDEX IF NOT EXISTS.
        migrate_v36(&conn).unwrap();
    }
}
