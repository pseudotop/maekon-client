//! V49: durable task lifecycle tables (ADR-028, #8577).
//!
//! Five additive tables backing user-confirmed tasks derived from reviewable
//! candidates. FK `REFERENCES` clauses are documentation only — this codebase
//! never enables the `foreign_keys` PRAGMA (ADR-028 Amendment B3), so cascade,
//! retention, and erasure are application-enforced child-first deletes. The
//! `UNIQUE` and `CHECK` constraints below ARE engine-enforced and carry the
//! durable invariants (one candidate -> one to-do, dedupe uniqueness, idempotent
//! receipts, no self-blocker edge).

use rusqlite::Connection;

pub(super) fn migrate_v49(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS task_candidates (
            id                 TEXT PRIMARY KEY,
            state              TEXT NOT NULL
                                 CHECK (state IN ('PROPOSED','CONFIRMED','DISMISSED','EXPIRED')),
            title              TEXT,
            body               TEXT,
            proposed_due       TEXT,
            proposed_owner_ref TEXT,
            expires_at         TEXT NOT NULL,
            dedupe_key         TEXT NOT NULL UNIQUE,
            revision           INTEGER NOT NULL DEFAULT 1,
            created_at         TEXT NOT NULL,
            updated_at         TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS task_source_refs (
            candidate_id        TEXT PRIMARY KEY
                                  REFERENCES task_candidates(id) ON DELETE CASCADE,
            source_kind         TEXT NOT NULL,
            extension_id        TEXT,
            install_id          TEXT,
            account_subject_ref TEXT,
            upstream_object_id  TEXT,
            upstream_revision   TEXT,
            upstream_etag       TEXT,
            occurred_at         TEXT,
            observed_at         TEXT NOT NULL,
            dedupe_namespace    TEXT NOT NULL,
            content_hash        TEXT NOT NULL,
            lifecycle           TEXT NOT NULL DEFAULT 'ACTIVE',
            source_outcome      TEXT
        );

        CREATE TABLE IF NOT EXISTS todo_items (
            id                  TEXT PRIMARY KEY,
            state               TEXT NOT NULL
                                  CHECK (state IN ('CONFIRMED','IN_PROGRESS','WAITING','DONE','CANCELLED')),
            title               TEXT NOT NULL,
            body                TEXT,
            due                 TEXT,
            owner_ref           TEXT,
            origin_candidate_id TEXT NOT NULL UNIQUE
                                  REFERENCES task_candidates(id) ON DELETE CASCADE,
            supersedes_todo_id  TEXT,
            revision            INTEGER NOT NULL DEFAULT 1,
            created_at          TEXT NOT NULL,
            updated_at          TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS todo_blockers (
            blocked_todo_id TEXT NOT NULL REFERENCES todo_items(id) ON DELETE CASCADE,
            blocker_todo_id TEXT NOT NULL REFERENCES todo_items(id) ON DELETE CASCADE,
            created_at      TEXT NOT NULL,
            PRIMARY KEY (blocked_todo_id, blocker_todo_id),
            CHECK (blocked_todo_id <> blocker_todo_id)
        );

        CREATE TABLE IF NOT EXISTS task_transition_receipts (
            id                  TEXT PRIMARY KEY,
            entity_kind         TEXT NOT NULL
                                  CHECK (entity_kind IN ('CANDIDATE','TODO','TODO_BLOCKER')),
            entity_id           TEXT NOT NULL,
            idempotency_key     TEXT NOT NULL,
            request_hash        TEXT NOT NULL,
            from_state          TEXT NOT NULL,
            to_state            TEXT NOT NULL,
            resulting_revision  INTEGER NOT NULL,
            resulting_entity_id TEXT,
            created_at          TEXT NOT NULL,
            UNIQUE (entity_kind, entity_id, idempotency_key)
        );

        CREATE INDEX IF NOT EXISTS idx_task_candidates_state_expiry
            ON task_candidates(state, expires_at);
        CREATE INDEX IF NOT EXISTS idx_todo_items_state_updated
            ON todo_items(state, updated_at);
        CREATE INDEX IF NOT EXISTS idx_task_source_refs_object_rev
            ON task_source_refs(upstream_object_id, upstream_revision);
        CREATE INDEX IF NOT EXISTS idx_todo_blockers_blocker
            ON todo_blockers(blocker_todo_id);

        INSERT OR IGNORE INTO schema_version (version) VALUES (49);",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_v48(conn: &Connection) {
        // Match production: the shared SqliteStorage connection runs with
        // `PRAGMA foreign_keys` OFF (ADR-028 Amendment B3), so these tests must
        // too, otherwise a bundled-rusqlite default of ON would mask the
        // engine-enforced UNIQUE/CHECK constraints we actually assert here.
        conn.execute_batch(
            "PRAGMA foreign_keys=OFF;
             CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version VALUES (48);",
        )
        .unwrap();
    }

    #[test]
    fn migration_creates_tables_and_records_version() {
        let conn = Connection::open_in_memory().unwrap();
        base_v48(&conn);
        migrate_v49(&conn).unwrap();

        for table in [
            "task_candidates",
            "task_source_refs",
            "todo_items",
            "todo_blockers",
            "task_transition_receipts",
        ] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "table {table} must exist after v49");
        }
        assert_eq!(
            conn.query_row("SELECT MAX(version) FROM schema_version", [], |r| r
                .get::<_, u32>(0))
                .unwrap(),
            49
        );
    }

    #[test]
    fn candidate_state_check_rejects_unknown() {
        let conn = Connection::open_in_memory().unwrap();
        base_v48(&conn);
        migrate_v49(&conn).unwrap();
        let err = conn
            .execute(
                "INSERT INTO task_candidates
                 (id, state, expires_at, dedupe_key, revision, created_at, updated_at)
                 VALUES ('tcand_1','BOGUS','2026-07-30T00:00:00Z','k1',1,'t','t')",
                [],
            )
            .unwrap_err();
        assert!(err.to_string().contains("CHECK constraint failed"));
    }

    #[test]
    fn dedupe_key_is_unique() {
        let conn = Connection::open_in_memory().unwrap();
        base_v48(&conn);
        migrate_v49(&conn).unwrap();
        conn.execute(
            "INSERT INTO task_candidates
             (id, state, expires_at, dedupe_key, revision, created_at, updated_at)
             VALUES ('tcand_1','PROPOSED','2026-07-30T00:00:00Z','dk',1,'t','t')",
            [],
        )
        .unwrap();
        let err = conn
            .execute(
                "INSERT INTO task_candidates
                 (id, state, expires_at, dedupe_key, revision, created_at, updated_at)
                 VALUES ('tcand_2','PROPOSED','2026-07-30T00:00:00Z','dk',1,'t','t')",
                [],
            )
            .unwrap_err();
        assert!(err.to_string().contains("UNIQUE constraint failed"));
    }

    #[test]
    fn origin_candidate_id_is_unique_one_todo_per_candidate() {
        let conn = Connection::open_in_memory().unwrap();
        base_v48(&conn);
        migrate_v49(&conn).unwrap();
        conn.execute(
            "INSERT INTO todo_items (id, state, title, origin_candidate_id, revision, created_at, updated_at)
             VALUES ('todo_1','CONFIRMED','t','tcand_1',1,'t','t')",
            [],
        )
        .unwrap();
        let err = conn
            .execute(
                "INSERT INTO todo_items (id, state, title, origin_candidate_id, revision, created_at, updated_at)
                 VALUES ('todo_2','CONFIRMED','t','tcand_1',1,'t','t')",
                [],
            )
            .unwrap_err();
        assert!(err.to_string().contains("UNIQUE constraint failed"));
    }

    #[test]
    fn blocker_rejects_self_edge_and_dupes() {
        let conn = Connection::open_in_memory().unwrap();
        base_v48(&conn);
        migrate_v49(&conn).unwrap();
        let self_err = conn
            .execute(
                "INSERT INTO todo_blockers (blocked_todo_id, blocker_todo_id, created_at)
                 VALUES ('todo_1','todo_1','t')",
                [],
            )
            .unwrap_err();
        assert!(self_err.to_string().contains("CHECK constraint failed"));

        conn.execute(
            "INSERT INTO todo_blockers (blocked_todo_id, blocker_todo_id, created_at)
             VALUES ('todo_1','todo_2','t')",
            [],
        )
        .unwrap();
        let dup_err = conn
            .execute(
                "INSERT INTO todo_blockers (blocked_todo_id, blocker_todo_id, created_at)
                 VALUES ('todo_1','todo_2','t')",
                [],
            )
            .unwrap_err();
        assert!(dup_err.to_string().contains("UNIQUE constraint failed"));
    }

    #[test]
    fn receipt_tuple_is_unique() {
        let conn = Connection::open_in_memory().unwrap();
        base_v48(&conn);
        migrate_v49(&conn).unwrap();
        let insert = |id: &str| {
            conn.execute(
                "INSERT INTO task_transition_receipts
                 (id, entity_kind, entity_id, idempotency_key, request_hash, from_state, to_state, resulting_revision, created_at)
                 VALUES (?1,'CANDIDATE','tcand_1','key-1','h','PROPOSED','CONFIRMED',2,'t')",
                [id],
            )
        };
        insert("tmut_1").unwrap();
        let err = insert("tmut_2").unwrap_err();
        assert!(err.to_string().contains("UNIQUE constraint failed"));
    }
}
