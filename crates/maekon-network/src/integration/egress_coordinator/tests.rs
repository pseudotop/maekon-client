//! Tests for IntegrationEgressCoordinator.
//!
//! All test fixtures (mock ports, mock transport) live here to keep mod.rs
//! focused on the production implementation.

use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use maekon_core::error::CoreError;
use maekon_core::models::integration::{
    InsightPacket, InsightSourceWindow, IntegrationAckCursor, IntegrationCapabilityScope,
    IntegrationEnvelope, IntegrationMessageType, IntegrationOrigin, IntegrationOutboundPayload,
    IntegrationPrivacyClassification, IntegrationSessionState, IntegrationSessionStatus,
    QueuedIntegrationEgressMessage,
};
use maekon_core::ports::integration::{
    IntegrationEgressPort, IntegrationOutboxPort, IntegrationSessionPort,
};
use tokio::sync::Mutex;

use super::budget::{estimate_queued_bytes, MAX_OUTBOX_BYTES, MAX_OUTBOX_ITEMS};
use super::IntegrationEgressCoordinator;
use crate::integration::transport::{
    IntegrationEgressTransportClient, IntegrationEgressTransportResponse,
};

// ── Mock implementations ─────────────────────────────────────────────────────

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
                code: maekon_core::error_codes::ServiceCode::Unavailable,
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
                code: maekon_core::error_codes::ServiceCode::Unavailable,
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
                code: maekon_core::error_codes::ServiceCode::Unavailable,
                message: "no session".to_string(),
            })?;
        if state.session_id != session_id {
            return Err(CoreError::NotFound {
                code: maekon_core::error_codes::NotFoundCode::ResourceMissing,
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

// ── Fixture helpers ──────────────────────────────────────────────────────────

fn queued_item(id: &str) -> QueuedIntegrationEgressMessage {
    QueuedIntegrationEgressMessage {
        queue_id: format!("queue-{id}"),
        envelope: IntegrationEnvelope {
            envelope_id: format!("env-{id}"),
            schema_version: "integration.envelope.v1".to_string(),
            message_type: maekon_core::models::integration::IntegrationMessageType::InsightPacket,
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
        transport_kind: maekon_core::models::integration::IntegrationTransportKind::WebSocket,
        auth_scheme: maekon_core::models::integration::IntegrationAuthScheme::BearerToken,
        connected_at: Some(Utc::now()),
        last_heartbeat_at: Some(Utc::now()),
        requested_scopes: vec![IntegrationCapabilityScope::InsightWrite],
        granted_scopes: vec![IntegrationCapabilityScope::InsightWrite],
        ack_cursors: Vec::new(),
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

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

/// F-PF-C24-05: enqueue_message must return ServiceUnavailable once the
/// outbox reaches MAX_OUTBOX_ITEMS pending items.
#[tokio::test]
async fn egress_coordinator_outbox_exhausted_on_max_items() {
    use chrono::Utc as ChronomUtc;

    // Pre-fill the outbox to exactly MAX_OUTBOX_ITEMS.
    let items: VecDeque<QueuedIntegrationEgressMessage> = (0..MAX_OUTBOX_ITEMS)
        .map(|i| queued_item(&i.to_string()))
        .collect();

    let outbox = Arc::new(MockOutbox {
        items: Arc::new(Mutex::new(items)),
        last_cursor: Arc::new(Mutex::new(None)),
    });
    let coordinator = IntegrationEgressCoordinator::new(
        Arc::new(MockSessionPort {
            state: Arc::new(Mutex::new(Some(connected_session()))),
        }),
        outbox.clone(),
        Arc::new(MockEgressTransport {
            acknowledged_queue_ids: vec![],
            cursor: None,
        }),
        10,
    );

    // One more enqueue must fail with ServiceUnavailable.
    let extra_envelope = IntegrationEnvelope {
        envelope_id: "env-extra".to_string(),
        schema_version: "integration.envelope.v1".to_string(),
        message_type: IntegrationMessageType::InsightPacket,
        timestamp: ChronomUtc::now(),
        nonce: "nonce-extra".to_string(),
        origin: IntegrationOrigin {
            device_id: "device-1".to_string(),
            workspace_id: None,
            session_id: Some("session-1".to_string()),
            source: "desktop-client".to_string(),
        },
        capability_scope: IntegrationCapabilityScope::InsightWrite,
    };
    let extra_payload = IntegrationOutboundPayload::Insight(InsightPacket {
        packet_id: "extra".to_string(),
        summary: "summary-extra".to_string(),
        derived_tags: vec![],
        source_window: InsightSourceWindow {
            started_at: ChronomUtc::now(),
            ended_at: ChronomUtc::now(),
        },
        privacy_classification: IntegrationPrivacyClassification::DerivedSummary,
        audit_reference_id: None,
    });

    let err = coordinator
        .enqueue_message(extra_envelope, extra_payload)
        .await
        .expect_err("F-PF-C24-05: enqueue past MAX_OUTBOX_ITEMS must fail");
    assert!(
        matches!(err, CoreError::ServiceUnavailable { .. }),
        "F-PF-C24-05: expected ServiceUnavailable, got: {err:?}"
    );
    // Outbox count must be unchanged.
    assert_eq!(
        outbox.pending_count().await.unwrap(),
        MAX_OUTBOX_ITEMS,
        "F-PF-C24-05: outbox must not grow past MAX_OUTBOX_ITEMS"
    );
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
                    maekon_core::models::integration::IntegrationTransportKind::WebSocket,
                auth_scheme: maekon_core::models::integration::IntegrationAuthScheme::BearerToken,
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

/// F-RR-C25-05: enqueue_message must return ServiceUnavailable when the
/// accumulated pending bytes would exceed MAX_OUTBOX_BYTES.
#[tokio::test]
async fn enqueue_message_rejected_when_byte_cap_exceeded() {
    use std::sync::atomic::Ordering;

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
            acknowledged_queue_ids: vec![],
            cursor: None,
        }),
        10,
    );

    // 바이트 카운터를 MAX_OUTBOX_BYTES 바로 아래까지 수동으로 설정
    coordinator
        .pending_bytes
        .store(MAX_OUTBOX_BYTES, Ordering::Relaxed);

    let envelope = IntegrationEnvelope {
        envelope_id: "env-byte-test".to_string(),
        schema_version: "integration.envelope.v1".to_string(),
        message_type: IntegrationMessageType::InsightPacket,
        timestamp: Utc::now(),
        nonce: "nonce-byte-test".to_string(),
        origin: IntegrationOrigin {
            device_id: "device-1".to_string(),
            workspace_id: None,
            session_id: Some("session-1".to_string()),
            source: "desktop-client".to_string(),
        },
        capability_scope: IntegrationCapabilityScope::InsightWrite,
    };
    let payload = IntegrationOutboundPayload::Insight(InsightPacket {
        packet_id: "byte-test".to_string(),
        summary: "x".to_string(),
        derived_tags: vec![],
        source_window: InsightSourceWindow {
            started_at: Utc::now(),
            ended_at: Utc::now(),
        },
        privacy_classification: IntegrationPrivacyClassification::DerivedSummary,
        audit_reference_id: None,
    });

    let err = coordinator
        .enqueue_message(envelope, payload)
        .await
        .expect_err("F-RR-C25-05: byte cap 초과 시 ServiceUnavailable 반환 필수");

    assert!(
        matches!(err, CoreError::ServiceUnavailable { .. }),
        "F-RR-C25-05: expected ServiceUnavailable, got: {err:?}"
    );
    assert_eq!(
        outbox.pending_count().await.unwrap(),
        0,
        "F-RR-C25-05: 바이트 cap 거부 시 outbox에 메시지가 추가되면 안 됨"
    );
}

/// F-RR-C26-01: N개 동시 enqueue 호출이 있어도 pending_bytes 가 MAX_OUTBOX_BYTES 를
/// 초과하지 않는지 검증한다 (원자적 예약-후-검사 패턴 정합성 확인).
#[tokio::test]
async fn enqueue_message_byte_cap_not_exceeded_under_concurrency() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc as StdArc;

    let outbox = StdArc::new(MockOutbox {
        items: std::sync::Arc::new(Mutex::new(std::collections::VecDeque::new())),
        last_cursor: std::sync::Arc::new(Mutex::new(None)),
    });
    let coordinator = StdArc::new(IntegrationEgressCoordinator::new(
        StdArc::new(MockSessionPort {
            state: std::sync::Arc::new(Mutex::new(Some(connected_session()))),
        }),
        outbox.clone(),
        StdArc::new(MockEgressTransport {
            acknowledged_queue_ids: vec![],
            cursor: None,
        }),
        1024,
    ));

    const TASK_COUNT: usize = 100;
    let peak_bytes = StdArc::new(AtomicUsize::new(0));

    let handles: Vec<_> = (0..TASK_COUNT)
        .map(|i| {
            let c = coordinator.clone();
            let peak = peak_bytes.clone();
            tokio::spawn(async move {
                let envelope = IntegrationEnvelope {
                    envelope_id: format!("env-conc-{i}"),
                    schema_version: "integration.envelope.v1".to_string(),
                    message_type: IntegrationMessageType::InsightPacket,
                    timestamp: Utc::now(),
                    nonce: format!("nonce-conc-{i}"),
                    origin: IntegrationOrigin {
                        device_id: "device-1".to_string(),
                        workspace_id: None,
                        session_id: Some("session-1".to_string()),
                        source: "desktop-client".to_string(),
                    },
                    capability_scope: IntegrationCapabilityScope::InsightWrite,
                };
                let payload = IntegrationOutboundPayload::Insight(InsightPacket {
                    packet_id: format!("conc-{i}"),
                    summary: format!("summary-conc-{i}"),
                    derived_tags: vec![],
                    source_window: InsightSourceWindow {
                        started_at: Utc::now(),
                        ended_at: Utc::now(),
                    },
                    privacy_classification: IntegrationPrivacyClassification::DerivedSummary,
                    audit_reference_id: None,
                });
                let _ = c.enqueue_message(envelope, payload).await;
                let current = c.pending_bytes();
                let mut p = peak.load(Ordering::Relaxed);
                while current > p {
                    match peak.compare_exchange_weak(
                        p,
                        current,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => break,
                        Err(actual) => p = actual,
                    }
                }
            })
        })
        .collect();

    for h in handles {
        h.await.unwrap();
    }

    let observed_peak = peak_bytes.load(Ordering::Relaxed);
    assert!(
        observed_peak <= MAX_OUTBOX_BYTES,
        "F-RR-C26-01: pending_bytes peak {observed_peak} exceeded MAX_OUTBOX_BYTES {MAX_OUTBOX_BYTES}"
    );
}

/// F-RR-C25-05: pending_bytes 카운터가 flush 후 ack된 바이트만큼 감소하는지 검증.
#[tokio::test]
async fn pending_bytes_decremented_after_successful_flush() {
    use std::sync::atomic::Ordering;

    // Build the two items ONCE and reuse the same instances for both the
    // outbox and the initial-bytes estimate. `queued_item` stamps
    // `timestamp: Utc::now()`, whose RFC3339 serialization length varies with
    // trailing-zero trimming — estimating from freshly-built items (a second
    // `now()`) would diverge from the bytes `flush()` decrements, leaving a
    // few-byte residual (the F-RR-C25-05 flake first seen on the linux runner).
    let item1 = queued_item("1");
    let item2 = queued_item("2");
    let initial_bytes = estimate_queued_bytes(std::slice::from_ref(&item1))
        + estimate_queued_bytes(std::slice::from_ref(&item2));

    let outbox = Arc::new(MockOutbox {
        items: Arc::new(Mutex::new(VecDeque::from(vec![item1, item2]))),
        last_cursor: Arc::new(Mutex::new(None)),
    });
    let coordinator = IntegrationEgressCoordinator::new(
        Arc::new(MockSessionPort {
            state: Arc::new(Mutex::new(Some(connected_session()))),
        }),
        outbox.clone(),
        Arc::new(MockEgressTransport {
            acknowledged_queue_ids: vec!["queue-1".to_string(), "queue-2".to_string()],
            cursor: None,
        }),
        10,
    );

    coordinator
        .pending_bytes
        .store(initial_bytes, Ordering::Relaxed);

    let flushed = coordinator.flush().await.unwrap();
    assert_eq!(flushed, 2);

    let remaining = coordinator.pending_bytes();
    assert_eq!(
        remaining, 0,
        "F-RR-C25-05: 모든 아이템 ack 후 pending_bytes는 0이어야 함 (got {remaining})"
    );
}

/// F-RR-C27-01: enqueue 동시 호출과 flush 의 load→store 경합으로
/// pending_bytes 가 drift 하지 않는지 검증.
#[tokio::test]
async fn enqueue_concurrent_with_flush_no_pending_bytes_drift() {
    use std::sync::Arc as StdArc;

    const ENQUEUE_TASKS: usize = 20;

    struct AckAllTransport;

    #[async_trait::async_trait]
    impl IntegrationEgressTransportClient for AckAllTransport {
        async fn send_messages(
            &self,
            _session_id: &str,
            items: Vec<QueuedIntegrationEgressMessage>,
        ) -> Result<IntegrationEgressTransportResponse, CoreError> {
            Ok(IntegrationEgressTransportResponse {
                acknowledged_queue_ids: items.iter().map(|item| item.queue_id.clone()).collect(),
                ack_cursor: None,
            })
        }
    }

    let outbox = StdArc::new(MockOutbox {
        items: StdArc::new(Mutex::new(std::collections::VecDeque::new())),
        last_cursor: StdArc::new(Mutex::new(None)),
    });
    let coordinator = StdArc::new(IntegrationEgressCoordinator::new(
        StdArc::new(MockSessionPort {
            state: StdArc::new(Mutex::new(Some(connected_session()))),
        }),
        outbox.clone(),
        StdArc::new(AckAllTransport),
        usize::MAX / 2,
    ));

    let enqueue_handles: Vec<_> = (0..ENQUEUE_TASKS)
        .map(|i| {
            let c = coordinator.clone();
            tokio::spawn(async move {
                let envelope = IntegrationEnvelope {
                    envelope_id: format!("env-drift-{i}"),
                    schema_version: "integration.envelope.v1".to_string(),
                    message_type: IntegrationMessageType::InsightPacket,
                    timestamp: Utc::now(),
                    nonce: format!("nonce-drift-{i}"),
                    origin: IntegrationOrigin {
                        device_id: "device-1".to_string(),
                        workspace_id: None,
                        session_id: Some("session-1".to_string()),
                        source: "desktop-client".to_string(),
                    },
                    capability_scope: IntegrationCapabilityScope::InsightWrite,
                };
                let payload = IntegrationOutboundPayload::Insight(InsightPacket {
                    packet_id: format!("pkt-drift-{i}"),
                    summary: format!("summary-drift-{i}"),
                    derived_tags: vec!["focus".to_string()],
                    source_window: InsightSourceWindow {
                        started_at: Utc::now(),
                        ended_at: Utc::now(),
                    },
                    privacy_classification: IntegrationPrivacyClassification::DerivedSummary,
                    audit_reference_id: None,
                });
                c.enqueue_message(envelope, payload).await
            })
        })
        .collect();

    for h in enqueue_handles {
        h.await.unwrap().expect("enqueue should succeed");
    }

    let flushed = coordinator.flush().await.unwrap();
    assert_eq!(
        flushed, ENQUEUE_TASKS,
        "F-RR-C27-01: {ENQUEUE_TASKS}개 enqueue 후 flush 는 전부 ack 해야 함"
    );

    let remaining = coordinator.pending_bytes();
    assert_eq!(
        remaining, 0,
        "F-RR-C27-01: enqueue+flush race 후 pending_bytes drift 감지 (got {remaining})"
    );
}

/// F-QA-C27-03: fetch_add → 초과 → fetch_sub 롤백 정합성 검증.
#[tokio::test]
async fn enqueue_message_byte_cap_rollback_correctness() {
    use std::sync::atomic::Ordering as AtomOrd;

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
            acknowledged_queue_ids: vec![],
            cursor: None,
        }),
        1024,
    );

    // ── 단계 1: 첫 번째 메시지 정상 enqueue ────────────────────────────
    let small_envelope = IntegrationEnvelope {
        envelope_id: "env-rollback-first".to_string(),
        schema_version: "integration.envelope.v1".to_string(),
        message_type: IntegrationMessageType::InsightPacket,
        timestamp: Utc::now(),
        nonce: "nonce-rb-first".to_string(),
        origin: IntegrationOrigin {
            device_id: "device-rb".to_string(),
            workspace_id: None,
            session_id: Some("session-rb".to_string()),
            source: "desktop-client".to_string(),
        },
        capability_scope: IntegrationCapabilityScope::InsightWrite,
    };
    let small_payload = IntegrationOutboundPayload::Insight(InsightPacket {
        packet_id: "rb-first".to_string(),
        summary: "small".to_string(),
        derived_tags: vec![],
        source_window: InsightSourceWindow {
            started_at: Utc::now(),
            ended_at: Utc::now(),
        },
        privacy_classification: IntegrationPrivacyClassification::DerivedSummary,
        audit_reference_id: None,
    });

    coordinator
        .enqueue_message(small_envelope, small_payload)
        .await
        .expect("F-QA-C27-03: 첫 번째 소형 메시지 enqueue 성공 필수");

    let pre_reject_bytes = coordinator.pending_bytes.load(AtomOrd::Relaxed);
    assert!(
        pre_reject_bytes > 0,
        "F-QA-C27-03: 첫 번째 enqueue 후 pending_bytes 가 0보다 커야 함"
    );

    // ── 단계 2: cap 초과를 유발하도록 카운터를 MAX_OUTBOX_BYTES - 1 로 올린다 ─
    coordinator
        .pending_bytes
        .store(MAX_OUTBOX_BYTES - 1, AtomOrd::Relaxed);
    let pre_reject_bytes = coordinator.pending_bytes.load(AtomOrd::Relaxed);

    // ── 단계 3: cap 을 초과하는 메시지 enqueue 시도 ─────────────────────
    let big_envelope = IntegrationEnvelope {
        envelope_id: "env-rollback-over".to_string(),
        schema_version: "integration.envelope.v1".to_string(),
        message_type: IntegrationMessageType::InsightPacket,
        timestamp: Utc::now(),
        nonce: "nonce-rb-over".to_string(),
        origin: IntegrationOrigin {
            device_id: "device-rb".to_string(),
            workspace_id: None,
            session_id: Some("session-rb".to_string()),
            source: "desktop-client".to_string(),
        },
        capability_scope: IntegrationCapabilityScope::InsightWrite,
    };
    let big_payload = IntegrationOutboundPayload::Insight(InsightPacket {
        packet_id: "rb-over".to_string(),
        summary: "X".repeat(MAX_OUTBOX_BYTES + 1),
        derived_tags: vec![],
        source_window: InsightSourceWindow {
            started_at: Utc::now(),
            ended_at: Utc::now(),
        },
        privacy_classification: IntegrationPrivacyClassification::DerivedSummary,
        audit_reference_id: None,
    });

    let err = coordinator
        .enqueue_message(big_envelope, big_payload)
        .await
        .expect_err("F-QA-C27-03: cap 초과 메시지는 ServiceUnavailable 로 거부되어야 함");

    assert!(
        matches!(err, CoreError::ServiceUnavailable { .. }),
        "F-QA-C27-03: expected ServiceUnavailable, got: {err:?}"
    );

    // ── 단계 4: pending_bytes 가 거부 전 값으로 정확히 복원되었는지 확인 ─
    let post_reject_bytes = coordinator.pending_bytes.load(AtomOrd::Relaxed);
    assert_eq!(
        post_reject_bytes, pre_reject_bytes,
        "F-QA-C27-03: 거부 후 pending_bytes 롤백 불완전 \
         (pre={pre_reject_bytes}, post={post_reject_bytes}) — fetch_sub 누락 위험"
    );
}
