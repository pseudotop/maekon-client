//! GDPR Art.17 full-erasure coverage for the v51 work-context ledger
//! (ADR-030 §12, #8587 review BLOCKING 3).
//!
//! `delete_all_data` (full local erasure) MUST delete EVERY work-context plane
//! — including the content-free suppression tombstones, which uninstall retains
//! but full erasure destroys (Amendment B3). This test seeds one row into all
//! seven v51 tables and asserts every one is empty after the wipe. A table
//! missing from `ALL_TABLES` would leave its row behind and fail here.

use super::super::SqliteStorage;

/// Every v51 work-context table the full-erasure path must clear.
const WORK_CONTEXT_TABLES: &[&str] = &[
    "work_context_envelopes",
    "work_context_projections",
    "work_context_raw_blobs",
    "work_context_tombstones",
    "work_context_conflicts",
    "work_context_cursors",
    "work_context_access_epochs",
];

fn seed_one_row_per_table(storage: &SqliteStorage) {
    let conn = storage.connection_arc();
    let guard = conn.test_lock();
    guard
        .execute_batch(
            r#"
            INSERT INTO work_context_envelopes (
                envelope_id, schema_version, source_object_key, access_epoch_id,
                revision_fingerprint, extension_id, install_id, account_subject_ref,
                remote_type, remote_id, revision_model, content_hash, kind,
                classification, observed_at, ingested_at, ingest_run_id, lifecycle, expires_at
            ) VALUES ('wctx_1',1,'sok_1',1,'fp_1','ext','inst','acct','event','r1',
                      'monotonic','h','meeting','internal','t','t','run','active','t');

            INSERT INTO work_context_projections (
                projection_id, envelope_id, source_object_key, access_epoch_id,
                sanitized_title, sanitized_summary, created_at, expires_at
            ) VALUES ('proj_1','wctx_1','sok_1',1,'title','summary','t','t');

            INSERT INTO work_context_raw_blobs (
                raw_blob_ref, source_object_key, access_epoch_id, install_id,
                key_salt, nonce, ciphertext, created_at, expires_at
            ) VALUES ('raw_1','sok_1',1,'inst',x'01',x'02',x'03','t','t');

            INSERT INTO work_context_tombstones (
                tombstone_id, source_object_key, access_epoch_id, revision_fingerprint,
                lifecycle, deleted_at, expires_at
            ) VALUES ('tomb_1','sok_1',1,'fp_1','deleted','t','t');

            INSERT INTO work_context_conflicts (
                conflict_id, source_object_key, access_epoch_id, fingerprints_json,
                created_at, expires_at
            ) VALUES ('cf_1','sok_1',1,'[]','t','t');

            INSERT INTO work_context_cursors (
                install_id, account_subject_ref, cursor, access_epoch_id,
                created_at, updated_at
            ) VALUES ('inst','acct','c1',1,'t','t');

            INSERT INTO work_context_access_epochs (
                install_id, account_subject_ref, current_epoch, updated_at
            ) VALUES ('inst','acct',1,'t');
            "#,
        )
        .expect("seed one row into every work-context plane");
}

fn count(storage: &SqliteStorage, table: &str) -> i64 {
    let conn = storage.connection_arc();
    let guard = conn.test_lock();
    guard
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn full_erasure_clears_every_work_context_plane() {
    let storage = SqliteStorage::open_in_memory(30).expect("in-memory sqlite");
    seed_one_row_per_table(&storage);

    // Pre-condition: each plane holds exactly the seeded row.
    for &table in WORK_CONTEXT_TABLES {
        assert_eq!(count(&storage, table), 1, "{table} must be seeded");
    }

    storage.delete_all_data().expect("GDPR Art.17 full erasure");

    // Post-condition: full erasure clears every plane, tombstones included (§12/B3).
    for &table in WORK_CONTEXT_TABLES {
        assert_eq!(
            count(&storage, table),
            0,
            "{table} must be empty after delete_all_data (ADR-030 §12 full erasure)"
        );
    }
}
