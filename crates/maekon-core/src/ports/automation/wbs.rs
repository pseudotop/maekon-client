//! Operator-initiated WBS interaction. Raw cell addresses and names are never inputs.

use crate::models::automation::wbs::*;
use async_trait::async_trait;

#[async_trait]
pub trait WbsAutomationPort: Send + Sync {
    async fn offer_document_consent(&self) -> Result<WbsConsentOffer, WbsAutomationError>;
    async fn open_session(
        &self,
        acceptance: WbsConsentAcceptance,
    ) -> Result<WbsSessionView, WbsAutomationError>;
    async fn recommend(
        &self,
        auth: WbsSessionAuthorization,
    ) -> Result<WbsRecommendationView, WbsAutomationError>;
    async fn confirm_candidate(
        &self,
        auth: WbsSessionAuthorization,
        choice: WbsCandidateChoice,
    ) -> Result<WbsExecutionTicket, WbsAutomationError>;
    async fn execute(
        &self,
        auth: WbsSessionAuthorization,
        ticket: WbsExecutionTicket,
    ) -> Result<WbsExecutionView, WbsAutomationError>;
    async fn read_execution(
        &self,
        auth: WbsSessionAuthorization,
        operation: WbsOperationRef,
    ) -> Result<WbsExecutionView, WbsAutomationError>;
    async fn cancel_session(
        &self,
        auth: WbsSessionAuthorization,
    ) -> Result<WbsSessionCancellation, WbsAutomationError>;
}
