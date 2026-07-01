//! Managed (MDM) configuration overlay — issue #4832 (E20-40).
//!
//! An enterprise administrator can deploy a read-only `managed.json` policy file
//! to a system-wide, admin-writable location. Any field present in that file is
//! **locked**: the local user cannot override it. Enforcement happens at the
//! single `ConfigManager` write chokepoint (`update` / `update_with` /
//! construction), so every mutation path — the settings HTTP API, the Tauri
//! WebView IPC, backup restore, scheduler writers — is clamped by construction.
//!
//! ## Design
//! - `managed.json` ABSENT  ⇒ no policy ⇒ normal operation (consumer default).
//! - `managed.json` PRESENT but malformed / bad enum / future schema_version
//!   ⇒ **fail-closed**: the app refuses to start unmanaged (a present-but-broken
//!   policy file means the admin *intended* locks; silently ignoring it would be
//!   fail-open on privacy/telemetry).
//!
//! ## Scope (MVP)
//! Only the hardcoded lockable allowlist below is supported. The struct *is* the
//! allowlist — locking an arbitrary `AppConfig` path is compile-time impossible.
//! Native OS policy stores (registry/plist/dconf), ADMX templates (#4837), and
//! staged-rollout descriptors (#4836) are out of scope; this layer is their
//! foundation (plain serde, dotted-path lock identities, single load seam).

use crate::config::{
    AppConfig, CloudSttPolicy, LlmProviderType, OcrProviderType, PiiFilterLevel, SandboxProfile,
};
use crate::error::CoreError;
use crate::provider_surface::canonical_provider_surface_id;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

fn default_managed_schema_version() -> u32 {
    ManagedConfig::SUPPORTED_SCHEMA_VERSION
}

fn normalize_policy_token(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace('-', "_")
}

fn sandbox_profile_rank(profile: SandboxProfile) -> u8 {
    match profile {
        SandboxProfile::Permissive => 0,
        SandboxProfile::Standard => 1,
        SandboxProfile::Strict => 2,
    }
}

fn sandbox_profile_id(profile: SandboxProfile) -> &'static str {
    match profile {
        SandboxProfile::Permissive => "permissive",
        SandboxProfile::Standard => "standard",
        SandboxProfile::Strict => "strict",
    }
}

fn feature_pin_reason(feature: &str, pin: ManagedFeaturePin) -> String {
    let state = match pin {
        ManagedFeaturePin::Disabled => "disabled",
        ManagedFeaturePin::AllowedWithUserConsent => "allowed_with_user_consent",
    };
    format!("managed.feature.{feature}.{state}")
}

/// Locked privacy policy fields.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedPrivacy {
    /// Lock `privacy.pii_filter_level` to this value when present.
    #[serde(default)]
    pub pii_filter_level: Option<PiiFilterLevel>,
    /// Enforce a minimum PII filter floor without lowering stricter user values.
    #[serde(default)]
    pub min_pii_filter_level: Option<PiiFilterLevel>,
}

/// Locked telemetry policy fields.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedTelemetry {
    /// Lock `telemetry.enabled` to this value when present.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Lock `telemetry.crash_reports` to this value when present.
    #[serde(default)]
    pub crash_reports: Option<bool>,
}

/// Locked vision (screen-capture) policy fields.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedVision {
    /// Lock `vision.capture_enabled` to this value when present.
    #[serde(default)]
    pub capture_enabled: Option<bool>,
}

/// Locked audio policy fields.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedAudio {
    /// Lock `audio.cloud_stt_policy` to this value when present.
    #[serde(default)]
    pub cloud_stt_policy: Option<CloudSttPolicy>,
}

/// Locked update policy fields (kill-switch foundation, #4836).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedUpdate {
    /// Lock `update.enabled` to this value when present.
    /// Locking `false` is an immediate fleet kill-switch.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Lock `update.min_allowed_version` (semver floor) when present. Pins the
    /// oldest release the auto-updater may install — a downgrade/rollback guard
    /// below a security baseline (#4836).
    #[serde(default)]
    pub min_allowed_version: Option<String>,
    /// Lock `update.max_allowed_version` (semver ceiling) when present. Freezes
    /// the fleet at/below a known-good version — a soft update kill-switch that,
    /// unlike `enabled = false`, still permits updates up to the cap (#4836).
    #[serde(default)]
    pub max_allowed_version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedFeaturePin {
    Disabled,
    AllowedWithUserConsent,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedFeaturePins {
    #[serde(default)]
    pub automation: Option<ManagedFeaturePin>,
    #[serde(default)]
    pub browser_verification: Option<ManagedFeaturePin>,
    #[serde(default)]
    pub record_to_template: Option<ManagedFeaturePin>,
    #[serde(default)]
    pub remote_observe_approve: Option<ManagedFeaturePin>,
    #[serde(default)]
    pub external_ocr_llm: Option<ManagedFeaturePin>,
    #[serde(default)]
    pub telemetry: Option<ManagedFeaturePin>,
    #[serde(default)]
    pub sync: Option<ManagedFeaturePin>,
}

impl ManagedFeaturePins {
    fn unsupported_runtime_pins(&self) -> Vec<&'static str> {
        let mut fields = Vec::new();
        if self.browser_verification.is_some() {
            fields.push("features.browser_verification");
        }
        if self.record_to_template.is_some() {
            fields.push("features.record_to_template");
        }
        if self.remote_observe_approve.is_some() {
            fields.push("features.remote_observe_approve");
        }
        fields
    }

    pub fn locked_fields(&self) -> Vec<&'static str> {
        let mut fields = Vec::new();
        if self.automation.is_some() {
            fields.push("features.automation");
        }
        if self.external_ocr_llm.is_some() {
            fields.push("features.external_ocr_llm");
        }
        if self.telemetry.is_some() {
            fields.push("features.telemetry");
        }
        if self.sync.is_some() {
            fields.push("features.sync");
        }
        fields
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedPermissionProfilePolicy {
    #[serde(default)]
    pub allowed_profiles: Vec<String>,
    #[serde(default)]
    pub denied_profiles: Vec<String>,
}

impl ManagedPermissionProfilePolicy {
    pub fn allows_profile(&self, profile_id: &str) -> bool {
        let normalized = normalize_policy_token(profile_id);
        if self
            .denied_profiles
            .iter()
            .any(|candidate| normalize_policy_token(candidate) == normalized)
        {
            return false;
        }
        self.allowed_profiles.is_empty()
            || self
                .allowed_profiles
                .iter()
                .any(|candidate| normalize_policy_token(candidate) == normalized)
    }

    fn locked_fields(&self) -> Vec<&'static str> {
        let mut fields = Vec::new();
        if !self.allowed_profiles.is_empty() {
            fields.push("permission_profiles.allowed_profiles");
        }
        if !self.denied_profiles.is_empty() {
            fields.push("permission_profiles.denied_profiles");
        }
        fields
    }

    fn strictest_allowed_builtin_profile(&self) -> SandboxProfile {
        [
            SandboxProfile::Strict,
            SandboxProfile::Standard,
            SandboxProfile::Permissive,
        ]
        .into_iter()
        .find(|profile| self.allows_profile(sandbox_profile_id(*profile)))
        .unwrap_or(SandboxProfile::Strict)
    }

    fn has_supported_builtin_profile(&self) -> bool {
        [
            SandboxProfile::Strict,
            SandboxProfile::Standard,
            SandboxProfile::Permissive,
        ]
        .into_iter()
        .any(|profile| self.allows_profile(sandbox_profile_id(profile)))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedProviderSurfacePolicy {
    #[serde(default)]
    pub enforce: bool,
    #[serde(default)]
    pub allowlist: Vec<String>,
}

impl ManagedProviderSurfacePolicy {
    pub fn allows_surface(&self, surface_id: Option<&str>) -> bool {
        if !self.is_enforced() {
            return true;
        }

        let Some(surface_id) = surface_id.and_then(canonical_provider_surface_id) else {
            return false;
        };

        self.allowlist.iter().any(|candidate| {
            canonical_provider_surface_id(candidate).is_some_and(|allowed| allowed == surface_id)
        })
    }

    fn is_enforced(&self) -> bool {
        self.enforce || !self.allowlist.is_empty()
    }

    fn locked_fields(&self) -> Vec<&'static str> {
        if self.is_enforced() {
            vec!["provider_surfaces.allowlist"]
        } else {
            Vec::new()
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedSandboxNetworkPolicy {
    #[serde(default)]
    pub require_sandbox_enabled: bool,
    #[serde(default)]
    pub minimum_profile: Option<SandboxProfile>,
    #[serde(default)]
    pub deny_network: bool,
    #[serde(default)]
    pub deny_local_targets: bool,
    #[serde(default)]
    pub deny_local_binding: bool,
}

impl ManagedSandboxNetworkPolicy {
    fn locked_fields(&self) -> Vec<&'static str> {
        let mut fields = Vec::new();
        if self.require_sandbox_enabled {
            fields.push("sandbox_network.require_sandbox_enabled");
        }
        if self.minimum_profile.is_some() {
            fields.push("sandbox_network.minimum_profile");
        }
        if self.deny_network {
            fields.push("sandbox_network.deny_network");
        }
        if self.deny_local_targets {
            fields.push("sandbox_network.deny_local_targets");
        }
        if self.deny_local_binding {
            fields.push("sandbox_network.deny_local_binding");
        }
        fields
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedAuditPolicy {
    #[serde(default)]
    pub min_retention_days: Option<u32>,
    #[serde(default)]
    pub export_required: bool,
}

impl ManagedAuditPolicy {
    fn locked_fields(&self) -> Vec<&'static str> {
        let mut fields = Vec::new();
        if self.min_retention_days.is_some() {
            fields.push("audit.min_retention_days");
        }
        if self.export_required {
            fields.push("audit.export_required");
        }
        fields
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedSignaturePolicy {
    #[serde(default)]
    pub require_signature: bool,
    #[serde(default)]
    pub bundle_signature: Option<String>,
    #[serde(default)]
    pub key_id: Option<String>,
}

impl ManagedSignaturePolicy {
    fn validate(&self) -> Result<(), String> {
        if self.require_signature
            && self
                .bundle_signature
                .as_deref()
                .map(str::trim)
                .unwrap_or_default()
                .is_empty()
        {
            return Err("managed policy requires signature but bundle_signature is missing".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedEffectiveSetting {
    pub path: String,
    pub user_value: serde_json::Value,
    pub effective_value: serde_json::Value,
    pub managed_reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedEffectiveReport {
    pub entries: Vec<ManagedEffectiveSetting>,
}

/// Read-only managed policy overlay loaded from `managed.json`.
///
/// Each field carries an `Option`: `Some(v)` = locked to `v`, `None` = the user
/// remains free to set it. Unknown extra keys are tolerated (no
/// `deny_unknown_fields`) so a newer admin-authored file does not brick an older
/// client; forward incompatibility is handled by [`Self::SUPPORTED_SCHEMA_VERSION`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedConfig {
    /// Policy schema version — downgrade guard. A file authored for a *newer*
    /// schema than this client supports is rejected (fail-closed) rather than
    /// silently mis-parsed.
    #[serde(default = "default_managed_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub privacy: ManagedPrivacy,
    #[serde(default)]
    pub telemetry: ManagedTelemetry,
    #[serde(default)]
    pub vision: ManagedVision,
    #[serde(default)]
    pub audio: ManagedAudio,
    #[serde(default)]
    pub update: ManagedUpdate,
    #[serde(default)]
    pub features: ManagedFeaturePins,
    #[serde(default)]
    pub permission_profiles: ManagedPermissionProfilePolicy,
    #[serde(default)]
    pub provider_surfaces: ManagedProviderSurfacePolicy,
    #[serde(default)]
    pub sandbox_network: ManagedSandboxNetworkPolicy,
    #[serde(default)]
    pub audit: ManagedAuditPolicy,
    #[serde(default)]
    pub signature: ManagedSignaturePolicy,
}

impl ManagedConfig {
    /// Highest `managed.json` schema version this client understands.
    pub const SUPPORTED_SCHEMA_VERSION: u32 = 2;

    /// Clamp every locked field of `cfg` to its managed value.
    ///
    /// Returns the dotted-path identities of the fields that were actually
    /// changed (pre-clamp value differed from the managed value). An empty
    /// result means the config already complied with policy.
    pub fn apply(&self, cfg: &mut AppConfig) -> Vec<&'static str> {
        let mut clamped = Vec::new();

        if let Some(v) = self.privacy.pii_filter_level {
            if cfg.privacy.pii_filter_level != v {
                cfg.privacy.pii_filter_level = v;
                clamped.push("privacy.pii_filter_level");
            }
        }
        if let Some(v) = self.privacy.min_pii_filter_level {
            if cfg.privacy.pii_filter_level < v {
                cfg.privacy.pii_filter_level = v;
                clamped.push("privacy.min_pii_filter_level");
            }
        }
        if let Some(v) = self.telemetry.enabled {
            if cfg.telemetry.enabled != v {
                cfg.telemetry.enabled = v;
                clamped.push("telemetry.enabled");
            }
        }
        if let Some(v) = self.telemetry.crash_reports {
            if cfg.telemetry.crash_reports != v {
                cfg.telemetry.crash_reports = v;
                clamped.push("telemetry.crash_reports");
            }
        }
        if let Some(v) = self.vision.capture_enabled {
            if cfg.vision.capture_enabled != v {
                cfg.vision.capture_enabled = v;
                clamped.push("vision.capture_enabled");
            }
        }
        if let Some(v) = self.audio.cloud_stt_policy {
            if cfg.audio.cloud_stt_policy != v {
                cfg.audio.cloud_stt_policy = v;
                clamped.push("audio.cloud_stt_policy");
            }
        }
        if let Some(v) = self.update.enabled {
            if cfg.update.enabled != v {
                cfg.update.enabled = v;
                clamped.push("update.enabled");
            }
        }
        if let Some(ref v) = self.update.min_allowed_version {
            if cfg.update.min_allowed_version.as_deref() != Some(v.as_str()) {
                cfg.update.min_allowed_version = Some(v.clone());
                clamped.push("update.min_allowed_version");
            }
        }
        if let Some(ref v) = self.update.max_allowed_version {
            if cfg.update.max_allowed_version.as_deref() != Some(v.as_str()) {
                cfg.update.max_allowed_version = Some(v.clone());
                clamped.push("update.max_allowed_version");
            }
        }
        self.apply_feature_pins(cfg, &mut clamped);
        self.apply_permission_profile_policy(cfg, &mut clamped);
        self.apply_provider_surface_policy(cfg, &mut clamped);
        self.apply_sandbox_network_policy(cfg, &mut clamped);
        self.apply_audit_policy(cfg, &mut clamped);

        clamped
    }

    fn apply_feature_pins(&self, cfg: &mut AppConfig, clamped: &mut Vec<&'static str>) {
        if self.features.automation == Some(ManagedFeaturePin::Disabled) && cfg.automation.enabled {
            cfg.automation.enabled = false;
            clamped.push("features.automation");
        }

        if self.features.telemetry == Some(ManagedFeaturePin::Disabled) && cfg.telemetry.enabled {
            cfg.telemetry.enabled = false;
            clamped.push("features.telemetry");
        }

        if self.features.sync == Some(ManagedFeaturePin::Disabled) && cfg.sync.enabled {
            cfg.sync.enabled = false;
            clamped.push("features.sync");
        }

        if self.features.external_ocr_llm == Some(ManagedFeaturePin::Disabled) {
            let mut changed = false;
            if cfg.ai_provider.ocr_provider == OcrProviderType::Remote {
                cfg.ai_provider.ocr_provider = OcrProviderType::Local;
                cfg.ai_provider.ocr_api = None;
                changed = true;
            }
            if cfg.ai_provider.llm_provider == LlmProviderType::Remote {
                cfg.ai_provider.llm_provider = LlmProviderType::Local;
                cfg.ai_provider.llm_api = None;
                cfg.ai_provider.llm_api_fallback = None;
                changed = true;
            }
            if cfg.ai_provider.bypass_pii_filter_for_external_ocr {
                cfg.ai_provider.bypass_pii_filter_for_external_ocr = false;
                changed = true;
            }
            if changed {
                clamped.push("features.external_ocr_llm");
            }
        }
    }

    fn apply_provider_surface_policy(&self, cfg: &mut AppConfig, clamped: &mut Vec<&'static str>) {
        if !self.provider_surfaces.is_enforced() {
            return;
        }

        let mut changed = false;
        if cfg.ai_provider.ocr_provider == OcrProviderType::Remote
            && !self.provider_surfaces.allows_surface(
                cfg.ai_provider
                    .ocr_api
                    .as_ref()
                    .and_then(|api| api.surface_id.as_deref()),
            )
        {
            cfg.ai_provider.ocr_provider = OcrProviderType::Local;
            cfg.ai_provider.ocr_api = None;
            changed = true;
        }

        if cfg.ai_provider.llm_provider == LlmProviderType::Remote
            && !self.provider_surfaces.allows_surface(
                cfg.ai_provider
                    .llm_api
                    .as_ref()
                    .and_then(|api| api.surface_id.as_deref()),
            )
        {
            cfg.ai_provider.llm_provider = LlmProviderType::Local;
            cfg.ai_provider.llm_api = None;
            cfg.ai_provider.llm_api_fallback = None;
            changed = true;
        }

        if changed {
            clamped.push("provider_surfaces.allowlist");
        }
    }

    fn apply_permission_profile_policy(
        &self,
        cfg: &mut AppConfig,
        clamped: &mut Vec<&'static str>,
    ) {
        if self.permission_profiles.locked_fields().is_empty() {
            return;
        }

        let current_profile = sandbox_profile_id(cfg.automation.sandbox.profile);
        if self.permission_profiles.allows_profile(current_profile) {
            return;
        }

        cfg.automation.sandbox.profile =
            self.permission_profiles.strictest_allowed_builtin_profile();
        for field in self.permission_profiles.locked_fields() {
            clamped.push(field);
        }
    }

    fn apply_sandbox_network_policy(&self, cfg: &mut AppConfig, clamped: &mut Vec<&'static str>) {
        if self.sandbox_network.require_sandbox_enabled && !cfg.automation.sandbox.enabled {
            cfg.automation.sandbox.enabled = true;
            clamped.push("sandbox_network.require_sandbox_enabled");
        }

        if let Some(minimum) = self.sandbox_network.minimum_profile {
            if sandbox_profile_rank(cfg.automation.sandbox.profile) < sandbox_profile_rank(minimum)
            {
                cfg.automation.sandbox.profile = minimum;
                clamped.push("sandbox_network.minimum_profile");
            }
        }

        let network_denial_fields = [
            (
                self.sandbox_network.deny_network,
                "sandbox_network.deny_network",
            ),
            (
                self.sandbox_network.deny_local_targets,
                "sandbox_network.deny_local_targets",
            ),
            (
                self.sandbox_network.deny_local_binding,
                "sandbox_network.deny_local_binding",
            ),
        ];
        if cfg.automation.sandbox.allow_network
            && network_denial_fields
                .iter()
                .any(|(enabled, _field)| *enabled)
        {
            cfg.automation.sandbox.allow_network = false;
            for (enabled, field) in network_denial_fields {
                if enabled {
                    clamped.push(field);
                }
            }
        }
    }

    fn apply_audit_policy(&self, cfg: &mut AppConfig, clamped: &mut Vec<&'static str>) {
        if let Some(min_retention_days) = self.audit.min_retention_days {
            if cfg.ai_session.audit_retention_days < min_retention_days {
                cfg.ai_session.audit_retention_days = min_retention_days;
                clamped.push("audit.min_retention_days");
            }
        }
    }

    /// Dotted-path identities where `candidate` violates a locked managed value.
    ///
    /// Read-only: used by interactive write paths (settings API, Tauri IPC) to
    /// reject a user override with a clear "managed by your administrator"
    /// message *before* the silent clamp at the write chokepoint.
    pub fn violations(&self, candidate: &AppConfig) -> Vec<&'static str> {
        // Reuse the single source of truth: probe a clone and report what would
        // be clamped. Avoids divergence between detection and enforcement.
        let mut probe = candidate.clone();
        self.apply(&mut probe)
    }

    /// Dotted-path identities of every field this policy locks (regardless of
    /// whether the current config complies). Foundation for ADMX docs / UI
    /// greying (#4837).
    pub fn locked_fields(&self) -> Vec<&'static str> {
        let mut fields = Vec::new();
        if self.privacy.pii_filter_level.is_some() {
            fields.push("privacy.pii_filter_level");
        }
        if self.privacy.min_pii_filter_level.is_some() {
            fields.push("privacy.min_pii_filter_level");
        }
        if self.telemetry.enabled.is_some() {
            fields.push("telemetry.enabled");
        }
        if self.telemetry.crash_reports.is_some() {
            fields.push("telemetry.crash_reports");
        }
        if self.vision.capture_enabled.is_some() {
            fields.push("vision.capture_enabled");
        }
        if self.audio.cloud_stt_policy.is_some() {
            fields.push("audio.cloud_stt_policy");
        }
        if self.update.enabled.is_some() {
            fields.push("update.enabled");
        }
        if self.update.min_allowed_version.is_some() {
            fields.push("update.min_allowed_version");
        }
        if self.update.max_allowed_version.is_some() {
            fields.push("update.max_allowed_version");
        }
        fields.extend(self.features.locked_fields());
        fields.extend(self.permission_profiles.locked_fields());
        fields.extend(self.provider_surfaces.locked_fields());
        fields.extend(self.sandbox_network.locked_fields());
        fields.extend(self.audit.locked_fields());
        fields
    }

    pub fn validate_bundle(&self) -> Result<(), String> {
        self.signature.validate()?;
        let unsupported_feature_pins = self.features.unsupported_runtime_pins();
        if !unsupported_feature_pins.is_empty() {
            return Err(format!(
                "{} are not enforceable by this runtime; remove these reserved pins or upgrade to a client that wires their gates",
                unsupported_feature_pins.join(", ")
            ));
        }
        if !self.permission_profiles.locked_fields().is_empty()
            && !self.permission_profiles.has_supported_builtin_profile()
        {
            return Err(
                "permission_profiles leave no supported built-in sandbox profile available"
                    .to_string(),
            );
        }
        for surface_id in &self.provider_surfaces.allowlist {
            if canonical_provider_surface_id(surface_id).is_none() {
                return Err(format!(
                    "provider_surfaces.allowlist contains unknown surface `{surface_id}`"
                ));
            }
        }
        Ok(())
    }

    pub fn effective_report(&self, candidate: &AppConfig) -> ManagedEffectiveReport {
        let mut effective = candidate.clone();
        self.apply(&mut effective);

        let mut entries = Vec::new();
        self.push_feature_effective_entry(
            &mut entries,
            "automation",
            self.features.automation,
            candidate.automation.enabled,
            effective.automation.enabled,
        );
        self.push_feature_effective_entry(
            &mut entries,
            "telemetry",
            self.features.telemetry,
            candidate.telemetry.enabled,
            effective.telemetry.enabled,
        );
        self.push_feature_effective_entry(
            &mut entries,
            "sync",
            self.features.sync,
            candidate.sync.enabled,
            effective.sync.enabled,
        );
        self.push_feature_effective_entry(
            &mut entries,
            "external_ocr_llm",
            self.features.external_ocr_llm,
            candidate.ai_provider.ocr_provider == OcrProviderType::Remote
                || candidate.ai_provider.llm_provider == LlmProviderType::Remote,
            effective.ai_provider.ocr_provider == OcrProviderType::Remote
                || effective.ai_provider.llm_provider == LlmProviderType::Remote,
        );

        ManagedEffectiveReport { entries }
    }

    fn push_feature_effective_entry(
        &self,
        entries: &mut Vec<ManagedEffectiveSetting>,
        feature: &'static str,
        pin: Option<ManagedFeaturePin>,
        user_value: bool,
        effective_value: bool,
    ) {
        let Some(pin) = pin else {
            return;
        };

        entries.push(ManagedEffectiveSetting {
            path: format!("features.{feature}"),
            user_value: serde_json::json!(user_value),
            effective_value: serde_json::json!(effective_value),
            managed_reason: Some(feature_pin_reason(feature, pin).to_string()),
        });
    }
}

/// Load the managed policy from `path`.
///
/// - Path ABSENT ⇒ `Ok(None)` (no policy = normal operation). The `exists()`
///   check runs *before* any canonicalize/read so an absent file never errors
///   on dev/CI machines.
/// - Path present but unreadable / malformed JSON / bad enum token / a
///   `schema_version` newer than [`ManagedConfig::SUPPORTED_SCHEMA_VERSION`]
///   ⇒ `Err` (**fail-closed**): the caller must refuse to start unmanaged.
pub fn load_managed_config(path: &Path) -> Result<Option<ManagedConfig>, CoreError> {
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(path).map_err(|e| CoreError::Config {
        code: crate::error_codes::ConfigCode::Invalid,
        message: format!(
            "Managed config present but unreadable (fail-closed): {}: {}",
            path.display(),
            e
        ),
    })?;

    let managed: ManagedConfig = serde_json::from_str(&content).map_err(|e| CoreError::Config {
        code: crate::error_codes::ConfigCode::Invalid,
        message: format!(
            "Managed config present but malformed (fail-closed): {}: {}",
            path.display(),
            e
        ),
    })?;

    if managed.schema_version > ManagedConfig::SUPPORTED_SCHEMA_VERSION {
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::Invalid,
            message: format!(
                "Managed config schema_version {} exceeds supported {} (fail-closed; upgrade the client): {}",
                managed.schema_version,
                ManagedConfig::SUPPORTED_SCHEMA_VERSION,
                path.display()
            ),
        });
    }

    managed
        .validate_bundle()
        .map_err(|message| CoreError::Config {
            code: crate::error_codes::ConfigCode::Invalid,
            message: format!("Managed config failed validation (fail-closed): {message}"),
        })?;

    Ok(Some(managed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        AiProviderType, ExternalApiEndpoint, LlmProviderType, OcrProviderType, SandboxProfile,
    };

    fn locked_pii_off() -> ManagedConfig {
        ManagedConfig {
            privacy: ManagedPrivacy {
                pii_filter_level: Some(PiiFilterLevel::Off),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn apply_clamps_locked_field_and_reports_dotted_path() {
        let managed = locked_pii_off();
        let mut cfg = AppConfig::default_config();
        cfg.privacy.pii_filter_level = PiiFilterLevel::Strict;

        let clamped = managed.apply(&mut cfg);

        assert_eq!(cfg.privacy.pii_filter_level, PiiFilterLevel::Off);
        assert_eq!(clamped, vec!["privacy.pii_filter_level"]);
    }

    #[test]
    fn apply_is_noop_when_already_compliant() {
        let managed = locked_pii_off();
        let mut cfg = AppConfig::default_config();
        cfg.privacy.pii_filter_level = PiiFilterLevel::Off;

        let clamped = managed.apply(&mut cfg);

        assert!(clamped.is_empty());
        assert_eq!(cfg.privacy.pii_filter_level, PiiFilterLevel::Off);
    }

    #[test]
    fn policy_bundle_disables_features_without_granting_consent() {
        let managed = ManagedConfig {
            features: ManagedFeaturePins {
                automation: Some(ManagedFeaturePin::Disabled),
                telemetry: Some(ManagedFeaturePin::AllowedWithUserConsent),
                sync: Some(ManagedFeaturePin::Disabled),
                external_ocr_llm: Some(ManagedFeaturePin::Disabled),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut cfg = AppConfig::default_config();
        cfg.automation.enabled = true;
        cfg.telemetry.enabled = true;
        cfg.sync.enabled = true;
        cfg.ai_provider.ocr_provider = OcrProviderType::Remote;
        cfg.ai_provider.llm_provider = LlmProviderType::Remote;

        let mut clamped = managed.apply(&mut cfg);
        clamped.sort_unstable();

        assert!(!cfg.automation.enabled);
        assert!(
            cfg.telemetry.enabled,
            "managed allow must not grant consent or disable user intent"
        );
        assert!(!cfg.sync.enabled);
        assert_eq!(cfg.ai_provider.ocr_provider, OcrProviderType::Local);
        assert_eq!(cfg.ai_provider.llm_provider, LlmProviderType::Local);
        assert_eq!(
            clamped,
            vec![
                "features.automation",
                "features.external_ocr_llm",
                "features.sync",
            ]
        );
    }

    #[test]
    fn minimum_pii_floor_never_lowers_a_stricter_user_choice() {
        let managed = ManagedConfig {
            privacy: ManagedPrivacy {
                min_pii_filter_level: Some(PiiFilterLevel::Standard),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut basic = AppConfig::default_config();
        basic.privacy.pii_filter_level = PiiFilterLevel::Basic;

        let clamped = managed.apply(&mut basic);

        assert_eq!(basic.privacy.pii_filter_level, PiiFilterLevel::Standard);
        assert_eq!(clamped, vec!["privacy.min_pii_filter_level"]);

        let mut strict = AppConfig::default_config();
        strict.privacy.pii_filter_level = PiiFilterLevel::Strict;

        let clamped = managed.apply(&mut strict);

        assert_eq!(strict.privacy.pii_filter_level, PiiFilterLevel::Strict);
        assert!(clamped.is_empty());
    }

    #[test]
    fn provider_surface_allowlist_is_default_deny_for_remote_surfaces() {
        let managed = ManagedConfig {
            provider_surfaces: ManagedProviderSurfacePolicy {
                allowlist: vec!["provider_surface.openai.managed_oauth".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };
        let mut cfg = AppConfig::default_config();
        cfg.ai_provider.llm_provider = LlmProviderType::Remote;
        cfg.ai_provider.llm_api = Some(ExternalApiEndpoint {
            endpoint: "https://api.anthropic.com".to_string(),
            api_key: "not-used-in-test".to_string(),
            model: None,
            timeout_secs: 30,
            provider_type: AiProviderType::Anthropic,
            surface_id: Some("provider_surface.anthropic.direct_api".to_string()),
            credential: None,
        });

        let clamped = managed.apply(&mut cfg);

        assert_eq!(cfg.ai_provider.llm_provider, LlmProviderType::Local);
        assert!(cfg.ai_provider.llm_api.is_none());
        assert_eq!(clamped, vec!["provider_surfaces.allowlist"]);
        assert!(!managed
            .provider_surfaces
            .allows_surface(Some("provider_surface.anthropic.direct_api")));
        assert!(!managed.provider_surfaces.allows_surface(None));
        assert!(managed
            .provider_surfaces
            .allows_surface(Some("PROVIDER_SURFACE.OPENAI.MANAGED_OAUTH")));
    }

    #[test]
    fn sandbox_network_policy_can_only_narrow_runtime_authority() {
        let managed = ManagedConfig {
            sandbox_network: ManagedSandboxNetworkPolicy {
                require_sandbox_enabled: true,
                minimum_profile: Some(SandboxProfile::Strict),
                deny_network: true,
                deny_local_targets: true,
                deny_local_binding: true,
            },
            ..Default::default()
        };
        let mut cfg = AppConfig::default_config();
        cfg.automation.sandbox.enabled = false;
        cfg.automation.sandbox.profile = SandboxProfile::Permissive;
        cfg.automation.sandbox.allow_network = true;

        let mut clamped = managed.apply(&mut cfg);
        clamped.sort_unstable();

        assert!(cfg.automation.sandbox.enabled);
        assert_eq!(cfg.automation.sandbox.profile, SandboxProfile::Strict);
        assert!(!cfg.automation.sandbox.allow_network);
        assert_eq!(
            clamped,
            vec![
                "sandbox_network.deny_local_binding",
                "sandbox_network.deny_local_targets",
                "sandbox_network.deny_network",
                "sandbox_network.minimum_profile",
                "sandbox_network.require_sandbox_enabled",
            ]
        );
    }

    #[test]
    fn unsupported_feature_pins_fail_closed_instead_of_advertising_unenforced_locks() {
        let managed = ManagedConfig {
            features: ManagedFeaturePins {
                browser_verification: Some(ManagedFeaturePin::Disabled),
                record_to_template: Some(ManagedFeaturePin::Disabled),
                remote_observe_approve: Some(ManagedFeaturePin::Disabled),
                ..Default::default()
            },
            ..Default::default()
        };

        let err = managed
            .validate_bundle()
            .expect_err("unsupported feature pins must fail closed");

        assert!(err.contains("browser_verification"));
        assert!(err.contains("not enforceable"));
    }

    #[test]
    fn permission_profile_policy_requires_at_least_one_supported_builtin_profile() {
        let managed = ManagedConfig {
            permission_profiles: ManagedPermissionProfilePolicy {
                allowed_profiles: vec!["future_custom_profile".to_string()],
                denied_profiles: Vec::new(),
            },
            ..Default::default()
        };

        let err = managed
            .validate_bundle()
            .expect_err("unsupported-only profile policy must fail closed");

        assert!(err.contains("permission_profiles"));
        assert!(err.contains("no supported built-in"));
    }

    #[test]
    fn required_signature_without_signature_fails_closed() {
        let dir = std::env::temp_dir().join("maekon-managed-unsigned-required-7438");
        std::fs::create_dir_all(&dir).expect("test dir");
        let path = dir.join("managed.json");
        std::fs::write(
            &path,
            r#"{
                "schema_version": 2,
                "signature": { "require_signature": true }
            }"#,
        )
        .expect("write managed.json");

        let err =
            load_managed_config(&path).expect_err("unsigned required bundle must fail closed");

        assert!(err.to_string().contains("signature"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn effective_report_includes_user_value_effective_value_and_reason() {
        let managed = ManagedConfig {
            features: ManagedFeaturePins {
                automation: Some(ManagedFeaturePin::Disabled),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut cfg = AppConfig::default_config();
        cfg.automation.enabled = true;

        let report = managed.effective_report(&cfg);
        let automation = report
            .entries
            .iter()
            .find(|entry| entry.path == "features.automation")
            .expect("feature entry");

        assert_eq!(automation.user_value, serde_json::json!(true));
        assert_eq!(automation.effective_value, serde_json::json!(false));
        assert_eq!(
            automation.managed_reason.as_deref(),
            Some("managed.feature.automation.disabled")
        );
    }

    #[test]
    fn schema_v1_policy_migrates_with_new_bundle_defaults() {
        let json = r#"{
            "schema_version": 1,
            "telemetry": { "enabled": false }
        }"#;

        let managed: ManagedConfig = serde_json::from_str(json).expect("v1 policy must parse");

        assert_eq!(managed.schema_version, 1);
        assert_eq!(managed.telemetry.enabled, Some(false));
        assert!(managed.features.locked_fields().is_empty());
        assert!(managed.provider_surfaces.allowlist.is_empty());
    }

    #[test]
    fn apply_leaves_unlocked_fields_untouched() {
        // Only telemetry.enabled is locked; pii must pass through.
        let managed = ManagedConfig {
            telemetry: ManagedTelemetry {
                enabled: Some(false),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut cfg = AppConfig::default_config();
        cfg.privacy.pii_filter_level = PiiFilterLevel::Strict;
        cfg.telemetry.enabled = true;

        let clamped = managed.apply(&mut cfg);

        assert_eq!(clamped, vec!["telemetry.enabled"]);
        assert!(!cfg.telemetry.enabled);
        // Unlocked field preserved (no over-clamp).
        assert_eq!(cfg.privacy.pii_filter_level, PiiFilterLevel::Strict);
    }

    #[test]
    fn violations_matches_apply_without_mutating_candidate() {
        let managed = locked_pii_off();
        let mut cfg = AppConfig::default_config();
        cfg.privacy.pii_filter_level = PiiFilterLevel::Strict;

        let v = managed.violations(&cfg);

        assert_eq!(v, vec!["privacy.pii_filter_level"]);
        // Candidate is untouched by a read-only check.
        assert_eq!(cfg.privacy.pii_filter_level, PiiFilterLevel::Strict);
    }

    #[test]
    fn locked_fields_enumerates_only_present_locks() {
        let managed = ManagedConfig {
            privacy: ManagedPrivacy {
                pii_filter_level: Some(PiiFilterLevel::Off),
                ..Default::default()
            },
            update: ManagedUpdate {
                enabled: Some(false),
                ..Default::default()
            },
            ..Default::default()
        };

        let mut fields = managed.locked_fields();
        fields.sort_unstable();
        assert_eq!(fields, vec!["privacy.pii_filter_level", "update.enabled"]);
    }

    #[test]
    fn load_absent_path_is_normal_operation() {
        let dir = std::env::temp_dir().join("maekon-managed-absent-4832");
        let path = dir.join("does-not-exist-managed.json");
        // The parent may or may not exist; load must still return Ok(None).
        let result = load_managed_config(&path).expect("absent path must be Ok(None)");
        assert!(result.is_none());
    }

    #[test]
    fn deserialize_tolerates_unknown_keys() {
        let json = r#"{ "telemetry": { "enabled": false }, "future_key": { "x": 1 } }"#;
        let managed: ManagedConfig =
            serde_json::from_str(json).expect("unknown keys must be tolerated");
        assert_eq!(managed.telemetry.enabled, Some(false));
    }

    #[test]
    fn serde_tokens_match_appconfig_wire_format() {
        // Admin authors managed.json by hand; the tokens MUST match the
        // AppConfig wire format the rest of the client uses.
        let json = r#"{
            "privacy": { "pii_filter_level": "Off" },
            "audio": { "cloud_stt_policy": "disabled" }
        }"#;
        let managed: ManagedConfig = serde_json::from_str(json).expect("must parse admin tokens");
        assert_eq!(managed.privacy.pii_filter_level, Some(PiiFilterLevel::Off));
        assert_eq!(
            managed.audio.cloud_stt_policy,
            Some(CloudSttPolicy::Disabled)
        );
    }

    #[test]
    fn apply_clamps_locked_update_version_window() {
        // #4836: an MDM-locked update version window overwrites the user's
        // values at the write chokepoint and reports the dotted paths.
        let managed = ManagedConfig {
            update: ManagedUpdate {
                min_allowed_version: Some("1.2.0".to_string()),
                max_allowed_version: Some("2.0.0".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut cfg = AppConfig::default_config();
        cfg.update.min_allowed_version = None;
        cfg.update.max_allowed_version = Some("9.9.9".to_string());

        let mut clamped = managed.apply(&mut cfg);
        clamped.sort_unstable();

        assert_eq!(cfg.update.min_allowed_version.as_deref(), Some("1.2.0"));
        assert_eq!(cfg.update.max_allowed_version.as_deref(), Some("2.0.0"));
        assert_eq!(
            clamped,
            vec!["update.max_allowed_version", "update.min_allowed_version"]
        );
    }

    #[test]
    fn apply_update_version_window_is_noop_when_already_compliant() {
        let managed = ManagedConfig {
            update: ManagedUpdate {
                max_allowed_version: Some("2.0.0".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut cfg = AppConfig::default_config();
        cfg.update.max_allowed_version = Some("2.0.0".to_string());

        let clamped = managed.apply(&mut cfg);
        assert!(clamped.is_empty());
    }

    #[test]
    fn locked_fields_includes_update_version_window() {
        let managed = ManagedConfig {
            update: ManagedUpdate {
                min_allowed_version: Some("1.0.0".to_string()),
                max_allowed_version: Some("2.0.0".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut fields = managed.locked_fields();
        fields.sort_unstable();
        assert_eq!(
            fields,
            vec!["update.max_allowed_version", "update.min_allowed_version"]
        );
    }

    /// E20-45 #4837: the published JSON Schema (docs/contracts/managed.schema.json)
    /// MUST document every field the code can lock. If a new lockable field is
    /// added to `ManagedConfig` without updating the schema, this test fails —
    /// keeping the hand-written schema in lockstep with the allowlist.
    #[test]
    fn published_json_schema_covers_every_lockable_field() {
        let schema_src = include_str!("../../../../docs/contracts/managed.schema.json");
        let schema: serde_json::Value =
            serde_json::from_str(schema_src).expect("managed.schema.json must be valid JSON");
        let props = &schema["properties"];

        // Universe of lockable dotted paths = a ManagedConfig with every field set.
        let all_locked = ManagedConfig {
            schema_version: ManagedConfig::SUPPORTED_SCHEMA_VERSION,
            privacy: ManagedPrivacy {
                pii_filter_level: Some(PiiFilterLevel::Strict),
                min_pii_filter_level: Some(PiiFilterLevel::Standard),
            },
            telemetry: ManagedTelemetry {
                enabled: Some(true),
                crash_reports: Some(true),
            },
            vision: ManagedVision {
                capture_enabled: Some(true),
            },
            audio: ManagedAudio {
                cloud_stt_policy: Some(CloudSttPolicy::Disabled),
            },
            update: ManagedUpdate {
                enabled: Some(true),
                min_allowed_version: Some("1.0.0".to_string()),
                max_allowed_version: Some("2.0.0".to_string()),
            },
            features: ManagedFeaturePins {
                automation: Some(ManagedFeaturePin::Disabled),
                browser_verification: Some(ManagedFeaturePin::Disabled),
                record_to_template: Some(ManagedFeaturePin::Disabled),
                remote_observe_approve: Some(ManagedFeaturePin::Disabled),
                external_ocr_llm: Some(ManagedFeaturePin::Disabled),
                telemetry: Some(ManagedFeaturePin::AllowedWithUserConsent),
                sync: Some(ManagedFeaturePin::Disabled),
            },
            permission_profiles: ManagedPermissionProfilePolicy {
                allowed_profiles: vec!["standard".to_string()],
                denied_profiles: vec!["permissive".to_string()],
            },
            provider_surfaces: ManagedProviderSurfacePolicy {
                enforce: true,
                allowlist: vec!["provider_surface.openai.managed_oauth".to_string()],
            },
            sandbox_network: ManagedSandboxNetworkPolicy {
                require_sandbox_enabled: true,
                minimum_profile: Some(SandboxProfile::Strict),
                deny_network: true,
                deny_local_targets: true,
                deny_local_binding: true,
            },
            audit: ManagedAuditPolicy {
                min_retention_days: Some(90),
                export_required: true,
            },
            signature: ManagedSignaturePolicy::default(),
        };

        for path in all_locked.locked_fields() {
            let (parent, child) = path.split_once('.').expect("locked paths are dotted");
            assert!(
                props[parent]["properties"][child].is_object(),
                "docs/contracts/managed.schema.json is missing a definition for locked \
                 field `{path}` — add it to the schema so the published contract matches \
                 ManagedConfig"
            );
        }

        // The published schema must pin the version the client actually supports.
        assert_eq!(
            schema["properties"]["schema_version"]["const"],
            serde_json::json!(ManagedConfig::SUPPORTED_SCHEMA_VERSION),
            "managed.schema.json schema_version const must equal SUPPORTED_SCHEMA_VERSION"
        );
    }
}
