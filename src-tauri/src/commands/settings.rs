use tauri::command;

use crate::ipc_error::IpcError;
use crate::runtime_state::{AppState, ConfigRuntimeState};

use super::deep_merge;

/// Helper: wrap a string-valued validation error into an IpcError with the
/// canonical validation.invalid_arguments wire code. The helpers below
/// (`validate_config_bounds`, `reject_forbidden_allowed_subpaths`) return
/// `Result<(), String>` because they have existing tests that pattern-match
/// on the message substring; we preserve that interface and wrap only at
/// the command boundary.
fn validation_error(msg: impl Into<String>) -> IpcError {
    IpcError::new("validation.invalid_arguments", msg)
}

/// List of keys whose sensitive fields are masked before being exposed to the WebView.
#[cfg(test)]
const REDACTED_PATHS: &[(&str, &[&str])] = &[
    ("server", &["base_url", "api_key"]),
    ("ai_provider", &["ocr_api.api_key", "llm_api.api_key"]),
    ("web", &["integration_auth_token"]),
    ("tls", &["enabled", "allow_self_signed"]),
    (
        "grpc",
        &[
            "grpc_endpoint",
            "tls_domain_name",
            "tls_ca_cert_path",
            "tls_client_cert_path",
            "tls_client_key_path",
        ],
    ),
];

const FORBIDDEN_ALLOWED_SUBPATHS: &[(&str, &[&str])] = &[
    ("web", &["integration_auth_token"]),
    // #6256: the auto-updater trust chain must not be downgradable from the
    // WebView. `update` is an allowed top-level key (users toggle channel /
    // auto-install), but `require_signature_verification` and the
    // `signature_public_key` override are part of the Ed25519 verification
    // boundary — a WebView foothold setting `require_signature_verification=false`
    // would re-enable a forged-release install. Block these sub-paths here
    // (1st-line) in addition to the ConfigManager chokepoint integrity check
    // (defense-in-depth); dev self-signing still edits config.json directly.
    (
        "update",
        &["require_signature_verification", "signature_public_key"],
    ),
];

/// Whitelist of setting keys that can be modified from the WebView.
/// Shared by update_setting + get_allowed_setting_keys.
pub(crate) const ALLOWED_KEYS: &[&str] = &[
    "monitoring",
    "capture",
    "notification",
    "web",
    "schedule",
    "telemetry",
    "privacy",
    "update",
    "language",
    "theme",
    "analysis",
    "audio",
    "coaching",
    "tracking_schedule",
];

/// D-2 (#7740 write-guard slice): explicit, auditable exceptions to the
/// `ALLOWED_KEYS` <= AppConfig-paths guard (`allowed_keys_are_appconfig_paths_or_explicit_non_config_exceptions`
/// below) — WebView-writable top-level keys that are deliberately NOT
/// `AppConfig` fields. Adding a key here is a conscious, visible opt-out, not a
/// silent gap: the guard test fails for any `ALLOWED_KEYS` entry that is
/// neither a real `AppConfig` top-level path (after the same "capture"/
/// "monitoring" alias normalization `update_setting` applies) nor listed here.
#[cfg(test)]
const NON_CONFIG_ALLOWED_KEYS: &[&str] = &[
    // UI-local: language/theme selection is persisted by the WebView itself
    // (frontend local storage), not by AppConfig. `update_setting` still
    // accepts these top-level keys for a uniform settings-form save flow, but
    // there is no `AppConfig::language` / `AppConfig::theme` field to route
    // them to.
    "language", "theme",
];

#[cfg(test)]
fn redact_sensitive_fields(config: &mut serde_json::Value) {
    let redacted = serde_json::Value::String("[REDACTED]".to_string());
    for &(section, fields) in REDACTED_PATHS {
        if let Some(sec) = config.get_mut(section) {
            for &field in fields {
                // Handle nested paths such as "ocr_api.api_key".
                let parts: Vec<&str> = field.split('.').collect();
                let mut target = &mut *sec;
                let mut found = true;
                for (i, part) in parts.iter().enumerate() {
                    if i == parts.len() - 1 {
                        if let Some(obj) = target.as_object_mut() {
                            if obj.contains_key(*part) {
                                obj.insert((*part).to_string(), redacted.clone());
                            }
                        }
                    } else if let Some(next) = target.get_mut(*part) {
                        target = next;
                    } else {
                        found = false;
                        break;
                    }
                }
                let _ = found; // suppress unused warning
            }
        }
    }
}

/// Setting fields modifiable from the WebView — whitelist model.
///
/// Allowed: monitoring, capture, notification, web, schedule, telemetry, privacy, update, language, theme
/// All other keys are rejected (sandbox, ai_provider, file_access, server, etc.)
///
/// #5707: if the patch contains a `coaching` key, call `CoachingPort::apply_config`
/// after saving the config so the in-session setting change takes effect immediately.
/// When `app_state` is None (non-Tauri environment), only the config is saved and
/// apply_config is skipped.
#[command]
pub async fn update_setting(
    config_json: String,
    state: tauri::State<'_, ConfigRuntimeState>,
    app_state: tauri::State<'_, AppState>,
) -> Result<(), IpcError> {
    let patch: serde_json::Value =
        serde_json::from_str(&config_json).map_err(|e| validation_error(e.to_string()))?;

    let patch_obj = patch
        .as_object()
        .ok_or_else(|| validation_error("expected JSON object"))?;

    // Allowlist check — see module-level ALLOWED_KEYS
    reject_forbidden_top_level_keys(patch_obj)?;

    reject_forbidden_allowed_subpaths(&patch).map_err(validation_error)?;
    // #5707: record whether the `coaching` key is present before normalization
    // (the patch variable is shadowed after normalization).
    let has_coaching_patch = patch_obj.contains_key("coaching");
    let patch = normalize_webview_config_patch(patch);

    // Deep-merge allowed keys into current config.
    // This preserves existing sub-keys that the patch does not mention,
    // preventing silent resets to struct defaults (e.g. privacy.pii_filter_level).
    let current = state.config_manager().get();
    let mut current_val = serde_json::to_value(&current).map_err(IpcError::from)?;

    if let (Some(base), Some(patch)) = (current_val.as_object_mut(), patch.as_object()) {
        for (k, v) in patch {
            deep_merge(
                base.entry(k.clone()).or_insert(serde_json::Value::Null),
                v.clone(),
            );
        }
    }

    let new_config: maekon_core::config::AppConfig =
        serde_json::from_value(current_val).map_err(IpcError::from)?;

    validate_config_bounds(&new_config).map_err(validation_error)?;

    // Managed (MDM) policy: this WebView IPC is a second interactive write
    // surface alongside the settings HTTP API. Reject an attempt to change an
    // admin-locked field with a clear message rather than letting the write
    // chokepoint silently clamp it (#4832).
    let violations = state
        .config_manager()
        .detect_managed_violations(&new_config);
    if !violations.is_empty() {
        return Err(validation_error(format!(
            "the following settings are locked by your administrator and cannot be changed: {}",
            violations.join(", ")
        )));
    }

    state
        .config_manager()
        .update(new_config.clone())
        .map_err(IpcError::from)?;

    // #5707: if the patch contains the coaching key, apply it to the engine
    // immediately. `update_setting` is also the path that writes
    // `coaching.enabled=true` when onboarding completes.
    if has_coaching_patch {
        if let Some(ref engine) = app_state.coaching_engine {
            engine.apply_config(new_config.coaching.clone()).await;
        }
    }

    Ok(())
}

/// Validate numeric config bounds to prevent tight loops or resource exhaustion
/// from WebView-supplied values.
///
/// #7726 (ctd-W2 E4): delegates entirely to the core `AppConfig::validate_bounds()`
/// SSOT (`crates/maekon-core/src/config/app_config.rs`) rather than re-declaring
/// floor/cap literals here. This file previously hardcoded its own duplicate
/// constants (`poll_interval_ms`/`sync_interval_ms`/`heartbeat_interval_ms`/
/// `capture_throttle_ms`/`max_suggestions`/`throttle_secs`/
/// `idle_notification_mins`) that had drifted out of sync with core (most
/// notably `capture_throttle_ms`: this file required >= 1000 while core only
/// required >= 100 — see the core-side doc comment on
/// `VISION_CAPTURE_THROTTLE_MS_FLOOR` for the resolution). `ConfigManager::update`
/// already runs `AppConfig::validate_bounds()` internally on every write
/// chokepoint; this early call exists only to surface the
/// `validation.invalid_arguments` wire code (via `validation_error` below)
/// before the managed-violations check runs, instead of whatever code
/// `ConfigManager::update`'s internal `map_bounds_error` would produce.
fn validate_config_bounds(config: &maekon_core::config::AppConfig) -> Result<(), String> {
    config.validate_bounds()
}

/// Checks the patch object's top-level keys against the ALLOWED_KEYS whitelist.
///
/// First-line gate that prevents the WebView from directly overwriting sensitive
/// sections such as sandbox, server, and ai_provider. Called at the
/// `update_setting` command boundary. Structurally isomorphic to
/// `reject_forbidden_allowed_subpaths` — a pure function with no dependency on
/// Tauri State.
fn reject_forbidden_top_level_keys(
    patch_obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), IpcError> {
    for key in patch_obj.keys() {
        if !ALLOWED_KEYS.contains(&key.as_str()) {
            return Err(validation_error(format!(
                "modifying '{}' from the WebView is not permitted; allowed: {}",
                key,
                ALLOWED_KEYS.join(", "),
            )));
        }
    }
    Ok(())
}

fn reject_forbidden_allowed_subpaths(patch: &serde_json::Value) -> Result<(), String> {
    for &(section, fields) in FORBIDDEN_ALLOWED_SUBPATHS {
        let Some(section_value) = patch.get(section) else {
            continue;
        };

        for &field in fields {
            let mut target = section_value;
            let mut found = true;
            for part in field.split('.') {
                if let Some(next) = target.get(part) {
                    target = next;
                } else {
                    found = false;
                    break;
                }
            }

            if found {
                return Err(format!(
                    "modifying '{}.{}' from the WebView is not permitted",
                    section, field
                ));
            }
        }
    }

    Ok(())
}

fn normalize_webview_config_patch(patch: serde_json::Value) -> serde_json::Value {
    let Some(patch_obj) = patch.as_object() else {
        return patch;
    };

    let mut normalized = serde_json::Map::new();
    for (key, value) in patch_obj {
        match key.as_str() {
            "capture" => merge_patch_value(
                &mut normalized,
                "vision",
                normalize_capture_patch(value.clone()),
            ),
            "monitoring" => merge_patch_value(
                &mut normalized,
                "monitor",
                normalize_monitoring_patch(value.clone()),
            ),
            _ => merge_patch_value(&mut normalized, key, value.clone()),
        }
    }

    serde_json::Value::Object(normalized)
}

fn normalize_capture_patch(value: serde_json::Value) -> serde_json::Value {
    let Some(source) = value.as_object() else {
        return value;
    };

    let mut target = serde_json::Map::new();
    for (key, value) in source {
        let target_key = match key.as_str() {
            "enabled" => "capture_enabled",
            "throttle_ms" => "capture_throttle_ms",
            _ => key.as_str(),
        };
        target.insert(target_key.to_string(), value.clone());
    }

    serde_json::Value::Object(target)
}

fn normalize_monitoring_patch(value: serde_json::Value) -> serde_json::Value {
    let Some(source) = value.as_object() else {
        return value;
    };

    let mut target = serde_json::Map::new();
    for (key, value) in source {
        let target_key = match key.as_str() {
            "interval_seconds" => "poll_interval_ms",
            _ => key.as_str(),
        };
        let target_value = if key == "interval_seconds" {
            value
                .as_u64()
                .map(|seconds| serde_json::Value::from(seconds.saturating_mul(1000)))
                .unwrap_or_else(|| value.clone())
        } else {
            value.clone()
        };
        target.insert(target_key.to_string(), target_value);
    }

    serde_json::Value::Object(target)
}

fn merge_patch_value(
    target: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: serde_json::Value,
) {
    if let Some(existing) = target.get_mut(key) {
        deep_merge(existing, value);
    } else {
        target.insert(key.to_string(), value);
    }
}

/// Returns the list of allowed setting keys — for frontend allowlist validation
/// and drift detection.
#[command]
pub async fn get_allowed_setting_keys() -> Vec<String> {
    ALLOWED_KEYS.iter().map(|s| s.to_string()).collect()
}

/// Query the web server port — for determining the frontend API base URL.
#[command]
pub async fn get_web_port(state: tauri::State<'_, ConfigRuntimeState>) -> Result<u16, IpcError> {
    Ok(state.web_port())
}

/// E20-41 (#4833): query the per-session local-auth token the frontend attaches
/// to `/api` calls. A race-safe fallback for when `window.__MAEKON_LOCAL_AUTH__`
/// has not been injected yet. Exposed only over IPC (Tauri internal channel) —
/// never returned over HTTP.
fn local_auth_token_from_state(state: &crate::runtime_state::LocalAuthTokenState) -> String {
    state.0.to_string()
}

#[command]
pub async fn get_local_auth_token(
    state: tauri::State<'_, crate::runtime_state::LocalAuthTokenState>,
) -> Result<String, IpcError> {
    Ok(local_auth_token_from_state(state.inner()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── deep_merge ────────────────────────────────────────────

    #[test]
    fn deep_merge_replaces_flat_value() {
        let mut base = json!({"a": 1});
        deep_merge(&mut base, json!({"a": 2}));
        assert_eq!(base, json!({"a": 2}));
    }

    #[test]
    fn deep_merge_adds_new_key() {
        let mut base = json!({"a": 1});
        deep_merge(&mut base, json!({"b": 2}));
        assert_eq!(base, json!({"a": 1, "b": 2}));
    }

    #[test]
    fn deep_merge_recurses_into_objects() {
        let mut base = json!({"a": {"x": 1, "y": 2}});
        deep_merge(&mut base, json!({"a": {"y": 99, "z": 3}}));
        assert_eq!(base, json!({"a": {"x": 1, "y": 99, "z": 3}}));
    }

    #[test]
    fn deep_merge_replaces_non_object_with_object() {
        let mut base = json!({"a": "string"});
        deep_merge(&mut base, json!({"a": {"nested": true}}));
        assert_eq!(base, json!({"a": {"nested": true}}));
    }

    #[test]
    fn deep_merge_replaces_object_with_non_object() {
        let mut base = json!({"a": {"nested": true}});
        deep_merge(&mut base, json!({"a": "flat"}));
        assert_eq!(base, json!({"a": "flat"}));
    }

    #[test]
    fn normalize_webview_config_patch_maps_capture_alias() {
        let patch = normalize_webview_config_patch(json!({
            "capture": { "enabled": false, "throttle_ms": 2500 }
        }));

        assert_eq!(
            patch,
            json!({ "vision": { "capture_enabled": false, "capture_throttle_ms": 2500 } })
        );
    }

    #[test]
    fn normalize_webview_config_patch_maps_monitoring_alias() {
        let patch = normalize_webview_config_patch(json!({
            "monitoring": { "interval_seconds": 3, "process_monitoring": false }
        }));

        assert_eq!(
            patch,
            json!({ "monitor": { "poll_interval_ms": 3000, "process_monitoring": false } })
        );
    }

    #[test]
    fn local_auth_token_command_returns_managed_token() {
        let state = crate::runtime_state::LocalAuthTokenState(std::sync::Arc::from("ipc-token"));
        assert_eq!(local_auth_token_from_state(&state), "ipc-token");
    }

    // ── redact_sensitive_fields ───────────────────────────────

    #[test]
    fn redact_masks_server_keys() {
        let mut config = json!({
            "server": {"base_url": "http://real.com", "api_key": "secret123", "timeout": 30}
        });
        redact_sensitive_fields(&mut config);
        assert_eq!(config["server"]["base_url"], "[REDACTED]");
        assert_eq!(config["server"]["api_key"], "[REDACTED]");
        assert_eq!(config["server"]["timeout"], 30);
    }

    #[test]
    fn redact_masks_nested_ai_provider_keys() {
        let mut config = json!({
            "ai_provider": {
                "ocr_api": {"api_key": "ocr-secret", "model": "gpt4"},
                "llm_api": {"api_key": "llm-secret", "model": "claude"}
            }
        });
        redact_sensitive_fields(&mut config);
        assert_eq!(config["ai_provider"]["ocr_api"]["api_key"], "[REDACTED]");
        assert_eq!(config["ai_provider"]["ocr_api"]["model"], "gpt4");
        assert_eq!(config["ai_provider"]["llm_api"]["api_key"], "[REDACTED]");
    }

    #[test]
    fn redact_masks_tls_paths() {
        let mut config = json!({
            "grpc": {
                "grpc_endpoint": "https://grpc.example.com:50051",
                "tls_domain_name": "grpc.example.com",
                "tls_ca_cert_path": "/etc/ssl/ca.pem",
                "tls_client_cert_path": "/etc/ssl/client.pem",
                "tls_client_key_path": "/etc/ssl/client.key",
                "use_tls": true
            }
        });
        redact_sensitive_fields(&mut config);
        assert_eq!(config["grpc"]["grpc_endpoint"], "[REDACTED]");
        assert_eq!(config["grpc"]["tls_domain_name"], "[REDACTED]");
        assert_eq!(config["grpc"]["tls_ca_cert_path"], "[REDACTED]");
        assert_eq!(config["grpc"]["tls_client_cert_path"], "[REDACTED]");
        assert_eq!(config["grpc"]["tls_client_key_path"], "[REDACTED]");
        assert_eq!(config["grpc"]["use_tls"], true);
    }

    #[test]
    fn redact_masks_web_integration_auth_token() {
        let mut config = json!({
            "web": {
                "port": 10090,
                "allow_external": true,
                "integration_auth_token": "secret-token"
            }
        });
        redact_sensitive_fields(&mut config);
        assert_eq!(config["web"]["integration_auth_token"], "[REDACTED]");
        assert_eq!(config["web"]["port"], 10090);
    }

    #[test]
    fn redact_ignores_missing_sections() {
        let mut config = json!({"monitoring": {"interval": 10}});
        // Should not panic when sections like "server", "tls" are absent
        redact_sensitive_fields(&mut config);
        assert_eq!(config["monitoring"]["interval"], 10);
    }

    // ── ALLOWED_KEYS contract ─────────────────────────────────

    #[test]
    fn allowed_keys_matches_expected_set() {
        let expected: Vec<&str> = vec![
            "monitoring",
            "capture",
            "notification",
            "web",
            "schedule",
            "telemetry",
            "privacy",
            "update",
            "language",
            "theme",
            "analysis",
            "audio",
            "coaching",
            "tracking_schedule",
        ];
        assert_eq!(ALLOWED_KEYS, expected.as_slice());
    }

    #[test]
    fn allowed_keys_excludes_sensitive_sections() {
        let forbidden = [
            "server",
            "ai_provider",
            "tls",
            "grpc",
            "sandbox",
            "file_access",
        ];
        for key in &forbidden {
            assert!(
                !ALLOWED_KEYS.contains(key),
                "ALLOWED_KEYS must not contain sensitive key '{key}'"
            );
        }
    }

    /// D-2 (#7740 write-guard slice): the 2nd write path (`update_setting` IPC +
    /// `ALLOWED_KEYS`) must not silently diverge from the `AppConfig` write-guard
    /// covered by the D-1 destructure guard in `maekon-web`
    /// (`settings_config_mutation.rs`). This asserts `ALLOWED_KEYS` is a SUBSET
    /// of (the real `AppConfig` top-level JSON paths UNION the explicit,
    /// auditable `NON_CONFIG_ALLOWED_KEYS` exception set) — set-containment, not
    /// equality, because a couple of `ALLOWED_KEYS` entries (`language`,
    /// `theme`) are UI-local values persisted outside `AppConfig` and never map
    /// onto a config field.
    ///
    /// The "real AppConfig top-level JSON paths" side comes from serializing an
    /// actual `AppConfig::default_config()` — not a hand-maintained literal
    /// list — so this test cannot silently drift from the struct it guards. A
    /// new `ALLOWED_KEYS` entry must therefore be either:
    ///   1. a real `AppConfig` field name (optionally aliased via
    ///      `normalize_webview_config_patch`, e.g. `"capture"` -> `"vision"`), or
    ///   2. added to `NON_CONFIG_ALLOWED_KEYS` with a comment explaining where
    ///      it is actually persisted.
    #[test]
    fn allowed_keys_are_appconfig_paths_or_explicit_non_config_exceptions() {
        let config_value = serde_json::to_value(maekon_core::config::AppConfig::default_config())
            .expect("AppConfig must serialize to JSON");
        let config_top_level_keys: std::collections::BTreeSet<&str> = config_value
            .as_object()
            .expect("AppConfig serializes to a JSON object")
            .keys()
            .map(String::as_str)
            .collect();

        for &allowed_key in ALLOWED_KEYS {
            // Route the key through the SAME alias normalization `update_setting`
            // applies before merging into AppConfig (e.g. "capture" -> "vision",
            // "monitoring" -> "monitor"). A non-object placeholder value is fine
            // here — the key rename does not depend on the value shape.
            let normalized =
                normalize_webview_config_patch(json!({ allowed_key: serde_json::Value::Null }));
            let normalized_obj = normalized
                .as_object()
                .expect("normalize_webview_config_patch returns an object for an object input");
            let mapped_key = normalized_obj
                .keys()
                .next()
                .expect("a single-key input patch normalizes to a single-key output");

            let is_real_config_path = config_top_level_keys.contains(mapped_key.as_str());
            let is_declared_exception = NON_CONFIG_ALLOWED_KEYS.contains(&allowed_key);

            assert!(
                is_real_config_path || is_declared_exception,
                "ALLOWED_KEYS entry '{allowed_key}' (normalized: '{mapped_key}') is neither an \
                 AppConfig top-level field nor a declared NON_CONFIG_ALLOWED_KEYS exception — a \
                 WebView-writable key that silently does not persist anywhere. Either wire it to \
                 an AppConfig field or add it to NON_CONFIG_ALLOWED_KEYS with a comment."
            );
        }
    }

    // ── REDACTED_PATHS contract ───────────────────────────────

    #[test]
    fn redacted_paths_covers_all_sensitive_sections() {
        let sections: Vec<&str> = REDACTED_PATHS.iter().map(|(s, _)| *s).collect();
        assert!(sections.contains(&"server"));
        assert!(sections.contains(&"ai_provider"));
        assert!(sections.contains(&"web"));
        assert!(sections.contains(&"grpc"));
    }

    // ── validate_config_bounds ─────────────────────────────────────

    #[test]
    fn validate_config_bounds_rejects_low_poll_interval() {
        let mut config = maekon_core::config::AppConfig::default_config();
        config.monitor.poll_interval_ms = 500;
        let err = validate_config_bounds(&config).unwrap_err();
        assert!(err.contains("poll_interval_ms"), "err: {err}");
    }

    #[test]
    fn validate_config_bounds_rejects_high_max_suggestions() {
        let mut config = maekon_core::config::AppConfig::default_config();
        config.analysis.max_suggestions = 999;
        let err = validate_config_bounds(&config).unwrap_err();
        assert!(err.contains("max_suggestions"), "err: {err}");
    }

    #[test]
    fn validate_config_bounds_accepts_defaults() {
        // AppConfig::default_config() must produce values that satisfy every
        // bound in the core AppConfig::validate_bounds() SSOT this function now
        // delegates to (poll>=1000, sync>=1000, heartbeat>=5000,
        // capture_throttle>=1000, 1<=max_suggestions<=200, throttle_secs>=10,
        // idle_notification_mins>=1, plus storage/web/ai_provider bounds).
        let config = maekon_core::config::AppConfig::default_config();
        validate_config_bounds(&config)
            .expect("default AppConfig must pass all validate_config_bounds checks");
    }

    // ── #7726 (ctd-W2 E4): boundary-agreement regression tests ──────────
    //
    // Pre-fix, these values were accepted by the core `AppConfig::validate_bounds()`
    // SSOT (looser floor) but rejected by this file's now-deleted hardcoded
    // duplicate constants (stricter floor) — a live 3-boundary disagreement.
    // Now that this file delegates to core, both the core-side test (in
    // `crates/maekon-core/src/config/{app_config,sections/analysis}.rs`) and this
    // WebView-boundary test must agree on the same outcome for the same value.

    #[test]
    fn validate_config_bounds_rejects_capture_throttle_500() {
        // Pre-fix: core's VISION_CAPTURE_THROTTLE_MS_FLOOR was 100 (accepted 500),
        // this file's hardcoded floor was 1000 (rejected 500) — divergent.
        let mut config = maekon_core::config::AppConfig::default_config();
        config.vision.capture_throttle_ms = 500;
        let err = validate_config_bounds(&config).expect_err(
            "capture_throttle_ms=500 must be rejected — both boundaries now agree on floor=1000",
        );
        assert!(err.contains("capture_throttle_ms"), "err: {err}");
    }

    #[test]
    fn validate_config_bounds_rejects_analysis_throttle_secs_five() {
        // Pre-fix: core's ANALYSIS_THROTTLE_SECS_FLOOR was 1 (accepted 5), this
        // file's hardcoded floor was 10 (rejected 5) — divergent.
        let mut config = maekon_core::config::AppConfig::default_config();
        config.analysis.throttle_secs = 5;
        let err = validate_config_bounds(&config).expect_err(
            "analysis.throttle_secs=5 must be rejected — both boundaries now agree on floor=10",
        );
        assert!(err.contains("throttle_secs"), "err: {err}");
    }

    #[test]
    fn validate_config_bounds_accepts_capture_throttle_at_new_shared_floor() {
        // The harmonized floor (1000ms) itself must still be accepted (inclusive boundary).
        let mut config = maekon_core::config::AppConfig::default_config();
        config.vision.capture_throttle_ms = 1_000;
        validate_config_bounds(&config)
            .expect("capture_throttle_ms=1000 (the harmonized floor) must be accepted");
    }

    #[test]
    fn reject_forbidden_allowed_subpaths_rejects_web_integration_token() {
        let patch = json!({
            "web": {
                "integration_auth_token": "secret-token"
            }
        });
        let err = reject_forbidden_allowed_subpaths(&patch).expect_err("forbidden subpath");
        assert!(err.contains("web.integration_auth_token"));
    }

    // #6256: the WebView must not be able to downgrade the auto-updater Ed25519
    // signature-verification gate. `update` is an allowed top-level key, but
    // `update.require_signature_verification` is part of the trust chain and is
    // blocked at the allowed-subpath gate (the ConfigManager chokepoint provides
    // defense-in-depth for the HTTP API / backup-restore surfaces).
    #[test]
    fn reject_forbidden_allowed_subpaths_rejects_update_signature_verification() {
        let patch = json!({
            "update": {
                "require_signature_verification": false
            }
        });
        let err = reject_forbidden_allowed_subpaths(&patch)
            .expect_err("update signature-verification downgrade must be rejected");
        assert!(
            err.contains("update.require_signature_verification"),
            "got: {err}"
        );
    }

    #[test]
    fn reject_forbidden_allowed_subpaths_rejects_update_signature_public_key() {
        let patch = json!({
            "update": {
                "signature_public_key": "attacker-controlled-key"
            }
        });
        let err = reject_forbidden_allowed_subpaths(&patch)
            .expect_err("update signature_public_key override must be rejected");
        assert!(err.contains("update.signature_public_key"), "got: {err}");
    }

    // F1 (P1): Security gate — forbidden top-level keys must produce IpcError.
    //
    // `update_setting` checks every top-level key in the patch against ALLOWED_KEYS
    // via the production helper `reject_forbidden_top_level_keys` and returns early
    // with `validation_error(...)` if any key is not permitted. This is the primary
    // defence against WebView-originated writes to sensitive sections (server URL,
    // AI provider API keys, sandbox config, etc.).
    //
    // These tests call the production helper directly (Tauri State cannot be
    // constructed in unit tests without a running app), so deleting or weakening
    // the helper body turns them red immediately. Known residual: unwiring the
    // helper *call* from `update_setting` is not observable from unit tests —
    // that single call site is review-guarded.
    #[test]
    fn forbidden_top_level_key_gate_produces_validation_ipc_code_server() {
        // "server" — overwriting base_url is the most serious threat, since it
        // redirects all API traffic to an attacker-controlled server.
        assert!(
            !ALLOWED_KEYS.contains(&"server"),
            "pre-condition: 'server' must not be in ALLOWED_KEYS"
        );
        let patch = json!({ "server": { "base_url": "http://evil.example.com" } });
        let err = reject_forbidden_top_level_keys(patch.as_object().unwrap())
            .expect_err("'server' must be rejected");
        assert_eq!(
            err.code, "validation.invalid_arguments",
            "forbidden top-level key must produce validation.invalid_arguments wire code"
        );
        assert!(
            err.message.contains("server"),
            "rejection message must identify the forbidden key: {}",
            err.message
        );
        assert!(
            err.message.contains("not permitted"),
            "rejection message must include 'not permitted': {}",
            err.message
        );
    }

    #[test]
    fn forbidden_top_level_key_gate_produces_validation_ipc_code_ai_provider() {
        // "ai_provider" — an OCR/LLM API key exposure path; never writable from the WebView.
        assert!(
            !ALLOWED_KEYS.contains(&"ai_provider"),
            "pre-condition: 'ai_provider' must not be in ALLOWED_KEYS"
        );
        let patch = json!({ "ai_provider": { "ocr_api": { "api_key": "stolen-key" } } });
        let err = reject_forbidden_top_level_keys(patch.as_object().unwrap())
            .expect_err("'ai_provider' must be rejected");
        assert_eq!(err.code, "validation.invalid_arguments");
        assert!(err.message.contains("ai_provider"));
    }

    #[test]
    fn forbidden_top_level_key_gate_produces_validation_ipc_code_sandbox() {
        // "sandbox" — disabling OS isolation must never be allowed from the WebView.
        assert!(
            !ALLOWED_KEYS.contains(&"sandbox"),
            "pre-condition: 'sandbox' must not be in ALLOWED_KEYS"
        );
        let patch = json!({ "sandbox": { "enabled": false } });
        let err = reject_forbidden_top_level_keys(patch.as_object().unwrap())
            .expect_err("'sandbox' must be rejected");
        assert_eq!(err.code, "validation.invalid_arguments");
        assert!(err.message.contains("sandbox"));
    }

    // F1 (P1): Verify that FORBIDDEN_ALLOWED_SUBPATHS gate also returns an IpcError
    // with the canonical validation wire code when called through `validation_error`.
    #[test]
    fn forbidden_subpath_gate_returns_validation_ipc_code() {
        let patch = json!({
            "web": {
                "port": 10090,
                "integration_auth_token": "should-be-blocked"
            }
        });
        // reject_forbidden_allowed_subpaths returns Err(String) — validation_error
        // wraps it into IpcError with the canonical wire code.
        let string_err = reject_forbidden_allowed_subpaths(&patch)
            .expect_err("web.integration_auth_token must be rejected");
        let ipc_err = validation_error(string_err);
        assert_eq!(
            ipc_err.code, "validation.invalid_arguments",
            "forbidden subpath rejection must use validation.invalid_arguments wire code"
        );
        assert!(
            ipc_err.message.contains("web.integration_auth_token"),
            "IpcError message must identify the forbidden subpath: {}",
            ipc_err.message
        );
    }
}
