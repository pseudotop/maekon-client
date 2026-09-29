//! Provider-neutral advisory and local inference boundaries.

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::candidate_assessment::{
    AssessmentAuditRecord, CandidateAssessmentResult, LocalAssessmentAnswer, LocalModelApproval,
};
use crate::models::candidate_decision::{
    BackendFailure, CandidateDecisionRequest, DecisionText, DecisionUnavailable,
};

#[async_trait]
pub trait CandidateAssessmentPort: Send + Sync {
    async fn assess(&self, request: &CandidateDecisionRequest) -> CandidateAssessmentResult;
}

/// Implemented by trusted composition. Transports recheck between their awaits;
/// an adapter never turns a successful policy check into a reusable egress token.
#[async_trait]
pub trait CandidateAttemptGuard: Send + Sync {
    async fn checkpoint(&self) -> Result<(), DecisionUnavailable>;
}

/// Local model transport only. No automatic pull, fallback or account discovery.
#[async_trait]
pub trait LocalCandidateModelPort: Send + Sync {
    async fn infer(
        &self,
        text: &DecisionText,
        approval: &LocalModelApproval,
        guard: &dyn CandidateAttemptGuard,
    ) -> Result<LocalAssessmentAnswer, BackendFailure>;
}

/// Success means a durable commit, not an in-memory enqueue.
#[async_trait]
pub trait CandidateAssessmentAuditPort: Send + Sync {
    async fn record_assessment(&self, entry: AssessmentAuditRecord) -> Result<(), CoreError>;
}
