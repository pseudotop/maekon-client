use super::HttpsIntegrationInboxTransportClient;
use crate::integration::prompt_from_cloudevent;
use crate::integration::transport::{
    IntegrationInboxTransportClient, IntegrationInboxTransportResponse,
};
use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::models::integration::{IntegrationAckCursor, ProactivePrompt};
use std::time::Duration;
use tracing::warn;

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
                    code: maekon_core::error_codes::NotFoundCode::ResourceMissing,
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
                code: maekon_core::error_codes::ValidationCode::InvalidField,
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
        // #6940: cap the response body before parse (OOM guard, see egress.rs).
        let body = crate::outbound::read_body_capped(
            response,
            crate::outbound::MAX_INTEGRATION_RESPONSE_BYTES,
        )
        .await
        .map_err(super::map_integration_body_error)?;
        let payload: PromptPullResponse = serde_json::from_slice(&body).map_err(|error| {
            CoreError::Serialization(serde_json::Error::io(std::io::Error::other(format!(
                "failed to parse integration prompt pull response: {error}"
            ))))
        })?;

        let mut prompts = Vec::with_capacity(payload.events.len());
        for event in payload.events {
            // Log-and-skip per event (matching the live_channel WS path): a single
            // malformed/unsupported event must not abort the whole pull and discard
            // every valid sibling prompt in the same page.
            match prompt_from_cloudevent(event) {
                Ok(prompt) => prompts.push(prompt),
                Err(err) => warn!("integration prompt pull: skipping unparseable event: {err}"),
            }
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
            // #7617 (MED finding #3 / RL-01 sibling): park instead of
            // returning instantly. The upstream inbox_coordinator readiness
            // check normally prevents reaching this branch with a genuinely
            // unready session, but a transient binding-map race (e.g. a
            // concurrent reconnect evicting this session's binding) could
            // still hit it -- an instant `Ok(false)` here would reproduce the
            // same busy-spin as the coordinator-level bug this call is meant
            // to guard against.
            tokio::time::sleep(timeout).await;
            return Ok(false);
        };

        let Some(channel) = binding.live_session_channel else {
            tokio::time::sleep(timeout).await;
            return Ok(false);
        };

        channel.wait_for_prompt_signal(timeout).await
    }
}
