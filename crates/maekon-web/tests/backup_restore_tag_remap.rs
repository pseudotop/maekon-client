//! #9700 + #9708: restoring a backup into a POPULATED database must not
//! mis-attach or orphan frame-tag relations — on EITHER of the row's two
//! foreign keys.
//!
//! `tag_id` is arbitrated by name (#9700); `frame_id` is arbitrated by
//! relocation (#9708), because frames have no natural identity key and
//! `frames.id` is AUTOINCREMENT — on any device that has been capturing, the
//! archived ids are already occupied.
//!
//! ONE RESIDUAL HAZARD, deliberately open: when the archive carries NO frames
//! (`include_tags` and `include_frames` are independent flags), relation ids
//! refer to frames already on this device and are passed through. Existence is
//! checked, so no dangling row is written — but on a CROSS-DEVICE restore a
//! passed-through id may name a different screenshot than the one the archive
//! meant. That is unfixable from archive data alone: a tags-only archive
//! carries no way to identify the frame. Same-device restore, the common case,
//! is correct.
//!
//! Restore merges — nothing clears `tags` first — so an archived tag id may
//! already belong to a different tag on this device. The previous code wrote
//! `INSERT OR IGNORE`, discarded the affected-row count, counted the tag as
//! restored anyway, and then wrote `frame_tags` against the ARCHIVE's id. With
//! the FK not covering this direction, that produced, silently (#9735: the
//! PRAGMA is ON — the old comment here said otherwise):
//!   - a relation attached to the WRONG tag (id taken by a different tag), or
//!   - a relation pointing at nothing (name taken under a different id).
//!
//! These run against real in-memory SQLite so the actual PK/UNIQUE constraints
//! decide the outcome.

use maekon_api_contracts::backup::{
    BackupArchive, BackupIncludes, BackupMetadata, FrameBackup, FrameTagBackup, TagBackup,
};
use maekon_web::services::backup_service::BackupCommandService;
use maekon_web::services::web_contexts::BackupWebContext;

#[path = "support/in_memory_storage.rs"]
mod in_memory_storage;
use in_memory_storage::in_memory_storage;

fn archive(tags: Vec<TagBackup>, frame_tags: Vec<FrameTagBackup>) -> BackupArchive {
    BackupArchive {
        metadata: BackupMetadata {
            version: "1.0".to_string(),
            created_at: "2026-07-31T00:00:00Z".to_string(),
            app_version: "test".to_string(),
            includes: BackupIncludes {
                settings: false,
                tags: true,
                events: false,
                frames: false,
            },
        },
        settings: None,
        tags: Some(tags),
        frame_tags: Some(frame_tags),
        events: None,
        frames: None,
    }
}

fn tag(id: i64, name: &str) -> TagBackup {
    TagBackup {
        id,
        name: name.to_string(),
        color: "#3b82f6".to_string(),
        created_at: "2025-06-01T00:00:00Z".to_string(),
    }
}

fn relation(frame_id: i64, tag_id: i64) -> FrameTagBackup {
    FrameTagBackup {
        frame_id,
        tag_id,
        created_at: "2025-06-01T10:00:00Z".to_string(),
    }
}

fn service(
    storage: std::sync::Arc<dyn maekon_web::storage_port::WebStorage>,
) -> BackupCommandService {
    BackupCommandService::new(BackupWebContext {
        storage,
        config_manager: None,
        pii_sanitizer: None,
    })
}

/// `frame_tags` references a real frame row, so seed one before asserting on
/// relations (mirrors the storage-suite `insert_frame` prerequisite).
async fn seed_frame(storage: &std::sync::Arc<dyn maekon_web::storage_port::WebStorage>, id: i64) {
    storage
        .upsert_backup_frame(
            id,
            "2025-06-01T10:00:00Z",
            "manual",
            "app",
            "title",
            0.5,
            100,
            100,
            None,
        )
        .await
        .expect("seed frame");
}

#[tokio::test]
async fn a_relation_follows_its_tag_when_the_archived_id_is_already_taken() {
    let storage = in_memory_storage();
    seed_frame(&storage, 1).await;

    // This device already owns id 5, under a different name.
    storage
        .upsert_backup_tag(5, "personal", "#ef4444", "2025-05-01T00:00:00Z")
        .await
        .expect("seed the squatter");

    // The archive's tag 5 is a different tag, and frame 1 is tagged with it.
    let result = service(storage.clone())
        .restore_backup(&archive(vec![tag(5, "work")], vec![relation(1, 5)]))
        .await
        .expect("restore should not fail");

    assert_eq!(result.restored.tags, 1);
    assert_eq!(result.restored.frame_tags, 1);

    let tags = storage.list_backup_tags().await.expect("read tags");
    let work = tags
        .iter()
        .find(|t| t.name == "work")
        .expect("the archived tag must exist");
    assert_ne!(work.id, 5, "it cannot occupy the id the squatter holds");
    assert_eq!(
        tags.iter().find(|t| t.id == 5).unwrap().name,
        "personal",
        "the pre-existing tag must be untouched"
    );

    let links = storage
        .list_backup_frame_tags()
        .await
        .expect("read frame_tags");
    assert_eq!(links.len(), 1);
    assert_eq!(
        links[0].tag_id, work.id,
        "the relation must follow 'work', not stay on the squatter's id"
    );
}

#[tokio::test]
async fn a_relation_reuses_the_existing_row_when_the_name_is_already_present() {
    let storage = in_memory_storage();
    seed_frame(&storage, 1).await;

    // Same tag, different id on this device — the usual cross-device case.
    storage
        .upsert_backup_tag(3, "work", "#3b82f6", "2025-05-01T00:00:00Z")
        .await
        .expect("seed the local tag");

    service(storage.clone())
        .restore_backup(&archive(vec![tag(9, "work")], vec![relation(1, 9)]))
        .await
        .expect("restore should not fail");

    let tags = storage.list_backup_tags().await.expect("read tags");
    assert_eq!(tags.len(), 1, "a name that exists must not be duplicated");

    let links = storage
        .list_backup_frame_tags()
        .await
        .expect("read frame_tags");
    assert_eq!(
        links[0].tag_id, 3,
        "the relation must point at the local row, not the archived id 9"
    );
}

#[tokio::test]
async fn a_relation_whose_tag_is_missing_from_the_archive_is_reported_not_dangled() {
    let storage = in_memory_storage();
    seed_frame(&storage, 1).await;

    // frame_tags references tag 42, which the archive does not carry.
    let result = service(storage.clone())
        .restore_backup(&archive(vec![tag(1, "work")], vec![relation(1, 42)]))
        .await
        .expect("restore should not fail wholesale");

    assert_eq!(
        result.restored.frame_tags, 0,
        "a relation with no valid target must not be counted as restored"
    );
    // Tag axis is a FAILURE, not a note: tag deletion cleans up `frame_tags`
    // (#6246) and the export always emits tags with their relations, so a
    // missing tag means the archive is corrupt.
    assert!(
        result.errors.iter().any(|e| e.contains("42")),
        "the skip must be reported as an error — errors: {:?}",
        result.errors
    );
    assert!(
        !result.success,
        "a corrupt archive must not report a clean restore"
    );

    let links = storage
        .list_backup_frame_tags()
        .await
        .expect("read frame_tags");
    assert!(links.is_empty(), "no dangling row may be written");
}

#[tokio::test]
async fn a_relation_follows_its_frame_when_the_archived_frame_id_is_already_taken() {
    // #9708: the frame axis. Frame 1 is already this device's own screenshot;
    // the archive's frame 1 is a different capture entirely.
    let storage = in_memory_storage();
    seed_frame(&storage, 1).await;

    let result = service(storage.clone())
        .restore_backup(&BackupArchive {
            frames: Some(vec![FrameBackup {
                id: 1,
                timestamp: "2025-01-01T09:00:00Z".to_string(),
                trigger_type: "manual".to_string(),
                app_name: "archived-app".to_string(),
                window_title: "archived-title".to_string(),
                importance: 0.9,
                width: 200,
                height: 200,
                ocr_text: None,
            }]),
            ..archive(vec![tag(1, "work")], vec![relation(1, 1)])
        })
        .await
        .expect("restore should not fail");

    assert_eq!(result.restored.frames, 1);
    assert_eq!(result.restored.frame_tags, 1);

    let links = storage
        .list_backup_frame_tags()
        .await
        .expect("read frame_tags");
    assert_eq!(links.len(), 1);
    assert_ne!(
        links[0].frame_id, 1,
        "the relation must follow the archived frame to its new id, not stay on this device's own screenshot"
    );
}

#[tokio::test]
async fn relations_pass_through_when_the_archive_carries_no_frames() {
    // `include_frames` and `include_tags` are independent flags, so a
    // tags-only archive legitimately references frames already on this device.
    // Those ids must NOT be arbitrated away.
    let storage = in_memory_storage();
    seed_frame(&storage, 1).await;

    let result = service(storage.clone())
        .restore_backup(&archive(vec![tag(1, "work")], vec![relation(1, 1)]))
        .await
        .expect("restore should not fail");

    assert_eq!(result.restored.frame_tags, 1);
    let links = storage
        .list_backup_frame_tags()
        .await
        .expect("read frame_tags");
    assert_eq!(
        links[0].frame_id, 1,
        "a local frame reference must survive a frames-less archive"
    );
}

#[tokio::test]
async fn a_frames_less_archive_does_not_dangle_a_relation_onto_a_missing_frame() {
    // #9714 review I1: the pass-through must still verify the frame is here.
    // Writing it blind produced a dangling row that nothing reported.
    let storage = in_memory_storage();
    // No frame is seeded — the archive references frame 7, which is absent.

    let result = service(storage.clone())
        .restore_backup(&archive(vec![tag(1, "work")], vec![relation(7, 1)]))
        .await
        .expect("restore should not fail");

    assert_eq!(result.restored.frame_tags, 0);
    assert!(
        storage
            .list_backup_frame_tags()
            .await
            .expect("read frame_tags")
            .is_empty(),
        "no dangling row may be written"
    );
    assert!(
        result.success,
        "a stale relation is data-hygiene noise (#9721), not a failed restore"
    );
    assert!(
        result.notes.iter().any(|n| n.contains('7')),
        "the skip must be reported as a note — notes: {:?}",
        result.notes
    );
}

#[tokio::test]
async fn an_archive_with_an_empty_frames_list_skips_relations_without_failing() {
    // #9714 review I2: `frames: Some(vec![])` is NOT the same shape as
    // `frames: None` — the archive claimed to bring frames and brought none, so
    // its relations have no valid target. Skipping is right; failing is not.
    let storage = in_memory_storage();
    seed_frame(&storage, 1).await;

    let result = service(storage.clone())
        .restore_backup(&BackupArchive {
            frames: Some(vec![]),
            ..archive(vec![tag(1, "work")], vec![relation(1, 1)])
        })
        .await
        .expect("restore should not fail");

    assert_eq!(result.restored.frame_tags, 0);
    assert!(
        result.success,
        "orphan relations must not flip success — they are the steady state (#9721)"
    );
    assert!(!result.notes.is_empty(), "the skip must still be reported");
}

#[tokio::test]
async fn restoring_the_same_frames_less_archive_twice_does_not_report_a_missing_frame() {
    // The guarded insert uses `OR IGNORE`, so a relation that already exists
    // affects 0 rows — indistinguishable from "frame missing" unless the
    // storage layer disambiguates. Without that, a repeat restore would claim
    // every relation it already holds points at a frame that is not here.
    let storage = in_memory_storage();
    seed_frame(&storage, 1).await;

    let first = service(storage.clone())
        .restore_backup(&archive(vec![tag(1, "work")], vec![relation(1, 1)]))
        .await
        .expect("first restore");
    assert_eq!(first.restored.frame_tags, 1);

    let second = service(storage.clone())
        .restore_backup(&archive(vec![tag(1, "work")], vec![relation(1, 1)]))
        .await
        .expect("second restore");
    assert_eq!(
        second.restored.frame_tags, 1,
        "an already-present relation is still present, not missing"
    );
    assert!(
        second.notes.is_empty(),
        "a repeat restore must not claim the frame is gone — notes: {:?}",
        second.notes
    );
}
