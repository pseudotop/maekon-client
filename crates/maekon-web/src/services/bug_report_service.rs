use maekon_api_contracts::bug_report::{BugReportBundleDto, ConnectionStatusDto, SystemInfoDto};
use maekon_api_contracts::support::RuntimeLogSnapshotDto;
use maekon_core::config::PiiFilterLevel;
use maekon_core::models::bug_report::{BugId, RuntimeLogSnapshot};
use maekon_core::ports::pii_sanitizer::PiiSanitizer;

use crate::error::ApiError;
use crate::services::support_service::SupportDiagnosticsQueryService;
use crate::services::web_contexts::BugReportContext;

pub struct BugReportService {
    ctx: BugReportContext,
}

impl BugReportService {
    pub fn new(ctx: BugReportContext) -> Self {
        Self { ctx }
    }

    /// Create a bug report bundle. Returns `Err` if PII sanitizer is not wired,
    /// refusing to produce a bundle without privacy protection.
    pub async fn create_report(
        &self,
        include_logs: bool,
        pii_level: Option<String>,
    ) -> Result<BugReportBundleDto, ApiError> {
        let sanitizer = self.ctx.pii_sanitizer.as_ref().ok_or_else(|| {
            // Iter-101: safety-refusal because PII sanitizer wiring is
            // absent in this deployment. Route as 503 ServiceUnavailable
            // (admin action required) rather than 500 Internal (suggests
            // runtime crash). Semantic: the bug-report feature itself is
            // unavailable until the admin completes deployment wiring.
            ApiError::ServiceUnavailable(
                "PII sanitizer not configured — cannot produce bug report".into(),
            )
        })?;

        let diagnostics = SupportDiagnosticsQueryService::new(self.ctx.support.clone())
            .get_diagnostics()
            .await;

        let system = self.collect_system_info().await;
        let connection = self.collect_connection_status();

        let runtime_logs = if include_logs {
            if let Some(provider) = &self.ctx.runtime_log_provider {
                match provider.snapshot(200).await {
                    Ok(snap) => Some(runtime_log_snapshot_to_dto(snap)),
                    Err(e) => {
                        tracing::warn!("Failed to collect runtime logs: {e}");
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        let level = parse_pii_level(pii_level.as_deref());
        let bug_id = generate_bug_id(&system.app_version, &system.os_name);

        let mut bundle = BugReportBundleDto {
            bug_id: bug_id.to_string(),
            diagnostics,
            system,
            connection,
            runtime_logs,
            pii_filter_level: level,
        };

        sanitize_bundle(&**sanitizer, &mut bundle, level);

        Ok(bundle)
    }

    /// Collect system info off the tokio worker thread.
    ///
    /// `SystemInfoProvider::system_info` acquires a `Mutex` and calls
    /// `sysinfo::refresh_memory` (a blocking syscall) under the lock.  Calling
    /// it directly from an async context would stall a tokio worker for the
    /// duration of the syscall.  We therefore clone the `Arc<dyn …>` and
    /// dispatch via `tokio::task::spawn_blocking` so the work runs on the
    /// dedicated blocking thread pool (#5997).
    async fn collect_system_info(&self) -> SystemInfoDto {
        let provider = self.ctx.system_info_provider.clone();
        let static_info = if let Some(p) = provider {
            tokio::task::spawn_blocking(move || p.system_info())
                .await
                .ok()
        } else {
            None
        };
        SystemInfoDto {
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            os_name: std::env::consts::OS.to_string(),
            os_version: static_info
                .as_ref()
                .map(|s| s.os_version.clone())
                .unwrap_or_default(),
            arch: std::env::consts::ARCH.to_string(),
            runtime: "web".to_string(),
            cpu_count: static_info.as_ref().map(|s| s.cpu_count).unwrap_or(0),
            memory_total_mb: static_info
                .as_ref()
                .map(|s| s.memory_total_bytes / 1_048_576)
                .unwrap_or(0),
            memory_available_mb: static_info
                .as_ref()
                .map(|s| s.memory_available_bytes / 1_048_576)
                .unwrap_or(0),
            uptime_seconds: static_info.as_ref().map(|s| s.uptime_seconds).unwrap_or(0),
        }
    }

    fn collect_connection_status(&self) -> ConnectionStatusDto {
        ConnectionStatusDto {
            server_reachable: false,
            last_sync_at: None,
            grpc_enabled: false,
            websocket_connected: false,
        }
    }
}

fn runtime_log_snapshot_to_dto(snap: RuntimeLogSnapshot) -> RuntimeLogSnapshotDto {
    RuntimeLogSnapshotDto {
        generated_at: chrono::Utc::now().to_rfc3339(),
        log_dir: snap.log_dir,
        log_file: snap.log_file,
        line_count: snap.line_count,
        recent_text: snap.recent_text,
    }
}

fn generate_bug_id(app_version: &str, os_info: &str) -> BugId {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(app_version.as_bytes());
    hasher.update(b"|");
    hasher.update(os_info.as_bytes());
    hasher.update(b"|");
    hasher.update(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .to_le_bytes(),
    );
    let random_bytes: [u8; 8] = rand::random();
    hasher.update(random_bytes);
    let hash = hasher.finalize();
    BugId::new(format!("BUG-{}", hex::encode(&hash[..6])))
        .unwrap_or_else(|error| panic!("format is valid: {error}"))
}

fn parse_pii_level(level: Option<&str>) -> PiiFilterLevel {
    match level {
        Some("strict") => PiiFilterLevel::Strict,
        _ => PiiFilterLevel::Standard,
    }
}

fn sanitize_bundle(
    sanitizer: &dyn PiiSanitizer,
    bundle: &mut BugReportBundleDto,
    level: PiiFilterLevel,
) {
    let effective = match level {
        PiiFilterLevel::Off | PiiFilterLevel::Basic => PiiFilterLevel::Standard,
        other => other,
    };

    for entry in &mut bundle.diagnostics.recent_audit_entries {
        if let Some(ref mut details) = entry.details {
            *details = sanitizer.sanitize_text(details, effective);
        }
    }

    for entry in &mut bundle.diagnostics.recent_policy_events {
        if let Some(ref mut details) = entry.details {
            *details = sanitizer.sanitize_text(details, effective);
        }
    }

    if let Some(ref mut logs) = bundle.runtime_logs {
        logs.recent_text = sanitizer.sanitize_text(&logs.recent_text, effective);
        logs.log_dir = sanitizer.sanitize_text(&logs.log_dir, effective);
        if let Some(ref mut file) = logs.log_file {
            *file = sanitizer.sanitize_text(file, effective);
        }
    }

    if let Some(ref mut path) = bundle.diagnostics.health.frames_dir_path {
        *path = sanitizer.sanitize_text(path, effective);
    }
    if let Some(ref mut err) = bundle.diagnostics.health.storage_error {
        *err = sanitizer.sanitize_text(err, effective);
    }

    // Sanitize settings_snapshot fields that may contain user-identifying data
    let s = &mut bundle.diagnostics.settings_snapshot;
    s.sync.device_name = sanitizer.sanitize_text(&s.sync.device_name, effective);
    s.network.server_base_url = sanitizer.sanitize_text(&s.network.server_base_url, effective);
    s.network.grpc_endpoint = sanitizer.sanitize_text(&s.network.grpc_endpoint, effective);

    // Paths that likely contain OS usernames
    for path in &mut s.sandbox.allowed_read_paths {
        *path = sanitizer.sanitize_text(path, effective);
    }
    for path in &mut s.sandbox.allowed_write_paths {
        *path = sanitizer.sanitize_text(path, effective);
    }

    // App exclusion lists can reveal installed software
    for app in &mut s.privacy.excluded_apps {
        *app = sanitizer.sanitize_text(app, effective);
    }

    // Scene action override may contain person name
    s.ai_provider.scene_action_override.approved_by =
        sanitizer.sanitize_text(&s.ai_provider.scene_action_override.approved_by, effective);

    // SECURITY (#7066): defense-in-depth — hard-clear the cloud STT BYOK secret
    // so it can NEVER egress in the shareable bug-report bundle. The assembler
    // already masks it on the GET path, but this structured secret is never
    // routed through `sanitize_text` (free-text PII regexes cannot catch a key),
    // so we drop it entirely here rather than relying solely on the read-path
    // mask. Matches the no-secret-in-diagnostics standard the other ~10 scrubbed
    // fields follow.
    s.audio.cloud_api_key = String::new();

    // External API endpoints
    if let Some(ref mut api) = s.ai_provider.ocr_api {
        api.endpoint = sanitizer.sanitize_text(&api.endpoint, effective);
    }
    if let Some(ref mut api) = s.ai_provider.llm_api {
        api.endpoint = sanitizer.sanitize_text(&api.endpoint, effective);
    }

    for summary in &mut bundle.diagnostics.provider_cli {
        summary.surface_id = sanitizer.sanitize_text(&summary.surface_id, effective);
        if let Some(ref mut tool_id) = summary.tool_id {
            *tool_id = sanitizer.sanitize_text(tool_id, effective);
        }
        if let Some(ref mut candidate_name) = summary.candidate_name {
            *candidate_name = sanitizer.sanitize_text(candidate_name, effective);
        }
        if let Some(ref mut executable_hint) = summary.executable_hint {
            *executable_hint =
                executable_file_name_hint(&sanitizer.sanitize_text(executable_hint, effective));
        }
        if let Some(ref mut dependency_status) = summary.dependency_status {
            *dependency_status = sanitizer.sanitize_text(dependency_status, effective);
        }
        if let Some(ref mut status_reason) = summary.status_reason {
            *status_reason = sanitizer.sanitize_text(status_reason, effective);
        }
    }
}

fn executable_file_name_hint(value: &str) -> String {
    value
        .rsplit(['\\', '/'])
        .find(|part| !part.is_empty())
        .unwrap_or(value)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_bug_id_format() {
        let id = generate_bug_id("0.4.16", "macos");
        let s = id.as_str();
        assert!(s.starts_with("BUG-"));
        assert_eq!(s.len(), 16);
    }

    #[test]
    fn generate_bug_id_unique() {
        let a = generate_bug_id("0.4.16", "macos");
        let b = generate_bug_id("0.4.16", "macos");
        assert_ne!(a, b);
    }

    #[test]
    fn parse_pii_level_defaults_to_standard() {
        assert!(matches!(parse_pii_level(None), PiiFilterLevel::Standard));
        assert!(matches!(
            parse_pii_level(Some("anything")),
            PiiFilterLevel::Standard
        ));
    }

    #[test]
    fn parse_pii_level_strict() {
        assert!(matches!(
            parse_pii_level(Some("strict")),
            PiiFilterLevel::Strict
        ));
    }

    fn mock_sanitizer() -> maekon_core::ports::pii_sanitizer::FakePiiSanitizer {
        maekon_core::ports::pii_sanitizer::FakePiiSanitizer::new()
            .with_email("user@example.com")
            .with_replacement("/Users/alice", "[USER]")
            .with_replacement("sk_live_abc123", "[PROVIDER_SECRET]")
    }

    #[test]
    fn sanitize_bundle_filters_audit_details() {
        use maekon_api_contracts::automation::AuditEntryDto;
        use maekon_api_contracts::support::{DiagnosticsBundleDto, DiagnosticsHealthDto};

        let mut bundle = BugReportBundleDto {
            bug_id: "BUG-000000000000".to_string(),
            diagnostics: DiagnosticsBundleDto {
                schema_version: "test".to_string(),
                generated_at: "now".to_string(),
                health: DiagnosticsHealthDto {
                    storage_ok: true,
                    storage_error: None,
                    frames_dir_configured: false,
                    frames_dir_path: Some("/Users/alice/frames".to_string()),
                    frames_dir_exists: None,
                    config_manager_configured: false,
                    automation_controller_configured: false,
                    update_control_configured: false,
                },
                settings_snapshot: Default::default(),
                storage_stats: None,
                provider_cli: vec![],
                recent_audit_entries: vec![AuditEntryDto {
                    schema_version: "1".to_string(),
                    entry_id: "1".to_string(),
                    timestamp: "t".to_string(),
                    session_id: "s".to_string(),
                    command_id: "c".to_string(),
                    action_type: "test".to_string(),
                    status: "ok".to_string(),
                    details: Some("contact user@example.com".to_string()),
                    elapsed_ms: None,
                }],
                recent_policy_events: vec![],
            },
            system: SystemInfoDto {
                app_version: "0.4.16".to_string(),
                os_name: "macos".to_string(),
                os_version: "15.4".to_string(),
                arch: "aarch64".to_string(),
                runtime: "tauri-desktop".to_string(),
                cpu_count: 10,
                memory_total_mb: 16384,
                memory_available_mb: 8192,
                uptime_seconds: 3600,
            },
            connection: ConnectionStatusDto {
                server_reachable: false,
                last_sync_at: None,
                grpc_enabled: false,
                websocket_connected: false,
            },
            runtime_logs: None,
            pii_filter_level: PiiFilterLevel::Standard,
        };

        sanitize_bundle(&mock_sanitizer(), &mut bundle, PiiFilterLevel::Standard);

        let details = bundle.diagnostics.recent_audit_entries[0]
            .details
            .as_ref()
            .unwrap();
        assert!(details.contains("[EMAIL]"));
        assert!(!details.contains("user@example.com"));

        let path = bundle.diagnostics.health.frames_dir_path.as_ref().unwrap();
        assert!(path.contains("[USER]"));
        assert!(!path.contains("/Users/alice"));
    }

    /// #7066: the cloud STT BYOK secret must never egress in the shareable
    /// bug-report bundle. `sanitize_bundle` hard-clears it (defense-in-depth on
    /// top of the read-path mask) — a structured secret that free-text PII
    /// regexes cannot catch.
    #[test]
    fn sanitize_bundle_scrubs_audio_cloud_api_key() {
        use maekon_api_contracts::support::{DiagnosticsBundleDto, DiagnosticsHealthDto};

        let mut settings_snapshot = maekon_api_contracts::settings::AppSettings::default();
        settings_snapshot.audio.cloud_api_key = "sk-super-secret-cloud-stt-key".to_string();

        let mut bundle = BugReportBundleDto {
            bug_id: "BUG-000000000000".to_string(),
            diagnostics: DiagnosticsBundleDto {
                schema_version: "test".to_string(),
                generated_at: "now".to_string(),
                health: DiagnosticsHealthDto {
                    storage_ok: true,
                    storage_error: None,
                    frames_dir_configured: false,
                    frames_dir_path: None,
                    frames_dir_exists: None,
                    config_manager_configured: false,
                    automation_controller_configured: false,
                    update_control_configured: false,
                },
                settings_snapshot,
                storage_stats: None,
                provider_cli: vec![],
                recent_audit_entries: vec![],
                recent_policy_events: vec![],
            },
            system: SystemInfoDto {
                app_version: "0.4.16".to_string(),
                os_name: "macos".to_string(),
                os_version: "15.4".to_string(),
                arch: "aarch64".to_string(),
                runtime: "tauri-desktop".to_string(),
                cpu_count: 10,
                memory_total_mb: 16384,
                memory_available_mb: 8192,
                uptime_seconds: 3600,
            },
            connection: ConnectionStatusDto {
                server_reachable: false,
                last_sync_at: None,
                grpc_enabled: false,
                websocket_connected: false,
            },
            runtime_logs: None,
            pii_filter_level: PiiFilterLevel::Standard,
        };

        sanitize_bundle(&mock_sanitizer(), &mut bundle, PiiFilterLevel::Standard);

        assert_eq!(
            bundle.diagnostics.settings_snapshot.audio.cloud_api_key, "",
            "bug-report bundle must NOT carry the cloud STT secret"
        );
        // Belt-and-braces: the secret must not survive anywhere in the serialized
        // bundle either.
        let serialized = serde_json::to_string(&bundle).expect("bundle serializes");
        assert!(
            !serialized.contains("sk-super-secret-cloud-stt-key"),
            "serialized bug-report bundle must not contain the raw cloud STT key"
        );
    }

    #[test]
    fn sanitize_bundle_enforces_minimum_standard() {
        use maekon_api_contracts::support::{DiagnosticsBundleDto, DiagnosticsHealthDto};

        let mut bundle = BugReportBundleDto {
            bug_id: "BUG-000000000000".to_string(),
            diagnostics: DiagnosticsBundleDto {
                schema_version: "test".to_string(),
                generated_at: "now".to_string(),
                health: DiagnosticsHealthDto {
                    storage_ok: true,
                    storage_error: Some("error at /Users/alice/db".to_string()),
                    frames_dir_configured: false,
                    frames_dir_path: None,
                    frames_dir_exists: None,
                    config_manager_configured: false,
                    automation_controller_configured: false,
                    update_control_configured: false,
                },
                settings_snapshot: Default::default(),
                storage_stats: None,
                provider_cli: vec![
                    maekon_api_contracts::support::ProviderCliDiagnosticSummaryDto {
                        surface_id: "provider_surface.openai.subprocess_cli".to_string(),
                        tool_id: Some("codex".to_string()),
                        candidate_name: Some("codex".to_string()),
                        executable_hint: Some(
                            "C:\\Users\\alice\\AppData\\Local\\Programs\\Codex\\codex.exe"
                                .to_string(),
                        ),
                        readiness: "auth_required".to_string(),
                        availability: "partially_available".to_string(),
                        dependency_status: Some("ready".to_string()),
                        status_reason: Some(
                            "auth failed for user@example.com with sk_live_abc123".to_string(),
                        ),
                        env_refresh_required: false,
                    },
                ],
                recent_audit_entries: vec![],
                recent_policy_events: vec![],
            },
            system: SystemInfoDto {
                app_version: "0.4.16".to_string(),
                os_name: "macos".to_string(),
                os_version: "15.4".to_string(),
                arch: "aarch64".to_string(),
                runtime: "web".to_string(),
                cpu_count: 4,
                memory_total_mb: 8192,
                memory_available_mb: 4096,
                uptime_seconds: 100,
            },
            connection: ConnectionStatusDto {
                server_reachable: false,
                last_sync_at: None,
                grpc_enabled: false,
                websocket_connected: false,
            },
            runtime_logs: None,
            pii_filter_level: PiiFilterLevel::Off,
        };

        // Even with Off level, sanitize_bundle enforces Standard minimum
        sanitize_bundle(&mock_sanitizer(), &mut bundle, PiiFilterLevel::Off);

        let err = bundle.diagnostics.health.storage_error.as_ref().unwrap();
        assert!(err.contains("[USER]"));
        let provider_cli = &bundle.diagnostics.provider_cli[0];
        assert_eq!(provider_cli.executable_hint.as_deref(), Some("codex.exe"));
        assert_eq!(
            provider_cli.status_reason.as_deref(),
            Some("auth failed for [EMAIL] with [PROVIDER_SECRET]")
        );
    }
}
