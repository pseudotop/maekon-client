//! Two-process reopen tests for the Extension registry (ADR-029 §5, #8586).
//!
//! #8586's acceptance criteria require that installed version, grants, and
//! disabled/degraded state read back identically after a restart, and that a
//! failed update either rolls back to the previous known-good version or stays
//! explicitly unavailable. These exercise the on-disk boundary that in-memory
//! unit tests cannot.

use chrono::Utc;
use maekon_core::models::extension::{
    AccountAuthentication, Availability, CapabilityGrant, Contribution, ContributionKind,
    Enablement, ExecutionLocation, ExtensionManifest, ExtensionProvenance, Health, HealthReason,
    InstallationState, RuntimeKind, SignatureState, SourceKind, UnavailableReason, UpdateState,
};
use maekon_core::ports::extension_registry::{
    ExtensionRegistryCommandPort, ExtensionRegistryQueryPort, InstallRequest, RecordHealthRequest,
    RegisterPackageRequest, RegistryOutcome, RollbackRequest, SetEnablementRequest,
    SetGrantRequest, UpdateRequest,
};
use maekon_storage::sqlite::SqliteStorage;
use tempfile::TempDir;

fn manifest(extension_id: &str, location: ExecutionLocation) -> ExtensionManifest {
    ExtensionManifest {
        extension_id: extension_id.to_string(),
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
        execution_location: location,
        entry_point: "builtin:calendar".to_string(),
        contributions: vec![Contribution {
            contribution_id: "calendar.read".to_string(),
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

#[tokio::test]
async fn install_grant_and_disabled_state_survive_reopen() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("registry.db");

    // First "process": register, install, grant, then deliberately disable.
    {
        let s = SqliteStorage::open(&path, 30, None).unwrap();
        s.register_package(RegisterPackageRequest {
            install_id: "inst_1".into(),
            manifest: manifest("com.maekon.calendar", ExecutionLocation::Local),
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
        s.set_grant(SetGrantRequest {
            install_id: "inst_1".into(),
            grant: CapabilityGrant::Granted,
            expected_revision: 2,
            now: Utc::now(),
        })
        .await
        .unwrap();
        s.record_health(RecordHealthRequest {
            install_id: "inst_1".into(),
            health: Health::Degraded {
                reason: HealthReason::RateLimited,
            },
            now: Utc::now(),
        })
        .await
        .unwrap();
        // Disabled is a deliberate user state and must not "heal" on restart.
        s.set_enablement(SetEnablementRequest {
            install_id: "inst_1".into(),
            enabled: false,
            expected_revision: 3,
            now: Utc::now(),
        })
        .await
        .unwrap();
    }

    // Second "process": every axis reads back identically.
    {
        let s = SqliteStorage::open(&path, 30, None).unwrap();
        let row = s.get_install("inst_1").await.unwrap().unwrap();
        assert_eq!(row.version, "1.0.0");
        assert_eq!(row.provenance, ExtensionProvenance::Bundled);
        assert_eq!(row.installation, InstallationState::Installed);
        assert_eq!(
            row.enablement,
            Enablement::Disabled,
            "disabled must persist"
        );
        assert_eq!(row.grant, CapabilityGrant::Granted, "grant must persist");
        assert_eq!(
            row.health,
            Health::Degraded {
                reason: HealthReason::RateLimited
            },
            "degraded health must persist"
        );

        // The readiness projection stays honest after reopen.
        let readiness = &s.list_readiness().await.unwrap()[0];
        assert_eq!(readiness.availability, Availability::Available);
        assert_eq!(readiness.summary_label(), Some("installed"));
    }
}

#[tokio::test]
async fn update_then_rollback_survives_reopen() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("registry.db");

    {
        let s = SqliteStorage::open(&path, 30, None).unwrap();
        s.register_package(RegisterPackageRequest {
            install_id: "inst_1".into(),
            manifest: manifest("com.maekon.calendar", ExecutionLocation::Local),
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
        s.update(UpdateRequest {
            install_id: "inst_1".into(),
            target_version: "2.0.0".into(),
            expected_revision: 2,
            now: Utc::now(),
        })
        .await
        .unwrap();
    }

    // Reopen: the new version and its known-good predecessor both persisted.
    {
        let s = SqliteStorage::open(&path, 30, None).unwrap();
        let row = s.get_install("inst_1").await.unwrap().unwrap();
        assert_eq!(row.version, "2.0.0");
        assert_eq!(row.previous_version.as_deref(), Some("1.0.0"));

        // A failed update is recovered by rolling back to the known-good version.
        s.rollback(RollbackRequest {
            install_id: "inst_1".into(),
            expected_revision: row.revision,
            now: Utc::now(),
        })
        .await
        .unwrap();
    }

    // Reopen again: the rollback is durable.
    {
        let s = SqliteStorage::open(&path, 30, None).unwrap();
        let row = s.get_install("inst_1").await.unwrap().unwrap();
        assert_eq!(row.version, "1.0.0");
        assert!(row.previous_version.is_none());
    }
}

#[tokio::test]
async fn incompatible_package_stays_unavailable_across_reopen() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("registry.db");

    {
        let s = SqliteStorage::open(&path, 30, None).unwrap();
        s.register_package(RegisterPackageRequest {
            install_id: "inst_relay".into(),
            manifest: manifest("com.maekon.relay", ExecutionLocation::Relay),
            now: Utc::now(),
        })
        .await
        .unwrap();
        let out = s
            .install(InstallRequest {
                install_id: "inst_relay".into(),
                expected_revision: 1,
                now: Utc::now(),
            })
            .await
            .unwrap();
        assert_eq!(
            out,
            RegistryOutcome::Unavailable {
                reason: UnavailableReason::ExecutionLocationUnsupported
            }
        );
    }

    // After reopen it is still unavailable and still not installed — the
    // fail-closed verdict is re-derived from the stored manifest, not cached.
    {
        let s = SqliteStorage::open(&path, 30, None).unwrap();
        let readiness = &s.list_readiness().await.unwrap()[0];
        assert_eq!(
            readiness.availability,
            Availability::Unavailable {
                reason: UnavailableReason::ExecutionLocationUnsupported
            }
        );
        assert_eq!(readiness.installation, InstallationState::NotInstalled);
        assert_eq!(readiness.summary_label(), Some("incompatible"));
        // Authentication axis is honest for a package that needs no account.
        let row = s.get_install("inst_relay").await.unwrap().unwrap();
        assert_eq!(row.authentication, AccountAuthentication::NotRequired);
        assert_eq!(row.update, UpdateState::Current);
    }
}
