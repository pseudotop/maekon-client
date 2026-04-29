use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use oneshim_core::error::CoreError;
use oneshim_core::models::integration::{
    IntegrationAckCursor, IntegrationEnvelope, IntegrationOutboundPayload,
    IntegrationSessionStatus, QueuedIntegrationEgressMessage,
};
use oneshim_core::ports::integration::{
    IntegrationEgressPort, IntegrationEgressSignalPort, IntegrationOutboxPort,
    IntegrationSessionPort,
};
use tokio::sync::Notify;

use super::transport::IntegrationEgressTransportClient;

pub struct IntegrationEgressCoordinator {
    session_port: Arc<dyn IntegrationSessionPort>,
    outbox: Arc<dyn IntegrationOutboxPort>,
    transport: Arc<dyn IntegrationEgressTransportClient>,
    max_batch_size: usize,
    flush_notify: Arc<Notify>,
}

impl IntegrationEgressCoordinator {
    pub fn new(
        session_port: Arc<dyn IntegrationSessionPort>,
        outbox: Arc<dyn IntegrationOutboxPort>,
        transport: Arc<dyn IntegrationEgressTransportClient>,
        max_batch_size: usize,
    ) -> Self {
        Self {
            session_port,
            outbox,
            transport,
            max_batch_size: max_batch_size.max(1),
            flush_notify: Arc::new(Notify::new()),
        }
    }

    fn validate_scopes(
        session: &oneshim_core::models::integration::IntegrationSessionState,
        items: &[QueuedIntegrationEgressMessage],
    ) -> Result<(), CoreError> {
        let missing_scope = items.iter().find_map(|item| {
            (!session
                .granted_scopes
                .contains(&item.envelope.capability_scope))
            .then_some(item.envelope.capability_scope.clone())
        });

        if let Some(scope) = missing_scope {
            return Err(CoreError::Auth {
                code: oneshim_core::error_codes::AuthCode::Failed,
                message: format!("integration session is missing required scope: {scope:?}"),
            });
        }

        Ok(())
    }

    fn acknowledged_queue_ids(
        sent_items: &[QueuedIntegrationEgressMessage],
        response: &super::transport::IntegrationEgressTransportResponse,
    ) -> Result<Vec<String>, CoreError> {
        let sent_ids: BTreeSet<&str> = sent_items
            .iter()
            .map(|item| item.queue_id.as_str())
            .collect();
        if let Some(unknown_id) = response
            .acknowledged_queue_ids
            .iter()
            .find(|queue_id| !sent_ids.contains(queue_id.as_str()))
        {
            return Err(CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!(
                    "integration egress transport acknowledged unknown queue id: {unknown_id}"
                ),
            });
        }

        Ok(response.acknowledged_queue_ids.clone())
    }
}

#[async_trait]
impl IntegrationEgressSignalPort for IntegrationEgressCoordinator {
    async fn wait_for_pending_egress(&self, timeout: Duration) -> Result<bool, CoreError> {
        match tokio::time::timeout(timeout, self.flush_notify.notified()).await {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }
}

#[async_trait]
impl IntegrationEgressPort for IntegrationEgressCoordinator {
    async fn enqueue_message(
        &self,
        envelope: IntegrationEnvelope,
        payload: IntegrationOutboundPayload,
    ) -> Result<(), CoreError> {
        self.outbox.enqueue_message(envelope, payload).await?;
        self.flush_notify.notify_waiters();
        Ok(())
    }

    async fn flush(&self) -> Result<usize, CoreError> {
        let session = self.session_port.current_session().await?.ok_or_else(|| {
            CoreError::ServiceUnavailable {
                code: oneshim_core::error_codes::ServiceCode::Unavailable,
                message: "integration session is not connected".to_string(),
            }
        })?;

        if !matches!(
            session.status,
            IntegrationSessionStatus::Connected | IntegrationSessionStatus::Degraded
        ) || session.session_id.is_empty()
        {
            return Err(CoreError::ServiceUnavailable {
                code: oneshim_core::error_codes::ServiceCode::Unavailable,
                message: "integration session is not ready for outbound egress".to_string(),
            });
        }

        let items = self.outbox.list_pending(self.max_batch_size).await?;
        if items.is_empty() {
            return Ok(0);
        }
        Self::validate_scopes(&session, &items)?;

        let response = self
            .transport
            .send_messages(&session.session_id, items.clone())
            .await?;
        let acknowledged_queue_ids = Self::acknowledged_queue_ids(&items, &response)?;
        let accepted_count = response.accepted_count();

        if !acknowledged_queue_ids.is_empty() {
            self.outbox.delete(&acknowledged_queue_ids).await?;
        }
        if let Some(cursor) = response.ack_cursor {
            self.outbox.store_ack_cursor(cursor.clone()).await?;
            self.session_port
                .store_ack_cursor(&session.session_id, cursor)
                .await?;
        }

        Ok(accepted_count)
    }

    async fn last_ack_cursor(&self) -> Result<Option<IntegrationAckCursor>, CoreError> {
        self.outbox.last_ack_cursor().await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Arc;

    use async_trait::async_trait;
    use chrono::Utc;
    use oneshim_core::models::integration::{
        InsightPacket, InsightSourceWindow, IntegrationCapabilityScope, IntegrationOrigin,
        IntegrationOutboundPayload, IntegrationPrivacyClassification, IntegrationSessionState,
        IntegrationSessionStatus, QueuedIntegrationEgressMessage,
    };
    use oneshim_core::ports::integration::{IntegrationOutboxPort, IntegrationSessionPort};
    use tokio::sync::Mutex;

    use super::*;
    use crate::integration::transport::IntegrationEgressTransportResponse;

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

    struct MockOutbox {
        items: Arc<Mutex<VecDeque<QueuedIntegrationEgressMessage>>>,
        last_cursor: Arc<Mutex<Option<IntegrationAckCursor>>>,
    }

    #[async_trait]
    impl IntegrationOutboxPort for MockOutbox {
        async fn enqueue_message(
            &self,
            envelope: IntegrationEnvelope,
            payload: IntegrationOutboundPayload,
        ) -> Result<String, CoreError> {
            let queue_id = format!(
                "queue-{}",
                match &payload {
                    IntegrationOutboundPayload::Insight(packet) => packet.packet_id.clone(),
                    IntegrationOutboundPayload::PromptReceipt(receipt) => receipt.prompt_id.clone(),
                }
            );
            self.items
                .lock()
                .await
                .push_back(QueuedIntegrationEgressMessage {
                    queue_id: queue_id.clone(),
                    envelope,
                    payload,
                    queued_at: Utc::now(),
                });
            Ok(queue_id)
        }

        async fn list_pending(
            &self,
            limit: usize,
        ) -> Result<Vec<QueuedIntegrationEgressMessage>, CoreError> {
            Ok(self
                .items
                .lock()
                .await
                .iter()
                .take(limit)
                .cloned()
                .collect())
        }

        async fn pending_count(&self) -> Result<usize, CoreError> {
            Ok(self.items.lock().await.len())
        }

        async fn delete(&self, queue_ids: &[String]) -> Result<(), CoreError> {
            let mut guard = self.items.lock().await;
            guard.retain(|item| !queue_ids.contains(&item.queue_id));
            Ok(())
        }

        async fn last_ack_cursor(&self) -> Result<Option<IntegrationAckCursor>, CoreError> {
            Ok(self.last_cursor.lock().await.clone())
        }

        async fn store_ack_cursor(&self, cursor: IntegrationAckCursor) -> Result<(), CoreError> {
            *self.last_cursor.lock().await = Some(cursor);
            Ok(())
        }
    }

    struct MockEgressTransport {
        acknowledged_queue_ids: Vec<String>,
        cursor: Option<IntegrationAckCursor>,
    }

    #[async_trait]
    impl IntegrationEgressTransportClient for MockEgressTransport {
        async fn send_messages(
            &self,
            _session_id: &str,
            items: Vec<QueuedIntegrationEgressMessage>,
        ) -> Result<IntegrationEgressTransportResponse, CoreError> {
            Ok(IntegrationEgressTransportResponse {
                acknowledged_queue_ids: self
                    .acknowledged_queue_ids
                    .iter()
                    .filter(|queue_id| items.iter().any(|item| &item.queue_id == *queue_id))
                    .cloned()
                    .collect(),
                ack_cursor: self.cursor.clone(),
            })
        }
    }

    fn queued_item(id: &str) -> QueuedIntegrationEgressMessage {
        QueuedIntegrationEgressMessage {
            queue_id: format!("queue-{id}"),
            envelope: IntegrationEnvelope {
                envelope_id: format!("env-{id}"),
                schema_version: "integration.envelope.v1".to_string(),
                message_type:
                    oneshim_core::models::integration::IntegrationMessageType::InsightPacket,
                timestamp: Utc::now(),
                nonce: format!("nonce-{id}"),
                origin: IntegrationOrigin {
                    device_id: "device-1".to_string(),
                    workspace_id: None,
                    session_id: Some("session-1".to_string()),
                    source: "desktop-client".to_string(),
                },
                capability_scope: IntegrationCapabilityScope::InsightWrite,
            },
            payload: IntegrationOutboundPayload::Insight(InsightPacket {
                packet_id: id.to_string(),
                summary: format!("summary-{id}"),
                derived_tags: vec!["focus".to_string()],
                source_window: InsightSourceWindow {
                    started_at: Utc::now(),
                    ended_at: Utc::now(),
                },
                privacy_classification: IntegrationPrivacyClassification::DerivedSummary,
                audit_reference_id: None,
            }),
            queued_at: Utc::now(),
        }
    }

    fn connected_session() -> IntegrationSessionState {
        IntegrationSessionState {
            session_id: "session-1".to_string(),
            device_id: "device-1".to_string(),
            status: IntegrationSessionStatus::Connected,
            transport_kind: oneshim_core::models::integration::IntegrationTransportKind::WebSocket,
            auth_scheme: oneshim_core::models::integration::IntegrationAuthScheme::BearerToken,
            connected_at: Some(Utc::now()),
            last_heartbeat_at: Some(Utc::now()),
            requested_scopes: vec![IntegrationCapabilityScope::InsightWrite],
            granted_scopes: vec![IntegrationCapabilityScope::InsightWrite],
            ack_cursors: Vec::new(),
        }
    }

    #[tokio::test]
    async fn enqueue_persists_to_outbox() {
        let outbox = Arc::new(MockOutbox {
            items: Arc::new(Mutex::new(VecDeque::new())),
            last_cursor: Arc::new(Mutex::new(None)),
        });
        let coordinator = IntegrationEgressCoordinator::new(
            Arc::new(MockSessionPort {
                state: Arc::new(Mutex::new(Some(connected_session()))),
            }),
            outbox.clone(),
            Arc::new(MockEgressTransport {
                acknowledged_queue_ids: vec!["queue-1".to_string()],
                cursor: None,
            }),
            10,
        );

        let queued = queued_item("1");
        let IntegrationOutboundPayload::Insight(packet) = queued.payload else {
            panic!("expected insight payload");
        };
        coordinator
            .enqueue_insight(queued.envelope, packet)
            .await
            .unwrap();

        assert_eq!(outbox.list_pending(10).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn flush_sends_and_clears_pending_items() {
        let outbox = Arc::new(MockOutbox {
            items: Arc::new(Mutex::new(VecDeque::from(vec![
                queued_item("1"),
                queued_item("2"),
            ]))),
            last_cursor: Arc::new(Mutex::new(None)),
        });
        let coordinator = IntegrationEgressCoordinator::new(
            Arc::new(MockSessionPort {
                state: Arc::new(Mutex::new(Some(connected_session()))),
            }),
            outbox.clone(),
            Arc::new(MockEgressTransport {
                acknowledged_queue_ids: vec!["queue-1".to_string(), "queue-2".to_string()],
                cursor: Some(IntegrationAckCursor {
                    stream_id: "insights".to_string(),
                    cursor: "cursor-2".to_string(),
                    acknowledged_at: Utc::now(),
                }),
            }),
            10,
        );

        let flushed = coordinator.flush().await.unwrap();
        assert_eq!(flushed, 2);
        assert!(outbox.list_pending(10).await.unwrap().is_empty());
        assert_eq!(
            coordinator.last_ack_cursor().await.unwrap().unwrap().cursor,
            "cursor-2"
        );
    }

    #[tokio::test]
    async fn flush_requires_connected_session() {
        let coordinator = IntegrationEgressCoordinator::new(
            Arc::new(MockSessionPort {
                state: Arc::new(Mutex::new(None)),
            }),
            Arc::new(MockOutbox {
                items: Arc::new(Mutex::new(VecDeque::from(vec![queued_item("1")]))),
                last_cursor: Arc::new(Mutex::new(None)),
            }),
            Arc::new(MockEgressTransport {
                acknowledged_queue_ids: vec!["queue-1".to_string()],
                cursor: None,
            }),
            10,
        );

        let err = coordinator.flush().await.expect_err("flush should fail");
        assert!(matches!(err, CoreError::ServiceUnavailable { .. }));
    }

    #[tokio::test]
    async fn flush_only_clears_acknowledged_items() {
        let outbox = Arc::new(MockOutbox {
            items: Arc::new(Mutex::new(VecDeque::from(vec![
                queued_item("1"),
                queued_item("2"),
            ]))),
            last_cursor: Arc::new(Mutex::new(None)),
        });
        let coordinator = IntegrationEgressCoordinator::new(
            Arc::new(MockSessionPort {
                state: Arc::new(Mutex::new(Some(connected_session()))),
            }),
            outbox.clone(),
            Arc::new(MockEgressTransport {
                acknowledged_queue_ids: vec!["queue-1".to_string()],
                cursor: None,
            }),
            10,
        );

        let flushed = coordinator.flush().await.unwrap();
        assert_eq!(flushed, 1);
        let pending = outbox.list_pending(10).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].queue_id, "queue-2");
    }

    #[tokio::test]
    async fn flush_requires_matching_session_scope() {
        let outbox = Arc::new(MockOutbox {
            items: Arc::new(Mutex::new(VecDeque::from(vec![queued_item("1")]))),
            last_cursor: Arc::new(Mutex::new(None)),
        });
        let coordinator = IntegrationEgressCoordinator::new(
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
                    requested_scopes: vec![IntegrationCapabilityScope::PromptRead],
                    granted_scopes: vec![IntegrationCapabilityScope::PromptRead],
                    ack_cursors: Vec::new(),
                }))),
            }),
            outbox.clone(),
            Arc::new(MockEgressTransport {
                acknowledged_queue_ids: vec!["queue-1".to_string()],
                cursor: None,
            }),
            10,
        );

        let err = coordinator.flush().await.expect_err("flush should fail");
        assert!(matches!(err, CoreError::Auth { .. }));
        assert_eq!(outbox.list_pending(10).await.unwrap().len(), 1);
    }
}
