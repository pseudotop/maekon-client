use std::path::Path;

use chrono::Utc;
use maekon_api_contracts::support::RuntimeLogSnapshotDto;
use maekon_core::config::PiiFilterLevel;
use maekon_core::ports::pii_sanitizer::PiiSanitizer;
use tauri::command;

use crate::feature_capabilities::{
    build_feature_capability_snapshot,
    probe_provider_surface_endpoint as probe_provider_surface_endpoint_impl,
    FeatureCapabilitySnapshot, FeatureCapabilityState, ProviderEndpointProbeResult,
};
use crate::ipc_error::IpcError;
use crate::runtime_state::{ConfigRuntimeState, SecretBackendCapabilities, SecretBackendState};
use crate::services::log_helpers;
use crate::updater::{UpdatePreview, Updater};

const DEFAULT_LOG_LINE_LIMIT: usize = 200;
const MAX_LOG_LINE_LIMIT: usize = 500;
const MAX_FRONTEND_LOG_MESSAGE_LEN: usize = 4_000;
const MAX_FRONTEND_LOG_CONTEXT_LEN: usize = 12_000;

fn sanitize_runtime_log_snapshot(
    snapshot: RuntimeLogSnapshotDto,
    sanitizer: Option<&dyn PiiSanitizer>,
) -> RuntimeLogSnapshotDto {
    let Some(sanitizer) = sanitizer else {
        return snapshot;
    };

    // #6261: this snapshot is a support/share surface — it is rendered in the UI
    // and copied to the clipboard verbatim. Sanitize at Strict (not Standard) so
    // mask_api_keys (sk-/pk-/ghp_/AKIA/xoxb-/"Bearer <token>"/PEM PRIVATE KEY
    // blocks), mask_ip_addresses, and mask_passport run before any secret that
    // landed in a logged endpoint URL / structured field can be exfiltrated.
    // Standard masks only IBAN/email/phone/card/KR-ID/SSN/user-path.
    RuntimeLogSnapshotDto {
        generated_at: snapshot.generated_at,
        log_dir: sanitizer.sanitize_text(&snapshot.log_dir, PiiFilterLevel::Strict),
        log_file: snapshot
            .log_file
            .map(|file| sanitizer.sanitize_text(&file, PiiFilterLevel::Strict)),
        line_count: snapshot.line_count,
        recent_text: sanitizer.sanitize_text(&snapshot.recent_text, PiiFilterLevel::Strict),
    }
}

fn runtime_log_snapshot_from_dir(
    log_dir: &Path,
    line_limit: usize,
    sanitizer: Option<&dyn PiiSanitizer>,
) -> Result<RuntimeLogSnapshotDto, String> {
    let latest_log = log_helpers::newest_log_file(log_dir)?;
    let (log_file, line_count, recent_text) = if let Some(path) = latest_log {
        let (line_count, recent_text) = log_helpers::tail_log_file(&path, line_limit)?;
        (Some(path.display().to_string()), line_count, recent_text)
    } else {
        (None, 0, String::new())
    };

    let snapshot = RuntimeLogSnapshotDto {
        generated_at: Utc::now().to_rfc3339(),
        log_dir: log_dir.display().to_string(),
        log_file,
        line_count,
        recent_text,
    };

    Ok(sanitize_runtime_log_snapshot(snapshot, sanitizer))
}

pub(crate) fn sanitize_frontend_surface(surface: &str) -> String {
    let trimmed = surface.trim();
    if trimmed.is_empty() {
        return "unknown".to_string();
    }

    let normalized: String = trimmed
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect();

    normalized.trim_matches('-').to_string()
}

pub(crate) fn truncate_log_field(value: String, limit: usize) -> String {
    if value.len() <= limit {
        return value;
    }

    // Walk back to the previous UTF-8 char boundary so we never panic on
    // multi-byte sequences (Korean, emoji, accented Latin, etc.) that
    // straddle the byte limit.
    let mut cut = limit;
    while cut > 0 && !value.is_char_boundary(cut) {
        cut -= 1;
    }

    let mut truncated = value;
    truncated.truncate(cut);
    truncated.push_str(" …(truncated)");
    truncated
}

fn emit_frontend_log(level: &str, surface: &str, message: String, context: Option<String>) {
    match (level, context.as_deref()) {
        ("trace", Some(context)) => tracing::trace!(
            target: "webview.console",
            surface = %surface,
            message = %message,
            context = %context,
            "frontend runtime log"
        ),
        ("trace", None) => tracing::trace!(
            target: "webview.console",
            surface = %surface,
            message = %message,
            "frontend runtime log"
        ),
        ("debug", Some(context)) => tracing::debug!(
            target: "webview.console",
            surface = %surface,
            message = %message,
            context = %context,
            "frontend runtime log"
        ),
        ("debug", None) => tracing::debug!(
            target: "webview.console",
            surface = %surface,
            message = %message,
            "frontend runtime log"
        ),
        ("info", Some(context)) => tracing::info!(
            target: "webview.console",
            surface = %surface,
            message = %message,
            context = %context,
            "frontend runtime log"
        ),
        ("info", None) => tracing::info!(
            target: "webview.console",
            surface = %surface,
            message = %message,
            "frontend runtime log"
        ),
        ("warn", Some(context)) => tracing::warn!(
            target: "webview.console",
            surface = %surface,
            message = %message,
            context = %context,
            "frontend runtime log"
        ),
        ("warn", None) => tracing::warn!(
            target: "webview.console",
            surface = %surface,
            message = %message,
            "frontend runtime log"
        ),
        ("error", Some(context)) => tracing::error!(
            target: "webview.console",
            surface = %surface,
            message = %message,
            context = %context,
            "frontend runtime log"
        ),
        ("error", None) => tracing::error!(
            target: "webview.console",
            surface = %surface,
            message = %message,
            "frontend runtime log"
        ),
        _ => {}
    }
}

/// Query automation status — returns the value derived from user settings.
#[command]
pub async fn get_automation_status(
    state: tauri::State<'_, ConfigRuntimeState>,
) -> Result<bool, IpcError> {
    Ok(state.config_manager().get().automation.enabled)
}

/// Secret backend capability snapshot for desktop runtime surfaces.
#[command]
pub async fn get_secret_backend_capabilities(
    state: tauri::State<'_, SecretBackendState>,
) -> Result<SecretBackendCapabilities, IpcError> {
    Ok(state.0.clone())
}

/// Generic feature capability + maturity snapshot for desktop runtime surfaces.
#[command]
pub async fn get_feature_capabilities(
    state: tauri::State<'_, FeatureCapabilityState>,
) -> Result<FeatureCapabilitySnapshot, IpcError> {
    let secret_backend = state.0.clone();
    Ok(build_feature_capability_snapshot(&secret_backend).await)
}

/// Probe the currently configured provider endpoint for a direct/self-hosted surface.
#[command]
pub async fn probe_provider_surface_endpoint(
    surface_id: String,
    endpoint_kind: String,
    endpoint: String,
    allow_external_egress: Option<bool>,
) -> Result<ProviderEndpointProbeResult, IpcError> {
    Ok(probe_provider_surface_endpoint_impl(
        &surface_id,
        &endpoint_kind,
        &endpoint,
        allow_external_egress.unwrap_or(false),
    )
    .await)
}

#[command]
pub async fn get_runtime_log_snapshot(
    line_limit: Option<usize>,
) -> Result<RuntimeLogSnapshotDto, IpcError> {
    let line_limit = line_limit
        .unwrap_or(DEFAULT_LOG_LINE_LIMIT)
        .clamp(10, MAX_LOG_LINE_LIMIT);
    let sanitizer = maekon_vision::privacy::VisionPiiSanitizer;
    runtime_log_snapshot_from_dir(
        &log_helpers::runtime_log_dir(),
        line_limit,
        Some(&sanitizer),
    )
    .map_err(|msg| IpcError::new("internal.generic", msg))
}

#[command]
pub async fn record_frontend_log(
    surface: String,
    level: String,
    message: String,
    context: Option<String>,
) -> Result<(), IpcError> {
    let surface = sanitize_frontend_surface(&surface);
    let surface = if surface.is_empty() {
        "unknown".to_string()
    } else {
        surface
    };
    // #6266: sanitize frontend-supplied strings for PII before they reach the
    // log, matching report_frontend_error (error_report.rs). The frontend log
    // bridge forwards raw console.error/onerror/unhandledrejection payloads,
    // which can carry emails / tokens / file paths. Run the same Standard-level
    // masking, after trim+truncate, on each non-empty field.
    let message = truncate_log_field(message.trim().to_string(), MAX_FRONTEND_LOG_MESSAGE_LEN);
    let message =
        maekon_vision::privacy::sanitize_title_with_level(&message, PiiFilterLevel::Standard);
    let context = context
        .map(|value| truncate_log_field(value.trim().to_string(), MAX_FRONTEND_LOG_CONTEXT_LEN))
        .filter(|value| !value.is_empty())
        .map(|value| {
            maekon_vision::privacy::sanitize_title_with_level(&value, PiiFilterLevel::Standard)
        });

    let level = match level.trim().to_ascii_lowercase().as_str() {
        "trace" => "trace",
        "debug" => "debug",
        "info" => "info",
        "warn" | "warning" => "warn",
        "error" => "error",
        other => {
            return Err(IpcError::new(
                "validation.invalid_arguments",
                format!("Unsupported frontend log level: {other}"),
            ));
        }
    };
    emit_frontend_log(level, &surface, message, context);

    Ok(())
}

/// Preview available update info without downloading.
#[command]
pub async fn preview_update(
    state: tauri::State<'_, ConfigRuntimeState>,
) -> Result<UpdatePreview, IpcError> {
    let update_config = state.config_manager().get().update.clone();
    let updater = Updater::new(update_config);
    updater
        .preview_update_availability()
        .await
        .map_err(|e| IpcError::new("internal.generic", e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::thread;
    use std::time::Duration;

    struct MarkerSanitizer;

    impl PiiSanitizer for MarkerSanitizer {
        fn sanitize_text(&self, text: &str, level: PiiFilterLevel) -> String {
            // #5857: the Windows path separator is '\\' — a hardcoded POSIX path
            // ("/Users/alice") would not match the real path that tempdir creates,
            // so it would fail to produce the marker. Assemble it from the OS
            // separator so the wiring check carries the same meaning on both platforms.
            let sep = std::path::MAIN_SEPARATOR;
            let user_path = format!("{sep}Users{sep}alice");
            // Email + user-path mask at Standard and above.
            let mut out = text
                .replace("alice@example.com", "[EMAIL]")
                .replace(&user_path, "[USER]");
            // #6261: API-key/secret masking mirrors the real VisionPiiSanitizer
            // cascade — mask_api_keys runs ONLY at Strict. This makes the test a
            // genuine regression guard: if the snapshot sanitizer regresses to
            // Standard, the provider secret survives and the assertion below
            // fails (the previous level-agnostic mock masked unconditionally,
            // giving false confidence).
            if level == PiiFilterLevel::Strict {
                out = out.replace("sk-ant-secret", "[PROVIDER_SECRET]");
            }
            out
        }
    }

    #[test]
    fn runtime_log_snapshot_returns_empty_when_directory_is_missing() {
        let dir = PathBuf::from("/nonexistent/maekon-log-tests");
        let snapshot =
            runtime_log_snapshot_from_dir(&dir, 50, None).expect("snapshot should still succeed");

        assert_eq!(snapshot.log_dir, dir.display().to_string());
        assert!(snapshot.log_file.is_none());
        assert_eq!(snapshot.line_count, 0);
        assert!(snapshot.recent_text.is_empty());
    }

    #[test]
    fn runtime_log_snapshot_reads_tail_of_newest_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let older = temp.path().join("maekon.log.older");
        let newer = temp.path().join("maekon.log.newer");

        fs::write(&older, "old-1\nold-2\n").expect("write older log");
        thread::sleep(Duration::from_millis(20));
        fs::write(&newer, "new-1\nnew-2\nnew-3\n").expect("write newer log");

        let snapshot =
            runtime_log_snapshot_from_dir(temp.path(), 2, None).expect("snapshot should succeed");
        let log_file = snapshot.log_file.expect("newest file should be selected");

        assert_eq!(snapshot.log_dir, temp.path().display().to_string());
        assert!(log_file.ends_with("maekon.log.newer"));
        assert_eq!(snapshot.line_count, 2);
        assert_eq!(snapshot.recent_text, "new-2\nnew-3");
    }

    #[test]
    fn runtime_log_snapshot_sanitizes_support_display_and_copy_text() {
        let temp = tempfile::tempdir().expect("tempdir");
        let user_dir = temp.path().join("Users").join("alice").join("Library");
        fs::create_dir_all(&user_dir).expect("create nested log dir");
        let log = user_dir.join("maekon.log");
        fs::write(
            &log,
            "provider failed for alice@example.com\nendpoint token sk-ant-secret\n",
        )
        .expect("write log");
        let sanitizer = MarkerSanitizer;

        let snapshot = runtime_log_snapshot_from_dir(&user_dir, 20, Some(&sanitizer))
            .expect("snapshot should succeed");

        let sep = std::path::MAIN_SEPARATOR;
        let raw_user_path = format!("{sep}Users{sep}alice");
        assert!(snapshot.log_dir.contains("[USER]"));
        assert!(!snapshot.log_dir.contains(&raw_user_path));
        let log_file = snapshot.log_file.expect("log file should be selected");
        assert!(log_file.contains("[USER]"));
        assert!(!log_file.contains(&raw_user_path));
        assert!(snapshot.recent_text.contains("[EMAIL]"));
        assert!(snapshot.recent_text.contains("[PROVIDER_SECRET]"));
        assert!(!snapshot.recent_text.contains("alice@example.com"));
        assert!(!snapshot.recent_text.contains("sk-ant-secret"));
    }

    #[test]
    fn sanitize_frontend_surface_normalizes_unsafe_characters() {
        assert_eq!(
            sanitize_frontend_surface("tracking panel/main"),
            "tracking-panel-main"
        );
        assert_eq!(sanitize_frontend_surface(""), "unknown");
    }
}
