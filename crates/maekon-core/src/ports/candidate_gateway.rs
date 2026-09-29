//! Separately authorized Gateway transport, account evidence and durable audit.

use super::candidate_assessment::CandidateAttemptGuard;
use crate::error::CoreError;
use crate::models::candidate_decision::{DecisionText, DecisionUnavailable, SuitabilityEvidence};
use crate::models::candidate_gateway::{
    GatewayAuditRecord, GatewayAuthority, GatewayChoice, GatewayFailure, GatewayResponse,
};
use async_trait::async_trait;

/// Implemented only by trusted account/configuration observers. The default
/// product has no issuer; a public catalog or API-key string is insufficient.
#[async_trait]
pub trait GatewayAuthorityPort: Send + Sync {
    async fn observe(&self) -> Result<GatewayAuthority, DecisionUnavailable>;
}

#[async_trait]
pub trait GatewayCandidateBackendPort: Send + Sync {
    async fn choose(
        &self,
        text: &DecisionText,
        key: &str,
        guard: &dyn CandidateAttemptGuard,
    ) -> Result<GatewayResponse<GatewayChoice>, GatewayFailure>;
    async fn assess_selected(
        &self,
        text: &DecisionText,
        selected: &str,
        key: &str,
        guard: &dyn CandidateAttemptGuard,
    ) -> Result<GatewayResponse<SuitabilityEvidence>, GatewayFailure>;
}

/// Success requires a durable metadata-only commit, never an in-memory enqueue.
#[async_trait]
pub trait GatewayCandidateAuditPort: Send + Sync {
    async fn record_gateway(&self, record: GatewayAuditRecord) -> Result<(), CoreError>;
}
