use oneshim_api_contracts::integration::{
    IntegrationAckCursorSummary, IntegrationAuditRecordSummary, IntegrationInboxPromptSummary,
    IntegrationOutboundRuntimeStatus, IntegrationSessionSummary,
};
use oneshim_core::models::integration::{
    IntegrationAckCursor, IntegrationAuthProfileKind, IntegrationInboxItemStatus,
    IntegrationInsightAuditRecord, IntegrationPrivacyClassification, IntegrationSessionState,
    ProactivePromptCategory, ProactivePromptPriority, StoredProactivePrompt,
};

use crate::AppState;

#[derive(Debug, Clone, Default)]
pub(crate) struct IntegrationStatusConfigSnapshot {
    present: bool,
    pub(crate) external_access_enabled: bool,
    integration_enabled: bool,
    bootstrap_configured: bool,
    auth_profile_kind: IntegrationAuthProfileKind,
    auth_source_configured: bool,
    auth_token_env_var: Option<String>,
    resource_indicator_configured: bool,
}

impl IntegrationStatusConfigSnapshot {
    pub(crate) fn from_state(state: &AppState) -> Self {
        let Some(config) = state
            .core
            .config_manager
            .as_ref()
            .map(|config_manager| config_manager.get())
        else {
            return Self::default();
        };

        let auth_token_env_var = config
            .integration
            .auth_token_env_var
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let auth_source_configured = match config.integration.auth_profile_kind {
            IntegrationAuthProfileKind::EnvToken => auth_token_env_var.is_some(),
            IntegrationAuthProfileKind::OidcDeviceFlow => {
                config
                    .integration
                    .oidc_device_flow
                    .client_id
                    .as_deref()
                    .map(str::trim)
                    .is_some_and(|value| !value.is_empty())
                    && config
                        .integration
                        .oidc_device_flow
                        .device_authorization_url
                        .as_deref()
                        .map(str::trim)
                        .is_some_and(|value| !value.is_empty())
                    && config
                        .integration
                        .oidc_device_flow
                        .token_url
                        .as_deref()
                        .map(str::trim)
                        .is_some_and(|value| !value.is_empty())
            }
        };

        Self {
            present: true,
            external_access_enabled: config.web.allow_external,
            integration_enabled: config.integration.enabled,
            bootstrap_configured: config
                .integration
                .bootstrap_url
                .as_deref()
                .map(str::trim)
                .is_some_and(|value| !value.is_empty()),
            auth_profile_kind: config.integration.auth_profile_kind.clone(),
            auth_source_configured,
            auth_token_env_var,
            resource_indicator_configured: config
                .integration
                .resource_indicator
                .as_deref()
                .map(str::trim)
                .is_some_and(|value| !value.is_empty()),
        }
    }

    pub(crate) fn apply_to_runtime_status(
        &self,
        outbound_runtime: &mut IntegrationOutboundRuntimeStatus,
    ) {
        if !self.present {
            return;
        }
        outbound_runtime.enabled = self.integration_enabled;
        outbound_runtime.bootstrap_configured = self.bootstrap_configured;
        outbound_runtime.auth_profile_kind = self.auth_profile_kind.clone();
        outbound_runtime.auth_source_configured = self.auth_source_configured;
        outbound_runtime.auth_material_available = self
            .auth_token_env_var
            .as_deref()
            .and_then(|env_var| std::env::var(env_var).ok())
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false);
        outbound_runtime.resource_indicator_configured = self.resource_indicator_configured;
    }
}

pub(crate) fn map_session_summary(state: IntegrationSessionState) -> IntegrationSessionSummary {
    IntegrationSessionSummary {
        status: state.status,
        transport_kind: state.transport_kind,
        auth_scheme: state.auth_scheme,
        connected_at: state.connected_at,
        last_heartbeat_at: state.last_heartbeat_at,
        requested_scopes: state
            .requested_scopes
            .iter()
            .map(|scope| scope.as_str().to_string())
            .collect(),
        granted_scopes: state
            .granted_scopes
            .iter()
            .map(|scope| scope.as_str().to_string())
            .collect(),
    }
}

pub(crate) fn map_ack_cursor_summary(cursor: IntegrationAckCursor) -> IntegrationAckCursorSummary {
    IntegrationAckCursorSummary {
        stream_id: cursor.stream_id,
        cursor: cursor.cursor,
        acknowledged_at: cursor.acknowledged_at,
    }
}

pub(crate) fn map_audit_record(
    record: IntegrationInsightAuditRecord,
) -> IntegrationAuditRecordSummary {
    IntegrationAuditRecordSummary {
        record_id: record.record_id,
        envelope_id: record.envelope_id,
        packet_id: record.packet_id,
        disposition: match record.disposition {
            oneshim_core::models::integration::IntegrationEgressDisposition::Allow => "allow",
            oneshim_core::models::integration::IntegrationEgressDisposition::Deny => "deny",
            oneshim_core::models::integration::IntegrationEgressDisposition::RequireUserApproval => {
                "require_user_approval"
            }
        }
        .to_string(),
        reason: record.reason,
        privacy_classification: privacy_classification_label(record.privacy_classification)
            .to_string(),
        capability_scope: record.capability_scope.as_str().to_string(),
        occurred_at: record.occurred_at,
    }
}

pub(crate) fn map_prompt(prompt: StoredProactivePrompt) -> IntegrationInboxPromptSummary {
    IntegrationInboxPromptSummary {
        prompt_id: prompt.prompt.prompt_id,
        category: prompt_category_label(&prompt.prompt.category).to_string(),
        priority: prompt_priority_label(&prompt.prompt.priority).to_string(),
        title: prompt.prompt.title,
        body: prompt.prompt.body,
        status: inbox_status_label(&prompt.status).to_string(),
        received_at: prompt.received_at,
        status_updated_at: prompt.status_updated_at,
        presented_at: prompt.presented_at,
        expires_at: prompt.prompt.expires_at,
        source_system: prompt.prompt.provenance.source_system,
        source_actor: prompt.prompt.provenance.source_actor,
        correlation_id: prompt.prompt.provenance.correlation_id,
        dismiss_reason: prompt.dismiss_reason,
    }
}

fn privacy_classification_label(classification: IntegrationPrivacyClassification) -> &'static str {
    match classification {
        IntegrationPrivacyClassification::DeviceLocal => "device_local",
        IntegrationPrivacyClassification::DerivedSummary => "derived_summary",
        IntegrationPrivacyClassification::UserApprovedAttachment => "user_approved_attachment",
    }
}

fn prompt_category_label(category: &ProactivePromptCategory) -> &'static str {
    match category {
        ProactivePromptCategory::Insight => "insight",
        ProactivePromptCategory::Task => "task",
        ProactivePromptCategory::Reminder => "reminder",
        ProactivePromptCategory::Escalation => "escalation",
    }
}

fn prompt_priority_label(priority: &ProactivePromptPriority) -> &'static str {
    match priority {
        ProactivePromptPriority::Low => "low",
        ProactivePromptPriority::Medium => "medium",
        ProactivePromptPriority::High => "high",
        ProactivePromptPriority::Critical => "critical",
    }
}

fn inbox_status_label(status: &IntegrationInboxItemStatus) -> &'static str {
    match status {
        IntegrationInboxItemStatus::Pending => "pending",
        IntegrationInboxItemStatus::Acknowledged => "acknowledged",
        IntegrationInboxItemStatus::Dismissed => "dismissed",
        IntegrationInboxItemStatus::Expired => "expired",
    }
}
