//! V46: restart-surviving coaching `AdaptiveScorer` weights (#8058 P2-1).
//!
//! A singleton table (`id = 0`, mirroring `regime_manager_state`) holding the
//! coaching engine's online-logistic-regression model: the per-feature weight
//! vector (JSON-encoded so a future feature-count change is a data concern, not a
//! schema migration), the bias, and `train_count`. This was the LAST
//! feedback-learning component that reset on restart — its sibling stores
//! (`coaching_effectiveness` V17, `feedback_scorer_tallies`/`regime_reaction_stats`
//! V44) already persist. Because the scorer needs `MIN_TRAINING_SAMPLES` (50)
//! updates before `is_ready()`, losing it on every exit meant a user restarting
//! this 24/7 daemon mid-warm-up could never reach the ML gate.
//!
//! User-derived LEARNING state, so it is erased with activity data (added to
//! `ALL_TABLES` in maintenance/retention.rs), like the sibling learning tables.
//!
//! Renumbered V45 -> V46 (#8058) after #8073 (`v45_drop_search_trigram.rs`)
//! landed first and claimed V45.

use rusqlite::Connection;

pub(super) fn migrate_v46(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS adaptive_scorer_state (
            id           INTEGER PRIMARY KEY CHECK (id = 0),
            weights_json TEXT NOT NULL,
            bias         REAL NOT NULL,
            train_count  INTEGER NOT NULL,
            updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
        );

        INSERT OR IGNORE INTO schema_version (version) VALUES (46);",
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
             INSERT INTO schema_version VALUES (45);",
        )
        .unwrap();
    }

    #[test]
    fn migrate_v46_creates_adaptive_scorer_state_table() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v46(&conn).unwrap();

        conn.execute(
            "INSERT INTO adaptive_scorer_state (id, weights_json, bias, train_count)
             VALUES (0, '[0.0,0.0]', 0.0, 51)",
            [],
        )
        .unwrap();

        let train_count: u32 = conn
            .query_row(
                "SELECT train_count FROM adaptive_scorer_state WHERE id = 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(train_count, 51);
    }

    #[test]
    fn migrate_v46_enforces_singleton_via_check_constraint() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v46(&conn).unwrap();

        conn.execute(
            "INSERT INTO adaptive_scorer_state (id, weights_json, bias, train_count)
             VALUES (0, '[]', 0.0, 0)",
            [],
        )
        .unwrap();

        let err = conn
            .execute(
                "INSERT INTO adaptive_scorer_state (id, weights_json, bias, train_count)
                 VALUES (1, '[]', 0.0, 0)",
                [],
            )
            .unwrap_err();
        assert!(
            err.to_string().contains("CHECK"),
            "expected CHECK constraint failure, got: {err}"
        );
    }

    #[test]
    fn migrate_v46_records_version() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v46(&conn).unwrap();

        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 46);
    }

    #[test]
    fn migrate_v46_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v46(&conn).unwrap();
        migrate_v46(&conn).unwrap();
    }
}
