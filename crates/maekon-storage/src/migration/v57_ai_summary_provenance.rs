use rusqlite::Connection;

/// V57: Persist privacy-safe AI summary provenance and outcome metadata.
///
/// The JSON objects contain privacy-filtered presentation text, provider class,
/// generation timestamp, and a stable failure reason. Prompt text, endpoints,
/// credentials, model IDs, and provider error bodies are deliberately excluded.
pub(super) fn migrate_v57(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "ALTER TABLE activity_segments
             ADD COLUMN llm_summary_status_json TEXT NOT NULL DEFAULT '{}';
         ALTER TABLE daily_digests
             ADD COLUMN ai_narrative_status_json TEXT NOT NULL DEFAULT '{}';
         INSERT OR IGNORE INTO schema_version (version) VALUES (57);",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_adds_both_summary_status_columns() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             CREATE TABLE activity_segments (id TEXT PRIMARY KEY);
             CREATE TABLE daily_digests (date TEXT PRIMARY KEY);",
        )
        .unwrap();

        migrate_v57(&conn).unwrap();
        let segment_default: String = conn
            .query_row(
                "SELECT dflt_value FROM pragma_table_info('activity_segments')
                 WHERE name='llm_summary_status_json'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let narrative_default: String = conn
            .query_row(
                "SELECT dflt_value FROM pragma_table_info('daily_digests')
                 WHERE name='ai_narrative_status_json'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(segment_default, "'{}'");
        assert_eq!(narrative_default, "'{}'");
    }
}
