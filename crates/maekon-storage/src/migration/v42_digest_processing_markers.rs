use rusqlite::Connection;

/// V42: Record digest-side processing completion separately from digest rows.
///
/// A daily digest can exist while downstream ADR-023 claim promotion or belief
/// revision did not finish (for example, a crash after `save_daily_digest`). This
/// table is user-derived processing state, so it is erased with activity data
/// rather than retained in `app_meta`.
pub(super) fn migrate_v42(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS digest_processing_markers (
            kind         TEXT NOT NULL,
            period_key   TEXT NOT NULL,
            completed_at TEXT NOT NULL,
            PRIMARY KEY (kind, period_key)
        );
        INSERT OR IGNORE INTO schema_version (version) VALUES (42);",
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
             INSERT INTO schema_version VALUES (41);",
        )
        .unwrap();
    }

    #[test]
    fn migrate_v42_creates_digest_processing_markers() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v42(&conn).unwrap();

        conn.execute(
            "INSERT INTO digest_processing_markers (kind, period_key, completed_at)
             VALUES ('daily_claim_promotion', '2026-06-30', '2026-07-01T00:00:00Z')",
            [],
        )
        .unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM digest_processing_markers
                 WHERE kind = 'daily_claim_promotion' AND period_key = '2026-06-30'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn migrate_v42_records_version() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v42(&conn).unwrap();

        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 42);
    }
}
