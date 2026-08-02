//! V53: vault mirror change-detection state (ADR-033 §1.4, #9465).
//!
//! One additive table:
//!
//! * `vault_mirror_state` — one row per generated vault file, keyed by the
//!   **vault-relative file name** (`daily/2026-07-29.md`, `claims.md`,
//!   `README.md`), holding the last-rendered content hash. Deliberately never
//!   an absolute filesystem path: an absolute path would embed the OS
//!   username into the store, and the vault root can move (default ↔ custom)
//!   without invalidating state.
//!
//! The table is a member of the erasure `ALL_TABLES` sweep
//! (`maintenance/retention.rs`): hash state must never outlive the files it
//! describes — an erase-surviving row would make the next mirror cycle see
//! "hash matches" and silently skip regenerating files Art.17 just deleted
//! (the writer additionally re-checks file existence per cycle, ADR-033 §1.4).

use rusqlite::Connection;

pub(super) fn migrate_v53(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS vault_mirror_state (
            file_name    TEXT PRIMARY KEY,
            content_hash TEXT NOT NULL,
            updated_at   INTEGER NOT NULL
        );
        INSERT OR IGNORE INTO schema_version (version) VALUES (53);",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_version_table() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE schema_version (version INTEGER PRIMARY KEY);")
            .unwrap();
        conn
    }

    #[test]
    fn creates_table_with_expected_columns() {
        let conn = with_version_table();
        migrate_v53(&conn).unwrap();
        let mut stmt = conn
            .prepare("SELECT name FROM pragma_table_info('vault_mirror_state')")
            .unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(cols, vec!["file_name", "content_hash", "updated_at"]);
    }

    #[test]
    fn upsert_replaces_by_file_name() {
        let conn = with_version_table();
        migrate_v53(&conn).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO vault_mirror_state (file_name, content_hash, updated_at)
             VALUES ('claims.md', 'h1', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO vault_mirror_state (file_name, content_hash, updated_at)
             VALUES ('claims.md', 'h2', 2)",
            [],
        )
        .unwrap();
        let (hash, n): (String, i64) = conn
            .query_row(
                "SELECT content_hash, (SELECT COUNT(*) FROM vault_mirror_state)
                 FROM vault_mirror_state WHERE file_name = 'claims.md'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((hash.as_str(), n), ("h2", 1));
    }
}
