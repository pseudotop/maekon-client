use super::{OidcDeviceAuthorizationResponse, OidcTokenErrorResponse, OidcTokenSuccessResponse};
use crate::integration::transport::IntegrationRequestProofFactory;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use maekon_core::error::CoreError;
use maekon_core::models::integration::{
    IntegrationAuthContext, IntegrationAuthProfileKind, IntegrationAuthScheme,
    IntegrationAuthStatus, IntegrationAuthStatusKind, IntegrationCapabilityScope,
    IntegrationDeviceAuthorizationFlow,
};
use maekon_core::ports::integration::IntegrationAuthPort;
use maekon_core::ports::secret_store::{
    SecretStore, INTEGRATION_ACCESS_TOKEN_SECRET_KEY, INTEGRATION_AUTH_SECRET_NAMESPACE,
    INTEGRATION_EXPIRES_AT_SECRET_KEY, INTEGRATION_REFRESH_TOKEN_SECRET_KEY,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};

pub struct OidcDeviceFlowAuthConfig {
    pub client_id: String,
    pub device_authorization_url: String,
    pub token_url: String,
    pub default_scopes: Vec<String>,
    pub resource_indicator: Option<String>,
    pub scheme: IntegrationAuthScheme,
    pub request_timeout: Duration,
}

#[derive(Debug, Clone)]
struct StoredAuthMaterial {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: Option<DateTime<Utc>>,
    resource_indicator: Option<String>,
}

impl StoredAuthMaterial {
    fn is_expired(&self) -> bool {
        self.expires_at
            .is_some_and(|expires_at| expires_at <= Utc::now())
    }
}

#[derive(Debug, Clone)]
struct PendingDeviceAuthorization {
    flow: IntegrationDeviceAuthorizationFlow,
    device_code: String,
}

/// Manages stored authentication material (load/store/clear from SecretStore).
struct AuthMaterialManager {
    material: Arc<RwLock<Option<StoredAuthMaterial>>>,
    secret_store: Option<Arc<dyn SecretStore>>,
    resource_indicator: Option<String>,
}

impl AuthMaterialManager {
    async fn load_material(&self) -> Result<Option<StoredAuthMaterial>, CoreError> {
        if let Some(material) = self.material.read().await.clone() {
            return Ok(Some(material));
        }

        let Some(secret_store) = self.secret_store.as_ref() else {
            return Ok(None);
        };

        let access_token = secret_store
            .retrieve(
                INTEGRATION_AUTH_SECRET_NAMESPACE,
                INTEGRATION_ACCESS_TOKEN_SECRET_KEY,
            )
            .await?;
        let Some(access_token) = access_token else {
            return Ok(None);
        };

        let refresh_token = secret_store
            .retrieve(
                INTEGRATION_AUTH_SECRET_NAMESPACE,
                INTEGRATION_REFRESH_TOKEN_SECRET_KEY,
            )
            .await?;
        let expires_at = secret_store
            .retrieve(
                INTEGRATION_AUTH_SECRET_NAMESPACE,
                INTEGRATION_EXPIRES_AT_SECRET_KEY,
            )
            .await?
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(&value).ok())
            .map(|value| value.with_timezone(&Utc));

        let material = StoredAuthMaterial {
            access_token,
            refresh_token,
            expires_at,
            resource_indicator: self.resource_indicator.clone(),
        };
        *self.material.write().await = Some(material.clone());
        Ok(Some(material))
    }

    async fn store_material(&self, material: &StoredAuthMaterial) -> Result<(), CoreError> {
        if let Some(secret_store) = self.secret_store.as_ref() {
            secret_store
                .store(
                    INTEGRATION_AUTH_SECRET_NAMESPACE,
                    INTEGRATION_ACCESS_TOKEN_SECRET_KEY,
                    &material.access_token,
                )
                .await?;
            if let Some(refresh_token) = material.refresh_token.as_deref() {
                secret_store
                    .store(
                        INTEGRATION_AUTH_SECRET_NAMESPACE,
                        INTEGRATION_REFRESH_TOKEN_SECRET_KEY,
                        refresh_token,
                    )
                    .await?;
            } else {
                secret_store
                    .delete(
                        INTEGRATION_AUTH_SECRET_NAMESPACE,
                        INTEGRATION_REFRESH_TOKEN_SECRET_KEY,
                    )
                    .await?;
            }
            if let Some(expires_at) = material.expires_at {
                secret_store
                    .store(
                        INTEGRATION_AUTH_SECRET_NAMESPACE,
                        INTEGRATION_EXPIRES_AT_SECRET_KEY,
                        &expires_at.to_rfc3339(),
                    )
                    .await?;
            } else {
                secret_store
                    .delete(
                        INTEGRATION_AUTH_SECRET_NAMESPACE,
                        INTEGRATION_EXPIRES_AT_SECRET_KEY,
                    )
                    .await?;
            }
        }

        *self.material.write().await = Some(material.clone());
        Ok(())
    }

    async fn clear_material(&self) -> Result<(), CoreError> {
        if let Some(secret_store) = self.secret_store.as_ref() {
            secret_store
                .delete(
                    INTEGRATION_AUTH_SECRET_NAMESPACE,
                    INTEGRATION_ACCESS_TOKEN_SECRET_KEY,
                )
                .await?;
            secret_store
                .delete(
                    INTEGRATION_AUTH_SECRET_NAMESPACE,
                    INTEGRATION_REFRESH_TOKEN_SECRET_KEY,
                )
                .await?;
            secret_store
                .delete(
                    INTEGRATION_AUTH_SECRET_NAMESPACE,
                    INTEGRATION_EXPIRES_AT_SECRET_KEY,
                )
                .await?;
        }
        *self.material.write().await = None;
        Ok(())
    }
}

/// Manages pending device authorization flows.
struct PendingFlowManager {
    inner: Arc<RwLock<HashMap<String, PendingDeviceAuthorization>>>,
}

impl PendingFlowManager {
    async fn insert(&self, flow_id: String, entry: PendingDeviceAuthorization) {
        self.inner.write().await.insert(flow_id, entry);
    }

    async fn remove(&self, flow_id: &str) -> Option<PendingDeviceAuthorization> {
        self.inner.write().await.remove(flow_id)
    }

    async fn clear(&self) {
        self.inner.write().await.clear();
    }

    async fn find_first_active(&self) -> Option<IntegrationDeviceAuthorizationFlow> {
        let now = Utc::now();
        self.inner
            .read()
            .await
            .values()
            .find(|entry| entry.flow.expires_at > now)
            .map(|entry| entry.flow.clone())
    }

    async fn get(&self, flow_id: &str) -> Option<PendingDeviceAuthorization> {
        self.inner.read().await.get(flow_id).cloned()
    }

    async fn increase_interval(&self, flow_id: &str, delta: u64) {
        let mut guard = self.inner.write().await;
        if let Some(flow) = guard.get_mut(flow_id) {
            flow.flow.interval_secs = flow.flow.interval_secs.saturating_add(delta);
        }
    }

    async fn find_reusable_pending_flow(
        &self,
        requested_scopes: &[IntegrationCapabilityScope],
        resource_indicator: Option<&str>,
    ) -> Option<IntegrationDeviceAuthorizationFlow> {
        let now = Utc::now();
        let expected_resource_indicator = resource_indicator.map(str::to_string);
        let mut pending_flows = self.inner.write().await;
        pending_flows.retain(|_, entry| entry.flow.expires_at > now);
        pending_flows.values().find_map(|entry| {
            let same_scopes = entry.flow.requested_scopes.len() == requested_scopes.len()
                && requested_scopes
                    .iter()
                    .all(|scope| entry.flow.requested_scopes.contains(scope));
            let same_resource_indicator =
                entry.flow.resource_indicator == expected_resource_indicator;
            (same_scopes && same_resource_indicator).then(|| entry.flow.clone())
        })
    }
}

pub struct OidcDeviceFlowIntegrationAuthPort {
    config: OidcDeviceFlowAuthConfig,
    proof_factory: Arc<dyn IntegrationRequestProofFactory>,
    client: reqwest::Client,
    auth: AuthMaterialManager,
    flows: PendingFlowManager,
    last_error: Arc<RwLock<Option<String>>>,
    refresh_lock: Arc<Mutex<()>>,
}

/// Upper bound (30 days) on a server-supplied OIDC `expires_in` before it is
/// turned into a `chrono::Duration`. Neutralizes overflow/panic from a
/// hostile/buggy provider value (u64::MAX casts to -1; a huge positive value
/// overflows `TimeDelta`/`DateTime`); 30 days is far beyond any real token TTL
/// and trivially within range (#6201 sibling).
const MAX_OIDC_TTL_SECS: u64 = 60 * 60 * 24 * 30;

/// Clamp a server `expires_in` (seconds) to a safe TTL and return the future
/// instant `now + ttl`.
fn clamped_expiry(seconds: u64) -> DateTime<Utc> {
    Utc::now() + chrono::Duration::seconds(seconds.min(MAX_OIDC_TTL_SECS) as i64)
}

impl OidcDeviceFlowIntegrationAuthPort {
    pub fn new(
        config: OidcDeviceFlowAuthConfig,
        proof_factory: Arc<dyn IntegrationRequestProofFactory>,
        secret_store: Option<Arc<dyn SecretStore>>,
    ) -> Result<Self, CoreError> {
        let client = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(|error| CoreError::Network {
                code: maekon_core::error_codes::NetworkCode::Generic,
                message: format!("failed to build integration OIDC auth HTTP client: {error}"),
            })?;

        let auth = AuthMaterialManager {
            material: Arc::new(RwLock::new(None)),
            secret_store,
            resource_indicator: config.resource_indicator.clone(),
        };
        let flows = PendingFlowManager {
            inner: Arc::new(RwLock::new(HashMap::new())),
        };

        Ok(Self {
            config,
            proof_factory,
            client,
            auth,
            flows,
            last_error: Arc::new(RwLock::new(None)),
            refresh_lock: Arc::new(Mutex::new(())),
        })
    }

    async fn refresh_access_token_if_needed(
        &self,
        refresh_token: &str,
    ) -> Result<StoredAuthMaterial, CoreError> {
        let _guard = self.refresh_lock.lock().await;

        if let Some(material) = self.auth.load_material().await? {
            if !material.is_expired() {
                return Ok(material);
            }

            if let Some(current_refresh_token) = material.refresh_token.as_deref() {
                return self.refresh_access_token(current_refresh_token).await;
            }
        }

        self.refresh_access_token(refresh_token).await
    }

    fn combined_scope_string(
        &self,
        requested_scopes: &[IntegrationCapabilityScope],
    ) -> Option<String> {
        let mut scopes = self.config.default_scopes.clone();
        for scope in requested_scopes {
            let value = scope.as_str().to_string();
            if !scopes.contains(&value) {
                scopes.push(value);
            }
        }
        if scopes.is_empty() {
            None
        } else {
            Some(scopes.join(" "))
        }
    }

    async fn send_form(
        &self,
        url: &str,
        form: &[(String, String)],
        use_dpop: bool,
    ) -> Result<reqwest::Response, CoreError> {
        let mut request = self.client.post(url).form(form);
        if use_dpop {
            let proof = self
                .proof_factory
                .build_proof(
                    &IntegrationAuthContext {
                        access_token: String::new(),
                        scheme: IntegrationAuthScheme::DpopBearer,
                        expires_at: None,
                        resource_indicator: self.config.resource_indicator.clone(),
                    },
                    "POST",
                    url,
                )
                .await?;
            let proof = proof.ok_or_else(|| CoreError::Auth {
                code: maekon_core::error_codes::AuthCode::Failed,
                message: "DPoP integration auth requires a request proof, but none was provided."
                    .to_string(),
            })?;
            request = request.header(proof.header_name, proof.header_value);
        }

        request.send().await.map_err(|error| {
            if error.is_timeout() {
                CoreError::RequestTimeout {
                    code: maekon_core::error_codes::NetworkCode::Timeout,
                    timeout_ms: self.config.request_timeout.as_millis() as u64,
                }
            } else {
                CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    message: format!("integration auth request failed: {error}"),
                }
            }
        })
    }

    /// Returns `true` for OAuth token error codes (RFC 6749 §5.2) that mean the
    /// refresh grant is permanently unusable and the user must re-authorize.
    ///
    /// `invalid_grant` is the canonical "refresh token expired/revoked/invalid"
    /// signal; `invalid_client`/`unauthorized_client` mean the client registration
    /// is no longer accepted. Transient or ambiguous errors are deliberately
    /// excluded so a retry can recover without losing stored material.
    fn is_permanent_grant_error(error: &str) -> bool {
        matches!(
            error,
            "invalid_grant" | "invalid_client" | "unauthorized_client"
        )
    }

    async fn refresh_access_token(
        &self,
        refresh_token: &str,
    ) -> Result<StoredAuthMaterial, CoreError> {
        let mut form = vec![
            ("grant_type".to_string(), "refresh_token".to_string()),
            ("client_id".to_string(), self.config.client_id.clone()),
            ("refresh_token".to_string(), refresh_token.to_string()),
        ];
        if let Some(resource_indicator) = self.config.resource_indicator.as_deref() {
            form.push(("resource".to_string(), resource_indicator.to_string()));
        }

        let response = self
            .send_form(
                &self.config.token_url,
                &form,
                self.config.scheme == IntegrationAuthScheme::DpopBearer,
            )
            .await?;

        // Branch on the HTTP status BEFORE consuming the body so that a transient
        // failure never destroys the stored grant. Clearing material forces a full
        // re-authorization (the device flow), so we only do it when the grant is
        // *permanently* invalid. A transient 5xx/429 must surface as an Err the
        // scheduled refresh can retry, leaving the refresh token intact.
        let status = response.status();
        if !status.is_success() {
            // 429: rate limited. Honor Retry-After (clamped) and keep the grant.
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let retry_after_secs = crate::resilience::extract_retry_after(&response);
                return Err(CoreError::RateLimit {
                    code: maekon_core::error_codes::NetworkCode::RateLimit,
                    retry_after_secs,
                });
            }

            // 5xx: server-side / transient failure. Keep the grant for the next retry.
            // Do NOT echo the raw remote body into the error (it is logged/persisted
            // and may carry server-controlled content) — status only (#6196 parity).
            if status.is_server_error() {
                return Err(CoreError::ServiceUnavailable {
                    code: maekon_core::error_codes::ServiceCode::Unavailable,
                    message: format!(
                        "integration refresh failed transiently ({status}); body omitted_for_privacy"
                    ),
                });
            }

            // 400/401: the only statuses where an OAuth error body carries a
            // grant-invalidation signal (RFC 6749 §5.2). Parse it and clear material
            // ONLY for permanent errors; any other/unparseable 4xx keeps the grant.
            if status == reqwest::StatusCode::BAD_REQUEST
                || status == reqwest::StatusCode::UNAUTHORIZED
            {
                let error_body: OidcTokenErrorResponse =
                    response.json().await.unwrap_or(OidcTokenErrorResponse {
                        error: "unknown_error".to_string(),
                        error_description: None,
                    });
                let message = error_body
                    .error_description
                    .clone()
                    .unwrap_or_else(|| error_body.error.clone());

                if Self::is_permanent_grant_error(&error_body.error) {
                    self.auth.clear_material().await?;
                }

                return Err(CoreError::Auth {
                    code: maekon_core::error_codes::AuthCode::Failed,
                    message: format!("integration refresh failed: {message}"),
                });
            }

            // Any other non-success status (e.g. 403/404): treat as a non-permanent
            // failure and keep the grant rather than risk an unrecoverable logout.
            // Raw body omitted from the error (logged/persisted; #6196 parity).
            return Err(CoreError::Auth {
                code: maekon_core::error_codes::AuthCode::Failed,
                message: format!("integration refresh failed ({status}); body omitted_for_privacy"),
            });
        }

        let payload: OidcTokenSuccessResponse = response.json().await.map_err(|error| {
            CoreError::Serialization(serde_json::Error::io(std::io::Error::other(format!(
                "failed to parse integration refresh response: {error}"
            ))))
        })?;

        let material = StoredAuthMaterial {
            access_token: payload.access_token,
            refresh_token: payload
                .refresh_token
                .or_else(|| Some(refresh_token.to_string())),
            expires_at: payload.expires_in.map(clamped_expiry),
            resource_indicator: self.config.resource_indicator.clone(),
        };
        self.auth.store_material(&material).await?;
        Ok(material)
    }
}

#[async_trait]
impl IntegrationAuthPort for OidcDeviceFlowIntegrationAuthPort {
    async fn resolve_session_auth(
        &self,
        requested_scopes: &[IntegrationCapabilityScope],
        resource_indicator: Option<&str>,
    ) -> Result<IntegrationAuthContext, CoreError> {
        let material = self.auth.load_material().await?;
        if let Some(material) = material {
            let material = if material.is_expired() {
                if let Some(refresh_token) = material.refresh_token.as_deref() {
                    self.refresh_access_token_if_needed(refresh_token).await?
                } else {
                    return Err(CoreError::Auth {
                        code: maekon_core::error_codes::AuthCode::Failed,
                        message:
                            "integration device authorization has expired; re-authorize the device"
                                .to_string(),
                    });
                }
            } else {
                material
            };

            return Ok(IntegrationAuthContext {
                access_token: material.access_token,
                scheme: self.config.scheme.clone(),
                expires_at: material.expires_at,
                resource_indicator: resource_indicator
                    .map(str::to_string)
                    .or(material.resource_indicator),
            });
        }

        let status = self.current_auth_status().await?;
        Err(CoreError::Auth {
            code: maekon_core::error_codes::AuthCode::Failed,
            message: status.message.unwrap_or_else(|| {
                let scopes = self
                    .combined_scope_string(requested_scopes)
                    .unwrap_or_else(|| "requested scopes".to_string());
                format!(
                    "integration device authorization is required before using scopes: {scopes}"
                )
            }),
        })
    }

    async fn current_auth_status(&self) -> Result<IntegrationAuthStatus, CoreError> {
        if let Some(material) = self.auth.load_material().await? {
            let material = if material.is_expired() {
                if let Some(refresh_token) = material.refresh_token.as_deref() {
                    match self.refresh_access_token_if_needed(refresh_token).await {
                        Ok(refreshed) => {
                            *self.last_error.write().await = None;
                            refreshed
                        }
                        Err(error) => {
                            let message = format!(
                                "integration auth refresh failed; re-authorize the device: {error}"
                            );
                            *self.last_error.write().await = Some(message.clone());
                            return Ok(IntegrationAuthStatus {
                                profile_kind: IntegrationAuthProfileKind::OidcDeviceFlow,
                                status: IntegrationAuthStatusKind::Error,
                                interactive: true,
                                authenticated: false,
                                expires_at: material.expires_at,
                                resource_indicator: material.resource_indicator,
                                pending_flow: None,
                                message: Some(message),
                            });
                        }
                    }
                } else {
                    material
                }
            } else {
                material
            };
            let expired = material.is_expired();
            return Ok(IntegrationAuthStatus {
                profile_kind: IntegrationAuthProfileKind::OidcDeviceFlow,
                status: if expired {
                    IntegrationAuthStatusKind::Expired
                } else {
                    IntegrationAuthStatusKind::Ready
                },
                interactive: true,
                authenticated: !expired,
                expires_at: material.expires_at,
                resource_indicator: material.resource_indicator,
                pending_flow: None,
                message: if expired {
                    Some(
                        "integration device authorization expired; re-authorize or refresh"
                            .to_string(),
                    )
                } else {
                    None
                },
            });
        }

        let pending_flow = self.flows.find_first_active().await;
        if let Some(flow) = pending_flow {
            return Ok(IntegrationAuthStatus {
                profile_kind: IntegrationAuthProfileKind::OidcDeviceFlow,
                status: IntegrationAuthStatusKind::AwaitingUserAuthorization,
                interactive: true,
                authenticated: false,
                expires_at: None,
                resource_indicator: flow.resource_indicator.clone(),
                pending_flow: Some(flow),
                message: Some(
                    "complete the device authorization flow to finish integration bootstrap"
                        .to_string(),
                ),
            });
        }

        Ok(IntegrationAuthStatus {
            profile_kind: IntegrationAuthProfileKind::OidcDeviceFlow,
            status: IntegrationAuthStatusKind::Unauthenticated,
            interactive: true,
            authenticated: false,
            expires_at: None,
            resource_indicator: self.config.resource_indicator.clone(),
            pending_flow: None,
            message: self.last_error.read().await.clone(),
        })
    }

    async fn start_device_authorization(
        &self,
        requested_scopes: &[IntegrationCapabilityScope],
        resource_indicator: Option<&str>,
    ) -> Result<IntegrationDeviceAuthorizationFlow, CoreError> {
        if let Some(flow) = self
            .flows
            .find_reusable_pending_flow(requested_scopes, resource_indicator)
            .await
        {
            *self.last_error.write().await = None;
            return Ok(flow);
        }

        let mut form = vec![("client_id".to_string(), self.config.client_id.clone())];
        if let Some(scope) = self.combined_scope_string(requested_scopes) {
            form.push(("scope".to_string(), scope));
        }
        let resource_indicator = resource_indicator
            .map(str::to_string)
            .or_else(|| self.config.resource_indicator.clone());
        if let Some(resource_indicator) = resource_indicator.as_deref() {
            form.push(("resource".to_string(), resource_indicator.to_string()));
        }

        let response = self
            .send_form(&self.config.device_authorization_url, &form, false)
            .await?;
        if !response.status().is_success() {
            let status = response.status();
            // Do NOT echo the raw remote body: `last_error` is persisted and surfaced
            // to the console/UI (current_auth_status().message), and the IdP body can
            // carry server-controlled content or PII — status only (#6196 parity,
            // matching the refresh_access_token sibling above).
            let message = format!(
                "integration device authorization bootstrap failed ({status}); body omitted_for_privacy"
            );
            *self.last_error.write().await = Some(message.clone());
            return Err(CoreError::Auth {
                code: maekon_core::error_codes::AuthCode::Failed,
                message,
            });
        }

        let payload: OidcDeviceAuthorizationResponse = response.json().await.map_err(|error| {
            CoreError::Serialization(serde_json::Error::io(std::io::Error::other(format!(
                "failed to parse integration device authorization response: {error}"
            ))))
        })?;
        let flow_id = maekon_core::generate_id("flow");
        let flow = IntegrationDeviceAuthorizationFlow {
            flow_id: flow_id.clone(),
            user_code: payload.user_code,
            verification_uri: payload.verification_uri,
            verification_uri_complete: payload.verification_uri_complete,
            expires_at: clamped_expiry(payload.expires_in),
            interval_secs: payload.interval.unwrap_or(5),
            requested_scopes: requested_scopes.to_vec(),
            resource_indicator,
        };
        self.flows
            .insert(
                flow_id,
                PendingDeviceAuthorization {
                    flow: flow.clone(),
                    device_code: payload.device_code,
                },
            )
            .await;
        *self.last_error.write().await = None;
        Ok(flow)
    }

    async fn poll_device_authorization(
        &self,
        flow_id: &str,
    ) -> Result<IntegrationAuthStatus, CoreError> {
        let pending = self
            .flows
            .get(flow_id)
            .await
            .ok_or_else(|| CoreError::NotFound {
                code: maekon_core::error_codes::NotFoundCode::ResourceMissing,
                resource_type: "integration_device_authorization_flow".to_string(),
                id: flow_id.to_string(),
            })?;

        if pending.flow.expires_at <= Utc::now() {
            self.flows.remove(flow_id).await;
            *self.last_error.write().await =
                Some("integration device authorization flow expired".to_string());
            return Ok(IntegrationAuthStatus {
                profile_kind: IntegrationAuthProfileKind::OidcDeviceFlow,
                status: IntegrationAuthStatusKind::Expired,
                interactive: true,
                authenticated: false,
                expires_at: None,
                resource_indicator: pending.flow.resource_indicator.clone(),
                pending_flow: None,
                message: Some("integration device authorization flow expired".to_string()),
            });
        }

        let mut form = vec![
            (
                "grant_type".to_string(),
                "urn:ietf:params:oauth:grant-type:device_code".to_string(),
            ),
            ("device_code".to_string(), pending.device_code.clone()),
            ("client_id".to_string(), self.config.client_id.clone()),
        ];
        if let Some(resource_indicator) = pending.flow.resource_indicator.as_deref() {
            form.push(("resource".to_string(), resource_indicator.to_string()));
        }

        let response = self
            .send_form(
                &self.config.token_url,
                &form,
                self.config.scheme == IntegrationAuthScheme::DpopBearer,
            )
            .await?;

        if response.status().is_success() {
            let payload: OidcTokenSuccessResponse = response.json().await.map_err(|error| {
                CoreError::Serialization(serde_json::Error::io(std::io::Error::other(format!(
                    "failed to parse integration token response: {error}"
                ))))
            })?;
            let material = StoredAuthMaterial {
                access_token: payload.access_token,
                refresh_token: payload.refresh_token,
                expires_at: payload.expires_in.map(clamped_expiry),
                resource_indicator: pending.flow.resource_indicator.clone(),
            };
            self.auth.store_material(&material).await?;
            self.flows.remove(flow_id).await;
            *self.last_error.write().await = None;
            return self.current_auth_status().await;
        }

        let error_body: OidcTokenErrorResponse =
            response.json().await.unwrap_or(OidcTokenErrorResponse {
                error: "unknown_error".to_string(),
                error_description: None,
            });
        let message = error_body
            .error_description
            .clone()
            .unwrap_or_else(|| error_body.error.clone());

        match error_body.error.as_str() {
            "authorization_pending" => {
                *self.last_error.write().await = Some(message);
                self.current_auth_status().await
            }
            "slow_down" => {
                self.flows.increase_interval(flow_id, 5).await;
                *self.last_error.write().await = Some(message);
                self.current_auth_status().await
            }
            "access_denied" | "expired_token" | "invalid_grant" => {
                self.flows.remove(flow_id).await;
                *self.last_error.write().await = Some(message.clone());
                Ok(IntegrationAuthStatus {
                    profile_kind: IntegrationAuthProfileKind::OidcDeviceFlow,
                    status: if error_body.error == "expired_token" {
                        IntegrationAuthStatusKind::Expired
                    } else {
                        IntegrationAuthStatusKind::Error
                    },
                    interactive: true,
                    authenticated: false,
                    expires_at: None,
                    resource_indicator: pending.flow.resource_indicator.clone(),
                    pending_flow: None,
                    message: Some(message),
                })
            }
            _ => {
                *self.last_error.write().await = Some(message.clone());
                Err(CoreError::Auth {
                    code: maekon_core::error_codes::AuthCode::Failed,
                    message: format!("integration token exchange failed: {message}"),
                })
            }
        }
    }

    async fn cancel_device_authorization(&self, flow_id: &str) -> Result<(), CoreError> {
        let removed = self.flows.remove(flow_id).await;
        if removed.is_none() {
            return Err(CoreError::NotFound {
                code: maekon_core::error_codes::NotFoundCode::ResourceMissing,
                resource_type: "integration_device_authorization_flow".to_string(),
                id: flow_id.to_string(),
            });
        }
        *self.last_error.write().await = None;
        Ok(())
    }

    async fn reset_auth_state(&self) -> Result<(), CoreError> {
        self.flows.clear().await;
        self.auth.clear_material().await?;
        *self.last_error.write().await = None;
        Ok(())
    }
}

#[cfg(test)]
mod permanent_grant_error_tests {
    use super::OidcDeviceFlowIntegrationAuthPort;

    #[test]
    fn classifies_permanent_oauth_errors() {
        // RFC 6749 §5.2 permanent grant-invalidation codes -> re-auth required.
        for code in ["invalid_grant", "invalid_client", "unauthorized_client"] {
            assert!(
                OidcDeviceFlowIntegrationAuthPort::is_permanent_grant_error(code),
                "{code} must be treated as permanent"
            );
        }
    }

    #[test]
    fn classifies_transient_or_ambiguous_errors_as_non_permanent() {
        // These must NOT clear stored material; the next refresh should retry.
        for code in [
            "temporarily_unavailable",
            "slow_down",
            "invalid_request",
            "invalid_scope",
            "unknown_error",
            "",
        ] {
            assert!(
                !OidcDeviceFlowIntegrationAuthPort::is_permanent_grant_error(code),
                "{code} must NOT be treated as permanent"
            );
        }
    }
}
