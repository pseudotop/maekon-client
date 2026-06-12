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

use crate::config::{AppConfig, CloudSttPolicy, PiiFilterLevel};
use crate::error::CoreError;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

fn default_managed_schema_version() -> u32 {
    ManagedConfig::SUPPORTED_SCHEMA_VERSION
}

/// Locked privacy policy fields.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedPrivacy {
    /// Lock `privacy.pii_filter_level` to this value when present.
    #[serde(default)]
    pub pii_filter_level: Option<PiiFilterLevel>,
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
}

impl ManagedConfig {
    /// Highest `managed.json` schema version this client understands.
    pub const SUPPORTED_SCHEMA_VERSION: u32 = 1;

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

        clamped
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
        fields
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

    Ok(Some(managed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locked_pii_off() -> ManagedConfig {
        ManagedConfig {
            privacy: ManagedPrivacy {
                pii_filter_level: Some(PiiFilterLevel::Off),
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
