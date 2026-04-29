//! gRPC health client — Consumer Contract (oneshim.client.v1.ClientHealth).
//! ```rust,ignore
//! use oneshim_network::grpc::{GrpcHealthClient, GrpcConfig};
//! let config = GrpcConfig::default();
//! let mut client = GrpcHealthClient::connect(config).await?;
//! let response = client.ping().await?;
//! println!("server_version={}, healthy={}", response.server_version, response.healthy);
//! if client.is_healthy().await {
//!     // server is reachable
//! }
//! ```

use tonic::transport::Channel;
use tracing::{debug, error, info};

use oneshim_core::error::CoreError;

use super::{map_grpc_status_error, GrpcConfig};
use crate::proto::client_v1::{
    client_health_client::ClientHealthClient, PingRequest, PingResponse,
};

/// Simple health check client using the Consumer Contract `ClientHealth.Ping` RPC.
pub struct GrpcHealthClient {
    client: ClientHealthClient<Channel>,
    #[allow(dead_code)]
    config: GrpcConfig,
}

impl GrpcHealthClient {
    pub async fn connect(config: GrpcConfig) -> Result<Self, CoreError> {
        let endpoints = config.all_endpoints();
        let mut last_error: Option<crate::error::NetworkError> = None;

        for endpoint_url in &endpoints {
            debug!("gRPC Health client connection attempt: {}", endpoint_url);

            match config.connect_channel(endpoint_url).await {
                Ok(channel) => {
                    info!("gRPC Health client connection: {}", endpoint_url);
                    let client = ClientHealthClient::new(channel);
                    return Ok(Self { client, config });
                }
                Err(e) => {
                    debug!(
                        "gRPC Health connection failure, next port attempt: {} - {}",
                        endpoint_url, e
                    );
                    last_error = Some(e);
                }
            }
        }

        error!("all gRPC endpoint connection failure: {:?}", endpoints);
        Err(last_error
            .unwrap_or_else(|| crate::error::NetworkError::Http("gRPC endpoint none".to_string()))
            .into())
    }

    /// Send a Ping RPC and return the server's response.
    pub async fn ping(&mut self) -> Result<PingResponse, CoreError> {
        debug!("Health ping request");

        let request = tonic::Request::new(PingRequest {});

        let response = self.client.ping(request).await.map_err(|status| {
            error!("Health ping failure: {}", status);
            CoreError::from(map_grpc_status_error("grpc health ping failed", status))
        })?;

        let inner = response.into_inner();
        debug!(
            "Health ping response: server_version={}, healthy={}",
            inner.server_version, inner.healthy
        );

        Ok(inner)
    }

    /// Convenience: returns `true` when the server responds with `healthy = true`.
    pub async fn is_healthy(&mut self) -> bool {
        matches!(self.ping().await, Ok(resp) if resp.healthy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ping_request_default() {
        let _request = PingRequest {};
        // PingRequest has no fields — just ensure it compiles.
    }

    #[test]
    fn test_ping_response_fields() {
        let response = PingResponse {
            server_version: "1.0.0".to_string(),
            healthy: true,
        };
        assert_eq!(response.server_version, "1.0.0");
        assert!(response.healthy);
    }
}
