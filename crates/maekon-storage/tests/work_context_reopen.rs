//! Two-process restart tests for the work-context ledger (ADR-030 §9, #8587).
//!
//! The #8587 acceptance criteria require that after a restart the cursor is
//! recovered and no duplicate envelope is produced, that the cursor CAS blocks
//! overlapping ingestion, and that a revoke is fail-closed from the next sync
//! onward. This exercises the on-disk boundaries that in-memory unit tests
//! cannot verify.

use chrono::{Duration, Utc};
use maekon_core::models::work_context::{
    compute_revision_fingerprint, compute_source_object_key, DataClassification, Lifecycle,
    RevisionModel, SourceObjectIdentity, WorkContextEnvelope, WorkContextKind,
    WORK_CONTEXT_SCHEMA_VERSION,
};
use maekon_core::ports::work_context::{
    CommitOutcome, CommitPageRequest, CursorAdvance, WorkContextStorePort,
};
use maekon_storage::sqlite::SqliteStorage;
use tempfile::TempDir;

const DEDUPE_KEY: &[u8] = &[7u8; 32];

fn identity(remote_id: &str) -> SourceObjectIdentity {
    SourceObjectIdentity {
        extension_id: "com.maekon.calendar".into(),
        install_id: "inst_1".into(),
        account_subject_ref: "acct_opaque_1".into(),
        remote_type: "event".into(),
        remote_id: remote_id.into(),
    }
}

fn envelope(remote_id: &str, order: i64, epoch: i64, content: &str) -> WorkContextEnvelope {
    let id = identity(remote_id);
    let now = Utc::now();
    let content_hash = format!("hash_{content}");
    let fingerprint = compute_revision_fingerprint(
        RevisionModel::Monotonic,
        Some(&order.to_string()),
        None,
        None,
        &content_hash,
        Lifecycle::Active,
    );
    WorkContextEnvelope {
        envelope_id: format!("wctx_{remote_id}_{order}_{epoch}"),
        schema_version: WORK_CONTEXT_SCHEMA_VERSION,
        access_epoch_id: epoch,
        source_object_key: compute_source_object_key(DEDUPE_KEY, &id),
        identity: id,
        revision_model: RevisionModel::Monotonic,
        remote_revision: Some(order.to_string()),
        etag: None,
        source_order: Some(order),
        content_hash,
        revision_fingerprint: fingerprint,
        kind: WorkContextKind::Meeting,
        classification: DataClassification::Internal,
        retention_class: None,
        occurred_at: None,
        source_updated_at: None,
        observed_at: now,
        ingested_at: now,
        relations: vec![],
        access_snapshot: None,
        consent_snapshot: None,
        ingest_run_id: "run_1".into(),
        prior_envelope_id: None,
        source_cursor_digest: None,
        projection_ref: None,
        raw_blob_ref: None,
        lifecycle: Lifecycle::Active,
    }
}

/// Opaque-revision envelope — with no source_order, differing contents are
/// **incomparable** (§5). Since the etag differs per content, the fingerprint
/// diverges, so two contents of the same object are quarantined as an
/// incomparable conflict (§6 rule 7).
fn opaque_envelope(remote_id: &str, epoch: i64, content: &str) -> WorkContextEnvelope {
    let id = identity(remote_id);
    let now = Utc::now();
    let content_hash = format!("hash_{content}");
    let fingerprint = compute_revision_fingerprint(
        RevisionModel::Opaque,
        None,
        Some(content),
        None,
        &content_hash,
        Lifecycle::Active,
    );
    WorkContextEnvelope {
        envelope_id: format!("wctx_{remote_id}_{content}_{epoch}"),
        schema_version: WORK_CONTEXT_SCHEMA_VERSION,
        access_epoch_id: epoch,
        source_object_key: compute_source_object_key(DEDUPE_KEY, &id),
        identity: id,
        revision_model: RevisionModel::Opaque,
        remote_revision: None,
        etag: Some(content.into()),
        source_order: None,
        content_hash,
        revision_fingerprint: fingerprint,
        kind: WorkContextKind::Meeting,
        classification: DataClassification::Internal,
        retention_class: None,
        occurred_at: None,
        source_updated_at: None,
        observed_at: now,
        ingested_at: now,
        relations: vec![],
        access_snapshot: None,
        consent_snapshot: None,
        ingest_run_id: "run_1".into(),
        prior_envelope_id: None,
        source_cursor_digest: None,
        projection_ref: None,
        raw_blob_ref: None,
        lifecycle: Lifecycle::Active,
    }
}

fn commit(
    epoch: i64,
    envelopes: Vec<WorkContextEnvelope>,
    expected_cursor: Option<&str>,
    next_cursor: Option<&str>,
) -> CommitPageRequest {
    CommitPageRequest {
        install_id: "inst_1".into(),
        account_subject_ref: "acct_opaque_1".into(),
        access_epoch_id: epoch,
        ingest_run_id: "run_1".into(),
        envelopes,
        contents: Vec::new(),
        cursor: CursorAdvance {
            install_id: "inst_1".into(),
            account_subject_ref: "acct_opaque_1".into(),
            expected_cursor: expected_cursor.map(String::from),
            next_cursor: next_cursor.map(String::from),
        },
        now: Utc::now(),
    }
}

#[tokio::test]
async fn cursor_and_envelopes_survive_reopen_without_duplicates() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("wctx.db");

    // First "process": issue an epoch → commit two records → advance the cursor.
    let epoch = {
        let s = SqliteStorage::open(&path, 30, None).unwrap();
        let epoch = s
            .begin_access_epoch("inst_1", "acct_opaque_1", Utc::now())
            .await
            .unwrap();
        assert_eq!(epoch, 1, "최초 epoch 는 1");
        let out = s
            .commit_page(commit(
                epoch,
                vec![
                    envelope("evt_a", 1, epoch, "a"),
                    envelope("evt_b", 1, epoch, "b"),
                ],
                None,
                Some("cursor_p1"),
            ))
            .await
            .unwrap();
        assert!(matches!(out, CommitOutcome::Committed { .. }));
        epoch
    };

    // Second "process": the cursor is recovered, and replaying the same page creates no duplicates.
    {
        let s = SqliteStorage::open(&path, 30, None).unwrap();
        let cursor = s
            .get_cursor("inst_1", "acct_opaque_1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cursor.cursor.as_deref(), Some("cursor_p1"), "커서 복구");
        assert_eq!(cursor.access_epoch_id, epoch);

        // Replay the same page (at-least-once). The local uniqueness key guarantees idempotency.
        let out = s
            .commit_page(commit(
                epoch,
                vec![
                    envelope("evt_a", 1, epoch, "a"),
                    envelope("evt_b", 1, epoch, "b"),
                ],
                Some("cursor_p1"),
                Some("cursor_p2"),
            ))
            .await
            .unwrap();
        assert!(matches!(out, CommitOutcome::Committed { .. }));

        // Still exactly 2 projectable — the replay produced no copies.
        let projectable = s.list_projectable(100).await.unwrap();
        assert_eq!(projectable.len(), 2, "재생 후에도 중복 없음");
    }
}

#[tokio::test]
async fn stale_cursor_loses_the_compare_and_swap() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("wctx.db");
    let s = SqliteStorage::open(&path, 30, None).unwrap();
    let epoch = s
        .begin_access_epoch("inst_1", "acct_opaque_1", Utc::now())
        .await
        .unwrap();

    // The first commit advances the cursor to cursor_p1.
    s.commit_page(commit(
        epoch,
        vec![envelope("evt_a", 1, epoch, "a")],
        None,
        Some("cursor_p1"),
    ))
    .await
    .unwrap();

    // An overlapping second ingestion still expects the old cursor (None) and
    // attempts to commit → the CAS fails and nothing is committed. This prevents
    // two loops from rolling back each other's cursor (revision I4).
    let out = s
        .commit_page(commit(
            epoch,
            vec![envelope("evt_b", 1, epoch, "b")],
            None,
            Some("cursor_bad"),
        ))
        .await
        .unwrap();
    assert_eq!(out, CommitOutcome::CursorConflict);

    // evt_b was not committed and the cursor is unchanged.
    let cursor = s
        .get_cursor("inst_1", "acct_opaque_1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cursor.cursor.as_deref(), Some("cursor_p1"));
    assert_eq!(s.list_projectable(100).await.unwrap().len(), 1);
}

#[tokio::test]
async fn revoke_makes_next_sync_fail_closed_across_reopen() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("wctx.db");

    let epoch = {
        let s = SqliteStorage::open(&path, 30, None).unwrap();
        let epoch = s
            .begin_access_epoch("inst_1", "acct_opaque_1", Utc::now())
            .await
            .unwrap();
        s.commit_page(commit(
            epoch,
            vec![envelope("evt_a", 1, epoch, "a")],
            None,
            Some("cursor_p1"),
        ))
        .await
        .unwrap();
        // Revoke — erase the content and remove the cursor.
        s.revoke_account("inst_1", "acct_opaque_1", Utc::now())
            .await
            .unwrap();
        epoch
    };

    // The revoked state persists across a restart.
    {
        let s = SqliteStorage::open(&path, 30, None).unwrap();
        // Since the cursor is gone, subsequent ingestion does not fall back to a
        // fresh state and cannot resume without a new epoch.
        assert!(s
            .get_cursor("inst_1", "acct_opaque_1")
            .await
            .unwrap()
            .is_none());
        // The content is no longer projectable (access_revoked terminal state).
        assert_eq!(s.list_projectable(100).await.unwrap().len(), 0);
        let _ = epoch;
    }
}

#[tokio::test]
async fn tombstone_blocks_replayed_older_update_across_reopen() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("wctx.db");

    let epoch = {
        let s = SqliteStorage::open(&path, 30, None).unwrap();
        let epoch = s
            .begin_access_epoch("inst_1", "acct_opaque_1", Utc::now())
            .await
            .unwrap();
        // Commit revision 2, then commit a delete tombstone (revision 3).
        s.commit_page(commit(
            epoch,
            vec![envelope("evt_a", 2, epoch, "v2")],
            None,
            Some("c1"),
        ))
        .await
        .unwrap();
        let mut deleted = envelope("evt_a", 3, epoch, "gone");
        deleted.lifecycle = Lifecycle::Deleted;
        deleted.revision_fingerprint = compute_revision_fingerprint(
            RevisionModel::Monotonic,
            Some("3"),
            None,
            None,
            &deleted.content_hash,
            Lifecycle::Deleted,
        );
        s.commit_page(commit(epoch, vec![deleted], Some("c1"), Some("c2")))
            .await
            .unwrap();
        epoch
    };

    // After a restart, a replayed stale update (revision 1) does not resurrect the object.
    {
        let s = SqliteStorage::open(&path, 30, None).unwrap();
        let out = s
            .commit_page(commit(
                epoch,
                vec![envelope("evt_a", 1, epoch, "stale")],
                Some("c2"),
                Some("c3"),
            ))
            .await
            .unwrap();
        assert!(matches!(out, CommitOutcome::Committed { .. }));
        // The deleted object is not projected — delete-before-update safety holds on-disk too.
        assert_eq!(s.list_projectable(100).await.unwrap().len(), 0);
    }
}

#[tokio::test]
async fn a_page_from_a_stale_epoch_is_discarded() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("wctx.db");
    let s = SqliteStorage::open(&path, 30, None).unwrap();

    let epoch1 = s
        .begin_access_epoch("inst_1", "acct_opaque_1", Utc::now())
        .await
        .unwrap();
    // Re-authorization opens a new epoch.
    let epoch2 = s
        .begin_access_epoch("inst_1", "acct_opaque_1", Utc::now() + Duration::seconds(1))
        .await
        .unwrap();
    assert_eq!(epoch2, epoch1 + 1);

    // A page arriving with the old epoch is discarded and does not advance the cursor (revision I2).
    let out = s
        .commit_page(commit(
            epoch1,
            vec![envelope("evt_a", 1, epoch1, "a")],
            None,
            Some("c1"),
        ))
        .await
        .unwrap();
    assert_eq!(
        out,
        CommitOutcome::EpochMismatch {
            current_epoch: epoch2
        }
    );
    assert_eq!(s.list_projectable(100).await.unwrap().len(), 0);
}

#[tokio::test]
async fn conflicting_opaque_revisions_expose_no_winner_in_either_delivery_order() {
    // BLOCKING 1 (§6 rule 7 / Frozen Invariant 5): while quarantined, no winner is
    // exposed to search, suggestions, tasks, or the graph. Without deactivating the
    // prior active envelope, the delivery order would decide the winner — v1→v2 and
    // v2→v1 would expose different content. Both orders must project 0 (quarantined,
    // no winner).
    for (first, second) in [("h1", "h2"), ("h2", "h1")] {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("wctx.db");
        let s = SqliteStorage::open(&path, 30, None).unwrap();
        let epoch = s
            .begin_access_epoch("inst_1", "acct_opaque_1", Utc::now())
            .await
            .unwrap();

        // The first opaque revision is accepted as active.
        let out1 = s
            .commit_page(commit(
                epoch,
                vec![opaque_envelope("evt_x", epoch, first)],
                None,
                Some("c1"),
            ))
            .await
            .unwrap();
        assert!(matches!(out1, CommitOutcome::Committed { .. }));

        // A changed opaque revision arrives → incomparable quarantine (not comparable).
        let out2 = s
            .commit_page(commit(
                epoch,
                vec![opaque_envelope("evt_x", epoch, second)],
                Some("c1"),
                Some("c2"),
            ))
            .await
            .unwrap();
        assert!(matches!(out2, CommitOutcome::Committed { .. }));

        // Quarantined — no winner is projected (0). The prior active revision is gone too.
        assert_eq!(
            s.list_projectable(100).await.unwrap().len(),
            0,
            "delivery order {first}->{second}: a quarantined object must expose no winner"
        );
    }
}

#[tokio::test]
async fn revoke_then_replayed_newer_revision_does_not_resurrect() {
    // BLOCKING 2 (rejected alternative F / Frozen Invariant 4): after a revoke, even
    // if an at-least-once replay page carries a higher revision, the content must not
    // resurrect. A revoke ① leaves a content-free access_revoked tombstone (belt) and
    // ② bumps the access epoch (suspenders). A stale page arriving with the pre-revoke
    // epoch is discarded at the epoch gate and never reaches the merge.
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("wctx.db");
    let s = SqliteStorage::open(&path, 30, None).unwrap();
    let epoch = s
        .begin_access_epoch("inst_1", "acct_opaque_1", Utc::now())
        .await
        .unwrap();
    s.commit_page(commit(
        epoch,
        vec![envelope("evt_a", 1, epoch, "v1")],
        None,
        Some("c1"),
    ))
    .await
    .unwrap();
    assert_eq!(s.list_projectable(100).await.unwrap().len(), 1);

    // Revoke: remove content + access_revoked tombstone + epoch bump + cursor deletion.
    s.revoke_account("inst_1", "acct_opaque_1", Utc::now())
        .await
        .unwrap();
    assert_eq!(s.list_projectable(100).await.unwrap().len(), 0);

    // An in-flight page with the pre-revoke epoch replays a **higher** revision (9).
    let out = s
        .commit_page(commit(
            epoch,
            vec![envelope("evt_a", 9, epoch, "resurrect")],
            None,
            Some("c2"),
        ))
        .await
        .unwrap();
    assert!(
        matches!(out, CommitOutcome::EpochMismatch { .. }),
        "stale-epoch replay must be discarded at the epoch gate, got {out:?}"
    );
    assert_eq!(
        s.list_projectable(100).await.unwrap().len(),
        0,
        "revoked content must never resurrect from a replayed page"
    );
}

#[tokio::test]
async fn a_record_carrying_a_mismatched_epoch_is_ignored() {
    // IMPORTANT 3 (revision I2): every record in a page must carry that page's epoch.
    // A record with a mismatched epoch is ignored and is not written to the wrong
    // (source_object_key, epoch) partition.
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("wctx.db");
    let s = SqliteStorage::open(&path, 30, None).unwrap();
    let epoch = s
        .begin_access_epoch("inst_1", "acct_opaque_1", Utc::now())
        .await
        .unwrap();

    let good = envelope("evt_good", 1, epoch, "good");
    let mut bad = envelope("evt_bad", 1, epoch, "bad");
    bad.access_epoch_id = epoch + 998; // Mismatched with the page epoch — must be ignored.
    let out = s
        .commit_page(commit(epoch, vec![good, bad], None, Some("c1")))
        .await
        .unwrap();
    assert!(matches!(out, CommitOutcome::Committed { .. }));

    // Only the good record is projected — the forged-epoch record never reached the merge.
    let proj = s.list_projectable(100).await.unwrap();
    assert_eq!(proj.len(), 1);
    assert_eq!(proj[0].identity.remote_id, "evt_good");
}

#[tokio::test]
async fn envelope_without_projection_or_reference_expires_at_its_ceiling() {
    // IMPORTANT 2 (revision B1): an envelope with neither a projection nor a firm
    // reference must expire at its ceiling (default 30 days). #8587 writes no
    // projection at all, so every envelope is this case. expire_planes flips an active
    // envelope past the ceiling to retention_expired and leaves only a content-free
    // tombstone.
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("wctx.db");
    let s = SqliteStorage::open(&path, 30, None).unwrap();
    let epoch = s
        .begin_access_epoch("inst_1", "acct_opaque_1", Utc::now())
        .await
        .unwrap();

    // Set ingest to 40 days ago → expires_at = ingest + 30 days = 10 days ago (past the ceiling).
    let mut old = envelope("evt_a", 1, epoch, "old");
    old.ingested_at = Utc::now() - Duration::days(40);
    let key = old.source_object_key.clone();
    s.commit_page(commit(epoch, vec![old], None, Some("c1")))
        .await
        .unwrap();

    // Before sweep: the row exists (not yet flipped).
    assert!(s.get_envelope(&key, epoch).await.unwrap().is_some());

    let removed = s.expire_planes(Utc::now()).await.unwrap();
    assert!(
        removed >= 1,
        "an expired envelope must be swept, removed={removed}"
    );

    // After expiry: the envelope is gone and projection is 0.
    assert!(s.get_envelope(&key, epoch).await.unwrap().is_none());
    assert_eq!(s.list_projectable(100).await.unwrap().len(), 0);

    // The remaining retention_expired tombstone suppresses a same-epoch replay (resurrection prevention).
    let out = s
        .commit_page(commit(
            epoch,
            vec![envelope("evt_a", 1, epoch, "old")],
            Some("c1"),
            Some("c2"),
        ))
        .await
        .unwrap();
    assert!(matches!(out, CommitOutcome::Committed { .. }));
    assert_eq!(
        s.list_projectable(100).await.unwrap().len(),
        0,
        "the retention-expiry tombstone must suppress a same-epoch replay"
    );
}
