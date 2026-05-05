//! Integration outbound egress ports, policy, and audit.
//!
//! # Errors (all traits in this module)
//! - `CoreError::Storage` (wire: `storage.failed`) — outbox enqueue /
//!   list_pending / delete / cursor persistence failure (SQLite-backed
//!   outbox), audit record persistence in `IntegrationAuditPort`.
//! - `CoreError::Network` (wire: `network.generic`),
//!   `CoreError::RequestTimeout`, `CoreError::RateLimit`,
//!   `CoreError::ServiceUnavailable` — `flush` delegating to the
//!   integration transport follows the canonical HTTP semantic mapping.
//! - `CoreError::Auth` (wire: `auth.failed`) — outbound transport
//!   auth rejection surfaces through `flush`.
//! - Policy denial is NOT an error — `authorize_insight` returns
//!   `Ok(IntegrationEgressDecision::deny(reason))` rather than Err.
//!   Policy engine failure (e.g., rule-load I/O) surfaces as
//!   `CoreError::Io` (wire: `internal.io`) or `CoreError::Internal`.
//! - Empty queue / no pending cursor = `Ok(0)` / `Ok(None)` / `Ok(vec![])`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::error::CoreError;
use crate::models::integration::{
    InsightPacket, IntegrationAckCursor, IntegrationEgressDisposition, IntegrationEnvelope,
    IntegrationInsightAuditRecord, IntegrationOutboundPayload, QueuedIntegrationEgressMessage,
};

#[async_trait]
pub trait IntegrationEgressPort: Send + Sync {
    /// Queue a typed outbound integration message for delivery.
    async fn enqueue_message(
        &self,
        envelope: IntegrationEnvelope,
        payload: IntegrationOutboundPayload,
    ) -> Result<(), CoreError>;

    /// Queue a privacy-filtered outbound insight packet.
    async fn enqueue_insight(
        &self,
        envelope: IntegrationEnvelope,
        packet: InsightPacket,
    ) -> Result<(), CoreError> {
        self.enqueue_message(envelope, IntegrationOutboundPayload::Insight(packet))
            .await
    }

    /// Flush queued outbound messages to the remote integration backend.
    async fn flush(&self) -> Result<usize, CoreError>;

    /// Read the latest acknowledged cursor returned by the remote side.
    async fn last_ack_cursor(&self) -> Result<Option<IntegrationAckCursor>, CoreError>;
}

#[async_trait]
pub trait IntegrationOutboxPort: Send + Sync {
    /// Persist an outbound integration message before transport delivery.
    async fn enqueue_message(
        &self,
        envelope: IntegrationEnvelope,
        payload: IntegrationOutboundPayload,
    ) -> Result<String, CoreError>;

    /// List pending outbound messages in delivery order.
    async fn list_pending(
        &self,
        limit: usize,
    ) -> Result<Vec<QueuedIntegrationEgressMessage>, CoreError>;

    /// Count currently pending outbound packets.
    async fn pending_count(&self) -> Result<usize, CoreError>;

    /// Remove acknowledged or successfully delivered queue items.
    async fn delete(&self, queue_ids: &[String]) -> Result<(), CoreError>;

    /// Read the latest acknowledged cursor returned by the remote side.
    async fn last_ack_cursor(&self) -> Result<Option<IntegrationAckCursor>, CoreError>;

    /// Persist the latest acknowledged cursor from the remote side.
    async fn store_ack_cursor(&self, cursor: IntegrationAckCursor) -> Result<(), CoreError>;
}

#[async_trait]
pub trait IntegrationEgressSignalPort: Send + Sync {
    /// Wait for a local outbound egress signal, returning `true` when a signal
    /// was observed before the timeout expires.
    async fn wait_for_pending_egress(&self, timeout: Duration) -> Result<bool, CoreError>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationEgressDecision {
    pub disposition: IntegrationEgressDisposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub audit_required: bool,
}

impl IntegrationEgressDecision {
    pub fn allow() -> Self {
        Self {
            disposition: IntegrationEgressDisposition::Allow,
            reason: None,
            audit_required: true,
        }
    }

    pub fn deny(reason: impl Into<String>) -> Self {
        Self {
            disposition: IntegrationEgressDisposition::Deny,
            reason: Some(reason.into()),
            audit_required: true,
        }
    }

    pub fn require_user_approval(reason: impl Into<String>) -> Self {
        Self {
            disposition: IntegrationEgressDisposition::RequireUserApproval,
            reason: Some(reason.into()),
            audit_required: true,
        }
    }
}

#[async_trait]
pub trait IntegrationEgressPolicyPort: Send + Sync {
    /// Evaluate whether an outbound insight packet may leave the device.
    async fn authorize_insight(
        &self,
        envelope: &IntegrationEnvelope,
        packet: &InsightPacket,
    ) -> Result<IntegrationEgressDecision, CoreError>;
}

#[async_trait]
pub trait IntegrationAuditPort: Send + Sync {
    /// Persist an auditable record for an outbound integration insight decision.
    async fn record_insight_decision(
        &self,
        record: IntegrationInsightAuditRecord,
    ) -> Result<(), CoreError>;

    /// Read recent auditable insight decisions in reverse chronological order.
    async fn recent_insight_decisions(
        &self,
        limit: usize,
    ) -> Result<Vec<IntegrationInsightAuditRecord>, CoreError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn egress_decision_builders_cover_all_dispositions() {
        let allow = IntegrationEgressDecision::allow();
        assert_eq!(allow.disposition, IntegrationEgressDisposition::Allow);
        assert!(allow.audit_required);

        let deny = IntegrationEgressDecision::deny("policy denied");
        assert_eq!(deny.disposition, IntegrationEgressDisposition::Deny);
        assert_eq!(deny.reason.as_deref(), Some("policy denied"));

        let approval = IntegrationEgressDecision::require_user_approval("needs consent");
        assert_eq!(
            approval.disposition,
            IntegrationEgressDisposition::RequireUserApproval
        );
        assert_eq!(approval.reason.as_deref(), Some("needs consent"));
    }
}
