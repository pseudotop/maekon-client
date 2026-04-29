use oneshim_api_contracts::bug_report::{BugReportBundleDto, ConnectionStatusDto, SystemInfoDto};
use oneshim_api_contracts::support::RuntimeLogSnapshotDto;
use oneshim_core::config::PiiFilterLevel;
use oneshim_core::models::bug_report::{BugId, RuntimeLogSnapshot};
use oneshim_core::ports::pii_sanitizer::PiiSanitizer;

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

        let system = self.collect_system_info();
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

    fn collect_system_info(&self) -> SystemInfoDto {
        let static_info = self
            .ctx
            .system_info_provider
            .as_ref()
            .map(|p| p.system_info());
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
    BugId::new(format!("BUG-{}", hex::encode(&hash[..6]))).expect("format is valid")
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

    // External API endpoints
    if let Some(ref mut api) = s.ai_provider.ocr_api {
        api.endpoint = sanitizer.sanitize_text(&api.endpoint, effective);
    }
    if let Some(ref mut api) = s.ai_provider.llm_api {
        api.endpoint = sanitizer.sanitize_text(&api.endpoint, effective);
    }
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

    struct MockSanitizer;
    impl PiiSanitizer for MockSanitizer {
        fn sanitize_text(&self, text: &str, _level: PiiFilterLevel) -> String {
            text.replace("user@example.com", "[EMAIL]")
                .replace("/Users/alice", "[USER]")
        }
    }

    #[test]
    fn sanitize_bundle_filters_audit_details() {
        use oneshim_api_contracts::automation::AuditEntryDto;
        use oneshim_api_contracts::support::{DiagnosticsBundleDto, DiagnosticsHealthDto};

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

        sanitize_bundle(&MockSanitizer, &mut bundle, PiiFilterLevel::Standard);

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

    #[test]
    fn sanitize_bundle_enforces_minimum_standard() {
        use oneshim_api_contracts::support::{DiagnosticsBundleDto, DiagnosticsHealthDto};

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
        sanitize_bundle(&MockSanitizer, &mut bundle, PiiFilterLevel::Off);

        let err = bundle.diagnostics.health.storage_error.as_ref().unwrap();
        assert!(err.contains("[USER]"));
    }
}
