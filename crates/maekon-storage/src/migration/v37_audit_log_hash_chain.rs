//! Migration V37: add SHA-256 hash-chain columns to the `audit_log` table (#4834, E20).
//!
//! Client-side mirror of ADR-072 (server audit chain integrity policy); makes
//! `audit_log` rows tamper-**evident**. Each row gains the following:
//! - `seq`        — monotonically increasing sequence number within the chain
//!                  (assigned only to chained rows).
//! - `prev_hash`  — `entry_hash` of the immediately-prior chain row (genesis is
//!                  the 64-char zero hex).
//! - `entry_hash` — hex of `SHA256(prev_hash_bytes || canonical(record))`.
//!
//! SQLite `ALTER TABLE ADD COLUMN` can only add nullable columns, so all three
//! columns allow NULL. Legacy rows written before v37 remain with a NULL chain
//! (not part of the chain) and are not re-hashed — because the ordering/canonical
//! form used at write time is unknown. The genesis is formed at the first write
//! after this migration.
//!
//! `seq` carries a partial UNIQUE INDEX (`WHERE seq IS NOT NULL`) so that legacy
//! NULL rows are allowed while sequence-number uniqueness is still guaranteed for
//! chained rows.
//!
//! ## tamper-evident vs tamper-proof
//! A SHA-256-only chain detects accidental/partial corruption and simple row
//! edits, deletions, or reordering. It does NOT defend against an insider who can
//! recompute and rewrite the entire chain (full rewrite) — that requires an
//! HMAC/Ed25519 signature (out-of-scope; a future `hash_version` seam).

use rusqlite::Connection;

/// V37: audit_log hash-chain columns (seq/prev_hash/entry_hash) + partial unique index.
pub(super) fn migrate_v37(conn: &Connection) -> Result<(), rusqlite::Error> {
    // SQLite can add only one column per ALTER — run them as individual statements.
    conn.execute_batch(
        "ALTER TABLE audit_log ADD COLUMN seq INTEGER;
         ALTER TABLE audit_log ADD COLUMN prev_hash TEXT;
         ALTER TABLE audit_log ADD COLUMN entry_hash TEXT;
         CREATE UNIQUE INDEX IF NOT EXISTS idx_audit_log_seq
             ON audit_log(seq) WHERE seq IS NOT NULL;
         INSERT OR IGNORE INTO schema_version (version) VALUES (37);",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// Build the minimal v36-equivalent schema (audit_log + schema_version).
    fn setup_v36(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version VALUES (36);
             CREATE TABLE audit_log (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 entry_id TEXT NOT NULL,
                 timestamp TEXT NOT NULL,
                 session_id TEXT NOT NULL,
                 command_id TEXT NOT NULL,
                 action_type TEXT NOT NULL,
                 status TEXT NOT NULL,
                 details TEXT,
                 execution_time_ms INTEGER,
                 UNIQUE(entry_id)
             );",
        )
        .unwrap();
    }

    fn column_names(conn: &Connection) -> Vec<String> {
        let mut stmt = conn.prepare("PRAGMA table_info(audit_log)").unwrap();
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        rows
    }

    #[test]
    fn migrate_v37_adds_chain_columns() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v36(&conn);
        migrate_v37(&conn).unwrap();

        let cols = column_names(&conn);
        for c in ["seq", "prev_hash", "entry_hash"] {
            assert!(cols.iter().any(|x| x == c), "column {c} should be added");
        }
    }

    #[test]
    fn migrate_v37_creates_partial_unique_index() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v36(&conn);
        migrate_v37(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type='index' AND name='idx_audit_log_seq'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "idx_audit_log_seq partial unique index should exist"
        );
    }

    #[test]
    fn migrate_v37_records_version() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v36(&conn);
        migrate_v37(&conn).unwrap();
        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 37);
    }

    #[test]
    fn migrate_v37_legacy_rows_survive_with_null_chain() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v36(&conn);
        // Legacy row written before v37.
        conn.execute(
            "INSERT INTO audit_log \
             (entry_id, timestamp, session_id, command_id, action_type, status) \
             VALUES ('legacy-1', '2026-01-01T00:00:00Z', 's', 'c', 'a', 'Completed')",
            [],
        )
        .unwrap();

        migrate_v37(&conn).unwrap();

        // The legacy row should survive with NULL chain columns.
        let (seq, prev, hash): (Option<i64>, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT seq, prev_hash, entry_hash FROM audit_log WHERE entry_id = 'legacy-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(seq, None);
        assert_eq!(prev, None);
        assert_eq!(hash, None);
    }

    #[test]
    fn migrate_v37_idempotent_index() {
        // Column ADD is not idempotent, so re-running the full migration is not
        // guaranteed; we only verify the IF NOT EXISTS idempotency of the partial
        // unique index.
        let conn = Connection::open_in_memory().unwrap();
        setup_v36(&conn);
        migrate_v37(&conn).unwrap();
        conn.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_audit_log_seq \
             ON audit_log(seq) WHERE seq IS NOT NULL;",
        )
        .unwrap();
    }

    #[test]
    fn migrate_v37_seq_unique_allows_multiple_nulls() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v36(&conn);
        migrate_v37(&conn).unwrap();
        // Multiple NULL-seq rows should be allowed (partial index).
        conn.execute(
            "INSERT INTO audit_log (entry_id, timestamp, session_id, command_id, action_type, status) \
             VALUES ('l-1', 't', 's', 'c', 'a', 'Completed')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO audit_log (entry_id, timestamp, session_id, command_id, action_type, status) \
             VALUES ('l-2', 't', 's', 'c', 'a', 'Completed')",
            [],
        )
        .unwrap();
        // Two identical non-NULL seq values should be rejected.
        conn.execute("UPDATE audit_log SET seq = 1 WHERE entry_id = 'l-1'", [])
            .unwrap();
        let dup_err = conn
            .execute("UPDATE audit_log SET seq = 1 WHERE entry_id = 'l-2'", [])
            .unwrap_err();
        let dup_msg = dup_err.to_string().to_uppercase();
        assert!(
            dup_msg.contains("UNIQUE"),
            "duplicate seq should be rejected by the partial unique index; got: {dup_err}"
        );
    }
}
