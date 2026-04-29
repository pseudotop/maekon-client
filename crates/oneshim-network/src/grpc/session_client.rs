//! gRPC session client — Consumer Contract (oneshim.client.v1.ClientSession).

use oneshim_core::error::CoreError;
use std::collections::HashMap;
use tonic::transport::Channel;
use tracing::{debug, error, info};

use super::{map_grpc_status_error, GrpcConfig};
use crate::proto::client_v1::{
    client_session_client::ClientSessionClient, CreateSessionRequest, CreateSessionResponse,
    EndSessionRequest, HeartbeatRequest,
};

pub struct GrpcSessionClient {
    client: ClientSessionClient<Channel>,
    config: GrpcConfig,
}

impl GrpcSessionClient {
    pub async fn connect(config: GrpcConfig) -> Result<Self, CoreError> {
        let endpoints = config.all_endpoints();
        let mut last_error: Option<crate::error::NetworkError> = None;

        for endpoint_url in &endpoints {
            info!(endpoint = %endpoint_url, "gRPC session client connection attempt");

            match config.connect_channel(endpoint_url).await {
                Ok(channel) => {
                    let client = ClientSessionClient::new(channel);
                    info!(endpoint = %endpoint_url, "gRPC session client connection completed");
                    return Ok(Self { client, config });
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

    pub async fn create_session(
        &mut self,
        client_id: &str,
        metadata: HashMap<String, String>,
    ) -> Result<CreateSessionResponse, CoreError> {
        debug!(client_id = %client_id, "gRPC session create request");

        let request = tonic::Request::new(CreateSessionRequest {
            client_id: client_id.to_string(),
            metadata,
        });

        let response = self
            .client
            .create_session(request)
            .await
            .map_err(|status| {
                error!(error = %status, "gRPC session create failure");
                CoreError::from(map_grpc_status_error(
                    "grpc session creation failed",
                    status,
                ))
            })?;

        Ok(response.into_inner())
    }

    pub async fn end_session(&mut self, session_id: &str) -> Result<(), CoreError> {
        debug!(session_id = %session_id, "gRPC session ended request");

        let request = tonic::Request::new(EndSessionRequest {
            session_id: session_id.to_string(),
        });

        self.client.end_session(request).await.map_err(|status| {
            error!(error = %status, "gRPC session ended failure");
            CoreError::from(map_grpc_status_error(
                "grpc session termination failed",
                status,
            ))
        })?;

        Ok(())
    }

    pub async fn heartbeat(&mut self, session_id: &str) -> Result<(), CoreError> {
        debug!(session_id = %session_id, "gRPC heartbeat sent");

        let request = tonic::Request::new(HeartbeatRequest {
            session_id: session_id.to_string(),
        });

        self.client.heartbeat(request).await.map_err(|status| {
            error!(error = %status, "gRPC heartbeat failure");
            CoreError::from(map_grpc_status_error(
                "grpc session heartbeat failed",
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
    fn test_create_session_request() {
        let request = CreateSessionRequest {
            client_id: "client-123".to_string(),
            metadata: HashMap::new(),
        };
        assert_eq!(request.client_id, "client-123");
    }

    #[test]
    fn test_heartbeat_request() {
        let request = HeartbeatRequest {
            session_id: "session-123".to_string(),
        };
        assert_eq!(request.session_id, "session-123");
    }
}
