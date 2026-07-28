//! V50: Extension registry tables (ADR-029, #8586).
//!
//! Three additive tables backing install/enable/update/rollback/uninstall and
//! the eight orthogonal readiness axes.
//!
//! Provenance (`provenance`) and installation state (`installation`) are two
//! separate columns on purpose: a bundled built-in is `BUNDLED` provenance and
//! still starts `NOT_INSTALLED` (ADR-029 Amendment B1). Collapsing them would
//! make "bundled built-in the user actually enabled" unrepresentable.
//!
//! FK `REFERENCES` clauses are documentation only — this workspace never enables
//! the `foreign_keys` PRAGMA (ADR-028 Amendment B3), so cleanup ordering is
//! enforced by the port layer, not the engine. The `UNIQUE`/`CHECK` constraints
//! below ARE engine-enforced and carry the durable invariants.

use rusqlite::Connection;

pub(super) fn migrate_v50(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS extension_installs (
            install_id        TEXT PRIMARY KEY,
            extension_id      TEXT NOT NULL,
            version           TEXT NOT NULL,
            provenance        TEXT NOT NULL
                                CHECK (provenance IN ('BUNDLED','CURATED_LOCAL')),
            source_kind       TEXT NOT NULL,
            signature_state   TEXT NOT NULL,
            installation      TEXT NOT NULL
                                CHECK (installation IN ('NOT_INSTALLED','INSTALLING','INSTALLED',
                                                        'INSTALL_FAILED','UNINSTALLING','UNINSTALLED')),
            enablement        TEXT NOT NULL CHECK (enablement IN ('DISABLED','ENABLED')),
            authentication    TEXT NOT NULL
                                CHECK (authentication IN ('NOT_REQUIRED','UNAUTHENTICATED','AUTHENTICATING',
                                                          'AUTHENTICATED','REVOKED','AUTH_ERROR')),
            capability_grant  TEXT NOT NULL
                                CHECK (capability_grant IN ('NOT_REQUESTED','PENDING','GRANTED',
                                                            'DENIED','REVOKED','EXPIRED')),
            update_state      TEXT NOT NULL
                                CHECK (update_state IN ('CURRENT','UPDATE_AVAILABLE','STAGED',
                                                        'ACTIVATING','UPDATE_FAILED','ROLLBACK_AVAILABLE')),
            health_json       TEXT NOT NULL,
            previous_version  TEXT,
            revision          INTEGER NOT NULL DEFAULT 1,
            created_at        TEXT NOT NULL,
            updated_at        TEXT NOT NULL,
            UNIQUE (extension_id)
        );

        CREATE TABLE IF NOT EXISTS extension_manifests (
            install_id        TEXT PRIMARY KEY
                                REFERENCES extension_installs(install_id) ON DELETE CASCADE,
            extension_id      TEXT NOT NULL,
            version           TEXT NOT NULL,
            publisher_id      TEXT NOT NULL,
            package_digest    TEXT NOT NULL,
            manifest_schema   INTEGER NOT NULL,
            host_api_min      INTEGER NOT NULL,
            host_api_max      INTEGER NOT NULL,
            manifest_json     TEXT NOT NULL,
            recorded_at       TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS extension_registry_audit (
            audit_id       TEXT PRIMARY KEY,
            install_id     TEXT NOT NULL,
            extension_id   TEXT NOT NULL,
            version        TEXT NOT NULL,
            source_kind    TEXT NOT NULL,
            signature_state TEXT NOT NULL,
            event_kind     TEXT NOT NULL,
            outcome        TEXT NOT NULL,
            occurred_at    TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_extension_installs_state
            ON extension_installs(installation, enablement);
        CREATE INDEX IF NOT EXISTS idx_extension_manifests_extension
            ON extension_manifests(extension_id, version);
        CREATE INDEX IF NOT EXISTS idx_extension_registry_audit_install
            ON extension_registry_audit(install_id, occurred_at);

        INSERT OR IGNORE INTO schema_version (version) VALUES (50);",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_v49(conn: &Connection) {
        // Match production: the shared connection runs with foreign_keys OFF
        // (ADR-028 Amendment B3), so these tests assert the engine-enforced
        // UNIQUE/CHECK constraints rather than FK behaviour.
        conn.execute_batch(
            "PRAGMA foreign_keys=OFF;
             CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version VALUES (49);",
        )
        .unwrap();
    }

    fn insert_install(
        conn: &Connection,
        id: &str,
        ext: &str,
        installation: &str,
    ) -> rusqlite::Result<usize> {
        conn.execute(
            "INSERT INTO extension_installs
             (install_id, extension_id, version, provenance, source_kind, signature_state,
              installation, enablement, authentication, capability_grant, update_state,
              health_json, revision, created_at, updated_at)
             VALUES (?1, ?2, '1.0.0', 'BUNDLED', 'APP_BUNDLE', 'APP_BUNDLE_TRUSTED',
                     ?3, 'DISABLED', 'NOT_REQUIRED', 'NOT_REQUESTED', 'CURRENT',
                     '{\"state\":\"unknown\"}', 1, 't', 't')",
            rusqlite::params![id, ext, installation],
        )
    }

    #[test]
    fn migration_creates_tables_and_records_version() {
        let conn = Connection::open_in_memory().unwrap();
        base_v49(&conn);
        migrate_v50(&conn).unwrap();

        for table in [
            "extension_installs",
            "extension_manifests",
            "extension_registry_audit",
        ] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "table {table} must exist after v50");
        }
        assert_eq!(
            conn.query_row("SELECT MAX(version) FROM schema_version", [], |r| r
                .get::<_, u32>(0))
                .unwrap(),
            50
        );
    }

    #[test]
    fn provenance_and_installation_are_independent_columns() {
        let conn = Connection::open_in_memory().unwrap();
        base_v49(&conn);
        migrate_v50(&conn).unwrap();
        // A BUNDLED extension that is NOT_INSTALLED is representable...
        insert_install(&conn, "inst_1", "com.maekon.a", "NOT_INSTALLED").unwrap();
        // ...and so is a BUNDLED extension the user actually installed.
        insert_install(&conn, "inst_2", "com.maekon.b", "INSTALLED").unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM extension_installs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn unknown_axis_values_are_rejected() {
        let conn = Connection::open_in_memory().unwrap();
        base_v49(&conn);
        migrate_v50(&conn).unwrap();
        let err = insert_install(&conn, "inst_1", "com.maekon.a", "BUNDLED").unwrap_err();
        assert!(
            err.to_string().contains("CHECK constraint failed"),
            "installation axis must reject provenance values like BUNDLED, got: {err}"
        );
    }

    #[test]
    fn one_install_row_per_extension_id() {
        let conn = Connection::open_in_memory().unwrap();
        base_v49(&conn);
        migrate_v50(&conn).unwrap();
        insert_install(&conn, "inst_1", "com.maekon.a", "INSTALLED").unwrap();
        let err = insert_install(&conn, "inst_2", "com.maekon.a", "INSTALLED").unwrap_err();
        assert!(err.to_string().contains("UNIQUE constraint failed"));
    }
}
