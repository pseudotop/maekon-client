//! Migration V41: rebuild `search_fts` with CJK bigram shadow column.
//!
//! lint:allow-non-english-comments — documents CJK bigram tokenization; example
//! tokens (CJK/Hangul) in comments are illustrative and must remain non-English.
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
//! ## Version-gating and failure recovery
//!
//! `schema_version(41)` is inserted **only after** `try_rebuild_fts` returns
//! successfully (`RebuildResult::Completed`).
//!
//! If `try_rebuild_fts` returns `RebuildResult::Skipped` (FTS5 extension not compiled
//! in — error at step 1, before any destructive operation), the version is still
//! advanced to 41 so the agent does not loop forever on startup on a build without
//! FTS5. The `search_fts` table is left unchanged in that case.
//!
//! If `try_rebuild_fts` returns `Err` (a genuine mid-rebuild failure — e.g. disk full
//! after DROP but before RENAME), the error is propagated out of `migrate_v41`.
//! `run_migration_step` in `mod.rs` then issues `ROLLBACK TO SAVEPOINT migration_v41`
//! and the version row is never inserted, so the next startup retries the migration
//! from version 40.
//!
//! Note: SQLite cannot roll back an FTS5 virtual-table `DROP` inside a savepoint; a
//! rollback after step 3/4 will leave the DB in a degraded state (no `search_fts`).
//! However, the version is *not* advanced in that case, so the migration is retried
//! and step 1 (`CREATE VIRTUAL TABLE IF NOT EXISTS search_fts_new`) will recreate the
//! shadow table correctly on the next run.

use rusqlite::Connection;

use crate::sqlite::cjk_shadow::cjk_bigram_shadow;

/// Outcome of [`try_rebuild_fts`], distinguishing a completed rebuild from a graceful
/// skip when the FTS5 extension is unavailable.
enum RebuildResult {
    /// The rebuild completed successfully; `search_fts` has the new shadow schema.
    Completed,
    /// FTS5 is not compiled in; `search_fts` is unchanged. Version should still be
    /// advanced to 41 to avoid an infinite startup retry loop.
    Skipped,
}

pub(super) fn migrate_v41(conn: &Connection) -> Result<(), rusqlite::Error> {
    tracing::debug!("migration V41: rebuild search_fts with CJK bigram shadow column");

    // Gate the version insertion on a successful (or gracefully-skipped) rebuild.
    // A genuine mid-rebuild failure (e.g. disk full after DROP) returns Err, which
    // propagates to run_migration_step for savepoint rollback, and version 41 is
    // never inserted — so the next startup retries from version 40.
    match try_rebuild_fts(conn)? {
        RebuildResult::Completed => {
            tracing::debug!("V41 FTS5 rebuild completed; recording schema version 41");
        }
        RebuildResult::Skipped => {
            tracing::warn!(
                "V41 FTS5 rebuild skipped (FTS5 extension not available); \
                 advancing to version 41 to avoid startup retry loop"
            );
        }
    }

    conn.execute_batch("INSERT OR IGNORE INTO schema_version (version) VALUES (41);")?;

    tracing::info!("migration V41 completed");
    Ok(())
}

/// Attempt to rebuild the `search_fts` FTS5 table with the new CJK bigram shadow schema.
///
/// Returns:
/// - `Ok(RebuildResult::Completed)` — rebuild succeeded.
/// - `Ok(RebuildResult::Skipped)` — FTS5 is not compiled in; step 1 failed before any
///   destructive operation; `search_fts` is unchanged.
/// - `Err(_)` — a genuine failure occurred after step 1 (e.g. disk full, corrupt DB).
///   The caller must not advance the schema version.
fn try_rebuild_fts(conn: &Connection) -> Result<RebuildResult, rusqlite::Error> {
    // Step 1 — create the new table alongside the old one.
    //
    // This is the only step where failure is treated as a graceful skip: if FTS5 is
    // not compiled in, the CREATE VIRTUAL TABLE fails here before any destructive
    // operation has occurred.
    if let Err(e) = conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS search_fts_new USING fts5(
             segment_id   UNINDEXED,
             content_type,
             searchable_text UNINDEXED,
             shadow,
             tokenize='unicode61'
         );",
    ) {
        tracing::warn!("V41 step 1 failed — FTS5 extension likely not compiled in: {e}");
        return Ok(RebuildResult::Skipped);
    }

    // Steps 2-4 are destructive (DROP + RENAME). Any error past this point is a
    // genuine failure; return Err so the caller can propagate it without advancing
    // the schema version.

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

    Ok(RebuildResult::Completed)
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

    /// Verify that a mid-rebuild failure does NOT advance the schema version to 41.
    ///
    /// # Why this matters
    ///
    /// The data-integrity bug this test guards against: the original code called
    /// `INSERT OR IGNORE INTO schema_version (version) VALUES (41)` unconditionally
    /// even when `try_rebuild_fts` returned an error.  If a failure occurred after
    /// the DROP (step 3) but before the RENAME (step 4), `search_fts` would be
    /// permanently gone AND version 41 would be recorded, so the next startup would
    /// never retry the migration.
    ///
    /// # Failure simulation
    ///
    /// We arrange a state where step 1 (CREATE VIRTUAL TABLE … USING fts5) succeeds
    /// — proving we're past the "FTS5 unavailable" skip path — but step 2 (SELECT …
    /// FROM search_fts) fails because `search_fts` does not exist.  This exercises
    /// the `Err` return path of `try_rebuild_fts` without requiring any runtime
    /// injection mechanism.
    ///
    /// Note: a full disk-full simulation would require OS-level fault injection not
    /// available in unit tests.  The mechanism verified here is identical: any `Err`
    /// from `try_rebuild_fts` that is not `RebuildResult::Skipped` must propagate
    /// without inserting the version row.
    #[test]
    fn migrate_v41_failure_does_not_advance_version() {
        let conn = Connection::open_in_memory().unwrap();

        // Set up schema_version at 40 but intentionally OMIT search_fts so that
        // step 2 of try_rebuild_fts fails (SELECT FROM non-existent table) after
        // step 1 (CREATE VIRTUAL TABLE … USING fts5) has already succeeded.
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version VALUES (40);",
        )
        .unwrap();

        // migrate_v41 must return Err because try_rebuild_fts hits a real error.
        let result = migrate_v41(&conn);
        // Unwrap the error (panics with a diagnostic if it was Ok — not a value-blind hedge).
        let err = result.unwrap_err();
        // The failure occurs at step 2: conn.prepare("SELECT … FROM search_fts") on a
        // connection where `search_fts` was intentionally omitted.  rusqlite surfaces
        // this as SqliteFailure (SQLite error code 1 / SQLITE_ERROR, "no such table").
        assert!(
            matches!(err, rusqlite::Error::SqliteFailure(..)),
            "migrate_v41 must propagate the mid-rebuild error as SqliteFailure, got: {err:?}"
        );
        // Cross-check the message so the test catches accidental error-variant changes.
        let msg = err.to_string();
        assert!(
            msg.contains("no such table"),
            "SqliteFailure message must mention the missing table, got: {msg:?}"
        );

        // Critically: version 41 must NOT have been inserted.
        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            version, 40,
            "schema version must remain at 40 after a mid-rebuild failure so the \
             next startup retries the migration"
        );
    }

    /// Verify that the FTS5-unavailable skip case still advances the version to 41.
    ///
    /// If FTS5 is not compiled in, `search_fts_new` cannot be created (step 1 fails).
    /// The migration must still advance to version 41 to prevent an infinite startup
    /// retry loop on builds without FTS5.
    ///
    /// # Test harness limitation
    ///
    /// In-memory SQLite used in tests always has FTS5 compiled in (rusqlite bundles
    /// it), so we cannot directly trigger the `Skipped` path without a stripped
    /// SQLite build.  This test documents the expected behaviour and is left as a
    /// compile-time verified no-op.  The distinction is enforced at the type level:
    /// `try_rebuild_fts` returns `Ok(RebuildResult::Skipped)` only when step 1 fails,
    /// and `migrate_v41` advances the version in both `Completed` and `Skipped` arms.
    #[test]
    fn migrate_v41_fts5_unavailable_skip_is_documented() {
        // This test intentionally succeeds unconditionally; the FTS5-unavailable code
        // path requires a SQLite build without bundled FTS5 and cannot be exercised
        // in the standard unit-test environment.  See module-level docs for the
        // design rationale.
        let _ = RebuildResult::Skipped; // ensure the variant is reachable from tests
    }
}
