use super::HttpsIntegrationEgressTransportClient;
use crate::integration::transport::{
    IntegrationEgressTransportClient, IntegrationEgressTransportResponse,
};
use crate::integration::{
    outbound_message_to_cloudevent, IntegrationOutboundCloudEventBatch,
    IntegrationOutboundCloudEventBatchItem,
};
use async_trait::async_trait;
use oneshim_core::error::CoreError;
use oneshim_core::models::integration::{IntegrationAckCursor, QueuedIntegrationEgressMessage};

#[async_trait]
impl IntegrationEgressTransportClient for HttpsIntegrationEgressTransportClient {
    async fn send_messages(
        &self,
        session_id: &str,
        items: Vec<QueuedIntegrationEgressMessage>,
    ) -> Result<IntegrationEgressTransportResponse, CoreError> {
        let binding =
            self.session_bindings
                .get(session_id)
                .await
                .ok_or_else(|| CoreError::NotFound {
                    code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                    resource_type: "integration_session".to_string(),
                    id: session_id.to_string(),
                })?;
        if let Some(channel) = binding.live_session_channel.clone() {
            for item in &items {
                channel
                    .send_json(&outbound_message_to_cloudevent(
                        &item.envelope,
                        &item.payload,
                        Some(&item.queue_id),
                    )?)
                    .await?;
            }
            let expected_queue_ids = items
                .iter()
                .map(|item| item.queue_id.clone())
                .collect::<Vec<_>>();
            return channel
                .wait_for_outbound_ack(&expected_queue_ids, self.shared.request_timeout)
                .await;
        }

        let url = binding
            .send_events_url
            .ok_or_else(|| CoreError::Validation {
                code: oneshim_core::error_codes::ValidationCode::InvalidField,
                field: "integration.session.send_events_url".to_string(),
                message: "active integration session does not have an outbound event URL."
                    .to_string(),
            })?;

        let mut batch_items = Vec::with_capacity(items.len());
        for item in &items {
            batch_items.push(IntegrationOutboundCloudEventBatchItem {
                queue_id: item.queue_id.clone(),
                event: outbound_message_to_cloudevent(
                    &item.envelope,
                    &item.payload,
                    Some(&item.queue_id),
                )?,
            });
        }
        let batch = IntegrationOutboundCloudEventBatch { items: batch_items };

        let response = self
            .shared
            .send_with_auth(reqwest::Method::POST, &url, &binding.auth, Some(&batch))
            .await?;
        let response = self
            .shared
            .check_response(response, "integration outbound event request failed")
            .await?;

        #[derive(serde::Deserialize)]
        struct OutboundEventResponseBody {
            #[serde(default)]
            accepted_ids: Vec<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            ack_cursor: Option<IntegrationAckCursor>,
        }

        let payload: OutboundEventResponseBody = response.json().await.map_err(|error| {
            CoreError::Serialization(serde_json::Error::io(std::io::Error::other(format!(
                "failed to parse integration outbound event response: {error}"
            ))))
        })?;

        Ok(IntegrationEgressTransportResponse {
            acknowledged_queue_ids: payload.accepted_ids,
            ack_cursor: payload.ack_cursor,
        })
    }
}
