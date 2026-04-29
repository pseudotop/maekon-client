use tauri::command;

use crate::ipc_error::IpcError;
use crate::runtime_state::ConfigRuntimeState;

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

/// WebView에 노출되는 민감 필드를 마스킹하는 키 목록
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

const FORBIDDEN_ALLOWED_SUBPATHS: &[(&str, &[&str])] = &[("web", &["integration_auth_token"])];

/// WebView에서 수정 가능한 설정 키 화이트리스트.
/// update_setting + get_allowed_setting_keys에서 공유.
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

#[cfg(test)]
fn redact_sensitive_fields(config: &mut serde_json::Value) {
    let redacted = serde_json::Value::String("[REDACTED]".to_string());
    for &(section, fields) in REDACTED_PATHS {
        if let Some(sec) = config.get_mut(section) {
            for &field in fields {
                // "ocr_api.api_key" 같은 중첩 경로 처리
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

/// WebView에서 수정 가능한 설정 필드 — 화이트리스트 모델
///
/// 허용: monitoring, capture, notification, web, schedule, telemetry, privacy, update, language, theme
/// 그 외 모든 키 거부 (sandbox, ai_provider, file_access, server 등)
#[command]
pub async fn update_setting(
    config_json: String,
    state: tauri::State<'_, ConfigRuntimeState>,
) -> Result<(), IpcError> {
    let patch: serde_json::Value =
        serde_json::from_str(&config_json).map_err(|e| validation_error(e.to_string()))?;

    let patch_obj = patch
        .as_object()
        .ok_or_else(|| validation_error("expected JSON object"))?;

    // Allowlist check — see module-level ALLOWED_KEYS
    for key in patch_obj.keys() {
        if !ALLOWED_KEYS.contains(&key.as_str()) {
            return Err(validation_error(format!(
                "modifying '{}' from the WebView is not permitted; allowed: {}",
                key,
                ALLOWED_KEYS.join(", "),
            )));
        }
    }

    reject_forbidden_allowed_subpaths(&patch).map_err(validation_error)?;

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

    let new_config: oneshim_core::config::AppConfig =
        serde_json::from_value(current_val).map_err(IpcError::from)?;

    validate_config_bounds(&new_config).map_err(validation_error)?;

    state
        .config_manager()
        .update(new_config)
        .map_err(IpcError::from)
}

/// Validate numeric config bounds to prevent tight loops or resource exhaustion
/// from WebView-supplied values.
fn validate_config_bounds(config: &oneshim_core::config::AppConfig) -> Result<(), String> {
    if config.monitor.poll_interval_ms < 1000 {
        return Err(format!(
            "monitor.poll_interval_ms must be >= 1000 (got {})",
            config.monitor.poll_interval_ms
        ));
    }
    if config.monitor.sync_interval_ms < 1000 {
        return Err(format!(
            "monitor.sync_interval_ms must be >= 1000 (got {})",
            config.monitor.sync_interval_ms
        ));
    }
    if config.monitor.heartbeat_interval_ms < 5000 {
        return Err(format!(
            "monitor.heartbeat_interval_ms must be >= 5000 (got {})",
            config.monitor.heartbeat_interval_ms
        ));
    }
    if config.vision.capture_throttle_ms < 1000 {
        return Err(format!(
            "vision.capture_throttle_ms must be >= 1000 (got {})",
            config.vision.capture_throttle_ms
        ));
    }
    if config.analysis.max_suggestions > 200 {
        return Err(format!(
            "analysis.max_suggestions must be <= 200 (got {})",
            config.analysis.max_suggestions
        ));
    }
    if config.analysis.throttle_secs < 10 {
        return Err(format!(
            "analysis.throttle_secs must be >= 10 (got {})",
            config.analysis.throttle_secs
        ));
    }
    if config.notification.idle_notification_mins == 0 {
        return Err("notification.idle_notification_mins must be >= 1 (got 0)".to_string());
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

/// 허용된 설정 키 목록 반환 — 프론트엔드 allowlist 검증 및 drift detection용
#[command]
pub async fn get_allowed_setting_keys() -> Vec<String> {
    ALLOWED_KEYS.iter().map(|s| s.to_string()).collect()
}

/// 웹 서버 포트 조회 — 프론트엔드 API base URL 결정용
#[command]
pub async fn get_web_port(state: tauri::State<'_, ConfigRuntimeState>) -> Result<u16, IpcError> {
    Ok(state.web_port())
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
        let mut config = oneshim_core::config::AppConfig::default_config();
        config.monitor.poll_interval_ms = 500;
        let err = validate_config_bounds(&config).unwrap_err();
        assert!(err.contains("poll_interval_ms"), "err: {err}");
    }

    #[test]
    fn validate_config_bounds_rejects_high_max_suggestions() {
        let mut config = oneshim_core::config::AppConfig::default_config();
        config.analysis.max_suggestions = 999;
        let err = validate_config_bounds(&config).unwrap_err();
        assert!(err.contains("max_suggestions"), "err: {err}");
    }

    #[test]
    fn validate_config_bounds_accepts_defaults() {
        let config = oneshim_core::config::AppConfig::default_config();
        assert!(validate_config_bounds(&config).is_ok());
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
}
