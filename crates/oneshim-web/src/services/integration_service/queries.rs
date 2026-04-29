use oneshim_api_contracts::integration::{IntegrationAuditLogResponse, IntegrationStatus};
use tracing::warn;

use crate::services::integration_assembler::{
    map_ack_cursor_summary, map_audit_record, map_session_summary,
};
use crate::services::web_contexts::IntegrationWebContext;

use super::{INTEGRATION_AUDIT_SCHEMA_VERSION, INTEGRATION_STATUS_SCHEMA_VERSION};

#[derive(Clone)]
pub struct IntegrationStatusQueryService {
    pub(super) ctx: IntegrationWebContext,
}

impl IntegrationStatusQueryService {
    pub fn new(ctx: IntegrationWebContext) -> Self {
        Self { ctx }
    }

    pub async fn build_status(&self) -> IntegrationStatus {
        let mut outbound_runtime = self.ctx.runtime_status_seed.clone();
        self.ctx
            .config
            .apply_to_runtime_status(&mut outbound_runtime);

        if let Some(auth_port) = self.ctx.auth.as_ref() {
            match auth_port.current_auth_status().await {
                Ok(auth_status) => {
                    outbound_runtime.auth_material_available = auth_status.authenticated;
                    outbound_runtime.auth_status = Some(auth_status);
                }
                Err(error) => {
                    warn!(error = %error, "failed to read integration auth status");
                }
            }
        }

        if let Some(session_port) = self.ctx.session.as_ref() {
            match session_port.current_session().await {
                Ok(Some(current_session)) => {
                    outbound_runtime.current_session = Some(map_session_summary(current_session));
                }
                Ok(None) => {}
                Err(error) => {
                    warn!(error = %error, "failed to read integration session state");
                }
            }
        }

        if let Some(outbox) = self.ctx.outbox.as_ref() {
            match outbox.pending_count().await {
                Ok(count) => outbound_runtime.outbox_pending_count = Some(count),
                Err(error) => warn!(error = %error, "failed to read integration outbox count"),
            }
            match outbox.last_ack_cursor().await {
                Ok(cursor) => {
                    outbound_runtime.outbox_ack_cursor = cursor.map(map_ack_cursor_summary)
                }
                Err(error) => warn!(error = %error, "failed to read integration outbox cursor"),
            }
        }

        if let Some(inbox_store) = self.ctx.inbox_store.as_ref() {
            match inbox_store.pending_count().await {
                Ok(count) => outbound_runtime.inbox_pending_count = Some(count),
                Err(error) => warn!(error = %error, "failed to read integration inbox count"),
            }
            match inbox_store.last_ack_cursor().await {
                Ok(cursor) => {
                    outbound_runtime.inbox_ack_cursor = cursor.map(map_ack_cursor_summary)
                }
                Err(error) => warn!(error = %error, "failed to read integration inbox cursor"),
            }
        }

        if let Some(telemetry_port) = self.ctx.telemetry.as_ref() {
            match telemetry_port.snapshot().await {
                Ok(telemetry) => outbound_runtime.runtime_telemetry = Some(telemetry),
                Err(error) => {
                    warn!(error = %error, "failed to read integration runtime telemetry")
                }
            }
        }

        IntegrationStatus {
            schema_version: INTEGRATION_STATUS_SCHEMA_VERSION.to_string(),
            external_access_enabled: self.ctx.config.external_access_enabled,
            automation_controller_configured: self.ctx.automation_controller_configured,
            ai_runtime_status: self.ctx.ai_runtime_status.clone(),
            outbound_runtime,
        }
    }
}

#[derive(Clone)]
pub struct IntegrationAuditQueryService {
    pub(super) ctx: IntegrationWebContext,
}

impl IntegrationAuditQueryService {
    pub fn new(ctx: IntegrationWebContext) -> Self {
        Self { ctx }
    }

    pub async fn build_audit_log(&self, limit: usize) -> IntegrationAuditLogResponse {
        let records = if let Some(audit) = self.ctx.audit.as_ref() {
            match audit.recent_insight_decisions(limit).await {
                Ok(records) => records.into_iter().map(map_audit_record).collect(),
                Err(error) => {
                    warn!(error = %error, "failed to read integration audit records");
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };

        IntegrationAuditLogResponse {
            schema_version: INTEGRATION_AUDIT_SCHEMA_VERSION.to_string(),
            records,
        }
    }
}
