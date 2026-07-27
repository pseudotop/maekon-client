//! v51 — work-context ledger (ADR-030, #8587/#8589).
//!
//! ADR-030 §7 defines the four planes raw / projection / envelope / tombstone as
//! **separate encryption / TTL / export / erasure targets**. Here we create those
//! four planes plus a per-account cursor.
//!
//! Retention caps (ADR-030 §7 + 2026-07-21 revision B1):
//!
//! | plane | default TTL | hard max |
//! |---|---|---|
//! | raw payload | 24 hours | 7 days |
//! | projection | 30 days | user/source retention period |
//! | envelope | max(projection retention, confirmed reference lifetime), 30 days if neither | 365 days |
//! | tombstone | max(replay horizon, projection retention, 90 days) | 365 days |
//!
//! Envelope rows originally had no numeric cap, so an envelope with neither a
//! projection nor a confirmed reference could persist indefinitely. Revision B1
//! closed that gap, and the columns below are its implementation.
//!
//! **Note**: the `foreign_keys` PRAGMA is OFF workspace-wide. The `REFERENCES`
//! clauses below are for documentation only and are not enforced by the DB.
//! Cleanup therefore has to be an explicit application-level DELETE in child →
//! parent order.

use rusqlite::{Connection, Result as SqlResult};

/// Per-plane default TTL (seconds). The policy engine may narrow it further but never extend it.
///
/// The raw-plane constants are fixed as schema/policy boundaries in this PR (#8587),
/// but the actual consumer (raw blob writer) belongs to #8589. Pinning the constants
/// now keeps #8589 from reinventing §7's 7-day cap.
#[allow(dead_code)] // consumed by the #8589 raw blob writer
pub const RAW_PLANE_DEFAULT_TTL_SECS: i64 = 24 * 60 * 60;
#[allow(dead_code)] // consumed by the #8589 raw blob writer
pub const RAW_PLANE_HARD_MAX_SECS: i64 = 7 * 24 * 60 * 60;
pub const PROJECTION_PLANE_DEFAULT_TTL_SECS: i64 = 30 * 24 * 60 * 60;
/// An envelope with no projection/confirmed-reference obligation inherits the projection default as its cap.
pub const ENVELOPE_PLANE_DEFAULT_TTL_SECS: i64 = PROJECTION_PLANE_DEFAULT_TTL_SECS;
pub const ENVELOPE_PLANE_HARD_MAX_SECS: i64 = 365 * 24 * 60 * 60;
pub const TOMBSTONE_PLANE_FLOOR_SECS: i64 = 90 * 24 * 60 * 60;
pub const TOMBSTONE_PLANE_HARD_MAX_SECS: i64 = 365 * 24 * 60 * 60;

/// Clamps the seconds remaining until an envelope's expiry to the hard max (revision B1).
///
/// A confirmed ADR-028 reference extends it up to that lifetime (`requested_secs`),
/// but it can never exceed the hard max (365 days) in any case. With no request or
/// a value of 0 or less, it inherits the projection default (30 days) as its cap.
pub fn clamp_envelope_ttl_secs(requested_secs: Option<i64>) -> i64 {
    let base = match requested_secs {
        Some(s) if s > 0 => s,
        _ => ENVELOPE_PLANE_DEFAULT_TTL_SECS,
    };
    base.min(ENVELOPE_PLANE_HARD_MAX_SECS)
}

/// Clamps the raw-plane TTL to the hard max (7 days) (§7).
///
/// The policy engine may narrow it below the default (24 hours), but even with
/// explicit consent it cannot be retained beyond 7 days.
#[allow(dead_code)] // consumed by the #8589 raw blob writer
pub fn clamp_raw_ttl_secs(requested_secs: Option<i64>) -> i64 {
    let base = match requested_secs {
        Some(s) if s > 0 => s,
        _ => RAW_PLANE_DEFAULT_TTL_SECS,
    };
    base.min(RAW_PLANE_HARD_MAX_SECS)
}

/// Clamps the tombstone retention period between the floor and the hard max (§7).
#[allow(dead_code)] // consumed by the #8589 raw/tombstone policy wiring
pub fn clamp_tombstone_ttl_secs(requested_secs: Option<i64>) -> i64 {
    let base = requested_secs.unwrap_or(TOMBSTONE_PLANE_FLOOR_SECS);
    base.clamp(TOMBSTONE_PLANE_FLOOR_SECS, TOMBSTONE_PLANE_HARD_MAX_SECS)
}

pub(super) fn migrate_v51(conn: &Connection) -> SqlResult<()> {
    // -- per-account cursor -------------------------------------------------
    //
    // Cursor advancement is a compare-and-swap (revision I4). `cursor_revision`
    // is the CAS token, preventing two overlapping ingests from overwriting or
    // rolling back each other's cursor.
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS work_context_cursors (
            install_id            TEXT NOT NULL,
            account_subject_ref   TEXT NOT NULL,
            cursor                TEXT,
            access_epoch_id       INTEGER NOT NULL,
            cursor_revision       INTEGER NOT NULL DEFAULT 0,
            last_ingested_at      TEXT,
            created_at            TEXT NOT NULL,
            updated_at            TEXT NOT NULL,
            PRIMARY KEY (install_id, account_subject_ref)
        );
        "#,
    )?;

    // -- envelope plane -----------------------------------------------------
    //
    // The local uniqueness key is (source_object_key, access_epoch_id,
    // revision_fingerprint). This UNIQUE makes at-least-once delivery idempotent —
    // replaying the same page does not create another envelope (ADR-030 §4).
    //
    // Message bodies, document text, attachments, provider JSON, tokens, and ACLs
    // are in none of the columns.
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS work_context_envelopes (
            envelope_id           TEXT PRIMARY KEY,
            schema_version        INTEGER NOT NULL,
            source_object_key     TEXT NOT NULL,
            access_epoch_id       INTEGER NOT NULL,
            revision_fingerprint  TEXT NOT NULL,

            extension_id          TEXT NOT NULL,
            install_id            TEXT NOT NULL,
            account_subject_ref   TEXT NOT NULL,
            remote_type           TEXT NOT NULL,
            remote_id             TEXT NOT NULL,

            revision_model        TEXT NOT NULL
                CHECK (revision_model IN ('monotonic','opaque','content_hash_only')),
            remote_revision       TEXT,
            etag                  TEXT,
            source_order          INTEGER,
            content_hash          TEXT NOT NULL,

            kind                  TEXT NOT NULL
                CHECK (kind IN ('message','meeting','document','issue','decision','task','unknown')),
            classification        TEXT NOT NULL
                CHECK (classification IN ('public','internal','confidential','restricted','unknown')),
            retention_class       TEXT,

            occurred_at           TEXT,
            source_updated_at     TEXT,
            observed_at           TEXT NOT NULL,
            ingested_at           TEXT NOT NULL,

            relations_json        TEXT NOT NULL DEFAULT '[]',
            access_snapshot_json  TEXT,
            consent_snapshot_json TEXT,

            ingest_run_id         TEXT NOT NULL,
            prior_envelope_id     TEXT,
            source_cursor_digest  TEXT,
            projection_ref        TEXT,
            raw_blob_ref          TEXT,

            lifecycle             TEXT NOT NULL
                CHECK (lifecycle IN ('active','deleted','access_revoked','retention_expired')),

            -- 개정 B1: 투영/확정 참조 의무가 없어도 반드시 만료 시점이 있다.
            expires_at            TEXT NOT NULL,
            -- 확정된 ADR-028 참조가 있으면 그 수명까지 연장되지만 하드 최대를 넘지 못한다.
            has_confirmed_ref     INTEGER NOT NULL DEFAULT 0,

            UNIQUE (source_object_key, access_epoch_id, revision_fingerprint)
        );

        CREATE INDEX IF NOT EXISTS idx_wctx_env_object_epoch
            ON work_context_envelopes (source_object_key, access_epoch_id);
        CREATE INDEX IF NOT EXISTS idx_wctx_env_account
            ON work_context_envelopes (install_id, account_subject_ref);
        CREATE INDEX IF NOT EXISTS idx_wctx_env_expiry
            ON work_context_envelopes (expires_at);
        CREATE INDEX IF NOT EXISTS idx_wctx_env_projectable
            ON work_context_envelopes (lifecycle, kind);
        "#,
    )?;

    // -- projection plane ---------------------------------------------------
    //
    // Holds only the **sanitized** title/summary and bounded references needed
    // for timeline/search. Not a copy of the original — if the original is
    // needed, open the raw plane under separate consent.
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS work_context_projections (
            projection_id     TEXT PRIMARY KEY,
            envelope_id       TEXT NOT NULL REFERENCES work_context_envelopes(envelope_id),
            source_object_key TEXT NOT NULL,
            access_epoch_id   INTEGER NOT NULL,
            sanitized_title   TEXT,
            sanitized_summary TEXT,
            created_at        TEXT NOT NULL,
            expires_at        TEXT NOT NULL,
            UNIQUE (source_object_key, access_epoch_id)
        );

        CREATE INDEX IF NOT EXISTS idx_wctx_proj_expiry
            ON work_context_projections (expires_at);
        "#,
    )?;

    // -- raw plane ----------------------------------------------------------
    //
    // Memory-only by default; it stays here only with explicit consent.
    // The key is an HKDF subkey of the existing EncryptionKey (revision I1):
    //   HKDF-SHA256(ikm=EncryptionKey, salt=install_id,
    //               info="maekon.raw-plane.v1" || account_subject_ref)
    // Destroying the `key_salt` record crypto-shreds that account's raw plane
    // without rewriting the DB — this is the implementation of "remove raw
    // content" on revocation.
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS work_context_raw_blobs (
            raw_blob_ref      TEXT PRIMARY KEY,
            source_object_key TEXT NOT NULL,
            access_epoch_id   INTEGER NOT NULL,
            install_id        TEXT NOT NULL,
            key_salt          BLOB NOT NULL,
            nonce             BLOB NOT NULL,
            ciphertext        BLOB NOT NULL,
            created_at        TEXT NOT NULL,
            expires_at        TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_wctx_raw_expiry
            ON work_context_raw_blobs (expires_at);
        CREATE INDEX IF NOT EXISTS idx_wctx_raw_account
            ON work_context_raw_blobs (install_id, source_object_key);
        "#,
    )?;

    // -- tombstone plane ----------------------------------------------------
    //
    // **Has no content.** Holds only the HMAC source key, epoch, revision
    // fingerprint, lifecycle, and deletion time. It is preserved for the replay
    // horizon even when an account is disconnected or the extension is removed —
    // because stale pages resent after reconnection must keep being suppressed.
    // Only a full local erasure (ADR-030 §12) deletes even this, and that is a
    // separate path unreachable as a side effect of uninstall (revision B3).
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS work_context_tombstones (
            tombstone_id         TEXT PRIMARY KEY,
            source_object_key    TEXT NOT NULL,
            access_epoch_id      INTEGER NOT NULL,
            revision_fingerprint TEXT NOT NULL,
            lifecycle            TEXT NOT NULL
                CHECK (lifecycle IN ('deleted','access_revoked','retention_expired')),
            source_order         INTEGER,
            deleted_at           TEXT NOT NULL,
            expires_at           TEXT NOT NULL,
            UNIQUE (source_object_key, access_epoch_id, revision_fingerprint)
        );

        CREATE INDEX IF NOT EXISTS idx_wctx_tomb_object
            ON work_context_tombstones (source_object_key, access_epoch_id);
        CREATE INDEX IF NOT EXISTS idx_wctx_tomb_expiry
            ON work_context_tombstones (expires_at);
        "#,
    )?;

    // -- conflict quarantine ------------------------------------------------
    //
    // Revision M1: one row per (source_object_key, access_epoch_id) rather than
    // one per delivery, following the envelope-plane cap. While quarantined, no
    // winner is exposed to search, suggestions, tasks, or the graph.
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS work_context_conflicts (
            conflict_id       TEXT NOT NULL,
            source_object_key TEXT NOT NULL,
            access_epoch_id   INTEGER NOT NULL,
            fingerprints_json TEXT NOT NULL,
            created_at        TEXT NOT NULL,
            expires_at        TEXT NOT NULL,
            PRIMARY KEY (source_object_key, access_epoch_id)
        );

        CREATE INDEX IF NOT EXISTS idx_wctx_conflict_expiry
            ON work_context_conflicts (expires_at);
        "#,
    )?;

    // -- access epoch counter -----------------------------------------------
    //
    // Revision I2: the epoch is **owned and issued by storage**. It is a separate
    // counter with no ordering relationship to the ADR-031 broker's revocation
    // epoch.
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS work_context_access_epochs (
            install_id          TEXT NOT NULL,
            account_subject_ref TEXT NOT NULL,
            current_epoch       INTEGER NOT NULL,
            updated_at          TEXT NOT NULL,
            PRIMARY KEY (install_id, account_subject_ref)
        );
        "#,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        // Production does not enable foreign_keys. If only the tests had it on,
        // they would verify a constraint that production lacks, so turn it off explicitly.
        conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        migrate_v51(&conn).unwrap();
        conn
    }

    #[test]
    fn migration_is_idempotent() {
        let conn = open();
        migrate_v51(&conn).unwrap();
        migrate_v51(&conn).unwrap();
    }

    #[test]
    fn every_plane_table_exists() {
        let conn = open();
        for t in [
            "work_context_cursors",
            "work_context_envelopes",
            "work_context_projections",
            "work_context_raw_blobs",
            "work_context_tombstones",
            "work_context_conflicts",
            "work_context_access_epochs",
        ] {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [t],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "missing table {t}");
        }
    }

    #[test]
    fn uniqueness_key_makes_replay_idempotent() {
        let conn = open();
        let insert = r#"
            INSERT INTO work_context_envelopes (
                envelope_id, schema_version, source_object_key, access_epoch_id,
                revision_fingerprint, extension_id, install_id, account_subject_ref,
                remote_type, remote_id, revision_model, content_hash, kind,
                classification, observed_at, ingested_at, ingest_run_id, lifecycle, expires_at
            ) VALUES (?1,1,'sok_1',1,'fp_1','ext','inst','acct','event','r1',
                      'monotonic','h','meeting','internal','t','t','run','active','t')
        "#;
        conn.execute(insert, ["wctx_1"]).unwrap();
        // Replaying the same (source_object_key, epoch, fingerprint) is rejected.
        let err = conn.execute(insert, ["wctx_2"]).unwrap_err();
        assert!(err.to_string().contains("UNIQUE"), "{err}");
    }

    #[test]
    fn same_object_in_a_new_epoch_is_a_separate_row() {
        let conn = open();
        let ins = |id: &str, epoch: i64| {
            conn.execute(
                r#"INSERT INTO work_context_envelopes (
                    envelope_id, schema_version, source_object_key, access_epoch_id,
                    revision_fingerprint, extension_id, install_id, account_subject_ref,
                    remote_type, remote_id, revision_model, content_hash, kind,
                    classification, observed_at, ingested_at, ingest_run_id, lifecycle, expires_at
                ) VALUES (?1,1,'sok_1',?2,'fp_1','ext','inst','acct','event','r1',
                          'monotonic','h','meeting','internal','t','t','run','active','t')"#,
                rusqlite::params![id, epoch],
            )
        };
        ins("wctx_1", 1).unwrap();
        // When re-authorization opens a new epoch, the same object is a separate
        // row — an old tombstone does not revert to active.
        ins("wctx_2", 2).unwrap();
    }

    #[test]
    fn lifecycle_and_kind_are_constrained() {
        let conn = open();
        let bad_lifecycle = conn.execute(
            r#"INSERT INTO work_context_envelopes (
                envelope_id, schema_version, source_object_key, access_epoch_id,
                revision_fingerprint, extension_id, install_id, account_subject_ref,
                remote_type, remote_id, revision_model, content_hash, kind,
                classification, observed_at, ingested_at, ingest_run_id, lifecycle, expires_at
            ) VALUES ('x',1,'sok',1,'fp','e','i','a','t','r','monotonic','h','meeting',
                      'internal','t','t','run','resurrected','t')"#,
            [],
        );
        // Check by value that the CHECK constraint rejects an unknown lifecycle.
        let err = bad_lifecycle.unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("constraint"),
            "lifecycle CHECK 위반이어야 함: {err}"
        );
    }

    #[test]
    fn tombstone_cannot_be_active() {
        let conn = open();
        // If 'active' could enter the tombstone plane, its suppression semantics would collapse.
        let r = conn.execute(
            r#"INSERT INTO work_context_tombstones
               (tombstone_id, source_object_key, access_epoch_id, revision_fingerprint,
                lifecycle, deleted_at, expires_at)
               VALUES ('tb','sok',1,'fp','active','t','t')"#,
            [],
        );
        let err = r.unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("constraint"),
            "tombstone lifecycle CHECK 가 'active' 를 거부해야 함: {err}"
        );
    }

    #[test]
    fn conflict_quarantine_is_one_row_per_object_and_epoch() {
        let conn = open();
        let ins = |cid: &str| {
            conn.execute(
                r#"INSERT INTO work_context_conflicts
                   (conflict_id, source_object_key, access_epoch_id, fingerprints_json,
                    created_at, expires_at)
                   VALUES (?1,'sok',1,'[]','t','t')"#,
                [cid],
            )
        };
        ins("c1").unwrap();
        // Revision M1: it does not accumulate per delivery. Verify the PK collision by value.
        let err = ins("c2").unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("unique")
                || err.to_string().to_lowercase().contains("constraint"),
            "(source_object_key, epoch) PK 충돌이어야 함: {err}"
        );
    }

    #[allow(clippy::assertions_on_constants)] // deliberate compile-time invariant documentation
    #[test]
    fn envelope_hard_max_is_a_year_and_raw_is_a_week() {
        // Pin the numbers from revision B1/§7 so they cannot silently grow in the code.
        assert_eq!(RAW_PLANE_DEFAULT_TTL_SECS, 86_400);
        assert_eq!(RAW_PLANE_HARD_MAX_SECS, 604_800);
        assert_eq!(
            ENVELOPE_PLANE_DEFAULT_TTL_SECS,
            PROJECTION_PLANE_DEFAULT_TTL_SECS
        );
        assert_eq!(ENVELOPE_PLANE_HARD_MAX_SECS, 31_536_000);
        assert_eq!(TOMBSTONE_PLANE_HARD_MAX_SECS, 31_536_000);
        assert!(TOMBSTONE_PLANE_FLOOR_SECS < TOMBSTONE_PLANE_HARD_MAX_SECS);
    }

    #[test]
    fn envelope_ttl_is_clamped_to_the_hard_max() {
        // Revision B1: no matter how long a lifetime a confirmed reference requests, it cannot exceed 365 days.
        assert_eq!(
            clamp_envelope_ttl_secs(None),
            ENVELOPE_PLANE_DEFAULT_TTL_SECS
        );
        assert_eq!(
            clamp_envelope_ttl_secs(Some(0)),
            ENVELOPE_PLANE_DEFAULT_TTL_SECS
        );
        assert_eq!(
            clamp_envelope_ttl_secs(Some(10 * 365 * 24 * 60 * 60)),
            ENVELOPE_PLANE_HARD_MAX_SECS
        );
        // A confirmed-reference lifetime shorter than the hard max is respected as-is.
        let two_months = 60 * 24 * 60 * 60;
        assert_eq!(clamp_envelope_ttl_secs(Some(two_months)), two_months);
    }

    #[test]
    fn raw_ttl_never_exceeds_seven_days_even_with_consent() {
        // §7: even with explicit consent, the raw plane cannot be retained beyond 7 days.
        assert_eq!(clamp_raw_ttl_secs(None), RAW_PLANE_DEFAULT_TTL_SECS);
        assert_eq!(
            clamp_raw_ttl_secs(Some(30 * 24 * 60 * 60)),
            RAW_PLANE_HARD_MAX_SECS
        );
        // A request shorter than the default is kept as-is.
        assert_eq!(clamp_raw_ttl_secs(Some(3600)), 3600);
    }

    #[test]
    fn tombstone_ttl_is_bounded_by_floor_and_hard_max() {
        // §7: 90-day floor, 365-day cap.
        assert_eq!(clamp_tombstone_ttl_secs(None), TOMBSTONE_PLANE_FLOOR_SECS);
        assert_eq!(
            clamp_tombstone_ttl_secs(Some(1)),
            TOMBSTONE_PLANE_FLOOR_SECS
        );
        assert_eq!(
            clamp_tombstone_ttl_secs(Some(10 * 365 * 24 * 60 * 60)),
            TOMBSTONE_PLANE_HARD_MAX_SECS
        );
    }
}
