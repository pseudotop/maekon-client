//! Migration V45: drop the dead `search_trigram` FTS5 table (#8056 P3).
//!
//! `search_trigram` (a Korean trigram FTS5 virtual table) was created by V18
//! and has been recreated on every install ever since, plus carried through the
//! GDPR erasure loop (`delete_all_data_inner`). It never had a production
//! reader or writer: search retrieval switched to the V41 `search_fts` CJK
//! bigram shadow column (Option F, #5758), which superseded the trigram
//! approach for ja/ko recall. A workspace grep confirms zero
//! `INSERT INTO search_trigram` / `SELECT ... FROM search_trigram` call sites
//! outside the schema-management code being removed here.
//!
//! Dropping the dead table:
//! - stops carrying an always-empty FTS5 virtual table (+ its five shadow
//!   tables) in every install, and
//! - lets the GDPR erase loop drop `search_trigram` from `ALL_TABLES` and the
//!   post-erase FTS rebuild list (retention.rs), so a future
//!   `DELETE FROM search_trigram` can no longer error the erase transaction on
//!   a DB where V18's `IF NOT EXISTS` create silently no-op'd (trigram
//!   tokenizer unavailable).
//!
//! `DROP TABLE IF EXISTS` is used because V18's create is best-effort: on a
//! build whose bundled FTS5 lacks the `trigram` tokenizer the table was never
//! created, so this migration must tolerate its absence. Dropping an FTS5
//! virtual table also drops its `*_data`/`*_idx`/`*_content`/`*_config`/
//! `*_docsize` shadow tables.

use rusqlite::Connection;

pub(super) fn migrate_v45(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS search_trigram;
         INSERT OR IGNORE INTO schema_version (version) VALUES (45);",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_schema(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version VALUES (44);",
        )
        .unwrap();
        // Mirror V18's best-effort create so the drop path exercises a real
        // FTS5 virtual table (with its shadow tables) when the tokenizer exists.
        let _ = conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS search_trigram
                 USING fts5(segment_id UNINDEXED, content, tokenize='trigram');",
        );
    }

    #[test]
    fn migrate_v45_drops_search_trigram() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);

        migrate_v45(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='search_trigram'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "search_trigram must be dropped by V45");
    }

    #[test]
    fn migrate_v45_is_idempotent_when_table_absent() {
        // On a build without the trigram tokenizer V18's create no-op'd, so the
        // table never existed. The drop must still succeed (IF EXISTS).
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version VALUES (44);",
        )
        .unwrap();

        migrate_v45(&conn).expect("drop of an absent search_trigram must not error");

        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 45);
    }

    #[test]
    fn migrate_v45_records_version() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v45(&conn).unwrap();

        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 45);
    }
}
