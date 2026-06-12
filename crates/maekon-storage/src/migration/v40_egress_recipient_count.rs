//! Migration V40: `egress_ledger.recipient_count` — LAN fan-out audit grain (#5147 item 2).
//!
//! `byte_count` is the PLAINTEXT single-serialization size (not on-wire). A LAN push
//! fans the SAME serialized changeset out to N discovered peers, so the true aggregate
//! egress volume is `byte_count * recipient_count`. Before this column a multi-peer LAN
//! push recorded one row implying a single recipient, under-counting the volume that
//! actually left the device. File/Remote are single-destination → recipient_count = 1.
//!
//! Default 1 backfills existing rows (every prior egress was effectively single-recipient,
//! since the LAN fan-out path was inert before #5212).

use rusqlite::Connection;

pub(super) fn migrate_v40(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "ALTER TABLE egress_ledger ADD COLUMN recipient_count INTEGER NOT NULL DEFAULT 1;
         INSERT OR IGNORE INTO schema_version (version) VALUES (40);",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// Minimal v39-equivalent: the egress_ledger table (v36 shape) + schema_version.
    fn setup_v39(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version VALUES (39);
             CREATE TABLE egress_ledger (
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
             INSERT INTO egress_ledger \
                 (record_id, event_type, byte_count, destination, disposition, consent_state, occurred_at) \
                 VALUES ('r1', 'CrossDeviceSync', 100, 'sync.lan', 'uploaded', 'cross_device_sync=true', '2026-01-01');",
        )
        .unwrap();
    }

    #[test]
    fn migrate_v40_adds_column_defaulting_existing_rows_to_1() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v39(&conn);
        migrate_v40(&conn).unwrap();

        let rc: i64 = conn
            .query_row(
                "SELECT recipient_count FROM egress_ledger WHERE record_id='r1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rc, 1, "existing rows backfill to recipient_count = 1");

        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 40);
    }
}
