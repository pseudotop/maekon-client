//! Migration V41: rebuild `search_fts` with CJK bigram shadow column.
//!
//! ## What changes
//!
//! The V11 schema used `tokenize='porter unicode61'`. This migration:
//!
//! 1. Drops the porter tokenizer in favour of `tokenize='unicode61'`.
//!    Porter is intentionally removed: applying English stemming to CJK bigrams would
//!    corrupt them (e.g. "売上" → unpredictable stem). The spike-validated R@3 figures
//!    use unicode61; no regression on the en/ko/lex baselines was observed.
//!
//! 2. Adds a `shadow` column (FTS-indexed) that holds the CJK-bigram expansion of
//!    `searchable_text`. Queries are issued against `shadow` rather than
//!    `searchable_text`.
//!
//! 3. Marks `searchable_text` as `UNINDEXED` (display / `matched_text` return path
//!    only). `content_type` remains indexed.
//!
//! ## Why a full rebuild rather than ALTER TABLE
//!
//! FTS5 virtual tables do not support `ALTER TABLE … ADD COLUMN` or schema changes via
//! `INSERT INTO <fts>(<fts>) VALUES('rebuild')`. The only supported path to changing
//! the column list or tokenizer of an existing FTS5 table is:
//! new-table → copy data → drop old → rename.
//!
//! ## Migration strategy
//!
//! All steps run inside the caller-supplied `SAVEPOINT migration_v41` (managed by
//! `run_migration_step` in `mod.rs`):
//!
//! 1. Create `search_fts_new` with the new schema.
//! 2. For each existing row, compute `cjk_bigram_shadow(searchable_text)` in Rust
//!    and insert into `search_fts_new`.
//! 3. Drop `search_fts`.
//! 4. Rename `search_fts_new` → `search_fts`.
//! 5. Record version 41.
//!
//! If FTS5 is not available (extension not compiled in), the whole step is skipped
//! with a warning and the `search_fts` table is left unchanged — identical graceful
//! pattern as V11/V18.

use rusqlite::Connection;

use crate::sqlite::cjk_shadow::cjk_bigram_shadow;

pub(super) fn migrate_v41(conn: &Connection) -> Result<(), rusqlite::Error> {
    tracing::debug!("migration V41: rebuild search_fts with CJK bigram shadow column");

    // FTS5 may not be available on all SQLite builds (e.g. stripped CI images).
    // Mirror the V11 graceful-skip pattern.
    let fts5_result = try_rebuild_fts(conn);
    if let Err(e) = fts5_result {
        tracing::warn!("V41 FTS5 rebuild skipped (FTS5 may not be available): {e}");
    }

    conn.execute_batch("INSERT OR IGNORE INTO schema_version (version) VALUES (41);")?;

    tracing::info!("migration V41 completed");
    Ok(())
}

fn try_rebuild_fts(conn: &Connection) -> Result<(), rusqlite::Error> {
    // Step 1 — create the new table alongside the old one.
    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS search_fts_new USING fts5(
             segment_id   UNINDEXED,
             content_type,
             searchable_text UNINDEXED,
             shadow,
             tokenize='unicode61'
         );",
    )?;

    // Step 2 — copy existing rows with computed shadow values.
    //
    // We read all rows first (collect to avoid holding a prepared statement and a
    // write statement simultaneously on the single connection).
    let rows: Vec<(String, String, String)> = {
        let mut stmt =
            conn.prepare("SELECT segment_id, content_type, searchable_text FROM search_fts")?;
        let result = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .collect();
        result
    };

    // Backfill rows into the new table. Each row's shadow is computed inline.
    for (segment_id, content_type, searchable_text) in &rows {
        let shadow = cjk_bigram_shadow(searchable_text);
        conn.execute(
            "INSERT INTO search_fts_new (segment_id, content_type, searchable_text, shadow)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![segment_id, content_type, searchable_text, shadow],
        )?;
    }

    // Step 3 — drop the old table.
    conn.execute_batch("DROP TABLE IF EXISTS search_fts;")?;

    // Step 4 — rename.
    conn.execute_batch("ALTER TABLE search_fts_new RENAME TO search_fts;")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// Build a minimal v40-equivalent schema: just `schema_version` + `search_fts`
    /// with the V11 porter tokenizer shape and some seed rows.
    fn setup_v40_with_fts(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version VALUES (40);

             CREATE VIRTUAL TABLE search_fts USING fts5(
                 segment_id UNINDEXED,
                 content_type,
                 searchable_text,
                 tokenize='porter unicode61'
             );

             INSERT INTO search_fts (segment_id, content_type, searchable_text)
             VALUES
                 ('seg-ja-1', 'segment', '月次売上レポートの集計'),
                 ('seg-ko-1', 'segment', '월별 급여 보고서'),
                 ('seg-en-1', 'segment', 'authentication module deep work');",
        )
        .unwrap();
    }

    #[test]
    fn migrate_v41_shadow_column_populated() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v40_with_fts(&conn);
        migrate_v41(&conn).unwrap();

        // Verify the shadow column was computed for the Japanese row.
        let shadow: String = conn
            .query_row(
                "SELECT shadow FROM search_fts WHERE segment_id = 'seg-ja-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(
            shadow, "月次 次売 売上 上レ レポ ポー ート トの の集 集計",
            "Japanese text must produce bigram shadow"
        );
    }

    #[test]
    fn migrate_v41_ja_text_matches_via_shadow() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v40_with_fts(&conn);
        migrate_v41(&conn).unwrap();

        // A ja bigram query ("売上") must now match the indexed row.
        // This test case documents the core value of the migration: before V41 this
        // query returned 0 rows; after V41 it returns 1.
        let count: i64 = conn
            .query_row(
                r#"SELECT COUNT(*) FROM search_fts WHERE shadow MATCH '"売上"'"#,
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(count, 1, "Japanese bigram query must match the indexed row");
    }

    #[test]
    fn migrate_v41_ko_text_matches_via_shadow() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v40_with_fts(&conn);
        migrate_v41(&conn).unwrap();

        // "급여" is a 2-char Korean run that passes through as a single bigram token.
        let count: i64 = conn
            .query_row(
                r#"SELECT COUNT(*) FROM search_fts WHERE shadow MATCH '"급여"'"#,
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(count, 1, "Korean bigram query must match the indexed row");
    }

    #[test]
    fn migrate_v41_en_query_still_matches() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v40_with_fts(&conn);
        migrate_v41(&conn).unwrap();

        // English text in shadow is unchanged (non-CJK passthrough).
        let count: i64 = conn
            .query_row(
                r#"SELECT COUNT(*) FROM search_fts WHERE shadow MATCH '"authentication"'"#,
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(
            count, 1,
            "English query must still match after V41 migration"
        );
    }

    #[test]
    fn migrate_v41_records_version() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v40_with_fts(&conn);
        migrate_v41(&conn).unwrap();

        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();

        assert_eq!(version, 41);
    }

    #[test]
    fn migrate_v41_searchable_text_preserved() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v40_with_fts(&conn);
        migrate_v41(&conn).unwrap();

        // `searchable_text` is UNINDEXED but must still be readable for the
        // `matched_text` return path in `search_fts` queries.
        let text: String = conn
            .query_row(
                "SELECT searchable_text FROM search_fts WHERE segment_id = 'seg-ja-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(
            text, "月次売上レポートの集計",
            "original searchable_text must be preserved as UNINDEXED column"
        );
    }

    #[test]
    fn migrate_v41_row_count_preserved() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v40_with_fts(&conn);
        migrate_v41(&conn).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM search_fts", [], |row| row.get(0))
            .unwrap();

        assert_eq!(count, 3, "all 3 seed rows must survive the migration");
    }

    #[test]
    fn migrate_v41_idempotent_version_insert() {
        // Running the migration twice must not fail (INSERT OR IGNORE).
        let conn = Connection::open_in_memory().unwrap();
        setup_v40_with_fts(&conn);
        migrate_v41(&conn).unwrap();
        // Second call: search_fts_new already matches the new schema name after rename;
        // the CREATE TABLE IF NOT EXISTS and DROP produce a clean slate again.
        // This is idempotency at the version-record level; full re-run is handled by
        // `run_migrations` guard (`if current < 41`).
        migrate_v41(&conn).unwrap();

        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 41);
    }
}
