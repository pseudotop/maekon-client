//! gRPC context + suggestion client — Consumer Contract
//! (oneshim.client.v1.ClientContext + oneshim.client.v1.ClientSuggestion).

use oneshim_core::error::CoreError;
use tonic::transport::Channel;
use tracing::{debug, error, info};

use super::{map_grpc_status_error, GrpcConfig};
use crate::proto::client_v1::{
    client_context_client::ClientContextClient, client_suggestion_client::ClientSuggestionClient,
    FeedbackAction, SendFeedbackRequest, SubscribeRequest, SuggestionEvent, UploadBatchRequest,
    UploadBatchResponse,
};

/// Wraps both ClientContext (batch upload) and ClientSuggestion (subscribe/feedback) services.
pub struct GrpcContextClient {
    context_client: ClientContextClient<Channel>,
    suggestion_client: ClientSuggestionClient<Channel>,
    config: GrpcConfig,
}

impl GrpcContextClient {
    pub async fn connect(config: GrpcConfig) -> Result<Self, CoreError> {
        let endpoints = config.all_endpoints();
        let mut last_error: Option<crate::error::NetworkError> = None;

        for endpoint_url in &endpoints {
            info!(endpoint = %endpoint_url, "gRPC context client connection attempt");

            match config.connect_channel(endpoint_url).await {
                Ok(channel) => {
                    let context_client = ClientContextClient::new(channel.clone());
                    let suggestion_client = ClientSuggestionClient::new(channel);
                    info!(endpoint = %endpoint_url, "gRPC context client connection completed");
                    return Ok(Self {
                        context_client,
                        suggestion_client,
                        config,
                    });
                }
                Err(e) => {
                    debug!(endpoint = %endpoint_url, error = %e, "gRPC connection failure, next port attempt");
                    last_error = Some(e);
                }
            }
        }

        error!(endpoints = ?endpoints, "all gRPC endpoint connection failure");
        Err(last_error
            .unwrap_or_else(|| crate::error::NetworkError::Http("gRPC endpoint none".to_string()))
            .into())
    }

    /// Upload a batch of events and frame metadata.
    pub async fn upload_batch(
        &mut self,
        request: UploadBatchRequest,
    ) -> Result<UploadBatchResponse, CoreError> {
        debug!("gRPC batch upload request");

        let response = self
            .context_client
            .upload_batch(tonic::Request::new(request))
            .await
            .map_err(|status| {
                error!(error = %status, "gRPC batch upload failure");
                CoreError::from(map_grpc_status_error("grpc batch upload failed", status))
            })?;

        Ok(response.into_inner())
    }

    /// Subscribe to server-streamed suggestions.
    pub async fn subscribe_suggestions(
        &mut self,
        session_id: &str,
    ) -> Result<tonic::Streaming<SuggestionEvent>, CoreError> {
        debug!("gRPC suggestion stream subscribe request");

        let request = tonic::Request::new(SubscribeRequest {
            session_id: session_id.to_string(),
        });

        let response = self
            .suggestion_client
            .subscribe(request)
            .await
            .map_err(|status| {
                error!(error = %status, "gRPC suggestion stream subscribe failure");
                CoreError::from(map_grpc_status_error(
                    "grpc suggestion stream subscription failed",
                    status,
                ))
            })?;

        Ok(response.into_inner())
    }

    /// Send feedback on a suggestion.
    pub async fn send_feedback(
        &mut self,
        suggestion_id: &str,
        action: FeedbackAction,
        comment: Option<&str>,
    ) -> Result<(), CoreError> {
        debug!(suggestion_id = %suggestion_id, "gRPC feedback sent");

        let request = tonic::Request::new(SendFeedbackRequest {
            suggestion_id: suggestion_id.to_string(),
            action: action as i32,
            comment: comment.unwrap_or_default().to_string(),
        });

        self.suggestion_client
            .send_feedback(request)
            .await
            .map_err(|status| {
                error!(error = %status, "gRPC feedback sent failure");
                CoreError::from(map_grpc_status_error(
                    "grpc feedback submission failed",
                    status,
                ))
            })?;

        Ok(())
    }

    pub fn config(&self) -> &GrpcConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upload_batch_request() {
        let request = UploadBatchRequest {
            session_id: "session-456".to_string(),
            events: vec![],
            frames: vec![],
        };
        assert_eq!(request.session_id, "session-456");
    }
}
