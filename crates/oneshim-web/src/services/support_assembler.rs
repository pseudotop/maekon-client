use chrono::Utc;
use oneshim_api_contracts::automation::AuditEntryDto;
use oneshim_api_contracts::settings::{AppSettings, StorageStats};
use oneshim_api_contracts::support::{DiagnosticsBundleDto, DiagnosticsHealthDto};
use oneshim_core::models::audit::AuditEntry;

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
) -> DiagnosticsBundleDto {
    DiagnosticsBundleDto {
        schema_version: SUPPORT_DIAGNOSTICS_SCHEMA_VERSION.to_string(),
        generated_at: Utc::now().to_rfc3339(),
        health,
        settings_snapshot,
        storage_stats,
        recent_audit_entries,
        recent_policy_events,
    }
}
