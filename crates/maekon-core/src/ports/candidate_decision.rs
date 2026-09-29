//! Async advisory decision, transport and durable audit boundaries.

use crate::error::CoreError;
use crate::models::candidate_decision::{
    BackendFailure, BackendResponse, CandidateDecisionRequest, CandidateDecisionResult,
    ChoiceEvidence, DecisionAuditRecord, DecisionText, SuitabilityEvidence,
};
use async_trait::async_trait;

#[async_trait]
pub trait CandidateDecisionPort: Send + Sync {
    async fn decide(&self, request: &CandidateDecisionRequest) -> CandidateDecisionResult;
}

/// Two explicit stages let composition revalidate authority between HTTP awaits.
#[async_trait]
pub trait CandidateDecisionBackendPort: Send + Sync {
    async fn choose(
        &self,
        text: &DecisionText,
        api_key: &str,
    ) -> Result<BackendResponse<ChoiceEvidence>, BackendFailure>;
    async fn assess_selected(
        &self,
        text: &DecisionText,
        selected: &str,
        api_key: &str,
    ) -> Result<BackendResponse<SuitabilityEvidence>, BackendFailure>;
}

/// Success means a durable commit, not acceptance by an in-memory queue.
#[async_trait]
pub trait CandidateDecisionAuditPort: Send + Sync {
    async fn record(&self, entry: DecisionAuditRecord) -> Result<(), CoreError>;
}
