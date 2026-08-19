//! Authenticated assignment-email-draft transport port (#9627).
//!
//! Actor and organization are intentionally absent. The adapter owns the
//! bearer, and the server resolves both identities from it.

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::assignment_email_draft::AssignmentEmailDraft;

#[async_trait]
pub trait AssignmentEmailDraftClient: Send + Sync {
    async fn generate(
        &self,
        assignment_receipt_id: &str,
    ) -> Result<AssignmentEmailDraft, CoreError>;
    async fn load(&self, draft_id: &str) -> Result<AssignmentEmailDraft, CoreError>;
    async fn regenerate(
        &self,
        draft_id: &str,
        assignment_receipt_id: &str,
    ) -> Result<AssignmentEmailDraft, CoreError>;
}
