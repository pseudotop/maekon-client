//! Ports for the Extension registry (ADR-029 §5/§8, #8586).
//!
//! The registry owns install/enable/disable/update/rollback/uninstall lifecycle
//! and reports the eight orthogonal readiness axes. It is a distinct owner from
//! the Provider Surface Catalog and never merges with it.
//!
//! Transitions are decided by the pure functions in
//! [`crate::models::extension`]; the adapter executes an already-decided
//! transition inside a transaction, exactly like the durable-task ports.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::CoreError;
use crate::models::extension::{
    CapabilityGrant, ExtensionInstall, ExtensionManifest, ExtensionReadiness, Health,
    UnavailableReason,
};

/// Register a discovered package (bundled or curated-local) without installing
/// it. Provenance is recorded; the installation axis stays `not_installed`.
#[derive(Debug, Clone)]
pub struct RegisterPackageRequest {
    /// Pre-minted `inst`-prefixed installation id.
    pub install_id: String,
    /// The manifest to register.
    pub manifest: ExtensionManifest,
    /// Transaction timestamp.
    pub now: DateTime<Utc>,
}

/// Install a previously registered package.
#[derive(Debug, Clone)]
pub struct InstallRequest {
    /// Installation id.
    pub install_id: String,
    /// Expected revision for compare-and-swap.
    pub expected_revision: i64,
    /// Transaction timestamp.
    pub now: DateTime<Utc>,
}

/// Enable or disable an installed package. Disable blocks new work immediately.
#[derive(Debug, Clone)]
pub struct SetEnablementRequest {
    /// Installation id.
    pub install_id: String,
    /// `true` to enable, `false` to disable.
    pub enabled: bool,
    /// Expected revision for compare-and-swap.
    pub expected_revision: i64,
    /// Transaction timestamp.
    pub now: DateTime<Utc>,
}

/// Stage and activate an update, keeping the previous known-good version for
/// rollback (§8).
#[derive(Debug, Clone)]
pub struct UpdateRequest {
    /// Installation id.
    pub install_id: String,
    /// Version being activated.
    pub target_version: String,
    /// Expected revision for compare-and-swap.
    pub expected_revision: i64,
    /// Transaction timestamp.
    pub now: DateTime<Utc>,
}

/// Roll back to the previous known-good version.
#[derive(Debug, Clone)]
pub struct RollbackRequest {
    /// Installation id.
    pub install_id: String,
    /// Expected revision for compare-and-swap.
    pub expected_revision: i64,
    /// Transaction timestamp.
    pub now: DateTime<Utc>,
}

/// Uninstall, requesting credential/cursor/cache cleanup. Provenance and audit
/// rows are preserved, never forged (§8).
#[derive(Debug, Clone)]
pub struct UninstallRequest {
    /// Installation id.
    pub install_id: String,
    /// Expected revision for compare-and-swap.
    pub expected_revision: i64,
    /// Cleanup steps the caller confirmed it performed.
    pub completed_cleanup: Vec<String>,
    /// Transaction timestamp.
    pub now: DateTime<Utc>,
}

/// Record an observed health transition.
#[derive(Debug, Clone)]
pub struct RecordHealthRequest {
    /// Installation id.
    pub install_id: String,
    /// Observed health.
    pub health: Health,
    /// Transaction timestamp.
    pub now: DateTime<Utc>,
}

/// Record a capability-grant decision for this installation.
#[derive(Debug, Clone)]
pub struct SetGrantRequest {
    /// Installation id.
    pub install_id: String,
    /// New grant state.
    pub grant: CapabilityGrant,
    /// Expected revision for compare-and-swap.
    pub expected_revision: i64,
    /// Transaction timestamp.
    pub now: DateTime<Utc>,
}

/// Typed outcome of a registry mutation. Mirrors the durable-task contract:
/// `AlreadyInState` is an idempotent no-op success, `RevisionConflict` means
/// refetch-then-retry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryOutcome {
    /// The mutation applied.
    Applied {
        /// Installation id.
        install_id: String,
        /// Resulting revision.
        revision: i64,
    },
    /// The requested end state was already in effect.
    AlreadyInState {
        /// Current state name.
        current: String,
    },
    /// The stored revision differs and the target was not reached.
    RevisionConflict,
    /// A fail-closed gate rejected the operation.
    Unavailable {
        /// Typed reason.
        reason: UnavailableReason,
    },
    /// The transition is not legal from the current state.
    IllegalTransition {
        /// Current state name.
        from: String,
        /// Requested state name.
        to: String,
    },
    /// Uninstall could not confirm every declared cleanup step.
    CleanupIncomplete {
        /// Steps still outstanding.
        missing: Vec<String>,
    },
}

/// Mutation surface for the Extension registry.
#[async_trait]
pub trait ExtensionRegistryCommandPort: Send + Sync {
    /// Register a discovered package without installing it.
    async fn register_package(
        &self,
        request: RegisterPackageRequest,
    ) -> Result<RegistryOutcome, CoreError>;

    /// Install a registered package (fail-closed on the §10 gates).
    async fn install(&self, request: InstallRequest) -> Result<RegistryOutcome, CoreError>;

    /// Enable or disable an installed package.
    async fn set_enablement(
        &self,
        request: SetEnablementRequest,
    ) -> Result<RegistryOutcome, CoreError>;

    /// Activate an update, retaining the previous known-good version.
    async fn update(&self, request: UpdateRequest) -> Result<RegistryOutcome, CoreError>;

    /// Roll back to the previous known-good version.
    async fn rollback(&self, request: RollbackRequest) -> Result<RegistryOutcome, CoreError>;

    /// Uninstall with cleanup confirmation.
    async fn uninstall(&self, request: UninstallRequest) -> Result<RegistryOutcome, CoreError>;

    /// Record an observed health transition.
    async fn record_health(
        &self,
        request: RecordHealthRequest,
    ) -> Result<RegistryOutcome, CoreError>;

    /// Record a capability-grant decision.
    async fn set_grant(&self, request: SetGrantRequest) -> Result<RegistryOutcome, CoreError>;
}

/// Read surface for the Extension registry.
#[async_trait]
pub trait ExtensionRegistryQueryPort: Send + Sync {
    /// List every installation record.
    async fn list_installs(&self) -> Result<Vec<ExtensionInstall>, CoreError>;

    /// Fetch one installation record.
    async fn get_install(&self, install_id: &str) -> Result<Option<ExtensionInstall>, CoreError>;

    /// Fetch the stored manifest for an installation.
    async fn get_manifest(&self, install_id: &str) -> Result<Option<ExtensionManifest>, CoreError>;

    /// Project the eight readiness axes for every installation.
    async fn list_readiness(&self) -> Result<Vec<ExtensionReadiness>, CoreError>;
}
