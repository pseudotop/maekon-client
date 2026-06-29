mod connect;
mod egress;
mod inbox;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use maekon_api_contracts::integration::IntegrationBootstrapResponse;
use maekon_core::error::CoreError;
use maekon_core::models::integration::{
    IntegrationAuthContext, IntegrationAuthScheme, IntegrationCapabilityScope,
    IntegrationTransportKind,
};
use maekon_core::ports::integration::IntegrationAuthPort;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use tokio::sync::RwLock;

use crate::provider_error_body::provider_error_body_state;
use crate::resilience::extract_retry_after;

use super::transport::{IntegrationRequestProofFactory, IntegrationTransportConnectRequest};
use super::WebSocketIntegrationSessionChannel;

/// #6940: map a capped-body-read error (outbound::BodyReadError) to the integration
/// transport's CoreError. Transport read failures and cap breaches both surface as
/// `CoreError::Network` (the parse step keeps its own `CoreError::Serialization`).
pub(super) fn map_integration_body_error(e: crate::outbound::BodyReadError) -> CoreError {
    let message = match e {
        crate::outbound::BodyReadError::Transport(err) => {
            format!("read integration response body: {err}")
        }
        crate::outbound::BodyReadError::TooLarge { len, cap } => {
            format!("integration response exceeded cap {cap} bytes (len {len})")
        }
    };
    CoreError::Network {
        code: maekon_core::error_codes::NetworkCode::Generic,
        message,
    }
}

/// Reject a transport-downgraded integration URL before any credential leaves
/// the device. Cleartext `http://`/`ws://` to a **non-loopback** host would
/// egress Bearer/DPoP tokens and payloads unencrypted (transport downgrade);
/// `https://`/`wss://` are always allowed, and cleartext is permitted only to
/// loopback development endpoints. Mirrors the SSE/HTTP `validated_base_url`
/// invariant (`build_reqwest_client_for_url`) — fail-closed (#6824).
pub(crate) fn reject_cleartext_remote_url(url: &str, field: &str) -> Result<(), CoreError> {
    // Decide "cleartext" from the PARSED scheme (what reqwest/tungstenite will
    // actually connect to), NOT a string prefix: the WHATWG URL parser strips
    // leading C0 control bytes before parsing, so a prefix test on e.g.
    // "\u{0}http://evil" would mis-classify it as non-cleartext and let a
    // server-controlled URL downgrade transport. Unparseable → fail-closed
    // (reqwest/tungstenite would also fail to connect, so nothing egresses).
    // (The WS `channel_url` path is also guarded here via `url`; tungstenite
    // additionally parses it with the stricter `http::Uri`, which rejects the
    // same control-prefixed vectors outright — both fail closed.)
    let is_cleartext = reqwest::Url::parse(url)
        .map(|parsed| matches!(parsed.scheme(), "http" | "ws"))
        .unwrap_or(false);
    if is_cleartext && !crate::http_client::host_is_loopback(url) {
        return Err(CoreError::Validation {
            code: maekon_core::error_codes::ValidationCode::InvalidField,
            field: field.to_string(),
            // Do not interpolate the raw (possibly server-controlled) URL into a
            // message that may be logged/persisted; `field` identifies the site.
            message: "remote cleartext URL is not allowed for integration transport; use https:// / wss:// (cleartext is permitted only for loopback development endpoints)".to_string(),
        });
    }
    Ok(())
}

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

    /// Remove the binding for `session_id` and, if it owned a live WebSocket
    /// channel, signal that channel to close so its detached read_loop task and
    /// TCP socket are released. Returns `true` when a binding was evicted.
    ///
    /// The map write lock is released before awaiting `close()` so the lock is
    /// never held across an `.await`. Dropping the removed `SessionBinding` (and
    /// thus the last `Arc<WebSocketIntegrationSessionChannel>` it held) also
    /// fires the channel's `Drop` cancel signal as a backstop. (#6204)
    async fn evict(&self, session_id: &str) -> bool {
        let removed = self.sessions.write().await.remove(session_id);
        match removed {
            Some(binding) => {
                if let Some(channel) = binding.live_session_channel {
                    if let Err(error) = channel.close().await {
                        // Best-effort: the read_loop is still aborted via
                        // cancel_notify even when the Close frame send fails
                        // (e.g. the socket is already gone on reconnect).
                        tracing::debug!(
                            session_id,
                            "integration binding eviction: live channel close failed: {error}"
                        );
                    }
                }
                true
            }
            None => false,
        }
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
            // Disable redirect following: integration transport URLs are direct
            // API endpoints, and a server-controlled 30x to a cleartext host
            // would bypass the per-request scheme check (reqwest does not strip
            // the custom DPoP header on redirect and re-sends the body on
            // 307/308), re-opening the transport-downgrade hole (#6824).
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| CoreError::Network {
                code: maekon_core::error_codes::NetworkCode::Generic,
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
                code: maekon_core::error_codes::ValidationCode::InvalidField,
                field: "integration.authorization".to_string(),
                message: format!("invalid authorization header value: {error}"),
            })?,
        );

        let maybe_proof = self.proof_factory.build_proof(auth, method, url).await?;
        if auth.scheme == IntegrationAuthScheme::DpopBearer {
            let proof = maybe_proof.ok_or_else(|| CoreError::Auth {
                code: maekon_core::error_codes::AuthCode::Failed,
                message: "DPoP auth scheme requires a request proof, but none was provided."
                    .to_string(),
            })?;
            let name = HeaderName::from_bytes(proof.header_name.as_bytes()).map_err(|error| {
                CoreError::Validation {
                    code: maekon_core::error_codes::ValidationCode::InvalidField,
                    field: "integration.request_proof.header_name".to_string(),
                    message: format!("invalid proof header name: {error}"),
                }
            })?;
            let value = HeaderValue::from_str(&proof.header_value).map_err(|error| {
                CoreError::Validation {
                    code: maekon_core::error_codes::ValidationCode::InvalidField,
                    field: "integration.request_proof.header_value".to_string(),
                    message: format!("invalid proof header value: {error}"),
                }
            })?;
            headers.insert(name, value);
        } else if let Some(proof) = maybe_proof {
            let name = HeaderName::from_bytes(proof.header_name.as_bytes()).map_err(|error| {
                CoreError::Validation {
                    code: maekon_core::error_codes::ValidationCode::InvalidField,
                    field: "integration.request_proof.header_name".to_string(),
                    message: format!("invalid proof header name: {error}"),
                }
            })?;
            let value = HeaderValue::from_str(&proof.header_value).map_err(|error| {
                CoreError::Validation {
                    code: maekon_core::error_codes::ValidationCode::InvalidField,
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
        // Fail-closed before any credential egresses: refuse cleartext transport
        // to a remote host (bootstrap_url + all server-provided session URLs flow
        // through here) (#6824).
        reject_cleartext_remote_url(url, "integration.transport.url")?;
        let headers = self.build_headers(auth, method.as_str(), url).await?;
        let mut request = self.client.request(method, url).headers(headers);
        if let Some(body) = body {
            request = request.json(body);
        }
        request.send().await.map_err(|error| {
            if error.is_timeout() {
                CoreError::RequestTimeout {
                    code: maekon_core::error_codes::NetworkCode::Timeout,
                    timeout_ms: self.request_timeout.as_millis() as u64,
                }
            } else {
                CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
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
        // Privacy: the remote response body is server-controlled and may carry
        // PII, secrets, or injection payloads. Read it only to classify the
        // failure (empty vs. present vs. unreadable) via a fixed marker — never
        // interpolate the raw body into errors that are logged or persisted.
        // Mirrors `provider_error_body::provider_error_message`. (#6196)
        //
        // #6940: cap the ERROR body too — all 3 integration transports route their
        // non-2xx responses through here BEFORE the success-path cap, so without
        // this a compromised/misbehaving SaaS endpoint could stream a multi-GB
        // error body and OOM the agent. A cap breach/read error degrades to None
        // (classified as unreadable), never an unbounded buffer.
        let body = crate::outbound::read_text_capped(
            response,
            crate::outbound::MAX_INTEGRATION_RESPONSE_BYTES,
        )
        .await
        .ok();
        let body_state = provider_error_body_state(body.as_deref());

        match status.as_u16() {
            401 | 403 => Err(CoreError::Auth {
                code: maekon_core::error_codes::AuthCode::Failed,
                message: format!("{context}: {body_state}"),
            }),
            404 => Err(CoreError::NotFound {
                code: maekon_core::error_codes::NotFoundCode::ResourceMissing,
                resource_type: context.to_string(),
                id: body_state.to_string(),
            }),
            // 408/504 are timeout-class — wire code `network.timeout` (iter-55)
            408 | 504 => Err(CoreError::RequestTimeout {
                code: maekon_core::error_codes::NetworkCode::Timeout,
                timeout_ms: 0, // sentinel: server-side timeout, unknown budget
            }),
            429 => Err(CoreError::RateLimit {
                code: maekon_core::error_codes::NetworkCode::RateLimit,
                retry_after_secs: retry_after,
            }),
            // 502 Bad Gateway is a transient upstream failure (iter-55)
            502 | 503 => Err(CoreError::ServiceUnavailable {
                code: maekon_core::error_codes::ServiceCode::Unavailable,
                message: format!("{context}: {body_state}"),
            }),
            _ => Err(CoreError::Network {
                code: maekon_core::error_codes::NetworkCode::Generic,
                message: format!("{context}: HTTP {status} {body_state}"),
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
            code: maekon_core::error_codes::ValidationCode::InvalidField,
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
            code: maekon_core::error_codes::ValidationCode::InvalidField,
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
                    code: maekon_core::error_codes::ValidationCode::InvalidField,
                    field: "integration.bootstrap.granted_scopes".to_string(),
                    message: format!("unknown granted scope: {raw_scope}"),
                }
            })?;
            if !request.requested_scopes.contains(&scope) {
                return Err(CoreError::Validation {
                    code: maekon_core::error_codes::ValidationCode::InvalidField,
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
