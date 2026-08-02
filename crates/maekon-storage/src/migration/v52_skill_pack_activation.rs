//! V52: Skill Pack catalog + activation tables (ADR-029 §2/§6, #8588).
//!
//! v51 is reserved for the MK-CONTEXT work-context ledger (#8587/#8589), which
//! is landing concurrently; this slice takes v52 to avoid a version collision.
//!
//! Three additive tables:
//!
//! * `skill_pack_catalog` — one row per `skill_pack` contribution on a verified
//!   package. Stores the expected body digest, never the body: the body lives
//!   with the package artifact and is re-hashed at every activation, so a file
//!   swapped after registration cannot be activated.
//! * `skill_pack_activation` — the singleton explicit selection. A `CHECK` pins
//!   it to a single row, because "which skill is active" is one fact and two
//!   rows would make it ambiguous which one is in instruction position.
//! * `skill_pack_activation_audit` — decisions only. There is deliberately no
//!   body column and no context column, so a future writer cannot leak content
//!   into audit even by accident (#8588 acceptance criterion 7).
//!
//! #9735 CORRECTED: this module used to say the FK `REFERENCES` clauses were
//! documentation only because "this workspace never enables the `foreign_keys`
//! PRAGMA (ADR-028 Amendment B3)". Both halves were wrong — the PRAGMA is ON
//! (compile-time default, now explicit in `configure_connection`) and B3 has
//! been amended. The declared references are enforced. The port layer keeps its
//! explicit child-first `DELETE`s, redundant with the engine rather than a
//! substitute. The `UNIQUE`/`CHECK` constraints below are engine-enforced
//! either way.

use rusqlite::Connection;

pub(super) fn migrate_v52(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS skill_pack_catalog (
            skill_id              TEXT PRIMARY KEY,
            install_id            TEXT NOT NULL
                                    REFERENCES extension_installs(install_id) ON DELETE CASCADE,
            extension_id          TEXT NOT NULL,
            contribution_id       TEXT NOT NULL,
            contribution_kind     TEXT NOT NULL,
            version               TEXT NOT NULL,
            publisher_id          TEXT NOT NULL,
            body_sha256           TEXT NOT NULL,
            required_capabilities TEXT NOT NULL DEFAULT '[]',
            optional_capabilities TEXT NOT NULL DEFAULT '[]',
            skill_references      TEXT NOT NULL DEFAULT '[]',
            registered_at         TEXT NOT NULL,
            UNIQUE (install_id, contribution_id)
        );

        CREATE TABLE IF NOT EXISTS skill_pack_activation (
            singleton      INTEGER PRIMARY KEY CHECK (singleton = 1),
            activation_id  TEXT NOT NULL,
            skill_id       TEXT NOT NULL
                             REFERENCES skill_pack_catalog(skill_id) ON DELETE CASCADE,
            version        TEXT NOT NULL,
            body_sha256    TEXT NOT NULL,
            selection_kind TEXT NOT NULL
                             CHECK (selection_kind IN ('EXPLICIT_USER_SELECTION',
                                                       'APPROVED_WORKFLOW_BINDING')),
            binding_id     TEXT,
            activated_at   TEXT NOT NULL,
            expires_at     TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS skill_pack_activation_audit (
            audit_id                 TEXT PRIMARY KEY,
            skill_id                 TEXT NOT NULL,
            version                  TEXT NOT NULL,
            body_sha256              TEXT NOT NULL,
            selection_kind           TEXT NOT NULL,
            decision                 TEXT NOT NULL
                                       CHECK (decision IN ('activated','blocked')),
            blocked_reason           TEXT,
            capability_decisions_json TEXT NOT NULL DEFAULT '[]',
            occurred_at              TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_skill_pack_catalog_install
            ON skill_pack_catalog(install_id);
        CREATE INDEX IF NOT EXISTS idx_skill_pack_activation_audit_skill
            ON skill_pack_activation_audit(skill_id, occurred_at);

        INSERT OR IGNORE INTO schema_version (version) VALUES (52);",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_v51(conn: &Connection) {
        // #9735: match production, which runs with foreign_keys ON. This used
        // to force it OFF citing ADR-028 B3; that amendment has been corrected.
        conn.execute_batch(
            "PRAGMA foreign_keys=ON;
             CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version VALUES (51);",
        )
        .unwrap();
    }

    /// Insert the `extension_installs` parent row the catalog references.
    ///
    /// #9735: the FK is enforced, so a catalog insert cannot reach its own
    /// UNIQUE constraint without this. These tests used to skip it and pass
    /// only because the helper forced the PRAGMA off — the constraint each one
    /// names was never actually exercised in the direction production runs.
    fn seed_install(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS extension_installs (
                install_id      TEXT PRIMARY KEY,
                extension_id    TEXT NOT NULL,
                version         TEXT NOT NULL
             );
             INSERT OR IGNORE INTO extension_installs (install_id, extension_id, version)
             VALUES ('inst_1', 'com.maekon.review', '1.0.0');",
        )
        .unwrap();
    }

    fn insert_entry(conn: &Connection, skill: &str, contribution: &str) -> rusqlite::Result<usize> {
        conn.execute(
            "INSERT INTO skill_pack_catalog
             (skill_id, install_id, extension_id, contribution_id, contribution_kind,
              version, publisher_id, body_sha256, registered_at)
             VALUES (?1, 'inst_1', 'com.maekon.review', ?2, 'SKILL_PACK',
                     '1.0.0', 'maekon', 'sha256:aa', 't')",
            rusqlite::params![skill, contribution],
        )
    }

    fn insert_activation(conn: &Connection, singleton: i64) -> rusqlite::Result<usize> {
        conn.execute(
            "INSERT INTO skill_pack_activation
             (singleton, activation_id, skill_id, version, body_sha256,
              selection_kind, activated_at, expires_at)
             VALUES (?1, 'act_1', 'sk.review', '1.0.0', 'sha256:aa',
                     'EXPLICIT_USER_SELECTION', 't', 't')",
            rusqlite::params![singleton],
        )
    }

    #[test]
    fn migration_creates_tables_and_records_version() {
        let conn = Connection::open_in_memory().unwrap();
        base_v51(&conn);
        migrate_v52(&conn).unwrap();

        for table in [
            "skill_pack_catalog",
            "skill_pack_activation",
            "skill_pack_activation_audit",
        ] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "table {table} must exist after v52");
        }
        assert_eq!(
            conn.query_row("SELECT MAX(version) FROM schema_version", [], |r| r
                .get::<_, u32>(0))
                .unwrap(),
            52
        );
    }

    /// Exactly one skill can be active. A second row is a schema error, not a
    /// race the reader has to arbitrate.
    #[test]
    fn activation_is_a_singleton() {
        let conn = Connection::open_in_memory().unwrap();
        base_v51(&conn);
        migrate_v52(&conn).unwrap();
        seed_install(&conn);
        insert_entry(&conn, "sk.review", "review.pack").unwrap();
        insert_activation(&conn, 1).unwrap();
        let err = insert_activation(&conn, 2).unwrap_err();
        assert!(
            err.to_string().contains("CHECK constraint failed"),
            "a second activation row must be rejected, got: {err}"
        );
    }

    #[test]
    fn one_catalog_entry_per_install_contribution() {
        let conn = Connection::open_in_memory().unwrap();
        base_v51(&conn);
        migrate_v52(&conn).unwrap();
        seed_install(&conn);
        insert_entry(&conn, "sk.a", "review.pack").unwrap();
        let err = insert_entry(&conn, "sk.b", "review.pack").unwrap_err();
        assert!(err.to_string().contains("UNIQUE constraint failed"));
    }

    #[test]
    fn unknown_selection_kind_is_rejected() {
        let conn = Connection::open_in_memory().unwrap();
        base_v51(&conn);
        migrate_v52(&conn).unwrap();
        let err = conn
            .execute(
                "INSERT INTO skill_pack_activation
                 (singleton, activation_id, skill_id, version, body_sha256,
                  selection_kind, activated_at, expires_at)
                 VALUES (1, 'act_1', 'sk.review', '1.0.0', 'sha256:aa',
                         'INFERRED', 't', 't')",
                [],
            )
            .unwrap_err();
        assert!(
            err.to_string().contains("CHECK constraint failed"),
            "an implicit/inferred selection must be unrepresentable, got: {err}"
        );
    }

    /// The audit table has no body column, so content cannot be written there
    /// even by a future careless writer.
    #[test]
    fn the_audit_table_has_no_body_column() {
        let conn = Connection::open_in_memory().unwrap();
        base_v51(&conn);
        migrate_v52(&conn).unwrap();
        let mut stmt = conn
            .prepare("SELECT name FROM pragma_table_info('skill_pack_activation_audit')")
            .unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for forbidden in ["body", "content", "context", "text", "payload"] {
            assert!(
                !cols.contains(&forbidden.to_string()),
                "audit table must not have a {forbidden:?} column; has {cols:?}"
            );
        }
        assert!(cols.contains(&"body_sha256".to_string()));
    }
}
