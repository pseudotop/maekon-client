//! V47: `transcripts` table for persisted speech-to-text output (#8059).
//!
//! Voice transcripts produced by the STT pipeline (whisper / cloud / fallback)
//! were previously never stored — they were handed to the IPC caller or emitted
//! to the chat input and then discarded. This table persists the (PII-masked)
//! transcript so "meetings stay on your machine too": the text is indexed into
//! `search_fts` (`content_type = 'transcript'`) for keyword/hybrid search and is
//! swept by the same range-delete / full-wipe / age-retention erasure paths as
//! other user activity data.
//!
//! LOCAL-ONLY by design: NO HLC / sync columns. Transcripts are deliberately
//! kept off the cross-device sync surface, so there is no `hlc_wall_ms` /
//! `hlc_counter` / `origin_device_id` and no `sync_tombstones` participation.
//!
//! `timestamp` is stored RFC3339 (matching `TimeWindow::to_sql_pair`) so the
//! closed-closed range delete/query predicates compare exactly; `idx` on it
//! keeps range sweeps O(log n).

use rusqlite::Connection;

pub(super) fn migrate_v47(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS transcripts (
            id            TEXT PRIMARY KEY,
            timestamp     TEXT NOT NULL,
            duration_secs REAL NOT NULL DEFAULT 0,
            source        TEXT NOT NULL,
            language      TEXT,
            text          TEXT NOT NULL,
            created_at    TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_transcripts_timestamp
            ON transcripts(timestamp);

        INSERT OR IGNORE INTO schema_version (version) VALUES (47);",
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
             INSERT INTO schema_version VALUES (46);",
        )
        .unwrap();
    }

    #[test]
    fn migrate_v47_creates_transcripts_table() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v47(&conn).unwrap();

        conn.execute(
            "INSERT INTO transcripts (id, timestamp, duration_secs, source, language, text)
             VALUES ('t-1', '2026-07-11T10:00:00+00:00', 4.2, 'whisper', 'en', 'hello world')",
            [],
        )
        .unwrap();

        let (source, text): (String, String) = conn
            .query_row(
                "SELECT source, text FROM transcripts WHERE id = 't-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(source, "whisper");
        assert_eq!(text, "hello world");
    }

    #[test]
    fn migrate_v47_language_is_nullable() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v47(&conn).unwrap();

        conn.execute(
            "INSERT INTO transcripts (id, timestamp, duration_secs, source, text)
             VALUES ('t-2', '2026-07-11T11:00:00+00:00', 1.0, 'cloud', 'no language here')",
            [],
        )
        .unwrap();

        let language: Option<String> = conn
            .query_row(
                "SELECT language FROM transcripts WHERE id = 't-2'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(language, None, "language column must accept NULL");
    }

    #[test]
    fn migrate_v47_records_version() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v47(&conn).unwrap();

        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 47);
    }

    #[test]
    fn migrate_v47_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v47(&conn).unwrap();
        migrate_v47(&conn).unwrap();
    }
}
