//! Integration inbound inbox ports — prompt delivery, lifecycle, and signaling.
//!
//! # Errors (all traits in this module)
//! - `CoreError::Network` / `CoreError::RequestTimeout` /
//!   `CoreError::RateLimit` / `CoreError::ServiceUnavailable` /
//!   `CoreError::Auth` — `refresh` and `acknowledge`/`dismiss` delegate
//!   to the transport and follow the canonical HTTP semantic mapping.
//! - `CoreError::Storage` (wire: `storage.failed`) — all local
//!   persistence methods (upsert_prompts, list_pending/unpresented,
//!   pending_count, mark_presented, update_status, expire_stale,
//!   ack-cursor load/store, record_prompt_receipt).
//! - `CoreError::NotFound` (wire: `resource.not_found`,
//!   `resource_type = "prompt"`) — `acknowledge`/`dismiss`/
//!   `mark_presented`/`update_status` with an unknown `prompt_id`.
//!   Some adapters treat unknown id as rowcount=0 no-op (Ok(())); check
//!   the specific impl for behavior.
//! - `CoreError::Internal` (wire: `internal.generic`) — OS-level
//!   notification presentation failure in `IntegrationPromptPresenterPort`
//!   (desktop notifier backend unavailable / stale handle).

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::time::Duration;

use crate::error::CoreError;
use crate::models::integration::{
    IntegrationAckCursor, IntegrationEnvelope, IntegrationInboxItemStatus,
    IntegrationPromptReceipt, StoredProactivePrompt,
};

#[async_trait]
pub trait IntegrationInboxPort: Send + Sync {
    /// Pull new inbound prompts/tasks from the integration backend.
    async fn refresh(&self) -> Result<usize, CoreError>;

    /// List pending inbound proactive prompts/tasks.
    async fn list_pending(&self) -> Result<Vec<StoredProactivePrompt>, CoreError>;

    /// Acknowledge receipt of a prompt/task.
    async fn acknowledge(&self, prompt_id: &str) -> Result<(), CoreError>;

    /// Dismiss a prompt/task with an optional local reason.
    async fn dismiss(&self, prompt_id: &str, reason: Option<String>) -> Result<(), CoreError>;

    /// Read the latest acknowledged inbox cursor returned by the remote side.
    async fn last_ack_cursor(&self) -> Result<Option<IntegrationAckCursor>, CoreError>;
}

#[async_trait]
pub trait IntegrationInboxStorePort: Send + Sync {
    /// Upsert inbound prompts/tasks received from the remote side.
    ///
    /// Implementations must treat `prompt_id` as a durable identity and avoid
    /// resetting local lifecycle state (for example, acknowledged or dismissed)
    /// when the same prompt is redelivered by the remote side.
    async fn upsert_prompts(&self, prompts: Vec<StoredProactivePrompt>) -> Result<(), CoreError>;

    /// List locally pending prompts/tasks.
    async fn list_pending(&self) -> Result<Vec<StoredProactivePrompt>, CoreError>;

    /// List locally pending prompts/tasks that have not yet been presented.
    async fn list_unpresented(&self, limit: usize)
        -> Result<Vec<StoredProactivePrompt>, CoreError>;

    /// Count currently pending prompts/tasks.
    async fn pending_count(&self) -> Result<usize, CoreError>;

    /// Mark a prompt/task as presented to the local user experience.
    async fn mark_presented(
        &self,
        prompt_id: &str,
        presented_at: DateTime<Utc>,
    ) -> Result<(), CoreError>;

    /// Update the lifecycle state of a stored prompt/task.
    async fn update_status(
        &self,
        prompt_id: &str,
        status: IntegrationInboxItemStatus,
        reason: Option<String>,
    ) -> Result<(), CoreError>;

    /// Remove or mark expired prompts whose expiration time has passed.
    async fn expire_stale(&self) -> Result<usize, CoreError>;

    /// Read the latest acknowledged cursor returned by the remote side.
    async fn last_ack_cursor(&self) -> Result<Option<IntegrationAckCursor>, CoreError>;

    /// Persist the latest acknowledged cursor from the remote side.
    async fn store_ack_cursor(&self, cursor: IntegrationAckCursor) -> Result<(), CoreError>;
}

#[async_trait]
pub trait IntegrationInboxSignalPort: Send + Sync {
    /// Wait for a remote inbox signal, returning `true` when a signal was
    /// observed before the timeout expires.
    async fn wait_for_remote_prompt_signal(&self, timeout: Duration) -> Result<bool, CoreError>;
}

#[async_trait]
pub trait IntegrationPromptReceiptStorePort: Send + Sync {
    /// Persist a local inbox lifecycle transition and its corresponding outbound
    /// prompt receipt message atomically.
    async fn record_prompt_receipt(
        &self,
        prompt_id: &str,
        envelope: IntegrationEnvelope,
        receipt: IntegrationPromptReceipt,
    ) -> Result<String, CoreError>;
}

#[async_trait]
pub trait IntegrationPromptPresenterPort: Send + Sync {
    /// Present a proactive prompt to the local desktop user experience.
    async fn present_prompt(&self, prompt: &StoredProactivePrompt) -> Result<(), CoreError>;
}
