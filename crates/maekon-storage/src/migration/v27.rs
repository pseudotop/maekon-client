use rusqlite::Connection;

/// V27: Add `habit_streaks` table for daily regime habit tracking.
pub(crate) fn migrate_v27(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS habit_streaks (
            id INTEGER PRIMARY KEY,
            regime_label TEXT NOT NULL,
            date TEXT NOT NULL,
            minutes_logged INTEGER NOT NULL DEFAULT 0,
            target_minutes INTEGER NOT NULL,
            met BOOLEAN NOT NULL DEFAULT 0,
            UNIQUE(regime_label, date)
        );
        CREATE INDEX IF NOT EXISTS idx_habit_streaks_date ON habit_streaks(date);
        INSERT OR IGNORE INTO schema_version (version) VALUES (27);",
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
             INSERT INTO schema_version VALUES (26);",
        )
        .unwrap();
    }

    #[test]
    fn migrate_v27_creates_habit_streaks_table() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v27(&conn).unwrap();

        conn.execute(
            "INSERT INTO habit_streaks (regime_label, date, minutes_logged, target_minutes, met)
             VALUES ('deep_work', '2026-04-06', 120, 120, 1)",
            [],
        )
        .unwrap();

        let count: u32 = conn
            .query_row("SELECT COUNT(*) FROM habit_streaks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn migrate_v27_unique_constraint() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v27(&conn).unwrap();

        conn.execute(
            "INSERT INTO habit_streaks (regime_label, date, minutes_logged, target_minutes, met)
             VALUES ('deep_work', '2026-04-06', 60, 120, 0)",
            [],
        )
        .unwrap();

        // Duplicate (regime_label, date) should fail
        let result = conn.execute(
            "INSERT INTO habit_streaks (regime_label, date, minutes_logged, target_minutes, met)
             VALUES ('deep_work', '2026-04-06', 90, 120, 0)",
            [],
        );
        assert!(result.is_err());
    }

    #[test]
    fn migrate_v27_records_version() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v27(&conn).unwrap();

        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 27);
    }

    #[test]
    fn migrate_v27_allows_different_regimes_same_date() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v27(&conn).unwrap();

        conn.execute(
            "INSERT INTO habit_streaks (regime_label, date, minutes_logged, target_minutes, met)
             VALUES ('deep_work', '2026-04-06', 120, 120, 1)",
            [],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO habit_streaks (regime_label, date, minutes_logged, target_minutes, met)
             VALUES ('communication', '2026-04-06', 30, 60, 0)",
            [],
        )
        .unwrap();

        let count: u32 = conn
            .query_row("SELECT COUNT(*) FROM habit_streaks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }
}
