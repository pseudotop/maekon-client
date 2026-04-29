use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use oneshim_core::error::CoreError;
use oneshim_core::models::integration::{
    IntegrationAckCursor, IntegrationCapabilityScope, IntegrationEnvelope,
    IntegrationInboxItemStatus, IntegrationMessageType, IntegrationOrigin,
    IntegrationPromptReceipt, IntegrationPromptReceiptAction, IntegrationSessionState,
    IntegrationSessionStatus, ProactivePrompt, StoredProactivePrompt,
};
use oneshim_core::ports::integration::{
    IntegrationInboxPort, IntegrationInboxSignalPort, IntegrationInboxStorePort,
    IntegrationPromptReceiptStorePort, IntegrationSessionPort,
};
use uuid::Uuid;

use super::transport::IntegrationInboxTransportClient;

pub struct IntegrationInboxCoordinator {
    device_id: String,
    session_port: Arc<dyn IntegrationSessionPort>,
    inbox_store: Arc<dyn IntegrationInboxStorePort>,
    receipt_store: Arc<dyn IntegrationPromptReceiptStorePort>,
    transport: Arc<dyn IntegrationInboxTransportClient>,
    max_batch_size: usize,
}

impl IntegrationInboxCoordinator {
    pub fn new(
        device_id: impl Into<String>,
        session_port: Arc<dyn IntegrationSessionPort>,
        inbox_store: Arc<dyn IntegrationInboxStorePort>,
        receipt_store: Arc<dyn IntegrationPromptReceiptStorePort>,
        transport: Arc<dyn IntegrationInboxTransportClient>,
        max_batch_size: usize,
    ) -> Self {
        Self {
            device_id: device_id.into(),
            session_port,
            inbox_store,
            receipt_store,
            transport,
            max_batch_size: max_batch_size.max(1),
        }
    }

    fn session_ready_for_inbox(session: &IntegrationSessionState) -> Result<(), CoreError> {
        if !matches!(
            session.status,
            IntegrationSessionStatus::Connected | IntegrationSessionStatus::Degraded
        ) || session.session_id.is_empty()
        {
            return Err(CoreError::ServiceUnavailable {
                code: oneshim_core::error_codes::ServiceCode::Unavailable,
                message: "integration session is not ready for inbox refresh".to_string(),
            });
        }

        if !session
            .granted_scopes
            .contains(&IntegrationCapabilityScope::PromptRead)
        {
            return Err(CoreError::Auth {
                code: oneshim_core::error_codes::AuthCode::Failed,
                message: "integration session is missing required scope: PromptRead".to_string(),
            });
        }

        Ok(())
    }

    fn to_stored_prompts(prompts: Vec<ProactivePrompt>) -> Vec<StoredProactivePrompt> {
        let now = Utc::now();
        prompts
            .into_iter()
            .map(|prompt| StoredProactivePrompt {
                prompt,
                received_at: now,
                status: IntegrationInboxItemStatus::Pending,
                status_updated_at: now,
                presented_at: None,
                dismiss_reason: None,
            })
            .collect()
    }

    async fn build_receipt_envelope(
        &self,
        action: IntegrationPromptReceiptAction,
    ) -> Result<IntegrationEnvelope, CoreError> {
        let current_session = self.session_port.current_session().await?;
        Ok(IntegrationEnvelope {
            envelope_id: format!("integration-envelope-{}", Uuid::new_v4()),
            schema_version: "integration.prompt_receipt.v1".to_string(),
            message_type: IntegrationMessageType::PromptReceipt,
            timestamp: Utc::now(),
            nonce: Uuid::new_v4().to_string(),
            origin: IntegrationOrigin {
                device_id: current_session
                    .as_ref()
                    .map(|session| session.device_id.clone())
                    .unwrap_or_else(|| self.device_id.clone()),
                workspace_id: None,
                session_id: current_session.map(|session| session.session_id),
                source: "desktop-client".to_string(),
            },
            capability_scope: match action {
                IntegrationPromptReceiptAction::Acknowledged
                | IntegrationPromptReceiptAction::Dismissed => {
                    IntegrationCapabilityScope::PromptAck
                }
            },
        })
    }

    async fn record_prompt_receipt(
        &self,
        prompt_id: &str,
        action: IntegrationPromptReceiptAction,
        reason: Option<String>,
    ) -> Result<(), CoreError> {
        let envelope = self.build_receipt_envelope(action.clone()).await?;
        let receipt = IntegrationPromptReceipt {
            receipt_id: format!("integration-prompt-receipt-{}", Uuid::new_v4()),
            prompt_id: prompt_id.to_string(),
            action,
            occurred_at: Utc::now(),
            reason,
        };
        self.receipt_store
            .record_prompt_receipt(prompt_id, envelope, receipt)
            .await?;
        Ok(())
    }
}

#[async_trait]
impl IntegrationInboxSignalPort for IntegrationInboxCoordinator {
    async fn wait_for_remote_prompt_signal(&self, timeout: Duration) -> Result<bool, CoreError> {
        let Some(session) = self.session_port.current_session().await? else {
            return Ok(false);
        };
        if Self::session_ready_for_inbox(&session).is_err() {
            return Ok(false);
        }

        self.transport
            .wait_for_remote_signal(&session.session_id, timeout)
            .await
    }
}

#[async_trait]
impl IntegrationInboxPort for IntegrationInboxCoordinator {
    async fn refresh(&self) -> Result<usize, CoreError> {
        self.inbox_store.expire_stale().await?;

        let session = self.session_port.current_session().await?.ok_or_else(|| {
            CoreError::ServiceUnavailable {
                code: oneshim_core::error_codes::ServiceCode::Unavailable,
                message: "integration session is not connected".to_string(),
            }
        })?;
        Self::session_ready_for_inbox(&session)?;

        let current_cursor = self.inbox_store.last_ack_cursor().await?;
        let response = self
            .transport
            .receive_prompts(&session.session_id, current_cursor, self.max_batch_size)
            .await?;

        let prompt_count = response.prompts.len();
        if prompt_count > 0 {
            self.inbox_store
                .upsert_prompts(Self::to_stored_prompts(response.prompts))
                .await?;
        }

        if let Some(cursor) = response.ack_cursor {
            self.inbox_store.store_ack_cursor(cursor.clone()).await?;
            self.session_port
                .store_ack_cursor(&session.session_id, cursor)
                .await?;
        }

        Ok(prompt_count)
    }

    async fn list_pending(&self) -> Result<Vec<StoredProactivePrompt>, CoreError> {
        self.inbox_store.expire_stale().await?;
        self.inbox_store.list_pending().await
    }

    async fn acknowledge(&self, prompt_id: &str) -> Result<(), CoreError> {
        self.record_prompt_receipt(
            prompt_id,
            IntegrationPromptReceiptAction::Acknowledged,
            None,
        )
        .await
    }

    async fn dismiss(&self, prompt_id: &str, reason: Option<String>) -> Result<(), CoreError> {
        self.record_prompt_receipt(prompt_id, IntegrationPromptReceiptAction::Dismissed, reason)
            .await
    }

    async fn last_ack_cursor(&self) -> Result<Option<IntegrationAckCursor>, CoreError> {
        self.inbox_store.last_ack_cursor().await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use async_trait::async_trait;
    use chrono::{Duration, Utc};
    use tokio::sync::Mutex;

    use super::*;
    use crate::integration::transport::IntegrationInboxTransportResponse;

    struct MockSessionPort {
        state: Arc<Mutex<Option<IntegrationSessionState>>>,
    }

    #[async_trait]
    impl IntegrationSessionPort for MockSessionPort {
        async fn connect(
            &self,
            _requested_scopes: Vec<IntegrationCapabilityScope>,
        ) -> Result<IntegrationSessionState, CoreError> {
            self.state
                .lock()
                .await
                .clone()
                .ok_or_else(|| CoreError::ServiceUnavailable {
                    code: oneshim_core::error_codes::ServiceCode::Unavailable,
                    message: "no session".to_string(),
                })
        }

        async fn current_session(&self) -> Result<Option<IntegrationSessionState>, CoreError> {
            Ok(self.state.lock().await.clone())
        }

        async fn heartbeat(&self, _session_id: &str) -> Result<IntegrationSessionState, CoreError> {
            self.state
                .lock()
                .await
                .clone()
                .ok_or_else(|| CoreError::ServiceUnavailable {
                    code: oneshim_core::error_codes::ServiceCode::Unavailable,
                    message: "no session".to_string(),
                })
        }

        async fn store_ack_cursor(
            &self,
            session_id: &str,
            cursor: IntegrationAckCursor,
        ) -> Result<IntegrationSessionState, CoreError> {
            let mut guard = self.state.lock().await;
            let state = guard
                .as_mut()
                .ok_or_else(|| CoreError::ServiceUnavailable {
                    code: oneshim_core::error_codes::ServiceCode::Unavailable,
                    message: "no session".to_string(),
                })?;
            if state.session_id != session_id {
                return Err(CoreError::NotFound {
                    code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                    resource_type: "integration_session".to_string(),
                    id: session_id.to_string(),
                });
            }
            if let Some(existing) = state
                .ack_cursors
                .iter_mut()
                .find(|existing| existing.stream_id == cursor.stream_id)
            {
                *existing = cursor;
            } else {
                state.ack_cursors.push(cursor);
            }
            Ok(state.clone())
        }

        async fn disconnect(&self, _session_id: &str) -> Result<(), CoreError> {
            Ok(())
        }
    }

    struct MockInboxStore {
        prompts: Arc<Mutex<BTreeMap<String, StoredProactivePrompt>>>,
        last_cursor: Arc<Mutex<Option<IntegrationAckCursor>>>,
        receipts: Arc<Mutex<Vec<IntegrationPromptReceipt>>>,
    }

    #[async_trait]
    impl IntegrationInboxStorePort for MockInboxStore {
        async fn upsert_prompts(
            &self,
            prompts: Vec<StoredProactivePrompt>,
        ) -> Result<(), CoreError> {
            let mut guard = self.prompts.lock().await;
            for prompt in prompts {
                if let Some(existing) = guard.get_mut(&prompt.prompt.prompt_id) {
                    existing.prompt = prompt.prompt;
                } else {
                    guard.insert(prompt.prompt.prompt_id.clone(), prompt);
                }
            }
            Ok(())
        }

        async fn list_pending(&self) -> Result<Vec<StoredProactivePrompt>, CoreError> {
            Ok(self
                .prompts
                .lock()
                .await
                .values()
                .filter(|prompt| prompt.status == IntegrationInboxItemStatus::Pending)
                .cloned()
                .collect())
        }

        async fn list_unpresented(
            &self,
            limit: usize,
        ) -> Result<Vec<StoredProactivePrompt>, CoreError> {
            let mut prompts: Vec<_> = self
                .prompts
                .lock()
                .await
                .values()
                .filter(|prompt| {
                    prompt.status == IntegrationInboxItemStatus::Pending
                        && prompt.presented_at.is_none()
                })
                .cloned()
                .collect();
            prompts.sort_by_key(|prompt| prompt.received_at);
            prompts.truncate(limit);
            Ok(prompts)
        }

        async fn pending_count(&self) -> Result<usize, CoreError> {
            Ok(self
                .prompts
                .lock()
                .await
                .values()
                .filter(|prompt| prompt.status == IntegrationInboxItemStatus::Pending)
                .count())
        }

        async fn mark_presented(
            &self,
            prompt_id: &str,
            presented_at: chrono::DateTime<Utc>,
        ) -> Result<(), CoreError> {
            let mut guard = self.prompts.lock().await;
            let prompt = guard
                .get_mut(prompt_id)
                .ok_or_else(|| CoreError::NotFound {
                    code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                    resource_type: "integration_prompt".to_string(),
                    id: prompt_id.to_string(),
                })?;
            prompt.presented_at = Some(presented_at);
            Ok(())
        }

        async fn update_status(
            &self,
            prompt_id: &str,
            status: IntegrationInboxItemStatus,
            reason: Option<String>,
        ) -> Result<(), CoreError> {
            let mut guard = self.prompts.lock().await;
            let prompt = guard
                .get_mut(prompt_id)
                .ok_or_else(|| CoreError::NotFound {
                    code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                    resource_type: "integration_prompt".to_string(),
                    id: prompt_id.to_string(),
                })?;
            prompt.status = status;
            prompt.status_updated_at = Utc::now();
            prompt.dismiss_reason = reason;
            Ok(())
        }

        async fn expire_stale(&self) -> Result<usize, CoreError> {
            let now = Utc::now();
            let mut expired = 0usize;
            for prompt in self.prompts.lock().await.values_mut() {
                if prompt.status == IntegrationInboxItemStatus::Pending
                    && prompt
                        .prompt
                        .expires_at
                        .map(|expires_at| expires_at <= now)
                        .unwrap_or(false)
                {
                    prompt.status = IntegrationInboxItemStatus::Expired;
                    prompt.status_updated_at = now;
                    expired += 1;
                }
            }
            Ok(expired)
        }

        async fn last_ack_cursor(&self) -> Result<Option<IntegrationAckCursor>, CoreError> {
            Ok(self.last_cursor.lock().await.clone())
        }

        async fn store_ack_cursor(&self, cursor: IntegrationAckCursor) -> Result<(), CoreError> {
            *self.last_cursor.lock().await = Some(cursor);
            Ok(())
        }
    }

    #[async_trait]
    impl IntegrationPromptReceiptStorePort for MockInboxStore {
        async fn record_prompt_receipt(
            &self,
            prompt_id: &str,
            _envelope: IntegrationEnvelope,
            receipt: IntegrationPromptReceipt,
        ) -> Result<String, CoreError> {
            let mut prompts = self.prompts.lock().await;
            let prompt = prompts
                .get_mut(prompt_id)
                .ok_or_else(|| CoreError::NotFound {
                    code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                    resource_type: "integration_prompt".to_string(),
                    id: prompt_id.to_string(),
                })?;
            prompt.status = receipt.action.to_inbox_status();
            prompt.status_updated_at = receipt.occurred_at;
            prompt.dismiss_reason = receipt.reason.clone();
            self.receipts.lock().await.push(receipt);
            Ok(format!("queue-{prompt_id}"))
        }
    }

    struct MockInboxTransport {
        prompts: Vec<ProactivePrompt>,
        ack_cursor: Option<IntegrationAckCursor>,
    }

    #[async_trait]
    impl IntegrationInboxTransportClient for MockInboxTransport {
        async fn receive_prompts(
            &self,
            _session_id: &str,
            _after_cursor: Option<IntegrationAckCursor>,
            limit: usize,
        ) -> Result<IntegrationInboxTransportResponse, CoreError> {
            Ok(IntegrationInboxTransportResponse {
                prompts: self.prompts.iter().take(limit).cloned().collect(),
                ack_cursor: self.ack_cursor.clone(),
            })
        }
    }

    fn prompt(id: &str, expires_at: Option<chrono::DateTime<Utc>>) -> ProactivePrompt {
        ProactivePrompt {
            prompt_id: id.to_string(),
            category: oneshim_core::models::integration::ProactivePromptCategory::Task,
            title: format!("Prompt {id}"),
            body: "Review latest insight".to_string(),
            priority: oneshim_core::models::integration::ProactivePromptPriority::Medium,
            actions: Vec::new(),
            expires_at,
            provenance: oneshim_core::models::integration::PromptProvenance {
                source_system: "team-server".to_string(),
                source_actor: Some("scheduler".to_string()),
                correlation_id: Some(format!("corr-{id}")),
            },
        }
    }

    fn prompt_read_session() -> IntegrationSessionState {
        IntegrationSessionState {
            session_id: "session-1".to_string(),
            device_id: "device-1".to_string(),
            status: IntegrationSessionStatus::Connected,
            transport_kind: oneshim_core::models::integration::IntegrationTransportKind::WebSocket,
            auth_scheme: oneshim_core::models::integration::IntegrationAuthScheme::BearerToken,
            connected_at: Some(Utc::now()),
            last_heartbeat_at: Some(Utc::now()),
            requested_scopes: vec![IntegrationCapabilityScope::PromptRead],
            granted_scopes: vec![IntegrationCapabilityScope::PromptRead],
            ack_cursors: Vec::new(),
        }
    }

    #[tokio::test]
    async fn refresh_pulls_prompts_and_updates_cursor() {
        let session_port = Arc::new(MockSessionPort {
            state: Arc::new(Mutex::new(Some(prompt_read_session()))),
        });
        let store = Arc::new(MockInboxStore {
            prompts: Arc::new(Mutex::new(BTreeMap::new())),
            last_cursor: Arc::new(Mutex::new(None)),
            receipts: Arc::new(Mutex::new(Vec::new())),
        });
        let coordinator = IntegrationInboxCoordinator::new(
            "device-1",
            session_port.clone(),
            store.clone(),
            store.clone(),
            Arc::new(MockInboxTransport {
                prompts: vec![prompt("1", None), prompt("2", None)],
                ack_cursor: Some(IntegrationAckCursor {
                    stream_id: "inbox".to_string(),
                    cursor: "cursor-2".to_string(),
                    acknowledged_at: Utc::now(),
                }),
            }),
            10,
        );

        let refreshed = coordinator.refresh().await.unwrap();
        assert_eq!(refreshed, 2);
        assert_eq!(coordinator.list_pending().await.unwrap().len(), 2);
        assert_eq!(
            coordinator.last_ack_cursor().await.unwrap().unwrap().cursor,
            "cursor-2"
        );
        assert_eq!(
            session_port
                .current_session()
                .await
                .unwrap()
                .unwrap()
                .ack_cursors[0]
                .cursor,
            "cursor-2"
        );
    }

    #[tokio::test]
    async fn refresh_requires_prompt_read_scope() {
        let coordinator = IntegrationInboxCoordinator::new(
            "device-1",
            Arc::new(MockSessionPort {
                state: Arc::new(Mutex::new(Some(IntegrationSessionState {
                    session_id: "session-1".to_string(),
                    device_id: "device-1".to_string(),
                    status: IntegrationSessionStatus::Connected,
                    transport_kind:
                        oneshim_core::models::integration::IntegrationTransportKind::WebSocket,
                    auth_scheme:
                        oneshim_core::models::integration::IntegrationAuthScheme::BearerToken,
                    connected_at: Some(Utc::now()),
                    last_heartbeat_at: Some(Utc::now()),
                    requested_scopes: vec![IntegrationCapabilityScope::InsightWrite],
                    granted_scopes: vec![IntegrationCapabilityScope::InsightWrite],
                    ack_cursors: Vec::new(),
                }))),
            }),
            Arc::new(MockInboxStore {
                prompts: Arc::new(Mutex::new(BTreeMap::new())),
                last_cursor: Arc::new(Mutex::new(None)),
                receipts: Arc::new(Mutex::new(Vec::new())),
            }),
            Arc::new(MockInboxStore {
                prompts: Arc::new(Mutex::new(BTreeMap::new())),
                last_cursor: Arc::new(Mutex::new(None)),
                receipts: Arc::new(Mutex::new(Vec::new())),
            }),
            Arc::new(MockInboxTransport {
                prompts: vec![prompt("1", None)],
                ack_cursor: None,
            }),
            10,
        );

        let err = coordinator
            .refresh()
            .await
            .expect_err("refresh should fail");
        assert!(matches!(err, CoreError::Auth { .. }));
    }

    #[tokio::test]
    async fn acknowledge_and_dismiss_update_store_state() {
        let expired_at = Utc::now() - Duration::minutes(5);
        let store = Arc::new(MockInboxStore {
            prompts: Arc::new(Mutex::new(BTreeMap::from([
                (
                    "prompt-1".to_string(),
                    StoredProactivePrompt {
                        prompt: prompt("prompt-1", None),
                        received_at: Utc::now(),
                        status: IntegrationInboxItemStatus::Pending,
                        status_updated_at: Utc::now(),
                        presented_at: None,
                        dismiss_reason: None,
                    },
                ),
                (
                    "prompt-2".to_string(),
                    StoredProactivePrompt {
                        prompt: prompt("prompt-2", Some(expired_at)),
                        received_at: Utc::now(),
                        status: IntegrationInboxItemStatus::Pending,
                        status_updated_at: Utc::now(),
                        presented_at: None,
                        dismiss_reason: None,
                    },
                ),
            ]))),
            last_cursor: Arc::new(Mutex::new(None)),
            receipts: Arc::new(Mutex::new(Vec::new())),
        });
        let coordinator = IntegrationInboxCoordinator::new(
            "device-1",
            Arc::new(MockSessionPort {
                state: Arc::new(Mutex::new(Some(prompt_read_session()))),
            }),
            store.clone(),
            store.clone(),
            Arc::new(MockInboxTransport {
                prompts: Vec::new(),
                ack_cursor: None,
            }),
            10,
        );

        coordinator.acknowledge("prompt-1").await.unwrap();
        coordinator
            .dismiss("prompt-1", Some("user dismissed".to_string()))
            .await
            .unwrap();
        let pending = coordinator.list_pending().await.unwrap();
        assert!(pending.is_empty());

        let prompts = store.prompts.lock().await;
        assert_eq!(
            prompts.get("prompt-1").unwrap().status,
            IntegrationInboxItemStatus::Dismissed
        );
        assert_eq!(
            prompts.get("prompt-1").unwrap().dismiss_reason.as_deref(),
            Some("user dismissed")
        );
        assert_eq!(
            prompts.get("prompt-2").unwrap().status,
            IntegrationInboxItemStatus::Expired
        );
        let receipts = store.receipts.lock().await;
        assert_eq!(receipts.len(), 2);
        assert_eq!(
            receipts[0].action,
            IntegrationPromptReceiptAction::Acknowledged
        );
        assert_eq!(
            receipts[1].action,
            IntegrationPromptReceiptAction::Dismissed
        );
    }

    #[tokio::test]
    async fn refresh_does_not_resurrect_dismissed_prompt() {
        let store = Arc::new(MockInboxStore {
            prompts: Arc::new(Mutex::new(BTreeMap::from([(
                "prompt-1".to_string(),
                StoredProactivePrompt {
                    prompt: prompt("prompt-1", None),
                    received_at: Utc::now() - Duration::minutes(10),
                    status: IntegrationInboxItemStatus::Dismissed,
                    status_updated_at: Utc::now() - Duration::minutes(5),
                    presented_at: None,
                    dismiss_reason: Some("already handled".to_string()),
                },
            )]))),
            last_cursor: Arc::new(Mutex::new(None)),
            receipts: Arc::new(Mutex::new(Vec::new())),
        });
        let coordinator = IntegrationInboxCoordinator::new(
            "device-1",
            Arc::new(MockSessionPort {
                state: Arc::new(Mutex::new(Some(prompt_read_session()))),
            }),
            store.clone(),
            store.clone(),
            Arc::new(MockInboxTransport {
                prompts: vec![prompt("prompt-1", None)],
                ack_cursor: Some(IntegrationAckCursor {
                    stream_id: "inbox".to_string(),
                    cursor: "cursor-1".to_string(),
                    acknowledged_at: Utc::now(),
                }),
            }),
            10,
        );

        let refreshed = coordinator.refresh().await.unwrap();
        assert_eq!(refreshed, 1);
        assert!(coordinator.list_pending().await.unwrap().is_empty());

        let prompts = store.prompts.lock().await;
        let prompt = prompts.get("prompt-1").unwrap();
        assert_eq!(prompt.status, IntegrationInboxItemStatus::Dismissed);
        assert_eq!(prompt.dismiss_reason.as_deref(), Some("already handled"));
    }
}
