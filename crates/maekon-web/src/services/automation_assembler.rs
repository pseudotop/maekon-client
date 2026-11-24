use maekon_api_contracts::automation::AuditEntryDto;

use maekon_core::models::audit::AuditEntry;
use maekon_core::models::intent_execution::{IntentActionOutcome, IntentExecutionProvenance};

use crate::services::automation_service::AUTOMATION_AUDIT_SCHEMA_VERSION;

pub fn map_audit_entry(entry: AuditEntry) -> AuditEntryDto {
    let is_observation = entry.action_type == "intent.execution.observation";
    let execution_provenance = if is_observation {
        entry
            .details
            .as_deref()
            .and_then(|details| serde_json::from_str(details).ok())
    } else {
        None
    };
    AuditEntryDto {
        schema_version: AUTOMATION_AUDIT_SCHEMA_VERSION.to_string(),
        entry_id: entry.entry_id,
        timestamp: entry.timestamp.to_rfc3339(),
        session_id: entry.session_id,
        command_id: entry.command_id,
        action_type: entry.action_type,
        status: format!("{:?}", entry.status),
        // A malformed/legacy observation is unknown. Never pass unchecked
        // detail text through this new shared serialization surface.
        details: if is_observation { None } else { entry.details },
        elapsed_ms: entry.execution_time_ms,
        execution_provenance,
    }
}

/// Project a known process observation onto the existing audit lookup surface.
/// IDs/time come from its actual event; this is not a durable audit-row claim.
pub fn map_intent_observation(receipt: IntentExecutionProvenance) -> Option<AuditEntryDto> {
    let event = receipt.events.last()?;
    let status = match receipt.action_outcome {
        IntentActionOutcome::Completed => "Completed",
        IntentActionOutcome::Denied => "Denied",
        IntentActionOutcome::Failed | IntentActionOutcome::NotStarted => "Failed",
        IntentActionOutcome::Unknown => "Started",
    };
    Some(AuditEntryDto {
        schema_version: AUTOMATION_AUDIT_SCHEMA_VERSION.to_string(),
        entry_id: event.event_id.to_string(),
        timestamp: event.observed_at.to_rfc3339(),
        session_id: receipt.execution_id.to_string(),
        command_id: receipt.execution_id.to_string(),
        action_type: "intent.execution.observation".into(),
        status: status.into(),
        details: None,
        elapsed_ms: None,
        execution_provenance: Some(receipt),
    })
}
