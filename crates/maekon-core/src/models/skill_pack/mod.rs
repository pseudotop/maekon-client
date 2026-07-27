//! Trusted Skill Pack activation contracts (ADR-029 §2/§6/§9, #8588).
//!
//! A Skill Pack is *verified data that occupies instruction position*. That makes
//! it the single most dangerous contribution kind in the Extension taxonomy: if
//! unverified content — or worse, an external connector payload — can reach the
//! skill region of a prompt, the local-first context feature becomes a
//! prompt-injection channel (ADR-029 §9 "indirect prompt injection", Frozen
//! Invariant 5).
//!
//! Three rules are encoded structurally here rather than left to caller
//! discipline:
//!
//! * **Activation is never implicit.** [`SkillSelectionKind`] has exactly two
//!   variants — an explicit user selection or an approved workflow binding.
//!   There is deliberately no `Inferred`/`Default` variant, so "the planner
//!   guessed" cannot be represented.
//! * **Every gate fails closed with an exact reason.** [`SkillBlockedReason`]
//!   carries the specific failing gate; the resolver never falls back to a
//!   best-effort activation and never silently drops a required capability.
//! * **Audit records identity, not content.** [`SkillActivationAudit`] holds the
//!   skill id/version/body hash and the capability decisions only. The body and
//!   the user's context are structurally absent from the type.
//!
//! The prompt-side counterpart lives in [`crate::models::prompt_assembly`],
//! which is the only module that can move a verified activation into instruction
//! position.
//!
//! # Relationship to ADR-029 §2
//!
//! ADR-029 §2 lists `skill_pack` as "unavailable pending capability-resolver
//! ADR", and [`ContributionKind::is_supported`] therefore still returns `false`
//! for it — that flag means "Phase-1 read-only `context_source` **runtime** is
//! operable", and this module deliberately does not change it. A Skill Pack
//! loads no runtime: it is verified data, gated by
//! [`resolver::resolve_activation`], which applies the same §10 trust gates the
//! registry applies plus the skill-specific bounds below.

mod resolver;
#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) mod tests_support;

pub use resolver::{resolve_activation, SkillActivationRequest};

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::models::extension::{CapabilityGrant, ContributionKind};

/// Maximum verified skill body accepted into instruction position.
///
/// A body is fully trusted text once activated, so an unbounded body is both a
/// context-budget denial-of-service and a review-surface problem: nobody audits
/// a 10 MB "skill".
pub const MAX_SKILL_BODY_BYTES: usize = 64 * 1024;

/// Maximum depth of the skill reference graph reachable from an activation.
///
/// Bounds transitive instruction inclusion — a shallow, reviewable chain rather
/// than an arbitrarily deep one whose effective instructions nobody can predict.
pub const MAX_SKILL_REFERENCE_DEPTH: u32 = 4;

/// Maximum number of direct references a single skill may declare.
pub const MAX_SKILL_REFERENCES: usize = 16;

/// Maximum activation lifetime. Activations expire so a grant obtained once
/// cannot silently persist across a long-running session (ADR-029 §6 requires a
/// live check, not an install-time one).
pub const MAX_ACTIVATION_LIFETIME_SECS: i64 = 3_600;

/// Compute the canonical `sha256:<hex>` body digest used throughout this module.
pub fn body_digest(body: &str) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(body.as_bytes());
    let mut out = String::with_capacity(7 + digest.len() * 2);
    out.push_str("sha256:");
    for &byte in digest.iter() {
        // Infallible for a String sink.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// How the active skill was chosen.
///
/// There is intentionally no implicit/inferred variant: ADR-029 §6 forbids a
/// package from self-granting, and an automatically activated instruction set is
/// exactly that. A planner that has no selection gets
/// [`SkillBlockedReason::NoSelection`] and an empty skill region.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum SkillSelectionKind {
    /// The user explicitly picked this skill for this run.
    ExplicitUserSelection,
    /// A previously approved workflow binding selected it.
    ApprovedWorkflowBinding {
        /// Identifier of the approved binding.
        binding_id: String,
    },
}

impl SkillSelectionKind {
    /// Stable wire/SQL string.
    pub fn as_sql_str(&self) -> &'static str {
        match self {
            Self::ExplicitUserSelection => "EXPLICIT_USER_SELECTION",
            Self::ApprovedWorkflowBinding { .. } => "APPROVED_WORKFLOW_BINDING",
        }
    }
}

/// Whether a declared capability is required to run the skill at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityRequirement {
    /// The skill cannot run without it.
    Required,
    /// The skill runs without it, in a strictly narrower mode.
    Optional,
}

/// Outcome of one capability's resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum CapabilityOutcome {
    /// The effective intersection allows it; the skill may use it.
    Granted,
    /// Not available. The skill does **not** receive it.
    ///
    /// For an optional capability this is a normal, non-fatal narrowing: the
    /// activation proceeds with a smaller granted set. It never triggers a
    /// substitute provider or a widened scope (ADR-029 §6).
    Withheld {
        /// The grant state that failed the check.
        grant: CapabilityGrant,
    },
}

/// One capability decision. Safe to persist: it names a capability and a
/// decision, never user content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityDecision {
    /// Capability identifier.
    pub capability: String,
    /// Whether the skill declared it required or optional.
    pub requirement: CapabilityRequirement,
    /// The decision reached.
    pub outcome: CapabilityOutcome,
}

impl CapabilityDecision {
    /// Whether this capability may be offered to the skill.
    pub fn is_granted(&self) -> bool {
        matches!(self.outcome, CapabilityOutcome::Granted)
    }
}

/// Typed, exact reason an activation was refused (ADR-029 §10 fail-closed).
///
/// This is a skill-scoped namespace and, like the registry reason codes, does
/// **not** expand the ADR-019 wire-locked `CoreError` snapshot (ADR-029
/// Amendment I5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "code")]
pub enum SkillBlockedReason {
    /// No explicit selection and no approved binding. The benign default.
    NoSelection,
    /// The owning package failed a §10 availability gate.
    PackageUnavailable {
        /// The registry's typed reason (includes `signature_unverified`).
        reason: String,
    },
    /// The owning package is not in the `installed` state.
    PackageNotInstalled {
        /// Observed installation state.
        installation: String,
    },
    /// The owning package is installed but disabled.
    PackageDisabled,
    /// The referenced contribution is not of kind `skill_pack`.
    ContributionNotSkillPack {
        /// Observed contribution kind.
        kind: String,
    },
    /// The catalog entry's version no longer matches the installed version —
    /// an update or rollback happened since the activation was recorded.
    VersionChanged {
        /// Version the activation was bound to.
        activated: String,
        /// Version currently installed.
        current: String,
    },
    /// The presented body does not hash to the catalog's recorded digest.
    BodyHashMismatch {
        /// Digest the catalog expects.
        expected: String,
        /// Digest of the presented body.
        actual: String,
    },
    /// The body exceeds [`MAX_SKILL_BODY_BYTES`].
    BodyTooLarge {
        /// Observed size in bytes.
        bytes: usize,
        /// Enforced limit.
        limit: usize,
    },
    /// The reference graph is deeper than [`MAX_SKILL_REFERENCE_DEPTH`].
    ReferenceDepthExceeded {
        /// Observed depth.
        depth: u32,
        /// Enforced limit.
        limit: u32,
    },
    /// The skill declares more direct references than [`MAX_SKILL_REFERENCES`].
    TooManyReferences {
        /// Observed count.
        count: usize,
        /// Enforced limit.
        limit: usize,
    },
    /// The reference graph contains a cycle.
    DependencyCycle {
        /// The skill id at which the cycle was detected.
        at: String,
    },
    /// A stored activation outlived [`MAX_ACTIVATION_LIFETIME_SECS`].
    ActivationExpired,
    /// A required capability is not granted. Execution must not proceed.
    RequiredCapabilityMissing {
        /// The missing capability.
        capability: String,
        /// The grant state observed.
        grant: CapabilityGrant,
    },
}

impl SkillBlockedReason {
    /// Stable short code for audit rows and logs.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoSelection => "no_selection",
            Self::PackageUnavailable { .. } => "package_unavailable",
            Self::PackageNotInstalled { .. } => "package_not_installed",
            Self::PackageDisabled => "package_disabled",
            Self::ContributionNotSkillPack { .. } => "contribution_not_skill_pack",
            Self::VersionChanged { .. } => "version_changed",
            Self::BodyHashMismatch { .. } => "body_hash_mismatch",
            Self::BodyTooLarge { .. } => "body_too_large",
            Self::ReferenceDepthExceeded { .. } => "reference_depth_exceeded",
            Self::TooManyReferences { .. } => "too_many_references",
            Self::DependencyCycle { .. } => "dependency_cycle",
            Self::ActivationExpired => "activation_expired",
            Self::RequiredCapabilityMissing { .. } => "required_capability_missing",
        }
    }

    /// Whether this reason means "the user simply did not pick a skill".
    ///
    /// Callers use this to distinguish the benign no-skill path (run normally
    /// with an empty skill region) from a genuine refusal (do not execute).
    pub fn is_benign_absence(&self) -> bool {
        matches!(self, Self::NoSelection)
    }
}

/// A `skill_pack` contribution registered from a verified extension package.
///
/// The catalog stores identity and bounds metadata. It deliberately does **not**
/// store the body: the body lives with the package artifact and is re-hashed
/// against [`SkillPackEntry::body_sha256`] at every activation, so a swapped
/// on-disk file cannot be activated (ADR-029 §9 supply-chain substitution).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillPackEntry {
    /// Stable skill identifier, unique within the catalog.
    pub skill_id: String,
    /// Owning installation id.
    pub install_id: String,
    /// Owning extension id.
    pub extension_id: String,
    /// Contribution id inside the owning manifest.
    pub contribution_id: String,
    /// Kind as declared by the manifest — must be `skill_pack`.
    pub contribution_kind: ContributionKind,
    /// Version of the owning package this entry belongs to.
    pub version: String,
    /// Publisher identity recorded at registration.
    pub publisher_id: String,
    /// Expected `sha256:<hex>` digest of the skill body.
    pub body_sha256: String,
    /// Capabilities without which the skill must not run.
    pub required_capabilities: Vec<String>,
    /// Capabilities that narrow the skill when absent but never block it.
    pub optional_capabilities: Vec<String>,
    /// Direct references to other skill ids.
    pub references: Vec<String>,
}

/// A verified, live activation.
///
/// Only [`resolve_activation`] constructs this, and only after every gate has
/// passed. Holding one is the proof that a body may occupy instruction position;
/// [`crate::models::prompt_assembly::TrustedInstruction`] accepts nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveSkill {
    /// Activated skill id.
    pub skill_id: String,
    /// Version the activation is bound to.
    pub version: String,
    /// Digest of the verified body.
    pub body_sha256: String,
    /// How the skill was selected.
    pub selection: SkillSelectionKind,
    /// Capabilities that survived the intersection and may be offered.
    pub granted_capabilities: Vec<String>,
    /// Every capability decision, for audit.
    pub decisions: Vec<CapabilityDecision>,
    /// When the activation was resolved.
    pub activated_at: DateTime<Utc>,
    /// When the activation stops being valid.
    pub expires_at: DateTime<Utc>,
    /// The verified body. Private so it can only leave through
    /// [`ActiveSkill::verified_body`], keeping the "instruction position"
    /// entry point auditable and greppable.
    body: String,
}

impl ActiveSkill {
    /// The verified body text.
    ///
    /// The only supported consumer is
    /// [`crate::models::prompt_assembly::TrustedInstruction::from_activation`].
    pub fn verified_body(&self) -> &str {
        &self.body
    }

    /// Whether the activation is still within its lifetime.
    pub fn is_live_at(&self, now: DateTime<Utc>) -> bool {
        now < self.expires_at
    }

    /// Build the audit record for a successful activation. Identity and
    /// decisions only — never the body, never user context.
    pub fn to_audit(&self, audit_id: impl Into<String>) -> SkillActivationAudit {
        SkillActivationAudit {
            audit_id: audit_id.into(),
            skill_id: self.skill_id.clone(),
            version: self.version.clone(),
            body_sha256: self.body_sha256.clone(),
            selection_kind: self.selection.as_sql_str().to_string(),
            decision: "activated".to_string(),
            blocked_reason: None,
            capability_decisions: self.decisions.clone(),
            occurred_at: self.activated_at,
        }
    }
}

/// The outcome of an activation attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillActivationOutcome {
    /// Every gate passed.
    Activated(Box<ActiveSkill>),
    /// A gate refused, with the exact failing reason.
    Blocked {
        /// Why activation was refused.
        reason: SkillBlockedReason,
        /// Capability decisions reached before the refusal, for audit.
        decisions: Vec<CapabilityDecision>,
    },
}

impl SkillActivationOutcome {
    /// The activation, if any.
    pub fn activated(&self) -> Option<&ActiveSkill> {
        match self {
            Self::Activated(a) => Some(a),
            Self::Blocked { .. } => None,
        }
    }

    /// The blocking reason, if any.
    pub fn blocked_reason(&self) -> Option<&SkillBlockedReason> {
        match self {
            Self::Activated(_) => None,
            Self::Blocked { reason, .. } => Some(reason),
        }
    }

    /// Build the audit record for either outcome.
    pub fn to_audit(
        &self,
        audit_id: impl Into<String>,
        skill_id: &str,
        now: DateTime<Utc>,
    ) -> SkillActivationAudit {
        match self {
            Self::Activated(a) => a.to_audit(audit_id),
            Self::Blocked { reason, decisions } => SkillActivationAudit {
                audit_id: audit_id.into(),
                skill_id: skill_id.to_string(),
                version: String::new(),
                body_sha256: String::new(),
                selection_kind: String::new(),
                decision: "blocked".to_string(),
                blocked_reason: Some(reason.code().to_string()),
                capability_decisions: decisions.clone(),
                occurred_at: now,
            },
        }
    }
}

/// A durable activation record.
///
/// Re-validated against current registry truth on every use via
/// [`StoredActivation::revalidate`], which is what makes restart, update,
/// rollback, and revoke invalidate a stale activation instead of resurrecting
/// it from cache (ADR-029 §6/§8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredActivation {
    /// Activation id.
    pub activation_id: String,
    /// Activated skill id.
    pub skill_id: String,
    /// Version the activation was bound to.
    pub version: String,
    /// Digest the activation was bound to.
    pub body_sha256: String,
    /// How the skill was selected.
    pub selection: SkillSelectionKind,
    /// When it was activated.
    pub activated_at: DateTime<Utc>,
    /// When it stops being valid.
    pub expires_at: DateTime<Utc>,
}

impl StoredActivation {
    /// Clamp a requested lifetime to [`MAX_ACTIVATION_LIFETIME_SECS`].
    pub fn bounded_expiry(activated_at: DateTime<Utc>, lifetime_secs: i64) -> DateTime<Utc> {
        let clamped = lifetime_secs.clamp(0, MAX_ACTIVATION_LIFETIME_SECS);
        activated_at + Duration::seconds(clamped)
    }
}

/// Audit row for an activation decision.
///
/// Structurally incapable of holding the skill body or the user's context: the
/// struct has no field for either (ADR-029 §11, #8588 acceptance criterion 7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillActivationAudit {
    /// Audit row id.
    pub audit_id: String,
    /// Skill identity.
    pub skill_id: String,
    /// Version, empty when the block happened before version resolution.
    pub version: String,
    /// Body digest, empty when the block happened before hashing.
    pub body_sha256: String,
    /// Selection kind, empty when there was no selection.
    pub selection_kind: String,
    /// `activated` or `blocked`.
    pub decision: String,
    /// Short reason code when blocked.
    pub blocked_reason: Option<String>,
    /// Capability decisions reached.
    pub capability_decisions: Vec<CapabilityDecision>,
    /// When the decision was made.
    pub occurred_at: DateTime<Utc>,
}

/// Effective capability grants after the ADR-029 §6 intersection.
///
/// The resolver consumes this; it does not compute the intersection itself. The
/// intersection is the permission layer's job (consent ∩ manifest ∩ install
/// grant ∩ provider scope ∩ managed policy ∩ runtime readiness ∩ source
/// health), and duplicating it here would create a second, divergent authority.
pub type EffectiveGrants = BTreeMap<String, CapabilityGrant>;

/// Construct an [`ActiveSkill`]. Crate-internal so the resolver stays the only
/// producer of instruction-position material.
pub(crate) fn build_active_skill(
    entry: &SkillPackEntry,
    selection: SkillSelectionKind,
    body: String,
    decisions: Vec<CapabilityDecision>,
    activated_at: DateTime<Utc>,
    lifetime_secs: i64,
) -> ActiveSkill {
    let granted_capabilities = decisions
        .iter()
        .filter(|d| d.is_granted())
        .map(|d| d.capability.clone())
        .collect();
    ActiveSkill {
        skill_id: entry.skill_id.clone(),
        version: entry.version.clone(),
        body_sha256: entry.body_sha256.clone(),
        selection,
        granted_capabilities,
        decisions,
        activated_at,
        expires_at: StoredActivation::bounded_expiry(activated_at, lifetime_secs),
        body,
    }
}
