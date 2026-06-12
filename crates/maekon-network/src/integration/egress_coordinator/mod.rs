//! IntegrationEgressCoordinator — outbound egress orchestration.
//!
//! ADR-013: 500L threshold applied — original 1197L file converted to module
//! folder with submodules:
//!   budget.rs — MAX_OUTBOX_ITEMS / MAX_OUTBOX_BYTES / byte estimation helpers
//!   tests.rs  — all #[cfg(test)] fixtures and test cases

mod budget;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::models::integration::{
    IntegrationAckCursor, IntegrationEnvelope, IntegrationOutboundPayload,
    IntegrationSessionStatus, QueuedIntegrationEgressMessage,
};
use maekon_core::ports::integration::{
    IntegrationEgressPort, IntegrationEgressSignalPort, IntegrationOutboxPort,
    IntegrationSessionPort,
};
use tokio::sync::Notify;
use tracing::warn;

use super::transport::IntegrationEgressTransportClient;
use budget::{estimate_message_bytes, MAX_OUTBOX_BYTES, MAX_OUTBOX_ITEMS};

pub struct IntegrationEgressCoordinator {
    session_port: Arc<dyn IntegrationSessionPort>,
    outbox: Arc<dyn IntegrationOutboxPort>,
    transport: Arc<dyn IntegrationEgressTransportClient>,
    max_batch_size: usize,
    flush_notify: Arc<Notify>,
    /// F-RR-C25-05: 현재 outbox에 누적된 추정 바이트 수.
    /// enqueue_message 에서 증가, flush 후 ack 완료 시 감소합니다.
    pub(crate) pending_bytes: AtomicUsize,
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
            pending_bytes: AtomicUsize::new(0),
        }
    }

    /// 현재 추정 누적 바이트 수를 반환합니다 (모니터링/테스트용).
    ///
    /// # 주의: Relaxed 순서 스냅샷
    ///
    /// 이 값은 `Ordering::Relaxed` 로 읽으므로 best-effort 근사치입니다.
    /// 동시에 진행 중인 `enqueue_message` / `flush` 호출과의 순서가 보장되지 않아
    /// 호출 시점에 따라 직전 또는 직후 상태를 반영할 수 있습니다.
    ///
    /// 따라서 반환값을 **strict upper-bound** (예: `assert!(v <= MAX)`) 로
    /// 사용하면 비결정적 테스트 실패를 유발할 수 있습니다. 단조적 peak 추적
    /// (F-RR-C26-01 테스트 참조) 등 best-effort 용도로만 사용하세요.
    pub fn pending_bytes(&self) -> usize {
        self.pending_bytes.load(Ordering::Relaxed)
    }

    fn validate_scopes(
        session: &maekon_core::models::integration::IntegrationSessionState,
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
                code: maekon_core::error_codes::AuthCode::Failed,
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
                code: maekon_core::error_codes::InternalCode::Generic,
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
        // F-PF-C24-05: count 상한 — 아이템 수가 MAX_OUTBOX_ITEMS 에 도달하면 거부.
        let count = self.outbox.pending_count().await?;
        if count >= MAX_OUTBOX_ITEMS {
            return Err(CoreError::ServiceUnavailable {
                code: maekon_core::error_codes::ServiceCode::Unavailable,
                message: format!(
                    "integration outbox full ({MAX_OUTBOX_ITEMS} items pending); \
                     message dropped until flush drains the queue"
                ),
            });
        }

        // F-RR-C26-01: 원자적 예약-후-검사 패턴.
        let msg_bytes = estimate_message_bytes(&envelope, &payload);
        let new_total = self.pending_bytes.fetch_add(msg_bytes, Ordering::Relaxed) + msg_bytes;
        if new_total > MAX_OUTBOX_BYTES {
            self.pending_bytes.fetch_sub(msg_bytes, Ordering::Relaxed);
            warn!(
                new_total,
                msg_bytes,
                max_bytes = MAX_OUTBOX_BYTES,
                "outbox byte cap 초과 — 메시지 거부 (flush 후 재시도)"
            );
            return Err(CoreError::ServiceUnavailable {
                code: maekon_core::error_codes::ServiceCode::Unavailable,
                message: format!(
                    "integration outbox byte cap exceeded \
                     ({new_total} bytes after reservation > {MAX_OUTBOX_BYTES} limit); \
                     message dropped until flush drains the queue"
                ),
            });
        }

        match self.outbox.enqueue_message(envelope, payload).await {
            Ok(_queue_id) => {
                self.flush_notify.notify_waiters();
                Ok(())
            }
            Err(e) => {
                self.pending_bytes.fetch_sub(msg_bytes, Ordering::Relaxed);
                Err(e)
            }
        }
    }

    async fn flush(&self) -> Result<usize, CoreError> {
        let session = self.session_port.current_session().await?.ok_or_else(|| {
            CoreError::ServiceUnavailable {
                code: maekon_core::error_codes::ServiceCode::Unavailable,
                message: "integration session is not connected".to_string(),
            }
        })?;

        if !matches!(
            session.status,
            IntegrationSessionStatus::Connected | IntegrationSessionStatus::Degraded
        ) || session.session_id.is_empty()
        {
            return Err(CoreError::ServiceUnavailable {
                code: maekon_core::error_codes::ServiceCode::Unavailable,
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
            let acked_items: Vec<&QueuedIntegrationEgressMessage> = items
                .iter()
                .filter(|item| acknowledged_queue_ids.contains(&item.queue_id))
                .collect();
            let acked_bytes: usize = acked_items
                .iter()
                .map(|item| {
                    serde_json::to_vec(&item.envelope)
                        .map(|v| v.len())
                        .unwrap_or(0)
                        + serde_json::to_vec(&item.payload)
                            .map(|v| v.len())
                            .unwrap_or(0)
                })
                .sum();
            // F-RR-C27-01/F-RC-C27-01: fetch_sub — race-free decrement.
            self.pending_bytes.fetch_sub(acked_bytes, Ordering::Relaxed);

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
