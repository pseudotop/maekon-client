use super::{HttpsIntegrationHttpShared, HttpsIntegrationTransportClient, SessionBinding};
use crate::integration::transport::{
    IntegrationTransportClient, IntegrationTransportConnectRequest,
    IntegrationTransportConnectResponse,
};
use crate::integration::WebSocketIntegrationSessionChannel;
use async_trait::async_trait;
use chrono::Utc;
use oneshim_api_contracts::integration::{
    IntegrationBootstrapRequest, IntegrationBootstrapResponse, IntegrationSessionDisconnectPayload,
    IntegrationSessionHeartbeatPayload,
};
use oneshim_core::error::CoreError;
use oneshim_core::models::integration::{IntegrationCapabilityScope, IntegrationTransportKind};
use std::sync::Arc;

#[async_trait]
impl IntegrationTransportClient for HttpsIntegrationTransportClient {
    async fn connect(
        &self,
        request: IntegrationTransportConnectRequest,
    ) -> Result<IntegrationTransportConnectResponse, CoreError> {
        let resource_indicator = request.resource_indicator.clone();
        let auth = self
            .auth_port
            .resolve_session_auth(&request.requested_scopes, resource_indicator.as_deref())
            .await?;

        let bootstrap_request = IntegrationBootstrapRequest {
            client_version: request.client_version.clone(),
            device_id: Some(request.device_id.clone()),
            device_label: request.device_label.clone(),
            nonce: format!("nonce-{}", uuid::Uuid::new_v4()),
            requested_scopes: request
                .requested_scopes
                .iter()
                .map(IntegrationCapabilityScope::as_str)
                .map(str::to_string)
                .collect(),
            preferred_transports: request.preferred_transports.clone(),
            supported_auth_schemes: request.supported_auth_schemes.clone(),
            resource_indicator: resource_indicator.or_else(|| auth.resource_indicator.clone()),
        };

        let response = self
            .shared
            .send_with_auth(
                reqwest::Method::POST,
                &self.config.bootstrap_url,
                &auth,
                Some(&bootstrap_request),
            )
            .await?;
        let response = self
            .shared
            .check_response(response, "integration bootstrap request failed")
            .await?;
        let payload: IntegrationBootstrapResponse = response.json().await.map_err(|error| {
            CoreError::Serialization(serde_json::Error::io(std::io::Error::other(format!(
                "failed to parse integration bootstrap response: {error}"
            ))))
        })?;

        let session = payload
            .session
            .clone()
            .ok_or_else(|| CoreError::Validation {
                code: oneshim_core::error_codes::ValidationCode::InvalidField,
                field: "integration.bootstrap.session".to_string(),
                message: "bootstrap response did not include a session binding.".to_string(),
            })?;

        let transport_kind = payload
            .selected_transport
            .clone()
            .or_else(|| request.preferred_transports.first().cloned())
            .unwrap_or_default();
        HttpsIntegrationHttpShared::validate_selected_transport(
            &request,
            &payload,
            &transport_kind,
        )?;

        let auth_scheme = payload
            .selected_auth_scheme
            .clone()
            .unwrap_or_else(|| auth.scheme.clone());
        HttpsIntegrationHttpShared::validate_selected_auth_scheme(
            &request,
            &payload,
            &auth_scheme,
        )?;

        let granted_scopes = HttpsIntegrationHttpShared::parse_granted_scopes(&request, &payload)?;
        let connected_at = Utc::now();
        let live_control_channel = if transport_kind == IntegrationTransportKind::WebSocket {
            let channel_url = session
                .channel_url
                .clone()
                .ok_or_else(|| CoreError::Validation {
                    code: oneshim_core::error_codes::ValidationCode::InvalidField,
                    field: "integration.bootstrap.session.channel_url".to_string(),
                    message: "websocket session transport requires a channel URL.".to_string(),
                })?;
            let headers = self
                .shared
                .build_headers(&auth, reqwest::Method::GET.as_str(), &channel_url)
                .await?;
            Some(Arc::new(
                WebSocketIntegrationSessionChannel::connect(&channel_url, headers).await?,
            ))
        } else {
            None
        };

        self.session_bindings
            .insert(
                session.session_id.clone(),
                SessionBinding {
                    heartbeat_url: session.heartbeat_url.clone(),
                    disconnect_url: session.disconnect_url.clone(),
                    send_events_url: session.send_events_url.clone(),
                    receive_prompts_url: session.receive_prompts_url.clone(),
                    auth,
                    live_session_channel: live_control_channel,
                },
            )
            .await;

        Ok(IntegrationTransportConnectResponse {
            session_id: session.session_id,
            connected_at,
            granted_scopes,
            transport_kind,
            auth_scheme,
        })
    }

    async fn heartbeat(&self, session_id: &str) -> Result<chrono::DateTime<Utc>, CoreError> {
        let binding =
            self.session_bindings
                .get(session_id)
                .await
                .ok_or_else(|| CoreError::NotFound {
                    code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                    resource_type: "integration_session".to_string(),
                    id: session_id.to_string(),
                })?;

        if let Some(channel) = binding.live_session_channel.clone() {
            let heartbeat = IntegrationSessionHeartbeatPayload {
                session_id: session_id.to_string(),
                occurred_at: Utc::now(),
                cursor_snapshot: Vec::new(),
            };
            channel.send_json(&heartbeat).await?;
            return Ok(heartbeat.occurred_at);
        }

        let url = binding.heartbeat_url.ok_or_else(|| CoreError::Validation {
            code: oneshim_core::error_codes::ValidationCode::InvalidField,
            field: "integration.session.heartbeat_url".to_string(),
            message: "active integration session does not have a heartbeat URL.".to_string(),
        })?;

        let response = self
            .shared
            .send_with_auth(
                reqwest::Method::POST,
                &url,
                &binding.auth,
                Option::<&()>::None,
            )
            .await?;
        self.shared
            .check_response(response, "integration heartbeat request failed")
            .await?;
        Ok(Utc::now())
    }

    async fn disconnect(&self, session_id: &str) -> Result<(), CoreError> {
        let binding =
            self.session_bindings
                .get(session_id)
                .await
                .ok_or_else(|| CoreError::NotFound {
                    code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                    resource_type: "integration_session".to_string(),
                    id: session_id.to_string(),
                })?;

        if let Some(channel) = binding.live_session_channel.clone() {
            channel
                .send_json(&IntegrationSessionDisconnectPayload {
                    session_id: session_id.to_string(),
                    occurred_at: Utc::now(),
                    reason: None,
                })
                .await?;
            channel.close().await?;
            self.session_bindings.remove(session_id).await;
            return Ok(());
        }

        let url = binding
            .disconnect_url
            .ok_or_else(|| CoreError::Validation {
                code: oneshim_core::error_codes::ValidationCode::InvalidField,
                field: "integration.session.disconnect_url".to_string(),
                message: "active integration session does not have a disconnect URL.".to_string(),
            })?;

        let response = self
            .shared
            .send_with_auth(
                reqwest::Method::DELETE,
                &url,
                &binding.auth,
                Option::<&()>::None,
            )
            .await?;
        self.shared
            .check_response(response, "integration disconnect request failed")
            .await?;
        self.session_bindings.remove(session_id).await;
        Ok(())
    }
}
