use std::path::PathBuf;

use chrono::Utc;
use oneshim_core::models::integration::{
    IntegrationAckCursor, IntegrationEnvelope, IntegrationInboxItemStatus,
    IntegrationInsightAuditRecord, IntegrationOutboundPayload, IntegrationPromptReceipt,
    IntegrationPromptReceiptAction, IntegrationSessionState, QueuedIntegrationEgressMessage,
    StoredProactivePrompt,
};
use uuid::Uuid;

use crate::error::StorageError;

use super::{FileIntegrationStateRegistry, IntegrationStateStorePolicy, MAX_AUDIT_RECORDS};

pub(super) struct FileIntegrationStateInner {
    registry_path: PathBuf,
    policy: IntegrationStateStorePolicy,
    registry: parking_lot::Mutex<FileIntegrationStateRegistry>,
}

// P2 PR-A Category B: all write methods in this impl intentionally hold the
// `registry` mutex across `save_registry` (disk I/O: `create_dir_all` +
// atomic `write` + `rename`). The lock is the atomicity guard for "mutate
// in-memory state + write file to disk" — tightening it would allow
// in-memory state and on-disk state to diverge under concurrent writers.
// parking_lot::Mutex keeps contention cost low (I/O window is tens of ms).
// See `docs/reviews/2026-04-21-p2-significant-drop-tightening-spec.md`
// §Category B for the full rationale.
#[allow(clippy::significant_drop_tightening)]
impl FileIntegrationStateInner {
    pub(super) fn new(
        registry_path: PathBuf,
        policy: IntegrationStateStorePolicy,
    ) -> Result<Self, StorageError> {
        if let Some(parent) = registry_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let registry = FileIntegrationStateRegistry::load_or_default(&registry_path)?;
        Ok(Self {
            registry_path,
            policy,
            registry: parking_lot::Mutex::new(registry),
        })
    }

    fn redact_prompt_body_if_needed(
        &self,
        prompt: &mut StoredProactivePrompt,
        status: IntegrationInboxItemStatus,
    ) {
        if self.policy.redact_completed_prompt_bodies
            && status != IntegrationInboxItemStatus::Pending
        {
            prompt.prompt.body.clear();
        }
    }

    fn prune_inbox_locked(&self, registry: &mut FileIntegrationStateRegistry) {
        let max_stored_prompts = self.policy.max_stored_prompts.max(1);
        if registry.inbox.len() <= max_stored_prompts {
            return;
        }

        let overflow = registry.inbox.len() - max_stored_prompts;
        let mut candidates: Vec<_> = registry
            .inbox
            .values()
            .map(|prompt| {
                (
                    prompt.status == IntegrationInboxItemStatus::Pending,
                    prompt.received_at,
                    prompt.prompt.prompt_id.clone(),
                )
            })
            .collect();
        candidates.sort_by_key(|entry| (entry.0, entry.1));

        for (_, _, prompt_id) in candidates.into_iter().take(overflow) {
            registry.inbox.remove(&prompt_id);
        }
    }

    fn save_registry(&self, registry: &FileIntegrationStateRegistry) -> Result<(), StorageError> {
        registry.save(&self.registry_path)
    }

    pub(super) fn load_session_sync(&self) -> Option<IntegrationSessionState> {
        self.registry.lock().session.clone()
    }

    pub(super) fn store_session_sync(
        &self,
        state: IntegrationSessionState,
    ) -> Result<(), StorageError> {
        let mut registry = self.registry.lock();
        registry.session = Some(state);
        self.save_registry(&registry)
    }

    pub(super) fn clear_session_sync(&self) -> Result<(), StorageError> {
        let mut registry = self.registry.lock();
        registry.session = None;
        self.save_registry(&registry)
    }

    pub(super) fn enqueue_outbox_sync(
        &self,
        envelope: IntegrationEnvelope,
        payload: IntegrationOutboundPayload,
    ) -> Result<String, StorageError> {
        let mut registry = self.registry.lock();
        let queue_id = format!("integration_queue_{}", Uuid::new_v4());
        registry.outbox.push(QueuedIntegrationEgressMessage {
            queue_id: queue_id.clone(),
            envelope,
            payload,
            queued_at: Utc::now(),
        });
        self.save_registry(&registry)?;
        Ok(queue_id)
    }

    pub(super) fn list_outbox_sync(&self, limit: usize) -> Vec<QueuedIntegrationEgressMessage> {
        self.registry
            .lock()
            .outbox
            .iter()
            .take(limit)
            .cloned()
            .collect()
    }

    pub(super) fn outbox_pending_count_sync(&self) -> usize {
        self.registry.lock().outbox.len()
    }

    pub(super) fn delete_outbox_sync(&self, queue_ids: &[String]) -> Result<(), StorageError> {
        let mut registry = self.registry.lock();
        registry
            .outbox
            .retain(|item| !queue_ids.contains(&item.queue_id));
        self.save_registry(&registry)
    }

    pub(super) fn outbox_ack_cursor_sync(&self) -> Option<IntegrationAckCursor> {
        self.registry.lock().outbox_ack_cursor.clone()
    }

    pub(super) fn store_outbox_ack_cursor_sync(
        &self,
        cursor: IntegrationAckCursor,
    ) -> Result<(), StorageError> {
        let mut registry = self.registry.lock();
        registry.outbox_ack_cursor = Some(cursor);
        self.save_registry(&registry)
    }

    pub(super) fn upsert_inbox_sync(
        &self,
        prompts: Vec<StoredProactivePrompt>,
    ) -> Result<(), StorageError> {
        let mut registry = self.registry.lock();
        for prompt in prompts {
            if let Some(existing) = registry.inbox.get_mut(&prompt.prompt.prompt_id) {
                existing.prompt = prompt.prompt;
                existing.presented_at = existing.presented_at.or(prompt.presented_at);
            } else {
                registry
                    .inbox
                    .insert(prompt.prompt.prompt_id.clone(), prompt);
            }
        }
        self.prune_inbox_locked(&mut registry);
        self.save_registry(&registry)
    }

    pub(super) fn list_inbox_pending_sync(&self) -> Vec<StoredProactivePrompt> {
        let mut prompts: Vec<_> = self
            .registry
            .lock()
            .inbox
            .values()
            .filter(|prompt| prompt.status == IntegrationInboxItemStatus::Pending)
            .cloned()
            .collect();
        prompts.sort_by_key(|prompt| prompt.received_at);
        prompts
    }

    pub(super) fn list_inbox_unpresented_sync(&self, limit: usize) -> Vec<StoredProactivePrompt> {
        let mut prompts: Vec<_> = self
            .registry
            .lock()
            .inbox
            .values()
            .filter(|prompt| {
                prompt.status == IntegrationInboxItemStatus::Pending
                    && prompt.presented_at.is_none()
            })
            .cloned()
            .collect();
        prompts.sort_by_key(|prompt| prompt.received_at);
        prompts.truncate(limit);
        prompts
    }

    pub(super) fn inbox_pending_count_sync(&self) -> usize {
        self.registry
            .lock()
            .inbox
            .values()
            .filter(|prompt| prompt.status == IntegrationInboxItemStatus::Pending)
            .count()
    }

    pub(super) fn update_inbox_status_sync(
        &self,
        prompt_id: &str,
        status: IntegrationInboxItemStatus,
        reason: Option<String>,
    ) -> Result<(), StorageError> {
        let mut registry = self.registry.lock();
        let prompt = registry
            .inbox
            .get_mut(prompt_id)
            .ok_or_else(|| StorageError::NotFound {
                resource_type: "integration_prompt".to_string(),
                id: prompt_id.to_string(),
            })?;
        prompt.status = status.clone();
        prompt.status_updated_at = Utc::now();
        prompt.dismiss_reason = reason;
        self.redact_prompt_body_if_needed(prompt, status);
        self.save_registry(&registry)
    }

    pub(super) fn record_prompt_receipt_sync(
        &self,
        prompt_id: &str,
        envelope: IntegrationEnvelope,
        receipt: IntegrationPromptReceipt,
    ) -> Result<String, StorageError> {
        if receipt.prompt_id != prompt_id {
            return Err(StorageError::Validation {
                field: "integration.prompt_receipt.prompt_id".to_string(),
                message: "prompt receipt prompt_id does not match the stored prompt target"
                    .to_string(),
            });
        }

        let mut registry = self.registry.lock();
        let prompt = registry
            .inbox
            .get_mut(prompt_id)
            .ok_or_else(|| StorageError::NotFound {
                resource_type: "integration_prompt".to_string(),
                id: prompt_id.to_string(),
            })?;

        if prompt.status != IntegrationInboxItemStatus::Pending {
            return Err(StorageError::Validation {
                field: "integration.prompt_receipt.status".to_string(),
                message: format!(
                    "prompt receipts can only be recorded from the pending state, found {:?}",
                    prompt.status
                ),
            });
        }

        prompt.status = receipt.action.to_inbox_status();
        prompt.status_updated_at = receipt.occurred_at;
        prompt.dismiss_reason = match receipt.action {
            IntegrationPromptReceiptAction::Acknowledged => None,
            IntegrationPromptReceiptAction::Dismissed => receipt.reason.clone(),
        };
        self.redact_prompt_body_if_needed(prompt, prompt.status.clone());

        let queue_id = format!("integration_queue_{}", Uuid::new_v4());
        registry.outbox.push(QueuedIntegrationEgressMessage {
            queue_id: queue_id.clone(),
            envelope,
            payload: IntegrationOutboundPayload::PromptReceipt(receipt),
            queued_at: Utc::now(),
        });
        self.save_registry(&registry)?;
        Ok(queue_id)
    }

    pub(super) fn mark_presented_sync(
        &self,
        prompt_id: &str,
        presented_at: chrono::DateTime<Utc>,
    ) -> Result<(), StorageError> {
        let mut registry = self.registry.lock();
        let prompt = registry
            .inbox
            .get_mut(prompt_id)
            .ok_or_else(|| StorageError::NotFound {
                resource_type: "integration_prompt".to_string(),
                id: prompt_id.to_string(),
            })?;
        prompt.presented_at = Some(presented_at);
        self.save_registry(&registry)
    }

    pub(super) fn expire_inbox_sync(&self) -> Result<usize, StorageError> {
        let now = Utc::now();
        let mut expired = 0usize;
        let mut registry = self.registry.lock();
        for prompt in registry.inbox.values_mut() {
            if prompt.status == IntegrationInboxItemStatus::Pending
                && prompt
                    .prompt
                    .expires_at
                    .is_some_and(|expires_at| expires_at <= now)
            {
                prompt.status = IntegrationInboxItemStatus::Expired;
                prompt.status_updated_at = now;
                self.redact_prompt_body_if_needed(prompt, IntegrationInboxItemStatus::Expired);
                expired += 1;
            }
        }
        if expired > 0 {
            self.save_registry(&registry)?;
        }
        Ok(expired)
    }

    pub(super) fn inbox_ack_cursor_sync(&self) -> Option<IntegrationAckCursor> {
        self.registry.lock().inbox_ack_cursor.clone()
    }

    pub(super) fn store_inbox_ack_cursor_sync(
        &self,
        cursor: IntegrationAckCursor,
    ) -> Result<(), StorageError> {
        let mut registry = self.registry.lock();
        registry.inbox_ack_cursor = Some(cursor);
        self.save_registry(&registry)
    }

    pub(super) fn load_checkpoint_sync(&self, namespace: &str) -> Option<String> {
        self.registry
            .lock()
            .producer_checkpoints
            .get(namespace)
            .cloned()
    }

    pub(super) fn store_checkpoint_sync(
        &self,
        namespace: &str,
        cursor: String,
    ) -> Result<(), StorageError> {
        let mut registry = self.registry.lock();
        registry
            .producer_checkpoints
            .insert(namespace.to_string(), cursor);
        self.save_registry(&registry)
    }

    pub(super) fn record_audit_sync(
        &self,
        record: IntegrationInsightAuditRecord,
    ) -> Result<(), StorageError> {
        let mut registry = self.registry.lock();
        registry.audit_records.push(record);
        if registry.audit_records.len() > MAX_AUDIT_RECORDS {
            let overflow = registry.audit_records.len() - MAX_AUDIT_RECORDS;
            registry.audit_records.drain(0..overflow);
        }
        self.save_registry(&registry)
    }

    pub(super) fn recent_audit_sync(&self, limit: usize) -> Vec<IntegrationInsightAuditRecord> {
        let registry = self.registry.lock();
        let take = limit.max(1);
        registry
            .audit_records
            .iter()
            .rev()
            .take(take)
            .cloned()
            .collect()
    }
}
