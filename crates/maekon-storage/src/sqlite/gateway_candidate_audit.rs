//! Validated metadata and synced commits for the optional Gateway runtime.

use super::SqliteStorage;
use async_trait::async_trait;
use chrono::Utc;
use maekon_core::error::CoreError;
use maekon_core::error_codes::StorageCode;
use maekon_core::models::audit::{AuditEntry, AuditStatus};
use maekon_core::models::candidate_gateway::{
    gateway_digest_valid, GatewayAuditPhase, GatewayAuditRecord, GatewaySendState, GATEWAY_SCHEMA,
};
use maekon_core::ports::candidate_gateway::GatewayCandidateAuditPort;
use std::sync::Arc;

pub struct SqliteGatewayCandidateAudit {
    storage: Arc<SqliteStorage>,
}

impl SqliteGatewayCandidateAudit {
    pub fn new(storage: Arc<SqliteStorage>) -> Self {
        Self { storage }
    }
}

fn unavailable() -> CoreError {
    CoreError::Storage {
        code: StorageCode::Failed,
        message: "Gateway candidate audit commit unavailable".into(),
    }
}

fn valid_metadata(record: &GatewayAuditRecord) -> bool {
    let attempt = &record.attempt;
    [
        record.schema == GATEWAY_SCHEMA,
        !record.id.is_nil(),
        !record.decision_id.is_nil(),
        !attempt.id.is_nil(),
        gateway_digest_valid(&record.namespace_hash),
        attempt
            .request_hash
            .as_deref()
            .is_none_or(gateway_digest_valid),
        attempt
            .observation
            .as_ref()
            .is_none_or(|value| value.validate().is_ok()),
    ]
    .into_iter()
    .all(|valid| valid)
}

fn valid_phase(record: &GatewayAuditRecord) -> bool {
    let attempt = &record.attempt;
    match record.phase {
        GatewayAuditPhase::BeforeAttempt => [
            attempt.send_state == GatewaySendState::Pending,
            attempt.elapsed_ms == 0,
            attempt.request_hash.is_none(),
            attempt.observation.is_none(),
            attempt.estimated_cost_picos.is_none(),
            attempt.failure.is_none(),
        ]
        .into_iter()
        .all(|valid| valid),
        GatewayAuditPhase::AfterAttempt => match attempt.send_state {
            GatewaySendState::Pending => false,
            GatewaySendState::NotSent => [
                attempt.failure.is_some(),
                attempt.observation.is_none(),
                attempt.estimated_cost_picos.is_none(),
            ]
            .into_iter()
            .all(|valid| valid),
            // A failed attempt can have incomplete evidence or a real overrun.
            // Never erase observed costs merely because they exceed a reservation.
            GatewaySendState::Attempted => {
                attempt.failure.is_some()
                    || [
                        attempt.request_hash.is_some(),
                        attempt.observation.is_some(),
                        attempt.estimated_cost_picos.is_some(),
                    ]
                    .into_iter()
                    .all(|valid| valid)
            }
        },
    }
}

#[async_trait]
impl GatewayCandidateAuditPort for SqliteGatewayCandidateAudit {
    async fn record_gateway(&self, record: GatewayAuditRecord) -> Result<(), CoreError> {
        if !valid_metadata(&record) || !valid_phase(&record) {
            return Err(unavailable());
        }
        // All variable text is a bounded digest, pinned identity or exact decimal.
        let details = serde_json::to_string(&record).map_err(|_| unavailable())?;
        let entry = AuditEntry {
            entry_id: record.id.to_string(),
            timestamp: Utc::now(),
            session_id: record.decision_id.to_string(),
            command_id: "candidate-gateway".into(),
            action_type: "candidate_gateway.advisory".into(),
            status: match record.phase {
                GatewayAuditPhase::BeforeAttempt => AuditStatus::Started,
                GatewayAuditPhase::AfterAttempt if record.attempt.failure.is_some() => {
                    AuditStatus::Failed
                }
                GatewayAuditPhase::AfterAttempt => AuditStatus::Completed,
            },
            details: Some(details),
            execution_time_ms: Some(record.attempt.elapsed_ms),
        };
        let storage = self.storage.clone();
        let inserted =
            tokio::task::spawn_blocking(move || storage.try_save_durable_audit_entry(&entry))
                .await
                .map_err(|_| unavailable())?
                .map_err(|_| unavailable())?;
        if !inserted {
            return Err(unavailable());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "gateway_candidate_audit_tests.rs"]
mod tests;
