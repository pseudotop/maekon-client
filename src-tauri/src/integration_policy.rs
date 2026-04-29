use async_trait::async_trait;
use oneshim_core::error::CoreError;
use oneshim_core::models::integration::{
    InsightPacket, IntegrationEnvelope, IntegrationPrivacyClassification,
};
use oneshim_core::ports::integration::IntegrationEgressDecision;
use oneshim_core::ports::integration::IntegrationEgressPolicyPort;

#[derive(Debug, Default)]
#[cfg_attr(not(feature = "server"), allow(dead_code))]
pub struct DefaultIntegrationEgressPolicy;

#[async_trait]
impl IntegrationEgressPolicyPort for DefaultIntegrationEgressPolicy {
    async fn authorize_insight(
        &self,
        _envelope: &IntegrationEnvelope,
        packet: &InsightPacket,
    ) -> Result<IntegrationEgressDecision, CoreError> {
        let decision = match packet.privacy_classification {
            IntegrationPrivacyClassification::DerivedSummary => IntegrationEgressDecision::allow(),
            IntegrationPrivacyClassification::DeviceLocal => IntegrationEgressDecision::deny(
                "device-local insights must remain on-device and cannot be sent to integration backends",
            ),
            IntegrationPrivacyClassification::UserApprovedAttachment => {
                IntegrationEgressDecision::require_user_approval(
                    "integration delivery for user-approved attachments requires an explicit outbound approval path",
                )
            }
        };

        Ok(decision)
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use oneshim_core::models::integration::{
        InsightSourceWindow, IntegrationCapabilityScope, IntegrationMessageType, IntegrationOrigin,
    };

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

    fn sample_packet(classification: IntegrationPrivacyClassification) -> InsightPacket {
        InsightPacket {
            packet_id: "packet-1".to_string(),
            summary: "summary".to_string(),
            derived_tags: vec!["focus".to_string()],
            source_window: InsightSourceWindow {
                started_at: Utc::now(),
                ended_at: Utc::now(),
            },
            privacy_classification: classification,
            audit_reference_id: None,
        }
    }

    #[tokio::test]
    async fn derived_summary_is_allowed() {
        let policy = DefaultIntegrationEgressPolicy;
        let decision = policy
            .authorize_insight(
                &sample_envelope(),
                &sample_packet(IntegrationPrivacyClassification::DerivedSummary),
            )
            .await
            .unwrap();
        assert_eq!(
            decision.disposition,
            oneshim_core::models::integration::IntegrationEgressDisposition::Allow
        );
    }

    #[tokio::test]
    async fn device_local_is_denied() {
        let policy = DefaultIntegrationEgressPolicy;
        let decision = policy
            .authorize_insight(
                &sample_envelope(),
                &sample_packet(IntegrationPrivacyClassification::DeviceLocal),
            )
            .await
            .unwrap();
        assert_eq!(
            decision.disposition,
            oneshim_core::models::integration::IntegrationEgressDisposition::Deny
        );
    }
}
