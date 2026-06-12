use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use maekon_core::config::PiiFilterLevel;
use maekon_core::error::CoreError;
use maekon_core::error_codes::ConfigCode;
use maekon_core::models::integration::{
    InsightPacket, IntegrationAckCursor, IntegrationEgressDisposition, IntegrationEnvelope,
    IntegrationInsightAuditRecord, IntegrationOutboundPayload, IntegrationPromptReceipt,
};
use maekon_core::ports::integration::{
    IntegrationAuditPort, IntegrationEgressDecision, IntegrationEgressPolicyPort,
    IntegrationEgressPort,
};
use maekon_core::ports::pii_sanitizer::PiiSanitizer;

pub struct PolicyAwareIntegrationEgressCoordinator {
    inner: Arc<dyn IntegrationEgressPort>,
    policy: Arc<dyn IntegrationEgressPolicyPort>,
    audit: Arc<dyn IntegrationAuditPort>,
    /// D5 iter-12: sanitize InsightPacket.summary + derived_tags before
    /// egress to external integration backend (ERP/MES/CRM). Enterprise
    /// integrations are HIGH-risk for PII leaks.
    pii_sanitizer: Option<Arc<dyn PiiSanitizer>>,
    pii_level: PiiFilterLevel,
}

impl PolicyAwareIntegrationEgressCoordinator {
    pub fn new(
        inner: Arc<dyn IntegrationEgressPort>,
        policy: Arc<dyn IntegrationEgressPolicyPort>,
        audit: Arc<dyn IntegrationAuditPort>,
    ) -> Self {
        Self {
            inner,
            policy,
            audit,
            pii_sanitizer: None,
            pii_level: PiiFilterLevel::Strict,
        }
    }

    /// D5 iter-12: attach PII sanitizer. Default Strict (enterprise egress).
    pub fn with_pii_sanitizer(
        mut self,
        sanitizer: Arc<dyn PiiSanitizer>,
        level: PiiFilterLevel,
    ) -> Self {
        self.pii_sanitizer = Some(sanitizer);
        self.pii_level = level;
        self
    }

    fn sanitizer(&self) -> Result<&dyn PiiSanitizer, CoreError> {
        self.pii_sanitizer
            .as_deref()
            .ok_or_else(|| CoreError::Config {
                code: ConfigCode::Missing,
                message: "PII sanitizer not configured for integration egress".to_string(),
            })
    }

    /// D5 iter-12: produce a sanitized copy of an InsightPacket.
    fn sanitize_packet(&self, packet: &InsightPacket) -> Result<InsightPacket, CoreError> {
        let sanitizer = self.sanitizer()?;
        Ok(InsightPacket {
            packet_id: packet.packet_id.clone(),
            summary: sanitizer.sanitize_text(&packet.summary, self.pii_level),
            derived_tags: packet
                .derived_tags
                .iter()
                .map(|t| sanitizer.sanitize_text(t, self.pii_level))
                .collect(),
            source_window: packet.source_window.clone(),
            privacy_classification: packet.privacy_classification.clone(),
            audit_reference_id: packet.audit_reference_id.clone(),
        })
    }

    fn sanitize_prompt_receipt(
        &self,
        receipt: &IntegrationPromptReceipt,
    ) -> Result<IntegrationPromptReceipt, CoreError> {
        let sanitizer = self.sanitizer()?;
        Ok(IntegrationPromptReceipt {
            receipt_id: receipt.receipt_id.clone(),
            prompt_id: receipt.prompt_id.clone(),
            action: receipt.action.clone(),
            occurred_at: receipt.occurred_at,
            reason: receipt
                .reason
                .as_ref()
                .map(|reason| sanitizer.sanitize_text(reason, self.pii_level)),
        })
    }

    fn to_audit_record(
        envelope: &IntegrationEnvelope,
        packet: &InsightPacket,
        decision: &IntegrationEgressDecision,
    ) -> IntegrationInsightAuditRecord {
        IntegrationInsightAuditRecord {
            record_id: format!("{}:{}", envelope.envelope_id, packet.packet_id),
            envelope_id: envelope.envelope_id.clone(),
            packet_id: packet.packet_id.clone(),
            disposition: decision.disposition.clone(),
            reason: decision.reason.clone(),
            privacy_classification: packet.privacy_classification.clone(),
            capability_scope: envelope.capability_scope.clone(),
            occurred_at: Utc::now(),
        }
    }
}

#[async_trait]
impl IntegrationEgressPort for PolicyAwareIntegrationEgressCoordinator {
    async fn enqueue_message(
        &self,
        envelope: IntegrationEnvelope,
        payload: IntegrationOutboundPayload,
    ) -> Result<(), CoreError> {
        match &payload {
            IntegrationOutboundPayload::Insight(packet) => {
                let decision = self.policy.authorize_insight(&envelope, packet).await?;

                if decision.audit_required {
                    self.audit
                        .record_insight_decision(Self::to_audit_record(
                            &envelope, packet, &decision,
                        ))
                        .await?;
                }

                match decision.disposition {
                    IntegrationEgressDisposition::Allow => {
                        // D5 iter-12: sanitize packet before egress to external backend.
                        let sanitized_payload =
                            IntegrationOutboundPayload::Insight(self.sanitize_packet(packet)?);
                        self.inner
                            .enqueue_message(envelope, sanitized_payload)
                            .await
                    }
                    IntegrationEgressDisposition::Deny => Err(CoreError::PolicyDenied {
                        code: maekon_core::error_codes::PolicyCode::Denied,
                        message: decision
                            .reason
                            .unwrap_or_else(|| "integration egress denied".to_string()),
                    }),
                    IntegrationEgressDisposition::RequireUserApproval => {
                        Err(CoreError::ConsentRequired {
                            code: maekon_core::error_codes::ConsentCode::Required,
                            message: decision.reason.unwrap_or_else(|| {
                                "integration egress requires user approval".to_string()
                            }),
                        })
                    }
                }
            }
            IntegrationOutboundPayload::PromptReceipt(receipt) => {
                let decision = self
                    .policy
                    .authorize_prompt_receipt(&envelope, receipt)
                    .await?;

                match decision.disposition {
                    IntegrationEgressDisposition::Allow => {
                        let sanitized_payload = IntegrationOutboundPayload::PromptReceipt(
                            self.sanitize_prompt_receipt(receipt)?,
                        );
                        self.inner
                            .enqueue_message(envelope, sanitized_payload)
                            .await
                    }
                    IntegrationEgressDisposition::Deny => Err(CoreError::PolicyDenied {
                        code: maekon_core::error_codes::PolicyCode::Denied,
                        message: decision
                            .reason
                            .unwrap_or_else(|| "integration prompt receipt denied".to_string()),
                    }),
                    IntegrationEgressDisposition::RequireUserApproval => {
                        Err(CoreError::ConsentRequired {
                            code: maekon_core::error_codes::ConsentCode::Required,
                            message: decision.reason.unwrap_or_else(|| {
                                "integration prompt receipt requires user approval".to_string()
                            }),
                        })
                    }
                }
            }
        }
    }

    async fn flush(&self) -> Result<usize, CoreError> {
        self.inner.flush().await
    }

    async fn last_ack_cursor(&self) -> Result<Option<IntegrationAckCursor>, CoreError> {
        self.inner.last_ack_cursor().await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use chrono::Utc;
    use tokio::sync::Mutex;

    use super::*;
    use maekon_core::models::integration::{
        InsightSourceWindow, IntegrationCapabilityScope, IntegrationMessageType, IntegrationOrigin,
        IntegrationPrivacyClassification, IntegrationPromptReceipt, IntegrationPromptReceiptAction,
    };

    struct MockEgress {
        enqueued: Arc<Mutex<Vec<(IntegrationEnvelope, IntegrationOutboundPayload)>>>,
    }

    #[async_trait]
    impl IntegrationEgressPort for MockEgress {
        async fn enqueue_message(
            &self,
            envelope: IntegrationEnvelope,
            payload: IntegrationOutboundPayload,
        ) -> Result<(), CoreError> {
            self.enqueued.lock().await.push((envelope, payload));
            Ok(())
        }

        async fn flush(&self) -> Result<usize, CoreError> {
            Ok(0)
        }

        async fn last_ack_cursor(&self) -> Result<Option<IntegrationAckCursor>, CoreError> {
            Ok(None)
        }
    }

    struct MockPolicy {
        decision: IntegrationEgressDecision,
    }

    #[async_trait]
    impl IntegrationEgressPolicyPort for MockPolicy {
        async fn authorize_insight(
            &self,
            _envelope: &IntegrationEnvelope,
            _packet: &InsightPacket,
        ) -> Result<IntegrationEgressDecision, CoreError> {
            Ok(self.decision.clone())
        }
    }

    struct MockAudit {
        records: Arc<Mutex<Vec<IntegrationInsightAuditRecord>>>,
    }

    #[async_trait]
    impl IntegrationAuditPort for MockAudit {
        async fn record_insight_decision(
            &self,
            record: IntegrationInsightAuditRecord,
        ) -> Result<(), CoreError> {
            self.records.lock().await.push(record);
            Ok(())
        }

        async fn recent_insight_decisions(
            &self,
            limit: usize,
        ) -> Result<Vec<IntegrationInsightAuditRecord>, CoreError> {
            Ok(self
                .records
                .lock()
                .await
                .iter()
                .rev()
                .take(limit)
                .cloned()
                .collect())
        }
    }

    struct MockSanitizer;

    impl PiiSanitizer for MockSanitizer {
        fn sanitize_text(&self, text: &str, _level: PiiFilterLevel) -> String {
            text.replace("alice@example.com", "[EMAIL]")
                .replace("010-1234-5678", "[PHONE]")
        }
    }

    fn sample_envelope() -> IntegrationEnvelope {
        IntegrationEnvelope {
            envelope_id: "env-1".to_string(),
            schema_version: "integration.envelope.v1".to_string(),
            message_type: IntegrationMessageType::InsightPacket,
            timestamp: Utc::now(),
            nonce: "nonce-1".to_string(),
            origin: IntegrationOrigin {
                device_id: "device-1".to_string(),
                workspace_id: None,
                session_id: Some("session-1".to_string()),
                source: "desktop-client".to_string(),
            },
            capability_scope: IntegrationCapabilityScope::InsightWrite,
        }
    }

    fn sample_packet() -> InsightPacket {
        InsightPacket {
            packet_id: "packet-1".to_string(),
            summary: "summary".to_string(),
            derived_tags: vec!["focus".to_string()],
            source_window: InsightSourceWindow {
                started_at: Utc::now(),
                ended_at: Utc::now(),
            },
            privacy_classification: IntegrationPrivacyClassification::DerivedSummary,
            audit_reference_id: Some("audit-ref-1".to_string()),
        }
    }

    fn sample_receipt() -> IntegrationPromptReceipt {
        IntegrationPromptReceipt {
            receipt_id: "receipt-1".to_string(),
            prompt_id: "prompt-1".to_string(),
            action: IntegrationPromptReceiptAction::Dismissed,
            occurred_at: Utc::now(),
            reason: Some("user alice@example.com dismissed from 010-1234-5678".to_string()),
        }
    }

    #[tokio::test]
    async fn enqueue_allows_and_audits_when_policy_allows() {
        let enqueued = Arc::new(Mutex::new(Vec::new()));
        let records = Arc::new(Mutex::new(Vec::new()));
        let coordinator = PolicyAwareIntegrationEgressCoordinator::new(
            Arc::new(MockEgress {
                enqueued: enqueued.clone(),
            }),
            Arc::new(MockPolicy {
                decision: IntegrationEgressDecision::allow(),
            }),
            Arc::new(MockAudit {
                records: records.clone(),
            }),
        )
        .with_pii_sanitizer(Arc::new(MockSanitizer), PiiFilterLevel::Strict);

        coordinator
            .enqueue_insight(sample_envelope(), sample_packet())
            .await
            .unwrap();

        assert_eq!(enqueued.lock().await.len(), 1);
        let records = records.lock().await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].disposition, IntegrationEgressDisposition::Allow);
    }

    #[tokio::test]
    async fn enqueue_insight_fails_closed_without_pii_sanitizer() {
        let enqueued = Arc::new(Mutex::new(Vec::new()));
        let records = Arc::new(Mutex::new(Vec::new()));
        let coordinator = PolicyAwareIntegrationEgressCoordinator::new(
            Arc::new(MockEgress {
                enqueued: enqueued.clone(),
            }),
            Arc::new(MockPolicy {
                decision: IntegrationEgressDecision::allow(),
            }),
            Arc::new(MockAudit {
                records: records.clone(),
            }),
        );

        let err = coordinator
            .enqueue_insight(sample_envelope(), sample_packet())
            .await
            .expect_err("missing egress sanitizer should fail closed");

        assert!(matches!(err, CoreError::Config { .. }));
        assert!(enqueued.lock().await.is_empty());
    }

    #[tokio::test]
    async fn enqueue_prompt_receipt_sanitizes_reason_before_queueing() {
        let enqueued = Arc::new(Mutex::new(Vec::new()));
        let records = Arc::new(Mutex::new(Vec::new()));
        let coordinator = PolicyAwareIntegrationEgressCoordinator::new(
            Arc::new(MockEgress {
                enqueued: enqueued.clone(),
            }),
            Arc::new(MockPolicy {
                decision: IntegrationEgressDecision::allow(),
            }),
            Arc::new(MockAudit {
                records: records.clone(),
            }),
        )
        .with_pii_sanitizer(Arc::new(MockSanitizer), PiiFilterLevel::Strict);

        coordinator
            .enqueue_message(
                sample_envelope(),
                IntegrationOutboundPayload::PromptReceipt(sample_receipt()),
            )
            .await
            .unwrap();

        let enqueued = enqueued.lock().await;
        assert_eq!(enqueued.len(), 1);
        let IntegrationOutboundPayload::PromptReceipt(receipt) = &enqueued[0].1 else {
            panic!("expected prompt receipt payload");
        };
        assert_eq!(
            receipt.reason.as_deref(),
            Some("user [EMAIL] dismissed from [PHONE]")
        );
    }

    #[tokio::test]
    async fn enqueue_denied_by_policy_does_not_queue() {
        let enqueued = Arc::new(Mutex::new(Vec::new()));
        let records = Arc::new(Mutex::new(Vec::new()));
        let coordinator = PolicyAwareIntegrationEgressCoordinator::new(
            Arc::new(MockEgress {
                enqueued: enqueued.clone(),
            }),
            Arc::new(MockPolicy {
                decision: IntegrationEgressDecision::deny("policy blocked"),
            }),
            Arc::new(MockAudit {
                records: records.clone(),
            }),
        );

        let err = coordinator
            .enqueue_insight(sample_envelope(), sample_packet())
            .await
            .expect_err("enqueue should fail");

        assert!(matches!(err, CoreError::PolicyDenied { .. }));
        assert!(enqueued.lock().await.is_empty());
        assert_eq!(records.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn enqueue_requires_user_approval_without_queueing() {
        let enqueued = Arc::new(Mutex::new(Vec::new()));
        let records = Arc::new(Mutex::new(Vec::new()));
        let coordinator = PolicyAwareIntegrationEgressCoordinator::new(
            Arc::new(MockEgress {
                enqueued: enqueued.clone(),
            }),
            Arc::new(MockPolicy {
                decision: IntegrationEgressDecision::require_user_approval(
                    "requires explicit consent",
                ),
            }),
            Arc::new(MockAudit {
                records: records.clone(),
            }),
        );

        let err = coordinator
            .enqueue_insight(sample_envelope(), sample_packet())
            .await
            .expect_err("enqueue should fail");

        assert!(matches!(err, CoreError::ConsentRequired { .. }));
        assert!(enqueued.lock().await.is_empty());
        assert_eq!(records.lock().await.len(), 1);
    }
}
