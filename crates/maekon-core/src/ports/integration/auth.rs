//! Integration authentication ports.
//!
//! # Errors (all methods)
//! - `CoreError::Auth` (wire: `auth.failed`) — token rejection by the
//!   authorization server, expired refresh token, device-flow denial
//!   (user declined), unknown / expired `flow_id` on poll/cancel.
//! - `CoreError::RequestTimeout` (wire: `network.timeout`) — OIDC
//!   device-code polling exceeding authorization-server timeout.
//! - `CoreError::RateLimit` (wire: `network.rate_limit`) — 429 from
//!   the authorization server's token/device endpoint.
//! - `CoreError::Network` (wire: `network.generic`) — pre-response
//!   transport failures (DNS, refused connection) against the OIDC
//!   endpoint.
//! - `CoreError::Config` with `ConfigCode::Missing` (wire:
//!   `config.missing`) — auth profile not configured, required claims
//!   (client_id, token endpoint) absent.
//! - `CoreError::Storage` (wire: `storage.failed`) — auth-material
//!   persistence failure during `reset_auth_state`.
//! - `current_auth_status` does NOT surface "unauthenticated" as Err —
//!   it returns `IntegrationAuthStatus::Unauthenticated` instead.

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::integration::{
    IntegrationAuthContext, IntegrationAuthStatus, IntegrationCapabilityScope,
    IntegrationDeviceAuthorizationFlow,
};

#[async_trait]
pub trait IntegrationAuthPort: Send + Sync {
    /// Resolve outbound session auth material for the requested scopes and resource.
    async fn resolve_session_auth(
        &self,
        requested_scopes: &[IntegrationCapabilityScope],
        resource_indicator: Option<&str>,
    ) -> Result<IntegrationAuthContext, CoreError>;

    /// Return the current runtime status of the integration auth profile.
    async fn current_auth_status(&self) -> Result<IntegrationAuthStatus, CoreError>;

    /// Start a device authorization flow if the auth profile supports it.
    async fn start_device_authorization(
        &self,
        requested_scopes: &[IntegrationCapabilityScope],
        resource_indicator: Option<&str>,
    ) -> Result<IntegrationDeviceAuthorizationFlow, CoreError>;

    /// Poll a pending device authorization flow.
    async fn poll_device_authorization(
        &self,
        flow_id: &str,
    ) -> Result<IntegrationAuthStatus, CoreError>;

    /// Cancel a pending device authorization flow.
    async fn cancel_device_authorization(&self, flow_id: &str) -> Result<(), CoreError>;

    /// Clear locally persisted auth material and pending bootstrap state so the
    /// client can recover from a broken or stale device authorization flow.
    async fn reset_auth_state(&self) -> Result<(), CoreError>;
}
