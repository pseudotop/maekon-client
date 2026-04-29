mod connect;
mod egress;
mod inbox;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use oneshim_api_contracts::integration::IntegrationBootstrapResponse;
use oneshim_core::error::CoreError;
use oneshim_core::models::integration::{
    IntegrationAuthContext, IntegrationAuthScheme, IntegrationCapabilityScope,
    IntegrationTransportKind,
};
use oneshim_core::ports::integration::IntegrationAuthPort;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use tokio::sync::RwLock;

use crate::resilience::extract_retry_after;

use super::transport::{IntegrationRequestProofFactory, IntegrationTransportConnectRequest};
use super::WebSocketIntegrationSessionChannel;

#[derive(Debug, Clone)]
pub struct HttpsIntegrationTransportConfig {
    pub bootstrap_url: String,
    pub request_timeout: Duration,
}

impl HttpsIntegrationTransportConfig {
    pub fn new(bootstrap_url: impl Into<String>, request_timeout: Duration) -> Self {
        Self {
            bootstrap_url: bootstrap_url.into(),
            request_timeout,
        }
    }
}

#[derive(Clone)]
struct SessionBinding {
    heartbeat_url: Option<String>,
    disconnect_url: Option<String>,
    send_events_url: Option<String>,
    receive_prompts_url: Option<String>,
    auth: IntegrationAuthContext,
    live_session_channel: Option<Arc<WebSocketIntegrationSessionChannel>>,
}

#[derive(Clone, Default)]
pub struct HttpsIntegrationSessionBindings {
    sessions: Arc<RwLock<HashMap<String, SessionBinding>>>,
}

impl HttpsIntegrationSessionBindings {
    async fn insert(&self, session_id: String, binding: SessionBinding) {
        self.sessions.write().await.insert(session_id, binding);
    }

    async fn get(&self, session_id: &str) -> Option<SessionBinding> {
        self.sessions.read().await.get(session_id).cloned()
    }

    async fn remove(&self, session_id: &str) {
        self.sessions.write().await.remove(session_id);
    }
}

#[derive(Clone)]
struct HttpsIntegrationHttpShared {
    client: reqwest::Client,
    proof_factory: Arc<dyn IntegrationRequestProofFactory>,
    request_timeout: Duration,
}

pub struct HttpsIntegrationTransportClient {
    config: HttpsIntegrationTransportConfig,
    shared: HttpsIntegrationHttpShared,
    auth_port: Arc<dyn IntegrationAuthPort>,
    session_bindings: HttpsIntegrationSessionBindings,
}

pub struct HttpsIntegrationEgressTransportClient {
    shared: HttpsIntegrationHttpShared,
    session_bindings: HttpsIntegrationSessionBindings,
}

pub struct HttpsIntegrationInboxTransportClient {
    shared: HttpsIntegrationHttpShared,
    session_bindings: HttpsIntegrationSessionBindings,
}

impl HttpsIntegrationTransportClient {
    pub fn new(
        config: HttpsIntegrationTransportConfig,
        auth_port: Arc<dyn IntegrationAuthPort>,
        proof_factory: Arc<dyn IntegrationRequestProofFactory>,
    ) -> Result<Self, CoreError> {
        let request_timeout = config.request_timeout;
        let client = reqwest::Client::builder()
            .timeout(request_timeout)
            .build()
            .map_err(|error| CoreError::Network {
                code: oneshim_core::error_codes::NetworkCode::Generic,
                message: format!("Failed to build integration transport HTTP client: {error}"),
            })?;

        Ok(Self {
            config,
            shared: HttpsIntegrationHttpShared {
                client,
                proof_factory,
                request_timeout,
            },
            auth_port,
            session_bindings: HttpsIntegrationSessionBindings::default(),
        })
    }

    pub fn egress_transport(&self) -> HttpsIntegrationEgressTransportClient {
        HttpsIntegrationEgressTransportClient {
            shared: self.shared.clone(),
            session_bindings: self.session_bindings.clone(),
        }
    }

    pub fn inbox_transport(&self) -> HttpsIntegrationInboxTransportClient {
        HttpsIntegrationInboxTransportClient {
            shared: self.shared.clone(),
            session_bindings: self.session_bindings.clone(),
        }
    }
}

impl HttpsIntegrationHttpShared {
    async fn build_headers(
        &self,
        auth: &IntegrationAuthContext,
        method: &str,
        url: &str,
    ) -> Result<HeaderMap, CoreError> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let auth_value = match auth.scheme {
            IntegrationAuthScheme::BearerToken => format!("Bearer {}", auth.access_token),
            IntegrationAuthScheme::DpopBearer => format!("DPoP {}", auth.access_token),
        };
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&auth_value).map_err(|error| CoreError::Validation {
                code: oneshim_core::error_codes::ValidationCode::InvalidField,
                field: "integration.authorization".to_string(),
                message: format!("invalid authorization header value: {error}"),
            })?,
        );

        let maybe_proof = self.proof_factory.build_proof(auth, method, url).await?;
        if auth.scheme == IntegrationAuthScheme::DpopBearer {
            let proof = maybe_proof.ok_or_else(|| CoreError::Auth {
                code: oneshim_core::error_codes::AuthCode::Failed,
                message: "DPoP auth scheme requires a request proof, but none was provided."
                    .to_string(),
            })?;
            let name = HeaderName::from_bytes(proof.header_name.as_bytes()).map_err(|error| {
                CoreError::Validation {
                    code: oneshim_core::error_codes::ValidationCode::InvalidField,
                    field: "integration.request_proof.header_name".to_string(),
                    message: format!("invalid proof header name: {error}"),
                }
            })?;
            let value = HeaderValue::from_str(&proof.header_value).map_err(|error| {
                CoreError::Validation {
                    code: oneshim_core::error_codes::ValidationCode::InvalidField,
                    field: "integration.request_proof.header_value".to_string(),
                    message: format!("invalid proof header value: {error}"),
                }
            })?;
            headers.insert(name, value);
        } else if let Some(proof) = maybe_proof {
            let name = HeaderName::from_bytes(proof.header_name.as_bytes()).map_err(|error| {
                CoreError::Validation {
                    code: oneshim_core::error_codes::ValidationCode::InvalidField,
                    field: "integration.request_proof.header_name".to_string(),
                    message: format!("invalid proof header name: {error}"),
                }
            })?;
            let value = HeaderValue::from_str(&proof.header_value).map_err(|error| {
                CoreError::Validation {
                    code: oneshim_core::error_codes::ValidationCode::InvalidField,
                    field: "integration.request_proof.header_value".to_string(),
                    message: format!("invalid proof header value: {error}"),
                }
            })?;
            headers.insert(name, value);
        }

        Ok(headers)
    }

    async fn send_with_auth(
        &self,
        method: reqwest::Method,
        url: &str,
        auth: &IntegrationAuthContext,
        body: Option<&impl serde::Serialize>,
    ) -> Result<reqwest::Response, CoreError> {
        let headers = self.build_headers(auth, method.as_str(), url).await?;
        let mut request = self.client.request(method, url).headers(headers);
        if let Some(body) = body {
            request = request.json(body);
        }
        request.send().await.map_err(|error| {
            if error.is_timeout() {
                CoreError::RequestTimeout {
                    code: oneshim_core::error_codes::NetworkCode::Timeout,
                    timeout_ms: self.request_timeout.as_millis() as u64,
                }
            } else {
                CoreError::Network {
                    code: oneshim_core::error_codes::NetworkCode::Generic,
                    message: format!("integration transport request failed: {error}"),
                }
            }
        })
    }

    async fn check_response(
        &self,
        response: reqwest::Response,
        context: &str,
    ) -> Result<reqwest::Response, CoreError> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }

        let retry_after = extract_retry_after(&response);
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| String::from("<unreadable response body>"));

        match status.as_u16() {
            401 | 403 => Err(CoreError::Auth {
                code: oneshim_core::error_codes::AuthCode::Failed,
                message: format!("{context}: {body}"),
            }),
            404 => Err(CoreError::NotFound {
                code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                resource_type: context.to_string(),
                id: body,
            }),
            // 408/504 are timeout-class — wire code `network.timeout` (iter-55)
            408 | 504 => Err(CoreError::RequestTimeout {
                code: oneshim_core::error_codes::NetworkCode::Timeout,
                timeout_ms: 0, // sentinel: server-side timeout, unknown budget
            }),
            429 => Err(CoreError::RateLimit {
                code: oneshim_core::error_codes::NetworkCode::RateLimit,
                retry_after_secs: retry_after,
            }),
            // 502 Bad Gateway is a transient upstream failure (iter-55)
            502 | 503 => Err(CoreError::ServiceUnavailable {
                code: oneshim_core::error_codes::ServiceCode::Unavailable,
                message: body,
            }),
            _ => Err(CoreError::Network {
                code: oneshim_core::error_codes::NetworkCode::Generic,
                message: format!("{context}: HTTP {status} {body}"),
            }),
        }
    }

    fn validate_selected_transport(
        request: &IntegrationTransportConnectRequest,
        response: &IntegrationBootstrapResponse,
        transport_kind: &IntegrationTransportKind,
    ) -> Result<(), CoreError> {
        let client_supports = request.preferred_transports.contains(transport_kind);
        let server_advertises = response.supported_transports.is_empty()
            || response.supported_transports.contains(transport_kind);
        if client_supports && server_advertises {
            return Ok(());
        }

        Err(CoreError::Validation {
            code: oneshim_core::error_codes::ValidationCode::InvalidField,
            field: "integration.bootstrap.selected_transport".to_string(),
            message: format!(
                "server selected unsupported transport: {:?}",
                transport_kind
            ),
        })
    }

    fn validate_selected_auth_scheme(
        request: &IntegrationTransportConnectRequest,
        response: &IntegrationBootstrapResponse,
        auth_scheme: &IntegrationAuthScheme,
    ) -> Result<(), CoreError> {
        let client_supports = request.supported_auth_schemes.contains(auth_scheme);
        let server_advertises = response.supported_auth_schemes.is_empty()
            || response.supported_auth_schemes.contains(auth_scheme);
        if client_supports && server_advertises {
            return Ok(());
        }

        Err(CoreError::Validation {
            code: oneshim_core::error_codes::ValidationCode::InvalidField,
            field: "integration.bootstrap.selected_auth_scheme".to_string(),
            message: format!("server selected unsupported auth scheme: {:?}", auth_scheme),
        })
    }

    fn parse_granted_scopes(
        request: &IntegrationTransportConnectRequest,
        response: &IntegrationBootstrapResponse,
    ) -> Result<Vec<IntegrationCapabilityScope>, CoreError> {
        let mut granted = Vec::with_capacity(response.granted_scopes.len());
        for raw_scope in &response.granted_scopes {
            let scope = IntegrationCapabilityScope::parse(raw_scope).ok_or_else(|| {
                CoreError::Validation {
                    code: oneshim_core::error_codes::ValidationCode::InvalidField,
                    field: "integration.bootstrap.granted_scopes".to_string(),
                    message: format!("unknown granted scope: {raw_scope}"),
                }
            })?;
            if !request.requested_scopes.contains(&scope) {
                return Err(CoreError::Validation {
                    code: oneshim_core::error_codes::ValidationCode::InvalidField,
                    field: "integration.bootstrap.granted_scopes".to_string(),
                    message: format!("server granted an unexpected scope: {raw_scope}"),
                });
            }
            granted.push(scope);
        }
        Ok(granted)
    }
}

#[cfg(test)]
mod tests;
