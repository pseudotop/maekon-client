//! #9689: duplicate tag names must surface as 409 Conflict, not 500.
//!
//! `tags.name` is `NOT NULL UNIQUE` (migration v01_v08.rs:189). Before this,
//! the constraint violation travelled `StorageError::Internal` -> the ApiError
//! catch-all -> HTTP 500, whose body is replaced by the literal "internal
//! server error" — so a name collision was indistinguishable from a crash, in
//! every locale.
//!
//! These run against real in-memory SQLite rather than a fake, so they also pin
//! the collation assumption the guards are built on: `tags.name` carries no
//! `COLLATE NOCASE`, so "work" and "Work" are distinct rows and BOTH the
//! service pre-check and the client-side guards must be exact, not
//! case-insensitive.

use maekon_api_contracts::tags::{CreateTagRequest, UpdateTagRequest};
use maekon_web::error::ApiError;
use maekon_web::services::tags_service::TagsCommandService;
use maekon_web::services::web_contexts::StorageWebContext;

#[path = "support/in_memory_storage.rs"]
mod in_memory_storage;
use in_memory_storage::in_memory_storage;

fn command_service() -> TagsCommandService {
    let ctx = StorageWebContext {
        storage: in_memory_storage(),
        frames_dir: None,
        frame_storage: None,
        pii_sanitizer: None,
        erasure_requested: None,
        memory_vault_writer: None,
    };
    TagsCommandService::new(ctx)
}

fn create(service: &TagsCommandService, name: &str) -> i64 {
    futures::executor::block_on(service.create_tag(&CreateTagRequest {
        name: name.to_string(),
        color: Some("#3b82f6".to_string()),
    }))
    .expect("create should succeed")
    .id
}

#[tokio::test]
async fn renaming_onto_another_tags_name_is_a_conflict_not_an_internal_error() {
    let service = command_service();
    create(&service, "work");
    let personal = create(&service, "personal");

    let error = service
        .update_tag(
            personal,
            &UpdateTagRequest {
                name: "work".to_string(),
                color: "#ef4444".to_string(),
            },
        )
        .await
        .expect_err("renaming onto an existing name must fail");

    match error {
        ApiError::Conflict(message) => {
            assert!(
                message.contains("work"),
                "the conflict must name the tag so the client can explain it, got: {message}"
            );
        }
        other => panic!("expected Conflict, got {other:?}"),
    }
}

#[tokio::test]
async fn saving_a_tag_under_its_own_name_changes_only_the_colour() {
    // The `exclude_id` half of the predicate: a colour-only save keeps the
    // name, which must not be read as colliding with itself.
    let service = command_service();
    let work = create(&service, "work");

    let updated = service
        .update_tag(
            work,
            &UpdateTagRequest {
                name: "work".to_string(),
                color: "#10b981".to_string(),
            },
        )
        .await
        .expect("a colour-only save must not be treated as a name collision");

    assert_eq!(updated.name, "work");
    assert_eq!(updated.color, "#10b981");
}

#[tokio::test]
async fn creating_a_duplicate_name_is_a_conflict_not_an_internal_error() {
    // The create path was left asymmetric by the first pass: rename returned a
    // usable 409 while create still fell through to 500.
    let service = command_service();
    create(&service, "work");

    let error = service
        .create_tag(&CreateTagRequest {
            name: "work".to_string(),
            color: None,
        })
        .await
        .expect_err("creating a duplicate name must fail");

    assert!(
        matches!(error, ApiError::Conflict(_)),
        "expected Conflict, got {error:?}"
    );
}

#[tokio::test]
async fn names_differing_only_by_case_are_distinct_rows() {
    // SQLite compares TEXT as BINARY here, so both of these must be accepted.
    // A case-insensitive guard anywhere would reject work the database allows —
    // and, in the rename dialog, would disable Save on both tags permanently.
    let service = command_service();
    create(&service, "work");

    let upper = service
        .create_tag(&CreateTagRequest {
            name: "Work".to_string(),
            color: None,
        })
        .await
        .expect("a case variant is a different name to SQLite");

    assert_eq!(upper.name, "Work");

    // And renaming onto a case variant is likewise allowed.
    service
        .update_tag(
            upper.id,
            &UpdateTagRequest {
                name: "WORK".to_string(),
                color: "#3b82f6".to_string(),
            },
        )
        .await
        .expect("renaming to another case variant must be allowed");
}
