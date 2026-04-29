use chrono::{DateTime, Utc};
use oneshim_core::error::CoreError;
use oneshim_core::models::integration::{
    InsightPacket, IntegrationEnvelope, IntegrationOutboundPayload, IntegrationPromptReceipt,
    ProactivePrompt,
};
use serde::{Deserialize, Serialize};

const CLOUDEVENTS_SPEC_VERSION: &str = "1.0";
const INSIGHT_EVENT_TYPE: &str = "io.oneshim.integration.insight.v1";
const PROMPT_EVENT_TYPE: &str = "io.oneshim.integration.prompt.v1";
const PROMPT_RECEIPT_EVENT_TYPE: &str = "io.oneshim.integration.prompt_receipt.v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationCloudEvent<T> {
    pub specversion: String,
    pub id: String,
    pub source: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub subject: String,
    pub time: DateTime<Utc>,
    pub datacontenttype: String,
    pub data: T,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dataschema: Option<String>,
    pub oneshimscope: String,
    pub oneshimnonce: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oneshimsessionid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oneshimworkspaceid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oneshimprivacy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oneshimpromptcategory: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oneshimqueueid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationOutboundCloudEventBatch {
    pub items: Vec<IntegrationOutboundCloudEventBatchItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptCloudEventBatch {
    pub events: Vec<IntegrationCloudEvent<ProactivePrompt>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationOutboundCloudEventBatchItem {
    pub queue_id: String,
    pub event: IntegrationCloudEvent<serde_json::Value>,
}

pub fn insight_to_cloudevent(
    envelope: &IntegrationEnvelope,
    packet: &InsightPacket,
    queue_id: Option<&str>,
) -> IntegrationCloudEvent<InsightPacket> {
    IntegrationCloudEvent {
        specversion: CLOUDEVENTS_SPEC_VERSION.to_string(),
        id: envelope.envelope_id.clone(),
        source: format!("oneshim://devices/{}", envelope.origin.device_id),
        event_type: INSIGHT_EVENT_TYPE.to_string(),
        subject: packet.packet_id.clone(),
        time: envelope.timestamp,
        datacontenttype: "application/json".to_string(),
        data: packet.clone(),
        dataschema: Some(envelope.schema_version.clone()),
        oneshimscope: envelope.capability_scope.as_str().to_string(),
        oneshimnonce: envelope.nonce.clone(),
        oneshimsessionid: envelope.origin.session_id.clone(),
        oneshimworkspaceid: envelope.origin.workspace_id.clone(),
        oneshimprivacy: Some(match packet.privacy_classification {
            oneshim_core::models::integration::IntegrationPrivacyClassification::DeviceLocal => {
                "device_local"
            }
            oneshim_core::models::integration::IntegrationPrivacyClassification::DerivedSummary => {
                "derived_summary"
            }
            oneshim_core::models::integration::IntegrationPrivacyClassification::UserApprovedAttachment => {
                "user_approved_attachment"
            }
        }
        .to_string()),
        oneshimpromptcategory: None,
        oneshimqueueid: queue_id.map(str::to_string),
    }
}

pub fn prompt_from_cloudevent(
    event: IntegrationCloudEvent<ProactivePrompt>,
) -> Result<ProactivePrompt, CoreError> {
    if event.event_type != PROMPT_EVENT_TYPE {
        return Err(CoreError::Validation {
            code: oneshim_core::error_codes::ValidationCode::InvalidField,
            field: "integration.prompt.event_type".to_string(),
            message: format!("unsupported prompt event type: {}", event.event_type),
        });
    }
    Ok(event.data)
}

pub fn prompt_receipt_to_cloudevent(
    envelope: &IntegrationEnvelope,
    receipt: &IntegrationPromptReceipt,
    queue_id: Option<&str>,
) -> Result<IntegrationCloudEvent<serde_json::Value>, CoreError> {
    Ok(IntegrationCloudEvent {
        specversion: CLOUDEVENTS_SPEC_VERSION.to_string(),
        id: envelope.envelope_id.clone(),
        source: format!("oneshim://devices/{}", envelope.origin.device_id),
        event_type: PROMPT_RECEIPT_EVENT_TYPE.to_string(),
        subject: receipt.prompt_id.clone(),
        time: envelope.timestamp,
        datacontenttype: "application/json".to_string(),
        data: serde_json::to_value(receipt).map_err(|error| {
            CoreError::Serialization(serde_json::Error::io(std::io::Error::other(format!(
                "failed to serialize prompt receipt CloudEvent payload: {error}"
            ))))
        })?,
        dataschema: Some(envelope.schema_version.clone()),
        oneshimscope: envelope.capability_scope.as_str().to_string(),
        oneshimnonce: envelope.nonce.clone(),
        oneshimsessionid: envelope.origin.session_id.clone(),
        oneshimworkspaceid: envelope.origin.workspace_id.clone(),
        oneshimprivacy: None,
        oneshimpromptcategory: None,
        oneshimqueueid: queue_id.map(str::to_string),
    })
}

pub fn outbound_message_to_cloudevent(
    envelope: &IntegrationEnvelope,
    payload: &IntegrationOutboundPayload,
    queue_id: Option<&str>,
) -> Result<IntegrationCloudEvent<serde_json::Value>, CoreError> {
    match payload {
        IntegrationOutboundPayload::Insight(packet) => {
            serde_json::to_value(insight_to_cloudevent(envelope, packet, queue_id))
                .map_err(|error| {
                    CoreError::Serialization(serde_json::Error::io(std::io::Error::other(format!(
                        "failed to serialize insight CloudEvent payload: {error}"
                    ))))
                })
                .and_then(|value| {
                    serde_json::from_value(value).map_err(|error| {
                        CoreError::Serialization(serde_json::Error::io(std::io::Error::other(
                            format!("failed to normalize insight CloudEvent payload: {error}"),
                        )))
                    })
                })
        }
        IntegrationOutboundPayload::PromptReceipt(receipt) => {
            prompt_receipt_to_cloudevent(envelope, receipt, queue_id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::models::integration::{
        InsightSourceWindow, IntegrationCapabilityScope, IntegrationMessageType, IntegrationOrigin,
        IntegrationOutboundPayload, IntegrationPrivacyClassification, IntegrationPromptReceipt,
        IntegrationPromptReceiptAction, ProactivePromptCategory, ProactivePromptPriority,
        PromptProvenance,
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
                workspace_id: Some("workspace-1".to_string()),
                session_id: Some("session-1".to_string()),
                source: "desktop-client".to_string(),
            },
            capability_scope: IntegrationCapabilityScope::InsightWrite,
        }
    }

    #[test]
    fn insight_event_mapping_preserves_extensions() {
        let packet = InsightPacket {
            packet_id: "packet-1".to_string(),
            summary: "summary".to_string(),
            derived_tags: vec!["focus".to_string()],
            source_window: InsightSourceWindow {
                started_at: Utc::now(),
                ended_at: Utc::now(),
            },
            privacy_classification: IntegrationPrivacyClassification::DerivedSummary,
            audit_reference_id: None,
        };

        let event = insight_to_cloudevent(&sample_envelope(), &packet, Some("queue-1"));
        assert_eq!(event.event_type, INSIGHT_EVENT_TYPE);
        assert_eq!(event.oneshimscope, "insight:write");
        assert_eq!(event.oneshimprivacy.as_deref(), Some("derived_summary"));
        assert_eq!(event.oneshimqueueid.as_deref(), Some("queue-1"));
    }

    #[test]
    fn prompt_event_requires_supported_type() {
        let event = IntegrationCloudEvent {
            specversion: CLOUDEVENTS_SPEC_VERSION.to_string(),
            id: "prompt-env-1".to_string(),
            source: "oneshim://devices/device-1".to_string(),
            event_type: "io.oneshim.integration.prompt.v1".to_string(),
            subject: "prompt-1".to_string(),
            time: Utc::now(),
            datacontenttype: "application/json".to_string(),
            data: ProactivePrompt {
                prompt_id: "prompt-1".to_string(),
                category: ProactivePromptCategory::Task,
                title: "title".to_string(),
                body: "body".to_string(),
                priority: ProactivePromptPriority::Medium,
                actions: Vec::new(),
                expires_at: None,
                provenance: PromptProvenance {
                    source_system: "integration".to_string(),
                    source_actor: None,
                    correlation_id: None,
                },
            },
            dataschema: None,
            oneshimscope: "prompt:read".to_string(),
            oneshimnonce: "nonce-1".to_string(),
            oneshimsessionid: Some("session-1".to_string()),
            oneshimworkspaceid: None,
            oneshimprivacy: None,
            oneshimpromptcategory: Some("task".to_string()),
            oneshimqueueid: None,
        };

        assert_eq!(prompt_from_cloudevent(event).unwrap().prompt_id, "prompt-1");
    }

    #[test]
    fn outbound_prompt_receipt_maps_to_prompt_receipt_event_type() {
        let event = outbound_message_to_cloudevent(
            &IntegrationEnvelope {
                envelope_id: "receipt-env-1".to_string(),
                schema_version: "integration.prompt_receipt.v1".to_string(),
                message_type: IntegrationMessageType::PromptReceipt,
                timestamp: Utc::now(),
                nonce: "nonce-receipt-1".to_string(),
                origin: IntegrationOrigin {
                    device_id: "device-1".to_string(),
                    workspace_id: None,
                    session_id: Some("session-1".to_string()),
                    source: "desktop-client".to_string(),
                },
                capability_scope: IntegrationCapabilityScope::PromptAck,
            },
            &IntegrationOutboundPayload::PromptReceipt(IntegrationPromptReceipt {
                receipt_id: "receipt-1".to_string(),
                prompt_id: "prompt-1".to_string(),
                action: IntegrationPromptReceiptAction::Dismissed,
                occurred_at: Utc::now(),
                reason: Some("handled".to_string()),
            }),
            Some("queue-1"),
        )
        .unwrap();

        assert_eq!(event.event_type, PROMPT_RECEIPT_EVENT_TYPE);
        assert_eq!(event.subject, "prompt-1");
        assert_eq!(event.oneshimscope, "prompt:ack");
        assert_eq!(event.oneshimqueueid.as_deref(), Some("queue-1"));
    }
}
