use std::sync::Arc;

use async_trait::async_trait;
use oneshim_core::error::CoreError;
use oneshim_core::ports::integration::{
    IntegrationCheckpointStorePort, IntegrationEgressPort, IntegrationInsightProducerPort,
    IntegrationInsightSourcePort,
};

pub struct IntegrationInsightProducerCoordinator {
    source: Arc<dyn IntegrationInsightSourcePort>,
    checkpoint_store: Arc<dyn IntegrationCheckpointStorePort>,
    egress: Arc<dyn IntegrationEgressPort>,
    max_batch_size: usize,
}

impl IntegrationInsightProducerCoordinator {
    pub fn new(
        source: Arc<dyn IntegrationInsightSourcePort>,
        checkpoint_store: Arc<dyn IntegrationCheckpointStorePort>,
        egress: Arc<dyn IntegrationEgressPort>,
        max_batch_size: usize,
    ) -> Self {
        Self {
            source,
            checkpoint_store,
            egress,
            max_batch_size: max_batch_size.max(1),
        }
    }
}

#[async_trait]
impl IntegrationInsightProducerPort for IntegrationInsightProducerCoordinator {
    async fn produce_pending(&self) -> Result<usize, CoreError> {
        let namespace = self.source.checkpoint_namespace();
        let after_cursor = self.checkpoint_store.load_checkpoint(namespace).await?;
        let candidates = self
            .source
            .list_candidates_after(after_cursor, self.max_batch_size)
            .await?;

        let mut produced = 0usize;
        for candidate in candidates {
            self.egress
                .enqueue_insight(candidate.envelope, candidate.packet)
                .await?;
            self.checkpoint_store
                .store_checkpoint(namespace, candidate.source_cursor)
                .await?;
            produced += 1;
        }

        Ok(produced)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use chrono::Utc;
    use tokio::sync::Mutex;

    use super::*;
    use oneshim_core::models::integration::{
        InsightPacket, InsightSourceWindow, IntegrationAckCursor, IntegrationCapabilityScope,
        IntegrationEnvelope, IntegrationInsightCandidate, IntegrationMessageType,
        IntegrationOrigin, IntegrationPrivacyClassification,
    };

    struct MockSource {
        checkpoint_namespace: &'static str,
        candidates: Arc<Mutex<Vec<IntegrationInsightCandidate>>>,
        last_after_cursor: Arc<Mutex<Option<String>>>,
    }

    #[async_trait]
    impl IntegrationInsightSourcePort for MockSource {
        fn checkpoint_namespace(&self) -> &'static str {
            self.checkpoint_namespace
        }

        async fn list_candidates_after(
            &self,
            after_cursor: Option<String>,
            _limit: usize,
        ) -> Result<Vec<IntegrationInsightCandidate>, CoreError> {
            *self.last_after_cursor.lock().await = after_cursor;
            Ok(self.candidates.lock().await.clone())
        }
    }

    struct MockCheckpointStore {
        cursor: Arc<Mutex<Option<String>>>,
        stored: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl IntegrationCheckpointStorePort for MockCheckpointStore {
        async fn load_checkpoint(&self, _namespace: &str) -> Result<Option<String>, CoreError> {
            Ok(self.cursor.lock().await.clone())
        }

        async fn store_checkpoint(
            &self,
            _namespace: &str,
            cursor: String,
        ) -> Result<(), CoreError> {
            self.stored.lock().await.push(cursor.clone());
            *self.cursor.lock().await = Some(cursor);
            Ok(())
        }
    }

    struct MockEgress {
        packet_ids: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl IntegrationEgressPort for MockEgress {
        async fn enqueue_message(
            &self,
            _envelope: IntegrationEnvelope,
            payload: oneshim_core::models::integration::IntegrationOutboundPayload,
        ) -> Result<(), CoreError> {
            let oneshim_core::models::integration::IntegrationOutboundPayload::Insight(packet) =
                payload
            else {
                panic!("expected insight payload");
            };
            self.packet_ids.lock().await.push(packet.packet_id);
            Ok(())
        }

        async fn flush(&self) -> Result<usize, CoreError> {
            Ok(0)
        }

        async fn last_ack_cursor(&self) -> Result<Option<IntegrationAckCursor>, CoreError> {
            Ok(None)
        }
    }

    fn sample_candidate(id: &str) -> IntegrationInsightCandidate {
        IntegrationInsightCandidate {
            source_cursor: id.to_string(),
            envelope: IntegrationEnvelope {
                envelope_id: format!("env-{id}"),
                schema_version: "integration.envelope.v1".to_string(),
                message_type: IntegrationMessageType::InsightPacket,
                timestamp: Utc::now(),
                nonce: format!("nonce-{id}"),
                origin: IntegrationOrigin {
                    device_id: "device-1".to_string(),
                    workspace_id: None,
                    session_id: None,
                    source: "focus.local_suggestions".to_string(),
                },
                capability_scope: IntegrationCapabilityScope::InsightWrite,
            },
            packet: InsightPacket {
                packet_id: format!("packet-{id}"),
                summary: format!("summary-{id}"),
                derived_tags: vec!["focus".to_string()],
                source_window: InsightSourceWindow {
                    started_at: Utc::now(),
                    ended_at: Utc::now(),
                },
                privacy_classification: IntegrationPrivacyClassification::DerivedSummary,
                audit_reference_id: Some(format!("audit-{id}")),
            },
        }
    }

    #[tokio::test]
    async fn produce_pending_enqueues_and_advances_checkpoint_per_item() {
        let source = Arc::new(MockSource {
            checkpoint_namespace: "focus.local_suggestions",
            candidates: Arc::new(Mutex::new(vec![
                sample_candidate("1"),
                sample_candidate("2"),
            ])),
            last_after_cursor: Arc::new(Mutex::new(None)),
        });
        let checkpoint = Arc::new(MockCheckpointStore {
            cursor: Arc::new(Mutex::new(Some("0".to_string()))),
            stored: Arc::new(Mutex::new(Vec::new())),
        });
        let egress = Arc::new(MockEgress {
            packet_ids: Arc::new(Mutex::new(Vec::new())),
        });
        let coordinator = IntegrationInsightProducerCoordinator::new(
            source.clone(),
            checkpoint.clone(),
            egress.clone(),
            10,
        );

        let produced = coordinator.produce_pending().await.unwrap();

        assert_eq!(produced, 2);
        assert_eq!(
            source.last_after_cursor.lock().await.clone().as_deref(),
            Some("0")
        );
        assert_eq!(
            checkpoint.stored.lock().await.clone(),
            vec!["1".to_string(), "2".to_string()]
        );
        assert_eq!(
            egress.packet_ids.lock().await.clone(),
            vec!["packet-1".to_string(), "packet-2".to_string()]
        );
    }
}
