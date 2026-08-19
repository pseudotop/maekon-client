use rusqlite::Connection;

/// V44: restart-surviving feedback-learning tables (#7913 T2.1c).
///
/// Two INDEPENDENT stores — deliberately NOT unified (#7600 rejected correlating
/// suggestion-keyed and profile/trigger-keyed feedback):
///
/// - `feedback_scorer_tallies`: `FeedbackScorer`'s per-(suggestion_type, source)
///   accept/reject/defer counts. `last_updated` is persisted so the scorer's 12h
///   self-decay is WALL-CLOCK-anchored across restarts (a tally aged past the
///   window while the process was down reads as decayed, rather than resetting
///   the decay clock on every launch).
/// - `regime_reaction_stats`: `RegimeClassifier`'s per-regime (and aggregate,
///   under the reserved `regime_id = ''` sentinel) user-reaction counts (#7600).
///
/// Both are user-derived LEARNING state, so they are erased with activity data
/// (added to `ALL_TABLES` in maintenance/retention.rs), like the sibling
/// `coaching_effectiveness`.
pub(super) fn migrate_v44(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS feedback_scorer_tallies (
            suggestion_type TEXT NOT NULL,
            source          TEXT NOT NULL,
            accepted        INTEGER NOT NULL DEFAULT 0,
            rejected        INTEGER NOT NULL DEFAULT 0,
            deferred        INTEGER NOT NULL DEFAULT 0,
            last_updated    TEXT NOT NULL,
            PRIMARY KEY (suggestion_type, source)
        );

        CREATE TABLE IF NOT EXISTS regime_reaction_stats (
            regime_id  TEXT PRIMARY KEY,
            total      INTEGER NOT NULL DEFAULT 0,
            accepted   INTEGER NOT NULL DEFAULT 0,
            rejected   INTEGER NOT NULL DEFAULT 0,
            deferred   INTEGER NOT NULL DEFAULT 0
        );

        INSERT OR IGNORE INTO schema_version (version) VALUES (44);",
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
             INSERT INTO schema_version VALUES (43);",
        )
        .unwrap();
    }

    #[test]
    fn migrate_v44_creates_both_tables() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v44(&conn).unwrap();

        conn.execute(
            "INSERT INTO feedback_scorer_tallies
                (suggestion_type, source, accepted, rejected, deferred, last_updated)
             VALUES ('WORK_GUIDANCE', 'RULE_BASED', 3, 1, 0, '2026-07-08T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO regime_reaction_stats (regime_id, total, accepted, rejected, deferred)
             VALUES ('regime-3', 5, 4, 1, 0)",
            [],
        )
        .unwrap();

        let tallies: i64 = conn
            .query_row("SELECT COUNT(*) FROM feedback_scorer_tallies", [], |r| {
                r.get(0)
            })
            .unwrap();
        let reactions: i64 = conn
            .query_row("SELECT COUNT(*) FROM regime_reaction_stats", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(tallies, 1);
        assert_eq!(reactions, 1);
    }

    #[test]
    fn migrate_v44_tally_pk_dedupes_by_type_source() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v44(&conn).unwrap();

        conn.execute(
            "INSERT INTO feedback_scorer_tallies
                (suggestion_type, source, accepted, rejected, deferred, last_updated)
             VALUES ('WORK_GUIDANCE', 'RULE_BASED', 3, 1, 0, '2026-07-08T00:00:00Z')",
            [],
        )
        .unwrap();
        // Same (type, source) via a second plain INSERT must violate the PK.
        let err = conn
            .execute(
                "INSERT INTO feedback_scorer_tallies
                    (suggestion_type, source, accepted, rejected, deferred, last_updated)
                 VALUES ('WORK_GUIDANCE', 'RULE_BASED', 9, 9, 9, '2026-07-08T01:00:00Z')",
                [],
            )
            .unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("unique"),
            "PRIMARY KEY(suggestion_type, source) must reject a duplicate, got: {err}"
        );
    }

    #[test]
    fn migrate_v44_records_version() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v44(&conn).unwrap();

        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 44);
    }

    #[test]
    fn migrate_v44_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v44(&conn).unwrap();
        migrate_v44(&conn).unwrap();
    }
}
