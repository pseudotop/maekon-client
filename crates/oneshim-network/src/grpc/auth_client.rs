//! gRPC auth client — Consumer Contract (oneshim.client.v1.ClientAuth).

use oneshim_core::error::CoreError;
use tonic::transport::Channel;
use tracing::{debug, error, info};

use super::{map_grpc_status_error, GrpcConfig};
use crate::proto::client_v1::{
    client_auth_client::ClientAuthClient, GetTokenRequest, RefreshTokenRequest, TokenResponse,
};

pub struct GrpcAuthClient {
    client: ClientAuthClient<Channel>,
    config: GrpcConfig,
}

impl GrpcAuthClient {
    pub async fn connect(config: GrpcConfig) -> Result<Self, CoreError> {
        let endpoints = config.all_endpoints();
        let mut last_error: Option<crate::error::NetworkError> = None;

        for endpoint_url in &endpoints {
            info!(endpoint = %endpoint_url, "gRPC auth client connection attempt");

            match config.connect_channel(endpoint_url).await {
                Ok(channel) => {
                    let client = ClientAuthClient::new(channel);
                    info!(endpoint = %endpoint_url, "gRPC auth client connection completed");
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

    pub async fn get_token(
        &mut self,
        identifier: &str,
        credential: &str,
        organization_id: &str,
    ) -> Result<TokenResponse, CoreError> {
        debug!(identifier = %identifier, "gRPC get_token request");

        let request = tonic::Request::new(GetTokenRequest {
            identifier: identifier.to_string(),
            credential: credential.to_string(),
            organization_id: organization_id.to_string(),
        });

        let response = self.client.get_token(request).await.map_err(|status| {
            error!(error = %status, "gRPC get_token failure");
            CoreError::from(map_grpc_status_error("grpc get_token failed", status))
        })?;

        Ok(response.into_inner())
    }

    pub async fn refresh_token(&mut self, refresh_token: &str) -> Result<TokenResponse, CoreError> {
        debug!("gRPC token refresh request");

        let request = tonic::Request::new(RefreshTokenRequest {
            refresh_token: refresh_token.to_string(),
        });

        let response = self.client.refresh_token(request).await.map_err(|status| {
            error!(error = %status, "gRPC token refresh failure");
            CoreError::from(map_grpc_status_error("grpc token refresh failed", status))
        })?;

        Ok(response.into_inner())
    }

    pub fn config(&self) -> &GrpcConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_grpc_auth_client_config() {
        let config = GrpcConfig::default();
        assert!(!config.use_grpc_auth);
    }

    #[test]
    fn test_get_token_request_creation() {
        let request = GetTokenRequest {
            identifier: "test@example.com".to_string(),
            credential: "test-credential-placeholder".to_string(),
            organization_id: "org-1".to_string(),
        };
        assert_eq!(request.identifier, "test@example.com");
    }
}
