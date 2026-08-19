//! Extension registry IPC commands (ADR-029 §5, #8586).
//!
//! The view DTOs expose the eight orthogonal readiness axes plus an optional
//! derived summary label. The label is `None` whenever no honest single word
//! exists, so the UI must render the axes rather than inventing "connected".
//!
//! Registration is deliberately NOT an IPC command: bundled packages are
//! discovered by the runtime, so the WebView can never forge provenance, a
//! manifest, or a trust field.

use crate::ipc_error::IpcError;
use crate::runtime_state::AppState;
use chrono::Utc;
use maekon_core::models::extension::{
    AccountAuthentication, Availability, CapabilityGrant, Enablement, ExtensionProvenance,
    ExtensionReadiness, Health, InstallationState, UpdateState,
};
use maekon_core::ports::extension_registry::{
    ExtensionRegistryCommandPort, ExtensionRegistryQueryPort, InstallRequest, RegistryOutcome,
    RollbackRequest, SetEnablementRequest, UninstallRequest, UpdateRequest,
};
use serde::Serialize;

/// One extension's readiness, reported as separate axes (§5).
#[derive(Debug, Clone, Serialize)]
pub struct ExtensionView {
    pub install_id: String,
    pub extension_id: String,
    pub version: String,
    /// Where the code came from — orthogonal to `installation`.
    pub provenance: ExtensionProvenance,
    pub availability: Availability,
    pub installation: InstallationState,
    pub enablement: Enablement,
    pub authentication: AccountAuthentication,
    pub grant: CapabilityGrant,
    pub update: UpdateState,
    pub health: Health,
    /// Previous known-good version, when a rollback is possible.
    pub previous_version: Option<String>,
    /// Compare-and-swap revision the caller must echo back on a mutation.
    pub revision: i64,
    /// Derived single label, or `None` when no honest summary exists.
    pub summary_label: Option<String>,
}

/// Typed IPC result for a registry mutation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum ExtensionOutcomeView {
    /// The mutation applied.
    Applied { install_id: String, revision: i64 },
    /// The requested end state was already in effect (idempotent no-op).
    AlreadyInState { current: String },
    /// Stale revision — refetch and retry.
    RevisionConflict,
    /// A fail-closed gate rejected the operation.
    Unavailable { reason: String },
    /// The transition is not legal from the current state.
    IllegalTransition { from: String, to: String },
    /// Uninstall could not confirm every declared cleanup step.
    CleanupIncomplete { missing: Vec<String> },
}

impl From<RegistryOutcome> for ExtensionOutcomeView {
    fn from(outcome: RegistryOutcome) -> Self {
        match outcome {
            RegistryOutcome::Applied {
                install_id,
                revision,
            } => Self::Applied {
                install_id,
                revision,
            },
            RegistryOutcome::AlreadyInState { current } => Self::AlreadyInState { current },
            RegistryOutcome::RevisionConflict => Self::RevisionConflict,
            RegistryOutcome::Unavailable { reason } => Self::Unavailable {
                reason: reason.as_str().to_string(),
            },
            RegistryOutcome::IllegalTransition { from, to } => Self::IllegalTransition { from, to },
            RegistryOutcome::CleanupIncomplete { missing } => Self::CleanupIncomplete { missing },
        }
    }
}

fn to_view(
    readiness: ExtensionReadiness,
    version: String,
    previous_version: Option<String>,
    revision: i64,
) -> ExtensionView {
    let summary_label = readiness.summary_label().map(|s| s.to_string());
    ExtensionView {
        install_id: readiness.install_id,
        extension_id: readiness.extension_id,
        version,
        provenance: readiness.provenance,
        availability: readiness.availability,
        installation: readiness.installation,
        enablement: readiness.enablement,
        authentication: readiness.authentication,
        grant: readiness.grant,
        update: readiness.update,
        health: readiness.health,
        previous_version,
        revision,
        summary_label,
    }
}

/// List every registered extension with its eight readiness axes.
pub async fn list_extensions(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<ExtensionView>, IpcError> {
    let installs = state
        .storage
        .list_installs()
        .await
        .map_err(IpcError::from)?;
    let readiness = state
        .storage
        .list_readiness()
        .await
        .map_err(IpcError::from)?;
    let mut out = Vec::with_capacity(readiness.len());
    for r in readiness {
        // Pair each readiness projection with its durable row for version/revision.
        let row = installs.iter().find(|i| i.install_id == r.install_id);
        let (version, previous, revision) = match row {
            Some(i) => (i.version.clone(), i.previous_version.clone(), i.revision),
            None => (String::new(), None, 0),
        };
        out.push(to_view(r, version, previous, revision));
    }
    Ok(out)
}

/// Install a registered extension. Fail-closed gates run server side.
pub async fn install_extension(
    state: tauri::State<'_, AppState>,
    install_id: String,
    expected_revision: i64,
) -> Result<ExtensionOutcomeView, IpcError> {
    state
        .storage
        .install(InstallRequest {
            install_id,
            expected_revision,
            now: Utc::now(),
        })
        .await
        .map(Into::into)
        .map_err(IpcError::from)
}

/// Enable or disable an installed extension. Disable blocks new work at once.
pub async fn set_extension_enablement(
    state: tauri::State<'_, AppState>,
    install_id: String,
    enabled: bool,
    expected_revision: i64,
) -> Result<ExtensionOutcomeView, IpcError> {
    state
        .storage
        .set_enablement(SetEnablementRequest {
            install_id,
            enabled,
            expected_revision,
            now: Utc::now(),
        })
        .await
        .map(Into::into)
        .map_err(IpcError::from)
}

/// Activate an update, retaining the previous known-good version.
pub async fn update_extension(
    state: tauri::State<'_, AppState>,
    install_id: String,
    target_version: String,
    expected_revision: i64,
) -> Result<ExtensionOutcomeView, IpcError> {
    state
        .storage
        .update(UpdateRequest {
            install_id,
            target_version,
            expected_revision,
            now: Utc::now(),
        })
        .await
        .map(Into::into)
        .map_err(IpcError::from)
}

/// Roll back to the previous known-good version.
pub async fn rollback_extension(
    state: tauri::State<'_, AppState>,
    install_id: String,
    expected_revision: i64,
) -> Result<ExtensionOutcomeView, IpcError> {
    state
        .storage
        .rollback(RollbackRequest {
            install_id,
            expected_revision,
            now: Utc::now(),
        })
        .await
        .map(Into::into)
        .map_err(IpcError::from)
}

/// Uninstall, passing the cleanup steps the caller performed. A missing step
/// returns `cleanup_incomplete` and applies nothing.
pub async fn uninstall_extension(
    state: tauri::State<'_, AppState>,
    install_id: String,
    expected_revision: i64,
    completed_cleanup: Vec<String>,
) -> Result<ExtensionOutcomeView, IpcError> {
    state
        .storage
        .uninstall(UninstallRequest {
            install_id,
            expected_revision,
            completed_cleanup,
            now: Utc::now(),
        })
        .await
        .map(Into::into)
        .map_err(IpcError::from)
}

/// Identity of a Skill Pack activation, returned to the WebView. Never carries
/// the skill body — only the id/version/owning-extension the UI needs (#8588).
#[derive(Debug, Clone, Serialize)]
pub struct SkillPackActivationView {
    pub skill_id: String,
    pub version: String,
    pub extension_id: String,
}

/// Build the composition-root Skill Pack resolver from shared storage plus a
/// fresh on-disk body source. The loader scans `~/.agents/skills/*.md`; the
/// resolver re-hashes each body against the catalog digest, so the loader is a
/// byte source, never a trust authority (ADR-029 §4).
fn skill_pack_resolver_for(
    state: &AppState,
) -> crate::skill_pack_resolver::RegistryActiveSkillResolver {
    let mut roots = Vec::new();
    if let Some(base) = directories::BaseDirs::new() {
        roots.push(base.home_dir().to_path_buf());
    }
    let loader = std::sync::Arc::new(crate::skill_loader::FileSkillLoader::new(roots));
    crate::skill_pack_resolver::RegistryActiveSkillResolver::new(
        state.storage.clone(),
        state.storage.clone(),
        state.storage.clone(),
        loader,
    )
}

/// #8588: explicit user activation of a Skill Pack. Registers the pack from the
/// on-disk body attributed to a verified, enabled install, then records the
/// activation. The `body_sha256` is derived from disk server-side and the owning
/// install must clear every §10 availability gate — the WebView can name a skill
/// and its owning install, never forge a body hash, a version, or provenance.
pub async fn activate_skill_pack(
    state: tauri::State<'_, AppState>,
    install_id: String,
    skill_id: String,
) -> Result<SkillPackActivationView, IpcError> {
    let resolver = skill_pack_resolver_for(&state);
    let entry = resolver
        .activate_explicit(&install_id, &skill_id)
        .await
        .map_err(IpcError::from)?;
    Ok(SkillPackActivationView {
        skill_id: entry.skill_id,
        version: entry.version,
        extension_id: entry.extension_id,
    })
}

/// #8588: clear the current Skill Pack activation (explicit deselection). The
/// planner then runs with an empty skill region — the fail-closed default.
pub async fn clear_skill_pack_activation(
    state: tauri::State<'_, AppState>,
) -> Result<(), IpcError> {
    let resolver = skill_pack_resolver_for(&state);
    resolver.clear_active_skill().await.map_err(IpcError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::models::extension::UnavailableReason;

    #[test]
    fn outcome_view_preserves_typed_reason_strings() {
        let view: ExtensionOutcomeView = RegistryOutcome::Unavailable {
            reason: UnavailableReason::ExecutionLocationUnsupported,
        }
        .into();
        let json = serde_json::to_string(&view).unwrap();
        assert!(json.contains("execution_location_unsupported"), "{json}");
        assert!(json.contains("\"outcome\":\"unavailable\""), "{json}");
    }

    #[test]
    fn cleanup_incomplete_reports_the_missing_steps() {
        let view: ExtensionOutcomeView = RegistryOutcome::CleanupIncomplete {
            missing: vec!["cursors".to_string()],
        }
        .into();
        let json = serde_json::to_string(&view).unwrap();
        assert!(json.contains("cursors"), "{json}");
        assert!(json.contains("cleanup_incomplete"), "{json}");
    }

    #[test]
    fn view_carries_no_summary_label_mid_transition() {
        let readiness = ExtensionReadiness {
            extension_id: "com.maekon.a".to_string(),
            install_id: "inst_1".to_string(),
            provenance: ExtensionProvenance::Bundled,
            availability: Availability::Available,
            installation: InstallationState::Installing,
            enablement: Enablement::Disabled,
            authentication: AccountAuthentication::NotRequired,
            grant: CapabilityGrant::NotRequested,
            update: UpdateState::Current,
            health: Health::Unknown,
        };
        let view = to_view(readiness, "1.0.0".to_string(), None, 1);
        assert_eq!(view.summary_label, None);
        // The axes are still fully reported so the UI can be honest.
        assert_eq!(view.installation, InstallationState::Installing);
    }
}
