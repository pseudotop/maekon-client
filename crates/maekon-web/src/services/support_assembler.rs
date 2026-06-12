use chrono::Utc;
use maekon_api_contracts::automation::AuditEntryDto;
use maekon_api_contracts::settings::{AppSettings, StorageStats};
use maekon_api_contracts::support::{
    DiagnosticsBundleDto, DiagnosticsHealthDto, ProviderCliDiagnosticSummaryDto,
};
use maekon_core::models::audit::AuditEntry;

const SUPPORT_DIAGNOSTICS_SCHEMA_VERSION: &str = "support.diagnostics.v1";
const SUPPORT_AUDIT_SCHEMA_VERSION: &str = "automation.audit.v1";

pub(crate) struct DiagnosticsHealthInput {
    pub storage_error: Option<String>,
    pub frames_dir_path: Option<String>,
    pub frames_dir_exists: Option<bool>,
    pub config_manager_configured: bool,
    pub automation_controller_configured: bool,
    pub update_control_configured: bool,
}

pub(crate) fn assemble_diagnostics_health(input: DiagnosticsHealthInput) -> DiagnosticsHealthDto {
    DiagnosticsHealthDto {
        storage_ok: input.storage_error.is_none(),
        storage_error: input.storage_error,
        frames_dir_configured: input.frames_dir_path.is_some(),
        frames_dir_path: input.frames_dir_path,
        frames_dir_exists: input.frames_dir_exists,
        config_manager_configured: input.config_manager_configured,
        automation_controller_configured: input.automation_controller_configured,
        update_control_configured: input.update_control_configured,
    }
}

pub(crate) fn to_audit_entry_dto(entry: AuditEntry) -> AuditEntryDto {
    AuditEntryDto {
        schema_version: SUPPORT_AUDIT_SCHEMA_VERSION.to_string(),
        entry_id: entry.entry_id,
        timestamp: entry.timestamp.to_rfc3339(),
        session_id: entry.session_id,
        command_id: entry.command_id,
        action_type: entry.action_type,
        status: format!("{:?}", entry.status),
        details: entry.details,
        elapsed_ms: entry.execution_time_ms,
    }
}

pub(crate) fn assemble_diagnostics_bundle(
    health: DiagnosticsHealthDto,
    settings_snapshot: AppSettings,
    storage_stats: Option<StorageStats>,
    recent_audit_entries: Vec<AuditEntryDto>,
    recent_policy_events: Vec<AuditEntryDto>,
    provider_cli: Vec<ProviderCliDiagnosticSummaryDto>,
) -> DiagnosticsBundleDto {
    DiagnosticsBundleDto {
        schema_version: SUPPORT_DIAGNOSTICS_SCHEMA_VERSION.to_string(),
        generated_at: Utc::now().to_rfc3339(),
        health,
        settings_snapshot,
        storage_stats,
        provider_cli,
        recent_audit_entries,
        recent_policy_events,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn health() -> DiagnosticsHealthDto {
        DiagnosticsHealthDto {
            storage_ok: true,
            storage_error: None,
            frames_dir_configured: false,
            frames_dir_path: None,
            frames_dir_exists: None,
            config_manager_configured: true,
            automation_controller_configured: true,
            update_control_configured: true,
        }
    }

    #[test]
    fn assemble_diagnostics_bundle_includes_provider_cli_summaries() {
        let provider_cli = vec![ProviderCliDiagnosticSummaryDto {
            surface_id: "provider_surface.openai.subprocess_cli".to_string(),
            tool_id: Some("codex".to_string()),
            candidate_name: Some("codex".to_string()),
            executable_hint: Some("codex.exe".to_string()),
            readiness: "invocation_ready".to_string(),
            availability: "available".to_string(),
            dependency_status: Some("ready".to_string()),
            status_reason: Some("cli_ready".to_string()),
            env_refresh_required: false,
        }];

        let bundle = assemble_diagnostics_bundle(
            health(),
            AppSettings::default(),
            None,
            Vec::new(),
            Vec::new(),
            provider_cli,
        );

        assert_eq!(bundle.provider_cli.len(), 1);
        assert_eq!(
            bundle.provider_cli[0].surface_id,
            "provider_surface.openai.subprocess_cli"
        );
        assert_eq!(
            bundle.provider_cli[0].executable_hint.as_deref(),
            Some("codex.exe")
        );
    }
}
