//! End-to-end integration for the trusted Skill Pack resolver (#8588,
//! adversarial-review Fix 3).
//!
//! Drives a REAL `SqliteStorage` through the producer half — register the pack
//! from a verified, enabled install plus an on-disk body, then record the
//! explicit activation — and asserts `RegistryActiveSkillResolver` returns the
//! actual verified body (`Activated`). A second "process" (reopen) proves that a
//! stale activation whose owning package moved on (an update stranded its
//! version) is BLOCKED rather than resurrected, and that an activation against a
//! disabled install never registers at all.
//!
//! This is the reachability proof: register → activate → resolve → verified body.

use std::sync::Arc;

use chrono::Utc;
use tempfile::TempDir;

use maekon_app::skill_pack_resolver::RegistryActiveSkillResolver;
use maekon_core::error::CoreError;
use maekon_core::models::extension::{
    Contribution, ContributionKind, ExecutionLocation, ExtensionManifest, RuntimeKind,
    SignatureState, SourceKind,
};
use maekon_core::models::skill::{Skill, SkillMeta};
use maekon_core::models::skill_pack::{body_digest, SkillActivationOutcome, SkillBlockedReason};
use maekon_core::ports::extension_registry::{
    ExtensionRegistryCommandPort, InstallRequest, RegisterPackageRequest, SetEnablementRequest,
    UpdateRequest,
};
// After register (rev 1) + install (rev 2), enablement is a separate axis that
// defaults to Disabled — install ≠ enable (ADR-029). Tests that need an operable
// package enable it explicitly, taking the install to rev 3.
use maekon_core::ports::skill_loader::SkillLoader;
use maekon_core::ports::skill_pack_registry::ActiveSkillResolverPort;
use maekon_storage::sqlite::SqliteStorage;

const SKILL_NAME: &str = "review";
const BODY: &str = "Summarize the diff. Never run shell commands.";

/// A byte-only body source keyed by skill name — the same key the resolver
/// fetches by (`entry.skill_id`), so registration and body-source agree.
struct StubLoader {
    name: String,
    body: String,
}

impl SkillLoader for StubLoader {
    fn list_skills(&self) -> Vec<SkillMeta> {
        vec![SkillMeta {
            name: self.name.clone(),
            description: "test skill".to_string(),
        }]
    }

    fn get_skill(&self, name: &str) -> Result<Skill, CoreError> {
        if name == self.name {
            Ok(Skill {
                meta: SkillMeta {
                    name: self.name.clone(),
                    description: "test skill".to_string(),
                },
                body: self.body.clone(),
                source_path: "review.md".to_string(),
            })
        } else {
            Err(CoreError::NotFound {
                code: maekon_core::error_codes::NotFoundCode::ResourceMissing,
                resource_type: "Skill".into(),
                id: name.into(),
            })
        }
    }

    fn reload(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

/// A trusted, Phase-1-available owning package (context_source, local). Its
/// availability gate is what the resolver requires before a skill body may
/// occupy instruction position.
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

/// Register + install, leaving the package installed but Disabled (the default).
async fn setup_installed_package(storage: &SqliteStorage) {
    storage
        .register_package(RegisterPackageRequest {
            install_id: "inst_1".into(),
            manifest: manifest(),
            now: Utc::now(),
        })
        .await
        .unwrap();
    storage
        .install(InstallRequest {
            install_id: "inst_1".into(),
            expected_revision: 1,
            now: Utc::now(),
        })
        .await
        .unwrap();
}

/// Register + install + enable — an operable package at revision 3.
async fn setup_enabled_package(storage: &SqliteStorage) {
    setup_installed_package(storage).await;
    storage
        .set_enablement(SetEnablementRequest {
            install_id: "inst_1".into(),
            enabled: true,
            expected_revision: 2,
            now: Utc::now(),
        })
        .await
        .unwrap();
}

fn resolver(storage: &Arc<SqliteStorage>) -> RegistryActiveSkillResolver {
    let loader = Arc::new(StubLoader {
        name: SKILL_NAME.to_string(),
        body: BODY.to_string(),
    });
    RegistryActiveSkillResolver::new(storage.clone(), storage.clone(), storage.clone(), loader)
}

#[tokio::test]
async fn register_activate_resolve_returns_the_verified_body() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("skillpack.db");
    let storage = Arc::new(SqliteStorage::open(&path, 30, None).unwrap());
    setup_enabled_package(&storage).await;

    let resolver = resolver(&storage);
    let entry = resolver
        .activate_explicit("inst_1", SKILL_NAME)
        .await
        .expect("explicit activation of a verified, enabled package must succeed");
    // body_sha256 was derived from disk, not supplied by the caller.
    assert_eq!(entry.body_sha256, body_digest(BODY));
    assert_eq!(entry.skill_id, SKILL_NAME);
    assert_eq!(entry.contribution_kind, ContributionKind::SkillPack);

    match resolver.resolve_active_skill().await.unwrap() {
        SkillActivationOutcome::Activated(active) => {
            assert_eq!(active.verified_body(), BODY);
            assert_eq!(active.skill_id, SKILL_NAME);
            assert_eq!(active.version, "1.0.0");
        }
        other => panic!("expected an activated verified skill, got {other:?}"),
    }
}

#[tokio::test]
async fn stale_activation_is_blocked_after_reopen_and_update() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("skillpack.db");

    // First "process": activate, then the owning package is updated — which
    // strands the recorded activation's version.
    {
        let storage = Arc::new(SqliteStorage::open(&path, 30, None).unwrap());
        setup_enabled_package(&storage).await;
        resolver(&storage)
            .activate_explicit("inst_1", SKILL_NAME)
            .await
            .expect("activation");
        storage
            .update(UpdateRequest {
                install_id: "inst_1".into(),
                target_version: "2.0.0".into(),
                expected_revision: 3,
                now: Utc::now(),
            })
            .await
            .unwrap();
    }

    // Second "process": the stale activation must fail closed, not resurrect the
    // body under the new version.
    {
        let storage = Arc::new(SqliteStorage::open(&path, 30, None).unwrap());
        let outcome = resolver(&storage).resolve_active_skill().await.unwrap();
        assert!(
            matches!(
                outcome.blocked_reason(),
                Some(SkillBlockedReason::VersionChanged { .. })
            ),
            "a stale activation must be blocked after an update, got {outcome:?}"
        );
    }
}

#[tokio::test]
async fn activation_against_a_disabled_install_fails_closed() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("skillpack.db");
    let storage = Arc::new(SqliteStorage::open(&path, 30, None).unwrap());
    setup_installed_package(&storage).await;
    storage
        .set_enablement(SetEnablementRequest {
            install_id: "inst_1".into(),
            enabled: false,
            expected_revision: 2,
            now: Utc::now(),
        })
        .await
        .unwrap();

    let err = resolver(&storage)
        .activate_explicit("inst_1", SKILL_NAME)
        .await
        .expect_err("a disabled owning package must refuse registration/activation");
    assert!(
        err.to_string().contains("disabled"),
        "the refusal must name the disabled precondition, got: {err}"
    );
}
