use oneshim_api_contracts::automation::AuditEntryDto;

use oneshim_core::models::audit::AuditEntry;

use crate::services::automation_service::AUTOMATION_AUDIT_SCHEMA_VERSION;

pub fn map_audit_entry(entry: AuditEntry) -> AuditEntryDto {
    AuditEntryDto {
        schema_version: AUTOMATION_AUDIT_SCHEMA_VERSION.to_string(),
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
