//! Extension package + registry models (ADR-029, #8586).
//!
//! The public wire identifier is `maekon.extension_manifest.v1`. This module owns
//! the manifest contract and the orthogonal lifecycle axes; adapters may validate
//! artifacts but never invent trust or capability fields.
//!
//! Two contract points are easy to get wrong and are encoded structurally here:
//!
//! * Provenance (where the code came from) and installation state (what the user
//!   did) are **separate** facts — a bundled built-in still moves
//!   `not_installed -> installing -> installed` (ADR-029 Amendment B1).
//! * Readiness is eight orthogonal axes, never one collapsed boolean. A summary
//!   label may only be derived when every required axis is healthy (§5).

// OOS-TBD: ADR-013 file split — the axis enums could move into an
// `extension/axes.rs` sub-module if this file keeps growing.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Wire identifier for the manifest contract.
pub const EXTENSION_MANIFEST_WIRE_ID: &str = "maekon.extension_manifest.v1";

/// Host API version this build implements. Manifests declare an inclusive
/// minimum and an exclusive maximum around it (§10).
pub const CURRENT_HOST_API: u32 = 1;

/// Manifest schema major this build understands.
pub const SUPPORTED_MANIFEST_SCHEMA_MAJOR: u32 = 1;

/// Where an extension's code came from. Orthogonal to [`InstallationState`]:
/// a `Bundled` extension is shipped inside the signed app but is still not
/// installed until the user explicitly enables it (Amendment B1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExtensionProvenance {
    /// Code ships inside the signed application bundle.
    Bundled,
    /// Code/data came from a curated local registry artifact.
    CuratedLocal,
}

impl ExtensionProvenance {
    /// Wire/SQL string form.
    pub fn as_sql_str(&self) -> &'static str {
        match self {
            Self::Bundled => "BUNDLED",
            Self::CuratedLocal => "CURATED_LOCAL",
        }
    }

    /// Parse a known provenance value.
    pub fn from_sql_str(value: &str) -> Option<Self> {
        match value {
            "BUNDLED" => Some(Self::Bundled),
            "CURATED_LOCAL" => Some(Self::CuratedLocal),
            _ => None,
        }
    }
}

/// Bounded package source kind (§2 source trust).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SourceKind {
    /// Shipped in the signed app bundle — the only Phase-1 supported source.
    AppBundle,
    /// Curated local registry artifact (not supported in Phase 1).
    CuratedRegistry,
    /// Public marketplace (not supported in Phase 1).
    PublicMarketplace,
    /// Forward-compatible unknown value; inventory/export only.
    Other(String),
}

impl SourceKind {
    /// Wire/SQL string form.
    pub fn as_sql_str(&self) -> String {
        match self {
            Self::AppBundle => "APP_BUNDLE".to_string(),
            Self::CuratedRegistry => "CURATED_REGISTRY".to_string(),
            Self::PublicMarketplace => "PUBLIC_MARKETPLACE".to_string(),
            Self::Other(raw) => raw.clone(),
        }
    }

    /// Parse from storage; unknown values round-trip as [`SourceKind::Other`].
    pub fn from_sql_str(value: &str) -> Self {
        match value {
            "APP_BUNDLE" => Self::AppBundle,
            "CURATED_REGISTRY" => Self::CuratedRegistry,
            "PUBLIC_MARKETPLACE" => Self::PublicMarketplace,
            other => Self::Other(other.to_string()),
        }
    }

    /// Whether Phase 1 supports acquiring from this source.
    pub fn is_supported(&self) -> bool {
        matches!(self, Self::AppBundle)
    }
}

impl Serialize for SourceKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.as_sql_str())
    }
}

impl<'de> Deserialize<'de> for SourceKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from_sql_str(&String::deserialize(deserializer)?))
    }
}

/// Runtime kind (§3). Only `first_party_builtin` is supported in Phase 1.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RuntimeKind {
    /// Compiled into the signed app; the manifest describes it but never loads it.
    FirstPartyBuiltin,
    /// Declarative interpreter (unavailable in Phase 1).
    Declarative,
    /// MCP server (unavailable in Phase 1).
    Mcp,
    /// Forward-compatible unknown value.
    Other(String),
}

impl RuntimeKind {
    /// Wire/SQL string form.
    pub fn as_sql_str(&self) -> String {
        match self {
            Self::FirstPartyBuiltin => "FIRST_PARTY_BUILTIN".to_string(),
            Self::Declarative => "DECLARATIVE".to_string(),
            Self::Mcp => "MCP".to_string(),
            Self::Other(raw) => raw.clone(),
        }
    }

    /// Parse from storage; unknown values round-trip.
    pub fn from_sql_str(value: &str) -> Self {
        match value {
            "FIRST_PARTY_BUILTIN" => Self::FirstPartyBuiltin,
            "DECLARATIVE" => Self::Declarative,
            "MCP" => Self::Mcp,
            other => Self::Other(other.to_string()),
        }
    }

    /// Whether Phase 1 supports operating this runtime.
    pub fn is_supported(&self) -> bool {
        matches!(self, Self::FirstPartyBuiltin)
    }
}

impl Serialize for RuntimeKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.as_sql_str())
    }
}

impl<'de> Deserialize<'de> for RuntimeKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from_sql_str(&String::deserialize(deserializer)?))
    }
}

/// Where the runtime executes (§7). Phase 1 requires `local`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ExecutionLocation {
    /// Runs entirely on this device — the standalone success path.
    Local,
    /// Runs through a relay (unavailable in Phase 1).
    Relay,
    /// Either local or relay at the host's discretion (unavailable in Phase 1).
    Either,
    /// Forward-compatible unknown value.
    Other(String),
}

impl ExecutionLocation {
    /// Wire/SQL string form.
    pub fn as_sql_str(&self) -> String {
        match self {
            Self::Local => "LOCAL".to_string(),
            Self::Relay => "RELAY".to_string(),
            Self::Either => "EITHER".to_string(),
            Self::Other(raw) => raw.clone(),
        }
    }

    /// Parse from storage; unknown values round-trip.
    pub fn from_sql_str(value: &str) -> Self {
        match value {
            "LOCAL" => Self::Local,
            "RELAY" => Self::Relay,
            "EITHER" => Self::Either,
            other => Self::Other(other.to_string()),
        }
    }

    /// Whether Phase 1 supports this location.
    pub fn is_supported(&self) -> bool {
        matches!(self, Self::Local)
    }
}

impl Serialize for ExecutionLocation {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.as_sql_str())
    }
}

impl<'de> Deserialize<'de> for ExecutionLocation {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from_sql_str(&String::deserialize(deserializer)?))
    }
}

/// Contribution kind (§2 taxonomy). Phase 1 accepts exactly one contribution of
/// kind `context_source`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ContributionKind {
    /// Read-only context ingestion; remote payload is always untrusted data.
    ContextSource,
    /// Curated instructions; never grants a capability (unavailable in Phase 1).
    SkillPack,
    /// External-write boundary (unavailable in Phase 1).
    ActionAdapter,
    /// Forward-compatible unknown value; inventory/export only.
    Other(String),
}

impl ContributionKind {
    /// Wire/SQL string form.
    pub fn as_sql_str(&self) -> String {
        match self {
            Self::ContextSource => "CONTEXT_SOURCE".to_string(),
            Self::SkillPack => "SKILL_PACK".to_string(),
            Self::ActionAdapter => "ACTION_ADAPTER".to_string(),
            Self::Other(raw) => raw.clone(),
        }
    }

    /// Parse from storage; unknown values round-trip.
    pub fn from_sql_str(value: &str) -> Self {
        match value {
            "CONTEXT_SOURCE" => Self::ContextSource,
            "SKILL_PACK" => Self::SkillPack,
            "ACTION_ADAPTER" => Self::ActionAdapter,
            other => Self::Other(other.to_string()),
        }
    }

    /// Whether Phase 1 supports enabling this contribution.
    pub fn is_supported(&self) -> bool {
        matches!(self, Self::ContextSource)
    }
}

impl Serialize for ContributionKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.as_sql_str())
    }
}

impl<'de> Deserialize<'de> for ContributionKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from_sql_str(&String::deserialize(deserializer)?))
    }
}

/// Signature/trust state of the package artifact (§2 source trust).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SignatureState {
    /// Trust derives from the signed app bundle itself.
    AppBundleTrusted,
    /// Artifact signature verified against a known key.
    Verified,
    /// Signature present but not verifiable.
    Unverified,
    /// No signature present.
    Absent,
}

impl SignatureState {
    /// Wire/SQL string form.
    pub fn as_sql_str(&self) -> &'static str {
        match self {
            Self::AppBundleTrusted => "APP_BUNDLE_TRUSTED",
            Self::Verified => "VERIFIED",
            Self::Unverified => "UNVERIFIED",
            Self::Absent => "ABSENT",
        }
    }

    /// Parse a known signature state.
    pub fn from_sql_str(value: &str) -> Option<Self> {
        match value {
            "APP_BUNDLE_TRUSTED" => Some(Self::AppBundleTrusted),
            "VERIFIED" => Some(Self::Verified),
            "UNVERIFIED" => Some(Self::Unverified),
            "ABSENT" => Some(Self::Absent),
            _ => None,
        }
    }

    /// Whether this state clears the §10 signature gate.
    pub fn is_trusted(&self) -> bool {
        matches!(self, Self::AppBundleTrusted | Self::Verified)
    }
}

/// Typed unavailability reason (§10 + Amendment I2). This is a registry-scoped
/// namespace, deliberately NOT part of the ADR-019 wire-locked `CoreError` codes
/// (Amendment I5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnavailableReason {
    /// Manifest schema major is not understood.
    ManifestSchemaUnsupported,
    /// `host_api_min <= CURRENT_HOST_API < host_api_max` does not hold.
    HostApiIncompatible,
    /// Runtime kind is not supported in this phase.
    RuntimeUnsupported,
    /// Contribution kind is not supported in this phase.
    ContributionUnsupported,
    /// Package source kind is not supported in this phase.
    SourceUnsupported,
    /// Execution location is not supported in this phase (Amendment I2).
    ExecutionLocationUnsupported,
    /// A marketplace backend would be required but none exists.
    MarketplaceUnavailable,
    /// Publisher identity mismatch — a substitution signal, hard reject.
    PublisherUntrusted,
    /// Signature could not be verified.
    SignatureUnverified,
    /// Managed policy denies this package/version.
    ManagedPolicyDenied,
    /// Product consent is required before operating.
    ConsentRequired,
    /// Account authentication is required before operating.
    AuthenticationRequired,
    /// A capability grant is required before operating.
    CapabilityGrantRequired,
    /// The upstream source is unhealthy.
    SourceUnhealthy,
    /// Uninstall cleanup did not complete.
    CleanupIncomplete,
}

impl UnavailableReason {
    /// Stable wire string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ManifestSchemaUnsupported => "manifest_schema_unsupported",
            Self::HostApiIncompatible => "host_api_incompatible",
            Self::RuntimeUnsupported => "runtime_unsupported",
            Self::ContributionUnsupported => "contribution_unsupported",
            Self::SourceUnsupported => "source_unsupported",
            Self::ExecutionLocationUnsupported => "execution_location_unsupported",
            Self::MarketplaceUnavailable => "marketplace_unavailable",
            Self::PublisherUntrusted => "publisher_untrusted",
            Self::SignatureUnverified => "signature_unverified",
            Self::ManagedPolicyDenied => "managed_policy_denied",
            Self::ConsentRequired => "consent_required",
            Self::AuthenticationRequired => "authentication_required",
            Self::CapabilityGrantRequired => "capability_grant_required",
            Self::SourceUnhealthy => "source_unhealthy",
            Self::CleanupIncomplete => "cleanup_incomplete",
        }
    }
}

/// Health degradation reason (§5 + Amendment I4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthReason {
    /// Upstream rate limit hit.
    RateLimited,
    /// Upstream returned an error.
    UpstreamError,
    /// Upstream timed out.
    Timeout,
    /// Stored credentials expired.
    AuthExpired,
    /// Forward-compatible unknown cause.
    Unknown,
}

// ---- The eight orthogonal axes (§5) ----

/// Whether the package can be operated at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum Availability {
    /// Every compatibility/trust gate passed.
    Available,
    /// Fail-closed with a typed reason.
    Unavailable {
        /// Why the package cannot be operated.
        reason: UnavailableReason,
    },
}

/// What the user did with the package. Note `bundled` is NOT here — provenance
/// is a separate fact (Amendment B1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallationState {
    /// Present (possibly bundled) but not installed by the user.
    NotInstalled,
    /// Install in progress.
    Installing,
    /// Installed.
    Installed,
    /// Install attempt failed.
    InstallFailed,
    /// Uninstall in progress.
    Uninstalling,
    /// Uninstalled.
    Uninstalled,
}

impl InstallationState {
    /// Wire/SQL string form.
    pub fn as_sql_str(&self) -> &'static str {
        match self {
            Self::NotInstalled => "NOT_INSTALLED",
            Self::Installing => "INSTALLING",
            Self::Installed => "INSTALLED",
            Self::InstallFailed => "INSTALL_FAILED",
            Self::Uninstalling => "UNINSTALLING",
            Self::Uninstalled => "UNINSTALLED",
        }
    }

    /// Parse a known installation state.
    pub fn from_sql_str(value: &str) -> Option<Self> {
        match value {
            "NOT_INSTALLED" => Some(Self::NotInstalled),
            "INSTALLING" => Some(Self::Installing),
            "INSTALLED" => Some(Self::Installed),
            "INSTALL_FAILED" => Some(Self::InstallFailed),
            "UNINSTALLING" => Some(Self::Uninstalling),
            "UNINSTALLED" => Some(Self::Uninstalled),
            _ => None,
        }
    }

    /// Allowed installation transitions.
    pub fn can_transition_to(&self, target: InstallationState) -> bool {
        use InstallationState::*;
        match self {
            NotInstalled | Uninstalled | InstallFailed => matches!(target, Installing),
            Installing => matches!(target, Installed | InstallFailed),
            Installed => matches!(target, Uninstalling),
            Uninstalling => matches!(target, Uninstalled),
        }
    }
}

/// Whether the user enabled the installed package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Enablement {
    /// Disabled — blocks new work immediately.
    Disabled,
    /// Enabled.
    Enabled,
}

impl Enablement {
    /// Wire/SQL string form.
    pub fn as_sql_str(&self) -> &'static str {
        match self {
            Self::Disabled => "DISABLED",
            Self::Enabled => "ENABLED",
        }
    }

    /// Parse a known enablement value.
    pub fn from_sql_str(value: &str) -> Option<Self> {
        match value {
            "DISABLED" => Some(Self::Disabled),
            "ENABLED" => Some(Self::Enabled),
            _ => None,
        }
    }
}

/// Per-account authentication state. `NotRequired` is the correct value for a
/// bundled built-in that needs no account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountAuthentication {
    /// This extension needs no account.
    NotRequired,
    /// Account required but not connected.
    Unauthenticated,
    /// Authorization flow in progress.
    Authenticating,
    /// Account connected.
    Authenticated,
    /// Access was revoked.
    Revoked,
    /// Authentication failed.
    AuthError,
}

impl AccountAuthentication {
    /// Wire/SQL string form.
    pub fn as_sql_str(&self) -> &'static str {
        match self {
            Self::NotRequired => "NOT_REQUIRED",
            Self::Unauthenticated => "UNAUTHENTICATED",
            Self::Authenticating => "AUTHENTICATING",
            Self::Authenticated => "AUTHENTICATED",
            Self::Revoked => "REVOKED",
            Self::AuthError => "AUTH_ERROR",
        }
    }

    /// Parse a known authentication state.
    pub fn from_sql_str(value: &str) -> Option<Self> {
        match value {
            "NOT_REQUIRED" => Some(Self::NotRequired),
            "UNAUTHENTICATED" => Some(Self::Unauthenticated),
            "AUTHENTICATING" => Some(Self::Authenticating),
            "AUTHENTICATED" => Some(Self::Authenticated),
            "REVOKED" => Some(Self::Revoked),
            "AUTH_ERROR" => Some(Self::AuthError),
            _ => None,
        }
    }

    /// Whether this axis permits operation.
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::NotRequired | Self::Authenticated)
    }
}

/// Capability grant state per account/capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityGrant {
    /// Never requested.
    NotRequested,
    /// Requested, awaiting a decision.
    Pending,
    /// Granted.
    Granted,
    /// Explicitly denied.
    Denied,
    /// Previously granted, then revoked.
    Revoked,
    /// Grant lapsed.
    Expired,
}

impl CapabilityGrant {
    /// Wire/SQL string form.
    pub fn as_sql_str(&self) -> &'static str {
        match self {
            Self::NotRequested => "NOT_REQUESTED",
            Self::Pending => "PENDING",
            Self::Granted => "GRANTED",
            Self::Denied => "DENIED",
            Self::Revoked => "REVOKED",
            Self::Expired => "EXPIRED",
        }
    }

    /// Parse a known grant state.
    pub fn from_sql_str(value: &str) -> Option<Self> {
        match value {
            "NOT_REQUESTED" => Some(Self::NotRequested),
            "PENDING" => Some(Self::Pending),
            "GRANTED" => Some(Self::Granted),
            "DENIED" => Some(Self::Denied),
            "REVOKED" => Some(Self::Revoked),
            "EXPIRED" => Some(Self::Expired),
            _ => None,
        }
    }

    /// Whether this axis permits operation.
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Granted)
    }
}

/// Update state of the installed version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateState {
    /// Installed version is current.
    Current,
    /// A newer version exists.
    UpdateAvailable,
    /// A newer version is staged for activation.
    Staged,
    /// Activation in progress.
    Activating,
    /// Activation failed.
    UpdateFailed,
    /// A previous known-good version can be restored.
    RollbackAvailable,
}

impl UpdateState {
    /// Wire/SQL string form.
    pub fn as_sql_str(&self) -> &'static str {
        match self {
            Self::Current => "CURRENT",
            Self::UpdateAvailable => "UPDATE_AVAILABLE",
            Self::Staged => "STAGED",
            Self::Activating => "ACTIVATING",
            Self::UpdateFailed => "UPDATE_FAILED",
            Self::RollbackAvailable => "ROLLBACK_AVAILABLE",
        }
    }

    /// Parse a known update state.
    pub fn from_sql_str(value: &str) -> Option<Self> {
        match value {
            "CURRENT" => Some(Self::Current),
            "UPDATE_AVAILABLE" => Some(Self::UpdateAvailable),
            "STAGED" => Some(Self::Staged),
            "ACTIVATING" => Some(Self::Activating),
            "UPDATE_FAILED" => Some(Self::UpdateFailed),
            "ROLLBACK_AVAILABLE" => Some(Self::RollbackAvailable),
            _ => None,
        }
    }

    /// Whether an in-flight activation must be aborted before uninstall
    /// (Amendment I3a).
    pub fn is_activation_in_flight(&self) -> bool {
        matches!(self, Self::Staged | Self::Activating)
    }
}

/// Health of the installed extension.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum Health {
    /// Not yet observed.
    Unknown,
    /// Operating normally.
    Healthy,
    /// Working with reduced quality.
    Degraded {
        /// Why health is degraded.
        reason: HealthReason,
    },
    /// Not working.
    Unhealthy {
        /// Why health failed.
        reason: HealthReason,
    },
}

/// One declared contribution (§2). Phase 1 accepts exactly one, of kind
/// `context_source`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contribution {
    /// Stable contribution id within the extension.
    pub contribution_id: String,
    /// Bounded contribution kind.
    pub kind: ContributionKind,
    /// Contribution API version.
    pub api_version: u32,
    /// Requested capability identifiers.
    pub requested_capabilities: Vec<String>,
}

/// The versioned manifest contract (`maekon.extension_manifest.v1`, §2).
///
/// Contains no OAuth secret, account token, ACL member list, cursor, payload,
/// user path, or executable command line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionManifest {
    // identity
    /// Reverse-DNS style, stable across versions.
    pub extension_id: String,
    /// Semantic version of this package.
    pub version: String,
    /// Immutable publisher identity — a change is a substitution signal.
    pub publisher_id: String,
    /// Digest of the package artifact.
    pub package_digest: String,
    // source trust
    /// Bounded source kind.
    pub source_kind: SourceKind,
    /// Signature/trust state.
    pub signature_state: SignatureState,
    /// Signing key id when applicable.
    pub signature_key_id: Option<String>,
    /// Digest of the signed manifest.
    pub signed_manifest_digest: Option<String>,
    // compatibility
    /// Manifest schema major version.
    pub manifest_schema: u32,
    /// Inclusive minimum host API.
    pub host_api_min: u32,
    /// Exclusive maximum host API.
    pub host_api_max: u32,
    // runtime
    /// Runtime kind.
    pub runtime_kind: RuntimeKind,
    /// Where the runtime executes.
    pub execution_location: ExecutionLocation,
    /// Declared entry-point identifier (never a filesystem path in public DTOs).
    pub entry_point: String,
    // contributions
    /// Declared contributions.
    pub contributions: Vec<Contribution>,
    // policy
    /// Optional permission profile reference.
    pub permission_profile_id: Option<String>,
    /// Declared external egress destinations.
    pub external_egress: Vec<String>,
    /// Data classification label.
    pub data_classification: Option<String>,
    /// Retention class label.
    pub retention_class: Option<String>,
    // lifecycle
    /// Update channel.
    pub update_channel: Option<String>,
    /// Rollback window descriptor.
    pub rollback_window: Option<String>,
    /// Minimum allowed version under managed policy.
    pub minimum_allowed_version: Option<String>,
    /// Declared uninstall cleanup steps.
    pub uninstall_cleanup: Vec<String>,
}

impl ExtensionManifest {
    /// Evaluate the §10 fail-closed compatibility/trust gates in a fixed order.
    ///
    /// Returns [`Availability::Available`] only when every gate passes; the first
    /// failing gate's typed reason is returned otherwise. Never best-effort.
    pub fn evaluate_availability(&self) -> Availability {
        // Schema major must be understood before any other field is trusted.
        if self.manifest_schema != SUPPORTED_MANIFEST_SCHEMA_MAJOR {
            return Availability::Unavailable {
                reason: UnavailableReason::ManifestSchemaUnsupported,
            };
        }
        // Empty or inverted ranges fail closed.
        if self.host_api_min >= self.host_api_max
            || CURRENT_HOST_API < self.host_api_min
            || CURRENT_HOST_API >= self.host_api_max
        {
            return Availability::Unavailable {
                reason: UnavailableReason::HostApiIncompatible,
            };
        }
        if !self.source_kind.is_supported() {
            return Availability::Unavailable {
                reason: UnavailableReason::SourceUnsupported,
            };
        }
        if !self.signature_state.is_trusted() {
            return Availability::Unavailable {
                reason: UnavailableReason::SignatureUnverified,
            };
        }
        if !self.runtime_kind.is_supported() {
            return Availability::Unavailable {
                reason: UnavailableReason::RuntimeUnsupported,
            };
        }
        if !self.execution_location.is_supported() {
            return Availability::Unavailable {
                reason: UnavailableReason::ExecutionLocationUnsupported,
            };
        }
        // Phase 1 accepts exactly one contribution, of a supported kind.
        if self.contributions.len() != 1 || !self.contributions[0].kind.is_supported() {
            return Availability::Unavailable {
                reason: UnavailableReason::ContributionUnsupported,
            };
        }
        Availability::Available
    }
}

/// A persistent installation record — the registry's durable row (§5).
///
/// Provenance and installation state are stored as two independent fields so a
/// bundled built-in can still be `not_installed` (Amendment B1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionInstall {
    /// `inst`-prefixed opaque local installation id.
    pub install_id: String,
    /// Reverse-DNS extension id.
    pub extension_id: String,
    /// Installed version.
    pub version: String,
    /// Where the code came from (orthogonal to `installation`).
    pub provenance: ExtensionProvenance,
    /// Package source kind recorded at install time.
    pub source_kind: SourceKind,
    /// Signature/trust state recorded at install time.
    pub signature_state: SignatureState,
    /// Installation axis.
    pub installation: InstallationState,
    /// Enablement axis.
    pub enablement: Enablement,
    /// Account-authentication axis.
    pub authentication: AccountAuthentication,
    /// Capability-grant axis.
    pub grant: CapabilityGrant,
    /// Update axis.
    pub update: UpdateState,
    /// Health axis.
    pub health: Health,
    /// Previous known-good version available for rollback.
    pub previous_version: Option<String>,
    /// Compare-and-swap revision.
    pub revision: i64,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}

/// The full readiness projection: eight orthogonal axes, never collapsed (§5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionReadiness {
    /// Extension id.
    pub extension_id: String,
    /// Installation id.
    pub install_id: String,
    /// Where the code came from.
    pub provenance: ExtensionProvenance,
    /// Availability axis (fail-closed gates).
    pub availability: Availability,
    /// Installation axis.
    pub installation: InstallationState,
    /// Enablement axis.
    pub enablement: Enablement,
    /// Account-authentication axis.
    pub authentication: AccountAuthentication,
    /// Capability-grant axis.
    pub grant: CapabilityGrant,
    /// Update axis.
    pub update: UpdateState,
    /// Health axis.
    pub health: Health,
}

impl ExtensionReadiness {
    /// Derive the single human-facing summary label, using ADR-031 §5's canonical
    /// vocabulary. Returns `None` when no honest single label exists — callers
    /// must then show the axes rather than inventing one (§5).
    pub fn summary_label(&self) -> Option<&'static str> {
        if let Availability::Unavailable { reason } = &self.availability {
            return Some(match reason {
                UnavailableReason::ManagedPolicyDenied => "revoked",
                _ => "incompatible",
            });
        }
        match self.installation {
            InstallationState::NotInstalled => return Some("discovered"),
            InstallationState::Installed => {}
            _ => return None,
        }
        if self.enablement == Enablement::Disabled {
            return Some("installed");
        }
        match self.authentication {
            AccountAuthentication::Revoked => return Some("revoked"),
            AccountAuthentication::Authenticating => return Some("authorizing"),
            AccountAuthentication::NotRequired | AccountAuthentication::Authenticated => {}
            _ => return Some("enabled"),
        }
        match self.health {
            Health::Degraded { .. } => Some("degraded"),
            Health::Unhealthy { .. } => Some("stale"),
            _ => Some("ready"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builtin_manifest() -> ExtensionManifest {
        ExtensionManifest {
            extension_id: "com.maekon.calendar".to_string(),
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
            uninstall_cleanup: vec!["credentials".to_string(), "cursors".to_string()],
        }
    }

    #[test]
    fn supported_builtin_manifest_is_available() {
        assert_eq!(
            builtin_manifest().evaluate_availability(),
            Availability::Available
        );
    }

    #[test]
    fn unknown_schema_major_fails_closed_first() {
        let mut m = builtin_manifest();
        m.manifest_schema = 99;
        // Also break a later gate to prove ordering: schema is checked first.
        m.runtime_kind = RuntimeKind::Mcp;
        assert_eq!(
            m.evaluate_availability(),
            Availability::Unavailable {
                reason: UnavailableReason::ManifestSchemaUnsupported
            }
        );
    }

    #[test]
    fn host_api_range_is_inclusive_min_exclusive_max() {
        let mut m = builtin_manifest();
        // CURRENT_HOST_API == min is allowed.
        m.host_api_min = CURRENT_HOST_API;
        m.host_api_max = CURRENT_HOST_API + 1;
        assert_eq!(m.evaluate_availability(), Availability::Available);
        // CURRENT_HOST_API == max is rejected (exclusive).
        m.host_api_min = CURRENT_HOST_API.saturating_sub(1);
        m.host_api_max = CURRENT_HOST_API;
        assert_eq!(
            m.evaluate_availability(),
            Availability::Unavailable {
                reason: UnavailableReason::HostApiIncompatible
            }
        );
    }

    #[test]
    fn inverted_or_empty_range_fails_closed() {
        let mut m = builtin_manifest();
        m.host_api_min = 5;
        m.host_api_max = 2;
        assert_eq!(
            m.evaluate_availability(),
            Availability::Unavailable {
                reason: UnavailableReason::HostApiIncompatible
            }
        );
    }

    #[test]
    fn unsupported_runtime_location_source_and_contribution_fail_closed() {
        let mut m = builtin_manifest();
        m.source_kind = SourceKind::CuratedRegistry;
        assert_eq!(
            m.evaluate_availability(),
            Availability::Unavailable {
                reason: UnavailableReason::SourceUnsupported
            }
        );

        let mut m = builtin_manifest();
        m.runtime_kind = RuntimeKind::Declarative;
        assert_eq!(
            m.evaluate_availability(),
            Availability::Unavailable {
                reason: UnavailableReason::RuntimeUnsupported
            }
        );

        // execution_location gets its own code, never folded into runtime/source.
        let mut m = builtin_manifest();
        m.execution_location = ExecutionLocation::Relay;
        assert_eq!(
            m.evaluate_availability(),
            Availability::Unavailable {
                reason: UnavailableReason::ExecutionLocationUnsupported
            }
        );

        let mut m = builtin_manifest();
        m.contributions[0].kind = ContributionKind::ActionAdapter;
        assert_eq!(
            m.evaluate_availability(),
            Availability::Unavailable {
                reason: UnavailableReason::ContributionUnsupported
            }
        );

        // Bundling multiple contributions is deferred.
        let mut m = builtin_manifest();
        let extra = m.contributions[0].clone();
        m.contributions.push(extra);
        assert_eq!(
            m.evaluate_availability(),
            Availability::Unavailable {
                reason: UnavailableReason::ContributionUnsupported
            }
        );
    }

    #[test]
    fn untrusted_signature_fails_closed() {
        let mut m = builtin_manifest();
        m.signature_state = SignatureState::Unverified;
        assert_eq!(
            m.evaluate_availability(),
            Availability::Unavailable {
                reason: UnavailableReason::SignatureUnverified
            }
        );
    }

    #[test]
    fn provenance_is_independent_of_installation_state() {
        // A bundled built-in still starts not_installed and can install.
        assert!(InstallationState::NotInstalled.can_transition_to(InstallationState::Installing));
        assert!(InstallationState::Installing.can_transition_to(InstallationState::Installed));
        assert!(InstallationState::Installed.can_transition_to(InstallationState::Uninstalling));
        // No shortcut from not_installed straight to installed.
        assert!(!InstallationState::NotInstalled.can_transition_to(InstallationState::Installed));
        // Uninstalled can be re-installed.
        assert!(InstallationState::Uninstalled.can_transition_to(InstallationState::Installing));
    }

    #[test]
    fn unknown_forward_compatible_values_round_trip() {
        assert_eq!(SourceKind::from_sql_str("FUTURE").as_sql_str(), "FUTURE");
        assert!(!SourceKind::from_sql_str("FUTURE").is_supported());
        assert_eq!(RuntimeKind::from_sql_str("FUTURE").as_sql_str(), "FUTURE");
        assert!(!RuntimeKind::from_sql_str("FUTURE").is_supported());
        assert_eq!(
            ContributionKind::from_sql_str("FUTURE").as_sql_str(),
            "FUTURE"
        );
        assert!(!ContributionKind::from_sql_str("FUTURE").is_supported());
        assert_eq!(
            ExecutionLocation::from_sql_str("FUTURE").as_sql_str(),
            "FUTURE"
        );
        assert!(!ExecutionLocation::from_sql_str("FUTURE").is_supported());
    }

    fn readiness(installation: InstallationState, enablement: Enablement) -> ExtensionReadiness {
        ExtensionReadiness {
            extension_id: "com.maekon.calendar".to_string(),
            install_id: "inst_1".to_string(),
            provenance: ExtensionProvenance::Bundled,
            availability: Availability::Available,
            installation,
            enablement,
            authentication: AccountAuthentication::NotRequired,
            grant: CapabilityGrant::Granted,
            update: UpdateState::Current,
            health: Health::Healthy,
        }
    }

    #[test]
    fn summary_label_never_hides_a_partial_state() {
        // Fully ready.
        assert_eq!(
            readiness(InstallationState::Installed, Enablement::Enabled).summary_label(),
            Some("ready")
        );
        // Installed but disabled must not read as connected/ready.
        assert_eq!(
            readiness(InstallationState::Installed, Enablement::Disabled).summary_label(),
            Some("installed")
        );
        // Not installed reads as discovered.
        assert_eq!(
            readiness(InstallationState::NotInstalled, Enablement::Disabled).summary_label(),
            Some("discovered")
        );
        // In-flight installation has no honest single label.
        assert_eq!(
            readiness(InstallationState::Installing, Enablement::Disabled).summary_label(),
            None
        );
        // Unavailable never reads as installed/connected.
        let mut r = readiness(InstallationState::Installed, Enablement::Enabled);
        r.availability = Availability::Unavailable {
            reason: UnavailableReason::HostApiIncompatible,
        };
        assert_eq!(r.summary_label(), Some("incompatible"));
        // Revoked auth surfaces as revoked, not ready.
        let mut r = readiness(InstallationState::Installed, Enablement::Enabled);
        r.authentication = AccountAuthentication::Revoked;
        assert_eq!(r.summary_label(), Some("revoked"));
        // Degraded health is not ready.
        let mut r = readiness(InstallationState::Installed, Enablement::Enabled);
        r.health = Health::Degraded {
            reason: HealthReason::RateLimited,
        };
        assert_eq!(r.summary_label(), Some("degraded"));
    }

    #[test]
    fn reason_codes_have_stable_wire_strings() {
        assert_eq!(
            UnavailableReason::ExecutionLocationUnsupported.as_str(),
            "execution_location_unsupported"
        );
        assert_eq!(
            UnavailableReason::PublisherUntrusted.as_str(),
            "publisher_untrusted"
        );
        assert_eq!(
            UnavailableReason::CleanupIncomplete.as_str(),
            "cleanup_incomplete"
        );
    }

    #[test]
    fn activation_in_flight_blocks_naive_uninstall() {
        assert!(UpdateState::Staged.is_activation_in_flight());
        assert!(UpdateState::Activating.is_activation_in_flight());
        assert!(!UpdateState::Current.is_activation_in_flight());
    }

    #[test]
    fn manifest_serde_round_trips() {
        let m = builtin_manifest();
        let json = serde_json::to_string(&m).unwrap();
        let back: ExtensionManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m);
    }

    /// #10197 Wave 2: mutation guards. 51 mutants survived here — every
    /// `from_sql_str` delete-arm (these strings are the SQLite persistence
    /// format: a dropped arm makes a stored state unreadable after restart),
    /// the paired `as_sql_str` constant replacements, the `is_ready` axis, the
    /// host-API window comparisons, and three `summary_label` arms. Nested in
    /// `tests` to reuse `builtin_manifest()` / `readiness()`.
    mod mutation_guard {
        use super::*;

        /// Round-trips kill both sides at once: a deleted `from_sql_str` arm
        /// returns None for its own spelling, and a replaced `as_sql_str`
        /// constant ("", "xyzzy") no longer parses back.
        macro_rules! sql_round_trip {
            ($name:ident, $ty:ident, [$($variant:expr),+ $(,)?]) => {
                #[test]
                fn $name() {
                    for v in [$($variant),+] {
                        let s = v.as_sql_str();
                        assert!(!s.is_empty(), "{v:?} must have a wire spelling");
                        assert_eq!(
                            $ty::from_sql_str(s),
                            Some(v.clone()),
                            "{v:?} must round-trip through {s:?} — this is the \
                             SQLite persistence format, not a display string"
                        );
                    }
                    assert_eq!($ty::from_sql_str("NO_SUCH_STATE"), None);
                }
            };
        }

        sql_round_trip!(
            provenance_round_trips,
            ExtensionProvenance,
            [
                ExtensionProvenance::Bundled,
                ExtensionProvenance::CuratedLocal
            ]
        );
        sql_round_trip!(
            signature_state_round_trips,
            SignatureState,
            [
                SignatureState::AppBundleTrusted,
                SignatureState::Verified,
                SignatureState::Unverified,
                SignatureState::Absent,
            ]
        );
        sql_round_trip!(
            installation_state_round_trips,
            InstallationState,
            [
                InstallationState::NotInstalled,
                InstallationState::Installing,
                InstallationState::Installed,
                InstallationState::InstallFailed,
                InstallationState::Uninstalling,
                InstallationState::Uninstalled,
            ]
        );
        sql_round_trip!(
            enablement_round_trips,
            Enablement,
            [Enablement::Disabled, Enablement::Enabled]
        );
        sql_round_trip!(
            account_authentication_round_trips,
            AccountAuthentication,
            [
                AccountAuthentication::NotRequired,
                AccountAuthentication::Unauthenticated,
                AccountAuthentication::Authenticating,
                AccountAuthentication::Authenticated,
                AccountAuthentication::Revoked,
                AccountAuthentication::AuthError,
            ]
        );
        sql_round_trip!(
            capability_grant_round_trips,
            CapabilityGrant,
            [
                CapabilityGrant::NotRequested,
                CapabilityGrant::Pending,
                CapabilityGrant::Granted,
                CapabilityGrant::Denied,
                CapabilityGrant::Revoked,
                CapabilityGrant::Expired,
            ]
        );
        sql_round_trip!(
            update_state_round_trips,
            UpdateState,
            [
                UpdateState::Current,
                UpdateState::UpdateAvailable,
                UpdateState::Staged,
                UpdateState::Activating,
                UpdateState::UpdateFailed,
                UpdateState::RollbackAvailable,
            ]
        );

        #[test]
        fn is_ready_permits_exactly_the_two_operable_states() {
            assert!(AccountAuthentication::NotRequired.is_ready());
            assert!(AccountAuthentication::Authenticated.is_ready());
            assert!(!AccountAuthentication::Unauthenticated.is_ready());
            assert!(!AccountAuthentication::Revoked.is_ready());
            assert!(!AccountAuthentication::AuthError.is_ready());
        }

        /// The host-API window is `min <= CURRENT < max`, and empty/inverted
        /// ranges fail closed. Each disjunct is exercised where IT alone
        /// rejects, so `||` -> `&&` and the `<` flip both surface.
        #[test]
        fn host_api_window_rejections_are_each_sufficient() {
            // Empty range (min == max == CURRENT): only the inverted-range
            // disjunct is true — CURRENT < min is false.
            let mut manifest = builtin_manifest();
            manifest.host_api_min = CURRENT_HOST_API;
            manifest.host_api_max = CURRENT_HOST_API;
            assert_eq!(
                manifest.evaluate_availability(),
                Availability::Unavailable {
                    reason: UnavailableReason::HostApiIncompatible
                },
                "an empty window must fail closed on its own"
            );

            // Too-new manifest (min > CURRENT, well-formed range): only the
            // `CURRENT < min` disjunct is true.
            let mut manifest = builtin_manifest();
            manifest.host_api_min = CURRENT_HOST_API + 1;
            manifest.host_api_max = CURRENT_HOST_API + 2;
            assert_eq!(
                manifest.evaluate_availability(),
                Availability::Unavailable {
                    reason: UnavailableReason::HostApiIncompatible
                },
                "a future-only window must fail closed on its own"
            );
        }

        /// Three summary_label arms whose deletion falls through to a
        /// DIFFERENT label (the wildcard below each), so exact-label
        /// assertions are the only thing keeping them.
        #[test]
        fn summary_label_arms_that_shadow_a_wildcard() {
            // ManagedPolicyDenied -> "revoked"; the wildcard says "incompatible".
            let mut r = readiness(InstallationState::Installed, Enablement::Enabled);
            r.availability = Availability::Unavailable {
                reason: UnavailableReason::ManagedPolicyDenied,
            };
            assert_eq!(r.summary_label(), Some("revoked"));

            // Authenticating -> "authorizing"; the wildcard says "enabled".
            let mut r = readiness(InstallationState::Installed, Enablement::Enabled);
            r.authentication = AccountAuthentication::Authenticating;
            assert_eq!(r.summary_label(), Some("authorizing"));

            // Unhealthy -> "stale"; the wildcard says "ready".
            let mut r = readiness(InstallationState::Installed, Enablement::Enabled);
            r.health = Health::Unhealthy {
                reason: HealthReason::UpstreamError,
            };
            assert_eq!(r.summary_label(), Some("stale"));
        }
    }
}
