//! gRPC auth client — Consumer Contract (oneshim.client.v1.ClientAuth).

use maekon_core::error::CoreError;
use tonic::transport::Channel;
use tracing::{debug, error, info};

use super::{map_grpc_status_error, GrpcConfig};
use crate::proto::client_v1::{
    client_auth_client::ClientAuthClient, GetTokenRequest, RefreshTokenRequest, TokenResponse,
};

// #6442 F11: Clone lets UnifiedClient clone this out of its Mutex and drop the guard
// before the RPC await — the inner tonic client + GrpcConfig are both cheap to clone
// (the tonic client is an HTTP/2 channel handle).
#[derive(Clone)]
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

    /// Build the refresh payload — split out so the field mapping is unit-testable
    /// without a live server (the RPC itself needs one).
    fn build_refresh_request(refresh_token: &str, organization_id: &str) -> RefreshTokenRequest {
        RefreshTokenRequest {
            refresh_token: refresh_token.to_string(),
            organization_id: organization_id.to_string(),
        }
    }

    /// Reject a missing/blank tenant scope locally instead of paying a server
    /// round-trip that can only answer UNAUTHENTICATED.
    ///
    /// The natural source is `TokenManager::session_info().organization_id`, which is
    /// `Option<String>` — an unset session (or a `.unwrap_or_default()` at the callsite)
    /// would otherwise send `""` and fail server-side.
    fn require_org_scope(organization_id: Option<&str>) -> Result<&str, CoreError> {
        organization_id
            .map(str::trim)
            .filter(|org| !org.is_empty())
            .ok_or_else(|| CoreError::Auth {
                code: maekon_core::error_codes::AuthCode::Failed,
                message: "gRPC token refresh needs an organization scope — no session organization_id (re-login required)".to_string(),
            })
    }

    /// Refresh the token pair.
    ///
    /// `organization_id` is required (#9506): `/oneshim.client.v1.ClientAuth/RefreshToken`
    /// is a public RPC, so the server's auth middleware injects no `x-org-id`
    /// metadata and the request field is the only tenant scope the handler sees.
    /// Pass the value login recorded — `TokenManager::session_info().organization_id`
    /// (`Option<String>`); `None`/blank fails locally rather than server-side.
    ///
    /// Status (#9506): this path is **wired but not yet called** — the only wrapper,
    /// `UnifiedClient::refresh_token`, still routes refresh over REST by design.
    /// Switching that transport is a separate change (token persistence/rotation).
    pub async fn refresh_token(
        &mut self,
        refresh_token: &str,
        organization_id: Option<&str>,
    ) -> Result<TokenResponse, CoreError> {
        debug!("gRPC token refresh request");

        let organization_id = Self::require_org_scope(organization_id)?;
        let request =
            tonic::Request::new(Self::build_refresh_request(refresh_token, organization_id));

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

    /// #9506: refresh must carry the tenant scope in the request body — the RPC is
    /// public, so no `x-org-id` metadata reaches the server handler and an unset
    /// (or swapped) field makes the server reject every refresh as UNAUTHENTICATED.
    #[test]
    fn test_build_refresh_request_maps_organization_id() {
        let request = GrpcAuthClient::build_refresh_request("refresh-token-placeholder", "org-1");

        assert_eq!(request.refresh_token, "refresh-token-placeholder");
        assert_eq!(request.organization_id, "org-1");
    }

    /// #9506 review: a missing/blank scope must fail locally — a server round-trip
    /// could only answer UNAUTHENTICATED.
    #[test]
    fn test_require_org_scope_rejects_missing_and_blank() {
        for organization_id in [None, Some(""), Some("   ")] {
            let error = GrpcAuthClient::require_org_scope(organization_id).unwrap_err();

            match error {
                CoreError::Auth { code, message } => {
                    assert_eq!(code.as_str(), "auth.failed");
                    assert_eq!(
                        message,
                        "gRPC token refresh needs an organization scope — no session organization_id (re-login required)"
                    );
                }
                other => panic!("expected CoreError::Auth, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_require_org_scope_trims_accepted_value() {
        assert_eq!(
            GrpcAuthClient::require_org_scope(Some(" org-1 ")).expect("scope accepted"),
            "org-1"
        );
    }
}
