//! Ports for the Skill Pack catalog and activation (ADR-029 §2/§6, #8588).
//!
//! Split three ways on purpose:
//!
//! * [`SkillPackCatalogCommandPort`] / [`SkillPackCatalogQueryPort`] own durable
//!   catalog and activation rows, mirroring the extension-registry port shape.
//! * [`ActiveSkillResolverPort`] is the single question a planner asks: "is
//!   there a skill I am cleared to put in instruction position right now?" It
//!   returns a [`SkillActivationOutcome`], never a bare `Option<String>`, so a
//!   caller cannot lose the blocked reason on the way.

use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::CoreError;
use crate::models::skill_pack::{
    SkillActivationAudit, SkillActivationOutcome, SkillPackEntry, SkillSelectionKind,
    StoredActivation,
};

/// Register a `skill_pack` contribution discovered on a verified package.
#[derive(Debug, Clone)]
pub struct RegisterSkillPackRequest {
    /// Catalog entry to record.
    pub entry: SkillPackEntry,
    /// Transaction timestamp.
    pub now: DateTime<Utc>,
}

/// Record an explicit activation so it survives a restart.
#[derive(Debug, Clone)]
pub struct RecordActivationRequest {
    /// Pre-minted activation id.
    pub activation_id: String,
    /// Skill being activated.
    pub skill_id: String,
    /// Version the activation binds to.
    pub version: String,
    /// Body digest the activation binds to.
    pub body_sha256: String,
    /// How the skill was selected.
    pub selection: SkillSelectionKind,
    /// Activation timestamp.
    pub now: DateTime<Utc>,
    /// Requested lifetime; the adapter clamps it to the module maximum.
    pub lifetime_secs: i64,
}

/// Typed outcome of a catalog mutation, mirroring `RegistryOutcome`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillPackOutcome {
    /// The mutation applied.
    Applied {
        /// Affected row id.
        id: String,
    },
    /// The requested end state was already in effect.
    AlreadyInState {
        /// Current state name.
        current: String,
    },
    /// The referenced catalog entry does not exist.
    NotFound,
}

/// Mutation surface for the Skill Pack catalog.
#[async_trait]
pub trait SkillPackCatalogCommandPort: Send + Sync {
    /// Register (or refresh) a catalog entry.
    async fn register_skill_pack(
        &self,
        request: RegisterSkillPackRequest,
    ) -> Result<SkillPackOutcome, CoreError>;

    /// Record an explicit activation, replacing any previous one.
    async fn record_activation(
        &self,
        request: RecordActivationRequest,
    ) -> Result<SkillPackOutcome, CoreError>;

    /// Clear the current activation (explicit deselection).
    async fn clear_activation(&self) -> Result<SkillPackOutcome, CoreError>;

    /// Append an activation audit row. Identity and decisions only.
    async fn record_activation_audit(
        &self,
        audit: SkillActivationAudit,
    ) -> Result<SkillPackOutcome, CoreError>;

    /// Remove every catalog entry and activation belonging to an installation.
    ///
    /// Called on uninstall/revoke. Children are deleted before parents
    /// explicitly, because this workspace runs with `foreign_keys` OFF and the
    /// `REFERENCES` clauses are documentation only (ADR-028 Amendment B3).
    async fn remove_for_install(&self, install_id: &str) -> Result<SkillPackOutcome, CoreError>;
}

/// Read surface for the Skill Pack catalog.
#[async_trait]
pub trait SkillPackCatalogQueryPort: Send + Sync {
    /// Every registered catalog entry.
    async fn list_skill_packs(&self) -> Result<Vec<SkillPackEntry>, CoreError>;

    /// One catalog entry.
    async fn get_skill_pack(&self, skill_id: &str) -> Result<Option<SkillPackEntry>, CoreError>;

    /// The stored activation, if any.
    async fn get_activation(&self) -> Result<Option<StoredActivation>, CoreError>;

    /// The full `skill_id -> direct references` graph, for cycle/depth checks.
    async fn reference_graph(&self) -> Result<BTreeMap<String, Vec<String>>, CoreError>;
}

/// Resolves the skill, if any, cleared to occupy instruction position now.
///
/// The planner depends on this and nothing else. Returning the full
/// [`SkillActivationOutcome`] rather than an `Option<String>` is deliberate: a
/// blocked activation carries a reason the planner must act on (refuse to run),
/// and an `Option` would silently collapse "refused" into "no skill".
#[async_trait]
pub trait ActiveSkillResolverPort: Send + Sync {
    /// Resolve the current activation against live registry truth.
    async fn resolve_active_skill(&self) -> Result<SkillActivationOutcome, CoreError>;
}
