use rusqlite::Connection;

/// V35: enforce at-most-one edge per `(src_id, dst_id, edge_type)` triple in
/// `memory_edges` (ADR-023 hygiene).
///
/// `memory_edges.edge_id` is the PRIMARY KEY, but it is an always-fresh random id
/// (`generate_id("edg")`), so the `INSERT OR IGNORE` in `add_edge` /
/// `supersede_claim` could never actually dedupe a *logically* identical edge:
/// a fresh `edge_id` never collides on the PK. Each daily belief-revision pass
/// that re-proposed the same `supports`/`refines` relation between two
/// persistently-`active` claims therefore inserted a brand-new duplicate row,
/// growing `memory_edges` unbounded within the retention window and making
/// `edges_from` return duplicates.
///
/// This migration (1) collapses any already-accumulated duplicates down to the
/// earliest-inserted row per triple, then (2) adds a UNIQUE index so the
/// existing `INSERT OR IGNORE` finally dedupes as it always intended. The
/// pre-existing non-unique `idx_memory_edges_src` / `idx_memory_edges_dst` are
/// deliberately kept — they serve the `(src_id, edge_type)` / `(dst_id,
/// edge_type)` lookups that the `(src_id, dst_id, edge_type)` unique index
/// cannot (`edge_type` is not a left-prefix column there).
pub(super) fn migrate_v35(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        // 1) Collapse pre-existing duplicates: keep the earliest (lowest rowid)
        //    row per logical edge so the original created_at/confidence wins.
        "DELETE FROM memory_edges
         WHERE rowid NOT IN (
             SELECT MIN(rowid) FROM memory_edges
             GROUP BY src_id, dst_id, edge_type
         );
         -- 2) Now the INSERT OR IGNORE in add_edge/supersede_claim dedupes.
         CREATE UNIQUE INDEX IF NOT EXISTS idx_memory_edges_unique
             ON memory_edges(src_id, dst_id, edge_type);
         INSERT OR IGNORE INTO schema_version (version) VALUES (35);",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// Create the v34-shaped `memory_edges` table + `schema_version` at v34 so
    /// the migration runs against a realistic prior schema.
    fn setup_v34(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version VALUES (34);
             CREATE TABLE memory_edges (
                edge_id      TEXT PRIMARY KEY,
                src_id       TEXT NOT NULL,
                dst_id       TEXT NOT NULL,
                edge_type    TEXT NOT NULL,
                confidence   REAL NOT NULL,
                evidence_ref TEXT,
                source       TEXT NOT NULL,
                created_at   INTEGER NOT NULL
             );
             CREATE INDEX idx_memory_edges_src ON memory_edges(src_id, edge_type);
             CREATE INDEX idx_memory_edges_dst ON memory_edges(dst_id, edge_type);",
        )
        .unwrap();
    }

    fn edge_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM memory_edges", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn migrate_v35_collapses_existing_duplicates_to_earliest() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v34(&conn);
        // Two logically-identical (clm_a → clm_b, supports) edges with distinct
        // edge_ids (the bug) + one distinct edge.
        conn.execute_batch(
            "INSERT INTO memory_edges VALUES ('edg_1','clm_a','clm_b','supports',0.7,NULL,'llm',100);
             INSERT INTO memory_edges VALUES ('edg_2','clm_a','clm_b','supports',0.9,NULL,'llm',200);
             INSERT INTO memory_edges VALUES ('edg_3','clm_a','clm_c','refines',0.5,NULL,'llm',300);",
        )
        .unwrap();
        assert_eq!(edge_count(&conn), 3, "precondition: a duplicate exists");

        migrate_v35(&conn).unwrap();

        assert_eq!(
            edge_count(&conn),
            2,
            "the duplicate triple collapses to one"
        );
        // The earliest row (edg_1, created_at=100) is the survivor.
        let (kept_id, kept_created): (String, i64) = conn
            .query_row(
                "SELECT edge_id, created_at FROM memory_edges \
                 WHERE src_id='clm_a' AND dst_id='clm_b' AND edge_type='supports'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(kept_id, "edg_1");
        assert_eq!(kept_created, 100);
    }

    #[test]
    fn migrate_v35_unique_index_makes_insert_or_ignore_dedupe() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v34(&conn);
        conn.execute_batch(
            "INSERT INTO memory_edges VALUES ('edg_1','clm_a','clm_b','supports',0.7,NULL,'llm',100);",
        )
        .unwrap();
        migrate_v35(&conn).unwrap();

        // A fresh edge_id for the SAME triple is now silently ignored.
        conn.execute_batch(
            "INSERT OR IGNORE INTO memory_edges \
             VALUES ('edg_dup','clm_a','clm_b','supports',0.99,NULL,'llm',999);",
        )
        .unwrap();
        assert_eq!(edge_count(&conn), 1, "same triple does not duplicate");

        // A different edge_type between the same pair is a distinct triple → kept.
        conn.execute_batch(
            "INSERT OR IGNORE INTO memory_edges \
             VALUES ('edg_c','clm_a','clm_b','contradicts',0.6,NULL,'llm',1000);",
        )
        .unwrap();
        assert_eq!(edge_count(&conn), 2, "a different edge_type is a new edge");
    }

    #[test]
    fn migrate_v35_records_version() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v34(&conn);
        migrate_v35(&conn).unwrap();

        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 35);
    }

    #[test]
    fn migrate_v35_unique_index_exists() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v34(&conn);
        migrate_v35(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type='index' AND name='idx_memory_edges_unique'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "the unique index should exist");
    }

    #[test]
    fn migrate_v35_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        setup_v34(&conn);
        migrate_v35(&conn).unwrap();
        migrate_v35(&conn).unwrap();
    }
}
