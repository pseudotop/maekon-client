mod oidc_device_flow;
mod proof_factory;
mod static_auth;

pub use oidc_device_flow::{OidcDeviceFlowAuthConfig, OidcDeviceFlowIntegrationAuthPort};
pub use proof_factory::{
    Ed25519DpopProofFactory, NoopIntegrationRequestProofFactory,
    StaticIntegrationRequestProofFactory,
};
pub use static_auth::{EnvIntegrationAuthPort, StaticIntegrationAuthPort};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct OidcDeviceAuthorizationResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    expires_in: u64,
    #[serde(default)]
    interval: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct OidcTokenSuccessResponse {
    access_token: String,
    #[allow(dead_code)]
    token_type: String,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    refresh_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OidcTokenErrorResponse {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

#[cfg(test)]
mod tests;
