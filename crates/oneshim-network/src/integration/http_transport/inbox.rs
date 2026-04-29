use super::HttpsIntegrationInboxTransportClient;
use crate::integration::prompt_from_cloudevent;
use crate::integration::transport::{
    IntegrationInboxTransportClient, IntegrationInboxTransportResponse,
};
use async_trait::async_trait;
use oneshim_core::error::CoreError;
use oneshim_core::models::integration::{IntegrationAckCursor, ProactivePrompt};
use std::time::Duration;

#[derive(Debug, serde::Serialize)]
struct PromptPullRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    after_stream_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    after_cursor: Option<String>,
    #[serde(default)]
    limit: usize,
}

#[derive(Debug, serde::Deserialize)]
struct PromptPullResponse {
    #[serde(default)]
    events: Vec<crate::integration::IntegrationCloudEvent<ProactivePrompt>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ack_cursor: Option<IntegrationAckCursor>,
}

#[async_trait]
impl IntegrationInboxTransportClient for HttpsIntegrationInboxTransportClient {
    async fn receive_prompts(
        &self,
        session_id: &str,
        after_cursor: Option<IntegrationAckCursor>,
        limit: usize,
    ) -> Result<IntegrationInboxTransportResponse, CoreError> {
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
            return Ok(IntegrationInboxTransportResponse {
                prompts: channel.drain_prompts(limit).await,
                ack_cursor: None,
            });
        }

        let url = binding
            .receive_prompts_url
            .ok_or_else(|| CoreError::Validation {
                code: oneshim_core::error_codes::ValidationCode::InvalidField,
                field: "integration.session.receive_prompts_url".to_string(),
                message: "active integration session does not have a prompt receive URL."
                    .to_string(),
            })?;

        let request = PromptPullRequest {
            after_stream_id: after_cursor.as_ref().map(|cursor| cursor.stream_id.clone()),
            after_cursor: after_cursor.map(|cursor| cursor.cursor),
            limit,
        };

        let response = self
            .shared
            .send_with_auth(reqwest::Method::POST, &url, &binding.auth, Some(&request))
            .await?;
        let response = self
            .shared
            .check_response(response, "integration prompt pull request failed")
            .await?;
        let payload: PromptPullResponse = response.json().await.map_err(|error| {
            CoreError::Serialization(serde_json::Error::io(std::io::Error::other(format!(
                "failed to parse integration prompt pull response: {error}"
            ))))
        })?;

        let mut prompts = Vec::with_capacity(payload.events.len());
        for event in payload.events {
            prompts.push(prompt_from_cloudevent(event)?);
        }

        Ok(IntegrationInboxTransportResponse {
            prompts,
            ack_cursor: payload.ack_cursor,
        })
    }

    async fn wait_for_remote_signal(
        &self,
        session_id: &str,
        timeout: Duration,
    ) -> Result<bool, CoreError> {
        let Some(binding) = self.session_bindings.get(session_id).await else {
            return Ok(false);
        };

        let Some(channel) = binding.live_session_channel else {
            tokio::time::sleep(timeout).await;
            return Ok(false);
        };

        channel.wait_for_prompt_signal(timeout).await
    }
}
