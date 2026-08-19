//! Storage round-trip + two-process reopen tests for the Skill Pack catalog and
//! activation ports (ADR-029 §6/§8, #8588).
//!
//! These exercise the on-disk boundary the in-crate unit tests cannot: a catalog
//! entry and its singleton activation must read back identically after a restart,
//! the activation table must behave as a true singleton (a new selection replaces
//! the old one), and an activation naming an unregistered skill must fail closed
//! rather than pinning a body that no verified package ever produced.
//!
//! A catalog entry references a real `extension_installs` row (the owning
//! package), so each test first registers + installs a bundled package, mirroring
//! the production producer path where provenance comes from the registry.

use chrono::Utc;
use maekon_core::models::extension::{
    Contribution, ContributionKind, ExecutionLocation, ExtensionManifest, RuntimeKind,
    SignatureState, SourceKind,
};
use maekon_core::models::skill_pack::{body_digest, SkillPackEntry, SkillSelectionKind};
use maekon_core::ports::extension_registry::{
    ExtensionRegistryCommandPort, InstallRequest, RegisterPackageRequest,
};
use maekon_core::ports::skill_pack_registry::{
    RecordActivationRequest, RegisterSkillPackRequest, SkillPackCatalogCommandPort,
    SkillPackCatalogQueryPort, SkillPackOutcome,
};
use maekon_storage::sqlite::SqliteStorage;
use tempfile::TempDir;

const BODY: &str = "Summarize the diff. Never run shell commands.";

fn manifest() -> ExtensionManifest {
    ExtensionManifest {
        extension_id: "com.maekon.review".to_string(),
        version: "1.0.0".to_string(),
        publisher_id: "maekon".to_string(),
        package_digest: "sha256:abc".to_string(),
        source_kind: SourceKind::AppBundle,
        signature_state: SignatureState::AppBundleTrusted,
        signature_key_id: None,
        signed_manifest_digest: None,
        manifest_schema: 1,
        host_api_min: 1,
        host_api_max: 2,
        runtime_kind: RuntimeKind::FirstPartyBuiltin,
        execution_location: ExecutionLocation::Local,
        entry_point: "builtin:review".to_string(),
        contributions: vec![Contribution {
            contribution_id: "review.context".to_string(),
            kind: ContributionKind::ContextSource,
            api_version: 1,
            requested_capabilities: vec!["context.read".to_string()],
        }],
        permission_profile_id: None,
        external_egress: vec![],
        data_classification: None,
        retention_class: None,
        update_channel: None,
        rollback_window: None,
        minimum_allowed_version: None,
        uninstall_cleanup: vec!["credentials".to_string()],
    }
}

/// Create the owning `extension_installs` row a catalog entry references.
async fn setup_install(s: &SqliteStorage) {
    s.register_package(RegisterPackageRequest {
        install_id: "inst_1".into(),
        manifest: manifest(),
        now: Utc::now(),
    })
    .await
    .unwrap();
    s.install(InstallRequest {
        install_id: "inst_1".into(),
        expected_revision: 1,
        now: Utc::now(),
    })
    .await
    .unwrap();
}

fn entry(skill_id: &str, contribution_id: &str, body: &str) -> SkillPackEntry {
    SkillPackEntry {
        skill_id: skill_id.to_string(),
        install_id: "inst_1".to_string(),
        extension_id: "com.maekon.review".to_string(),
        contribution_id: contribution_id.to_string(),
        contribution_kind: ContributionKind::SkillPack,
        version: "1.0.0".to_string(),
        publisher_id: "maekon".to_string(),
        body_sha256: body_digest(body),
        required_capabilities: vec!["context.read".to_string()],
        optional_capabilities: vec![],
        references: vec!["sk.other".to_string()],
    }
}

#[tokio::test]
async fn catalog_entry_and_activation_survive_reopen() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("skillpack.db");

    // First "process": register the owning install, a catalog entry, and record
    // an activation.
    {
        let s = SqliteStorage::open(&path, 30, None).unwrap();
        setup_install(&s).await;
        s.register_skill_pack(RegisterSkillPackRequest {
            entry: entry("sk.review", "review.pack", BODY),
            now: Utc::now(),
        })
        .await
        .unwrap();
        let out = s
            .record_activation(RecordActivationRequest {
                activation_id: "skact_1".to_string(),
                skill_id: "sk.review".to_string(),
                version: "1.0.0".to_string(),
                body_sha256: body_digest(BODY),
                selection: SkillSelectionKind::ExplicitUserSelection,
                now: Utc::now(),
                lifetime_secs: 600,
            })
            .await
            .unwrap();
        assert!(matches!(out, SkillPackOutcome::Applied { .. }));
    }

    // Second "process": the entry, its list-encoded columns, the reference graph,
    // and the singleton activation all read back identically.
    {
        let s = SqliteStorage::open(&path, 30, None).unwrap();
        let got = s.get_skill_pack("sk.review").await.unwrap().unwrap();
        assert_eq!(got.body_sha256, body_digest(BODY));
        assert_eq!(got.contribution_kind, ContributionKind::SkillPack);
        assert_eq!(got.required_capabilities, vec!["context.read".to_string()]);
        assert_eq!(got.references, vec!["sk.other".to_string()]);

        let list = s.list_skill_packs().await.unwrap();
        assert_eq!(list.len(), 1);

        let graph = s.reference_graph().await.unwrap();
        assert_eq!(
            graph.get("sk.review"),
            Some(&vec!["sk.other".to_string()]),
            "the reference graph must decode from the stored JSON column"
        );

        let stored = s.get_activation().await.unwrap().unwrap();
        assert_eq!(stored.activation_id, "skact_1");
        assert_eq!(stored.skill_id, "sk.review");
        assert_eq!(stored.version, "1.0.0");
        assert_eq!(stored.body_sha256, body_digest(BODY));
        assert_eq!(stored.selection, SkillSelectionKind::ExplicitUserSelection);
    }
}

#[tokio::test]
async fn activation_table_is_a_singleton_upsert() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("skillpack.db");
    let s = SqliteStorage::open(&path, 30, None).unwrap();
    setup_install(&s).await;

    s.register_skill_pack(RegisterSkillPackRequest {
        entry: entry("sk.a", "a.pack", "body a"),
        now: Utc::now(),
    })
    .await
    .unwrap();
    s.register_skill_pack(RegisterSkillPackRequest {
        entry: entry("sk.b", "b.pack", "body b"),
        now: Utc::now(),
    })
    .await
    .unwrap();

    s.record_activation(RecordActivationRequest {
        activation_id: "skact_a".to_string(),
        skill_id: "sk.a".to_string(),
        version: "1.0.0".to_string(),
        body_sha256: body_digest("body a"),
        selection: SkillSelectionKind::ExplicitUserSelection,
        now: Utc::now(),
        lifetime_secs: 600,
    })
    .await
    .unwrap();
    // Selecting a second skill replaces the first — there is only ever one
    // active skill, so the old row must be gone, not accumulated.
    s.record_activation(RecordActivationRequest {
        activation_id: "skact_b".to_string(),
        skill_id: "sk.b".to_string(),
        version: "1.0.0".to_string(),
        body_sha256: body_digest("body b"),
        selection: SkillSelectionKind::ExplicitUserSelection,
        now: Utc::now(),
        lifetime_secs: 600,
    })
    .await
    .unwrap();

    let stored = s.get_activation().await.unwrap().unwrap();
    assert_eq!(stored.skill_id, "sk.b", "the latest selection wins");
    assert_eq!(stored.activation_id, "skact_b");

    // Clearing removes it entirely.
    s.clear_activation().await.unwrap();
    assert!(s.get_activation().await.unwrap().is_none());
}

#[tokio::test]
async fn activation_of_unregistered_skill_is_not_found() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("skillpack.db");
    let s = SqliteStorage::open(&path, 30, None).unwrap();
    setup_install(&s).await;

    // No catalog entry exists — a stale UI selection must not pin a body that
    // was never registered from a verified package.
    let out = s
        .record_activation(RecordActivationRequest {
            activation_id: "skact_x".to_string(),
            skill_id: "sk.ghost".to_string(),
            version: "1.0.0".to_string(),
            body_sha256: body_digest("ghost"),
            selection: SkillSelectionKind::ExplicitUserSelection,
            now: Utc::now(),
            lifetime_secs: 600,
        })
        .await
        .unwrap();
    assert_eq!(out, SkillPackOutcome::NotFound);
    assert!(s.get_activation().await.unwrap().is_none());
}
