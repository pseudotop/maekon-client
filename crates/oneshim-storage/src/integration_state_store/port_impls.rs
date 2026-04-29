use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use oneshim_core::error::CoreError;
use oneshim_core::models::integration::{
    IntegrationAckCursor, IntegrationEnvelope, IntegrationInboxItemStatus,
    IntegrationInsightAuditRecord, IntegrationOutboundPayload, IntegrationPromptReceipt,
    IntegrationSessionState, QueuedIntegrationEgressMessage, StoredProactivePrompt,
};
use oneshim_core::ports::integration::{
    IntegrationAuditPort, IntegrationCheckpointStorePort, IntegrationInboxStorePort,
    IntegrationOutboxPort, IntegrationPromptReceiptStorePort, IntegrationSessionStorePort,
};

use super::inner::FileIntegrationStateInner;

#[derive(Clone)]
pub struct FileIntegrationSessionStore {
    pub(super) inner: Arc<FileIntegrationStateInner>,
}

#[async_trait]
impl IntegrationSessionStorePort for FileIntegrationSessionStore {
    async fn load(&self) -> Result<Option<IntegrationSessionState>, CoreError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            Ok::<_, crate::error::StorageError>(inner.load_session_sync())
        })
        .await
        .map_err(|err| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("spawn_blocking: {err}"),
        })?
        .map_err(Into::into)
    }

    async fn store(&self, state: IntegrationSessionState) -> Result<(), CoreError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || inner.store_session_sync(state))
            .await
            .map_err(|err| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("spawn_blocking: {err}"),
            })?
            .map_err(Into::into)
    }

    async fn clear(&self) -> Result<(), CoreError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || inner.clear_session_sync())
            .await
            .map_err(|err| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("spawn_blocking: {err}"),
            })?
            .map_err(Into::into)
    }
}

#[derive(Clone)]
pub struct FileIntegrationOutboxStore {
    pub(super) inner: Arc<FileIntegrationStateInner>,
}

#[async_trait]
impl IntegrationOutboxPort for FileIntegrationOutboxStore {
    async fn enqueue_message(
        &self,
        envelope: IntegrationEnvelope,
        payload: IntegrationOutboundPayload,
    ) -> Result<String, CoreError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || inner.enqueue_outbox_sync(envelope, payload))
            .await
            .map_err(|err| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("spawn_blocking: {err}"),
            })?
            .map_err(Into::into)
    }

    async fn list_pending(
        &self,
        limit: usize,
    ) -> Result<Vec<QueuedIntegrationEgressMessage>, CoreError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            Ok::<_, crate::error::StorageError>(inner.list_outbox_sync(limit))
        })
        .await
        .map_err(|err| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("spawn_blocking: {err}"),
        })?
        .map_err(Into::into)
    }

    async fn delete(&self, queue_ids: &[String]) -> Result<(), CoreError> {
        let inner = self.inner.clone();
        let queue_ids = queue_ids.to_vec();
        tokio::task::spawn_blocking(move || inner.delete_outbox_sync(&queue_ids))
            .await
            .map_err(|err| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("spawn_blocking: {err}"),
            })?
            .map_err(Into::into)
    }

    async fn pending_count(&self) -> Result<usize, CoreError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            Ok::<_, crate::error::StorageError>(inner.outbox_pending_count_sync())
        })
        .await
        .map_err(|err| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("spawn_blocking: {err}"),
        })?
        .map_err(Into::into)
    }

    async fn last_ack_cursor(&self) -> Result<Option<IntegrationAckCursor>, CoreError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            Ok::<_, crate::error::StorageError>(inner.outbox_ack_cursor_sync())
        })
        .await
        .map_err(|err| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("spawn_blocking: {err}"),
        })?
        .map_err(Into::into)
    }

    async fn store_ack_cursor(&self, cursor: IntegrationAckCursor) -> Result<(), CoreError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || inner.store_outbox_ack_cursor_sync(cursor))
            .await
            .map_err(|err| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("spawn_blocking: {err}"),
            })?
            .map_err(Into::into)
    }
}

#[derive(Clone)]
pub struct FileIntegrationInboxStore {
    pub(super) inner: Arc<FileIntegrationStateInner>,
}

#[async_trait]
impl IntegrationPromptReceiptStorePort for FileIntegrationInboxStore {
    async fn record_prompt_receipt(
        &self,
        prompt_id: &str,
        envelope: IntegrationEnvelope,
        receipt: IntegrationPromptReceipt,
    ) -> Result<String, CoreError> {
        let inner = self.inner.clone();
        let prompt_id = prompt_id.to_string();
        tokio::task::spawn_blocking(move || {
            inner.record_prompt_receipt_sync(&prompt_id, envelope, receipt)
        })
        .await
        .map_err(|err| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("spawn_blocking: {err}"),
        })?
        .map_err(Into::into)
    }
}

#[async_trait]
impl IntegrationInboxStorePort for FileIntegrationInboxStore {
    async fn upsert_prompts(&self, prompts: Vec<StoredProactivePrompt>) -> Result<(), CoreError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || inner.upsert_inbox_sync(prompts))
            .await
            .map_err(|err| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("spawn_blocking: {err}"),
            })?
            .map_err(Into::into)
    }

    async fn list_pending(&self) -> Result<Vec<StoredProactivePrompt>, CoreError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            Ok::<_, crate::error::StorageError>(inner.list_inbox_pending_sync())
        })
        .await
        .map_err(|err| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("spawn_blocking: {err}"),
        })?
        .map_err(Into::into)
    }

    async fn list_unpresented(
        &self,
        limit: usize,
    ) -> Result<Vec<StoredProactivePrompt>, CoreError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            Ok::<_, crate::error::StorageError>(inner.list_inbox_unpresented_sync(limit))
        })
        .await
        .map_err(|err| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("spawn_blocking: {err}"),
        })?
        .map_err(Into::into)
    }

    async fn pending_count(&self) -> Result<usize, CoreError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            Ok::<_, crate::error::StorageError>(inner.inbox_pending_count_sync())
        })
        .await
        .map_err(|err| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("spawn_blocking: {err}"),
        })?
        .map_err(Into::into)
    }

    async fn mark_presented(
        &self,
        prompt_id: &str,
        presented_at: chrono::DateTime<Utc>,
    ) -> Result<(), CoreError> {
        let inner = self.inner.clone();
        let prompt_id = prompt_id.to_string();
        tokio::task::spawn_blocking(move || inner.mark_presented_sync(&prompt_id, presented_at))
            .await
            .map_err(|err| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("spawn_blocking: {err}"),
            })?
            .map_err(Into::into)
    }

    async fn update_status(
        &self,
        prompt_id: &str,
        status: IntegrationInboxItemStatus,
        reason: Option<String>,
    ) -> Result<(), CoreError> {
        let inner = self.inner.clone();
        let prompt_id = prompt_id.to_string();
        tokio::task::spawn_blocking(move || {
            inner.update_inbox_status_sync(&prompt_id, status, reason)
        })
        .await
        .map_err(|err| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("spawn_blocking: {err}"),
        })?
        .map_err(Into::into)
    }

    async fn expire_stale(&self) -> Result<usize, CoreError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || inner.expire_inbox_sync())
            .await
            .map_err(|err| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("spawn_blocking: {err}"),
            })?
            .map_err(Into::into)
    }

    async fn last_ack_cursor(&self) -> Result<Option<IntegrationAckCursor>, CoreError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            Ok::<_, crate::error::StorageError>(inner.inbox_ack_cursor_sync())
        })
        .await
        .map_err(|err| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("spawn_blocking: {err}"),
        })?
        .map_err(Into::into)
    }

    async fn store_ack_cursor(&self, cursor: IntegrationAckCursor) -> Result<(), CoreError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || inner.store_inbox_ack_cursor_sync(cursor))
            .await
            .map_err(|err| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("spawn_blocking: {err}"),
            })?
            .map_err(Into::into)
    }
}

#[derive(Clone)]
pub struct FileIntegrationAuditStore {
    pub(super) inner: Arc<FileIntegrationStateInner>,
}

#[async_trait]
impl IntegrationAuditPort for FileIntegrationAuditStore {
    async fn record_insight_decision(
        &self,
        record: IntegrationInsightAuditRecord,
    ) -> Result<(), CoreError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || inner.record_audit_sync(record))
            .await
            .map_err(|err| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("spawn_blocking: {err}"),
            })?
            .map_err(Into::into)
    }

    async fn recent_insight_decisions(
        &self,
        limit: usize,
    ) -> Result<Vec<IntegrationInsightAuditRecord>, CoreError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            Ok::<_, crate::error::StorageError>(inner.recent_audit_sync(limit))
        })
        .await
        .map_err(|err| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("spawn_blocking: {err}"),
        })?
        .map_err(Into::into)
    }
}

#[derive(Clone)]
pub struct FileIntegrationCheckpointStore {
    pub(super) inner: Arc<FileIntegrationStateInner>,
}

#[async_trait]
impl IntegrationCheckpointStorePort for FileIntegrationCheckpointStore {
    async fn load_checkpoint(&self, namespace: &str) -> Result<Option<String>, CoreError> {
        let inner = self.inner.clone();
        let namespace = namespace.to_string();
        tokio::task::spawn_blocking(move || {
            Ok::<_, crate::error::StorageError>(inner.load_checkpoint_sync(&namespace))
        })
        .await
        .map_err(|err| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("spawn_blocking: {err}"),
        })?
        .map_err(Into::into)
    }

    async fn store_checkpoint(&self, namespace: &str, cursor: String) -> Result<(), CoreError> {
        let inner = self.inner.clone();
        let namespace = namespace.to_string();
        tokio::task::spawn_blocking(move || inner.store_checkpoint_sync(&namespace, cursor))
            .await
            .map_err(|err| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("spawn_blocking: {err}"),
            })?
            .map_err(Into::into)
    }
}
