use rusqlite::Connection;

/// V34: Create `memory_claims` + `memory_edges` tables for the ADR-023 local
/// symbolic memory-graph substrate.
///
/// `created_at`/`updated_at` are epoch-second INTEGERs per ADR-023 (intentional
/// divergence from the TEXT/RFC3339 timestamp convention in some sibling tables).
pub(super) fn migrate_v34(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS memory_claims (
            claim_id     TEXT PRIMARY KEY,
            kind         TEXT NOT NULL, -- semantic|episodic|procedural|reflective (D5)
            text         TEXT NOT NULL,
            source       TEXT NOT NULL,
            confidence   REAL NOT NULL,
            status       TEXT NOT NULL, -- active|superseded|retracted
            created_at   INTEGER NOT NULL,
            updated_at   INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS memory_edges (
            edge_id      TEXT PRIMARY KEY,
            src_id       TEXT NOT NULL,
            dst_id       TEXT NOT NULL,
            edge_type    TEXT NOT NULL, -- evidence|associated(box5)|supports|refines|contradicts|supersedes
            confidence   REAL NOT NULL,
            evidence_ref TEXT,
            source       TEXT NOT NULL,
            created_at   INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_memory_edges_src ON memory_edges(src_id, edge_type);
        CREATE INDEX IF NOT EXISTS idx_memory_edges_dst ON memory_edges(dst_id, edge_type);
        CREATE INDEX IF NOT EXISTS idx_memory_claims_status ON memory_claims(status);
        INSERT OR IGNORE INTO schema_version (version) VALUES (34);",
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
             INSERT INTO schema_version VALUES (33);",
        )
        .unwrap();
    }

    #[test]
    fn migrate_v34_creates_memory_graph_tables() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v34(&conn).unwrap();

        conn.execute(
            "INSERT INTO memory_claims
             (claim_id, kind, text, source, confidence, status, created_at, updated_at)
             VALUES ('clm-1', 'reflective', 'note', 'digest_highlight', 0.8, 'active', 1700000000, 1700000000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO memory_edges
             (edge_id, src_id, dst_id, edge_type, confidence, evidence_ref, source, created_at)
             VALUES ('edg-1', 'clm-1', 'ses-42', 'evidence', 1.0, 'ses-42', 'rule', 1700000000)",
            [],
        )
        .unwrap();

        let kind: String = conn
            .query_row(
                "SELECT kind FROM memory_claims WHERE claim_id = 'clm-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(kind, "reflective");
        let edge_type: String = conn
            .query_row(
                "SELECT edge_type FROM memory_edges WHERE edge_id = 'edg-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(edge_type, "evidence");
    }

    #[test]
    fn migrate_v34_records_version() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v34(&conn).unwrap();

        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 34);
    }

    #[test]
    fn migrate_v34_idempotent_with_if_not_exists() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v34(&conn).unwrap();
        migrate_v34(&conn).unwrap();
    }

    #[test]
    fn migrate_v34_indexes_exist() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        migrate_v34(&conn).unwrap();

        for idx in [
            "idx_memory_edges_src",
            "idx_memory_edges_dst",
            "idx_memory_claims_status",
        ] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
                    [idx],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "index {idx} should exist");
        }
    }
}
