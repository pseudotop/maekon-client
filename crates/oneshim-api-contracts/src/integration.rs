use crate::stream::AiRuntimeStatus;
use chrono::{DateTime, Utc};
use oneshim_core::models::integration::{
    IntegrationAuthProfileKind, IntegrationAuthScheme, IntegrationAuthStatus,
    IntegrationDeviceAuthorizationFlow, IntegrationRuntimeTelemetry, IntegrationSessionStatus,
    IntegrationTransportKind,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct IntegrationStatus {
    pub schema_version: String,
    pub external_access_enabled: bool,
    pub automation_controller_configured: bool,
    pub ai_runtime_status: Option<AiRuntimeStatus>,
    pub outbound_runtime: IntegrationOutboundRuntimeStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IntegrationOutboundRuntimeStatus {
    pub enabled: bool,
    pub bootstrap_configured: bool,
    pub auth_source_configured: bool,
    pub auth_material_available: bool,
    pub runtime_configured: bool,
    pub resource_indicator_configured: bool,
    #[serde(default)]
    pub auth_profile_kind: IntegrationAuthProfileKind,
    #[serde(default)]
    pub preferred_transports: Vec<IntegrationTransportKind>,
    #[serde(default)]
    pub supported_auth_schemes: Vec<IntegrationAuthScheme>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outbox_pending_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inbox_pending_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outbox_ack_cursor: Option<IntegrationAckCursorSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inbox_ack_cursor: Option<IntegrationAckCursorSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_status: Option<IntegrationAuthStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_session: Option<IntegrationSessionSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_telemetry: Option<IntegrationRuntimeTelemetry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationDeviceAuthorizationCommandResult {
    pub auth_status: IntegrationAuthStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow: Option<IntegrationDeviceAuthorizationFlow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationDeviceAuthorizationFlowRequest {
    pub flow_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationSessionSummary {
    pub status: IntegrationSessionStatus,
    pub transport_kind: IntegrationTransportKind,
    pub auth_scheme: IntegrationAuthScheme,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connected_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_heartbeat_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub requested_scopes: Vec<String>,
    #[serde(default)]
    pub granted_scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationAckCursorSummary {
    pub stream_id: String,
    pub cursor: String,
    pub acknowledged_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationAuditRecordSummary {
    pub record_id: String,
    pub envelope_id: String,
    pub packet_id: String,
    pub disposition: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub privacy_classification: String,
    pub capability_scope: String,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationAuditLogResponse {
    pub schema_version: String,
    #[serde(default)]
    pub records: Vec<IntegrationAuditRecordSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationInboxPromptSummary {
    pub prompt_id: String,
    pub category: String,
    pub priority: String,
    pub title: String,
    pub body: String,
    pub status: String,
    pub received_at: DateTime<Utc>,
    pub status_updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presented_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    pub source_system: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_actor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismiss_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationInboxResponse {
    pub schema_version: String,
    #[serde(default)]
    pub prompts: Vec<IntegrationInboxPromptSummary>,
    pub pending_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationInboxRefreshResponse {
    pub schema_version: String,
    pub fetched_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationInboxActionResponse {
    pub schema_version: String,
    pub prompt_id: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IntegrationInboxDismissRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationBootstrapRequest {
    pub client_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_label: Option<String>,
    pub nonce: String,
    #[serde(default)]
    pub requested_scopes: Vec<String>,
    #[serde(default)]
    pub preferred_transports: Vec<IntegrationTransportKind>,
    #[serde(default)]
    pub supported_auth_schemes: Vec<IntegrationAuthScheme>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_indicator: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationBootstrapSessionBinding {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heartbeat_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disconnect_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_events_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receive_prompts_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationSessionHeartbeatPayload {
    pub session_id: String,
    pub occurred_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cursor_snapshot: Vec<oneshim_core::models::integration::IntegrationAckCursor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationSessionDisconnectPayload {
    pub session_id: String,
    pub occurred_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationAckPayload {
    pub session_id: String,
    #[serde(default)]
    pub acknowledged_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ack_cursor: Option<oneshim_core::models::integration::IntegrationAckCursor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationBootstrapResponse {
    pub schema_version: String,
    #[serde(default)]
    pub supported_scopes: Vec<String>,
    #[serde(default)]
    pub granted_scopes: Vec<String>,
    #[serde(default)]
    pub supported_transports: Vec<IntegrationTransportKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_transport: Option<IntegrationTransportKind>,
    #[serde(default)]
    pub supported_auth_schemes: Vec<IntegrationAuthScheme>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_auth_scheme: Option<IntegrationAuthScheme>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_indicator: Option<String>,
    pub session_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<IntegrationBootstrapSessionBinding>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_request_roundtrip() {
        let request = IntegrationBootstrapRequest {
            client_version: "0.3.8".to_string(),
            device_id: Some("device-001".to_string()),
            device_label: Some("macbook".to_string()),
            nonce: "nonce-001".to_string(),
            requested_scopes: vec!["insight:write".to_string(), "prompt:read".to_string()],
            preferred_transports: vec![IntegrationTransportKind::WebSocket],
            supported_auth_schemes: vec![
                IntegrationAuthScheme::DpopBearer,
                IntegrationAuthScheme::BearerToken,
            ],
            resource_indicator: Some("https://integration.example.com".to_string()),
        };

        let json = serde_json::to_string(&request).unwrap();
        let parsed: IntegrationBootstrapRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.client_version, "0.3.8");
        assert_eq!(parsed.requested_scopes.len(), 2);
        assert_eq!(parsed.nonce, "nonce-001");
        assert_eq!(parsed.preferred_transports.len(), 1);
    }

    #[test]
    fn bootstrap_response_roundtrip() {
        let response = IntegrationBootstrapResponse {
            schema_version: "integration.bootstrap.v1".to_string(),
            supported_scopes: vec!["insight:write".to_string()],
            granted_scopes: vec!["insight:write".to_string()],
            supported_transports: vec![IntegrationTransportKind::WebSocket],
            selected_transport: Some(IntegrationTransportKind::WebSocket),
            supported_auth_schemes: vec![IntegrationAuthScheme::DpopBearer],
            selected_auth_scheme: Some(IntegrationAuthScheme::DpopBearer),
            resource_indicator: Some("https://integration.example.com".to_string()),
            session_required: true,
            session: Some(IntegrationBootstrapSessionBinding {
                session_id: "session-001".to_string(),
                channel_url: Some("wss://integration.example.com/sessions/session-001".to_string()),
                heartbeat_url: Some(
                    "https://integration.example.com/sessions/session-001/heartbeat".to_string(),
                ),
                disconnect_url: Some(
                    "https://integration.example.com/sessions/session-001".to_string(),
                ),
                send_events_url: Some(
                    "https://integration.example.com/sessions/session-001/insights".to_string(),
                ),
                receive_prompts_url: Some(
                    "https://integration.example.com/sessions/session-001/prompts".to_string(),
                ),
            }),
        };

        let json = serde_json::to_string(&response).unwrap();
        let parsed: IntegrationBootstrapResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.schema_version, "integration.bootstrap.v1");
        assert!(parsed.session_required);
        assert_eq!(
            parsed.selected_transport,
            Some(IntegrationTransportKind::WebSocket)
        );
        assert_eq!(
            parsed.selected_auth_scheme,
            Some(IntegrationAuthScheme::DpopBearer)
        );
        assert_eq!(
            parsed
                .session
                .as_ref()
                .map(|session| session.session_id.as_str()),
            Some("session-001")
        );
    }

    #[test]
    fn integration_status_roundtrip() {
        let status = IntegrationStatus {
            schema_version: "integration.status.v1".to_string(),
            external_access_enabled: false,
            automation_controller_configured: true,
            ai_runtime_status: None,
            outbound_runtime: IntegrationOutboundRuntimeStatus {
                enabled: true,
                bootstrap_configured: true,
                auth_source_configured: true,
                auth_material_available: true,
                runtime_configured: true,
                resource_indicator_configured: false,
                auth_profile_kind: IntegrationAuthProfileKind::EnvToken,
                preferred_transports: vec![IntegrationTransportKind::WebSocket],
                supported_auth_schemes: vec![IntegrationAuthScheme::BearerToken],
                outbox_pending_count: Some(3),
                inbox_pending_count: Some(2),
                outbox_ack_cursor: Some(IntegrationAckCursorSummary {
                    stream_id: "insights".to_string(),
                    cursor: "cursor-1".to_string(),
                    acknowledged_at: Utc::now(),
                }),
                inbox_ack_cursor: Some(IntegrationAckCursorSummary {
                    stream_id: "prompts".to_string(),
                    cursor: "cursor-2".to_string(),
                    acknowledged_at: Utc::now(),
                }),
                auth_status: None,
                runtime_telemetry: None,
                current_session: Some(IntegrationSessionSummary {
                    status: oneshim_core::models::integration::IntegrationSessionStatus::Connected,
                    transport_kind: IntegrationTransportKind::WebSocket,
                    auth_scheme: IntegrationAuthScheme::BearerToken,
                    connected_at: None,
                    last_heartbeat_at: None,
                    requested_scopes: vec!["insight:write".to_string()],
                    granted_scopes: vec!["insight:write".to_string()],
                }),
            },
        };

        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(
            json["outbound_runtime"]["runtime_configured"],
            serde_json::Value::Bool(true)
        );
        assert_eq!(
            json["outbound_runtime"]["current_session"]["granted_scopes"][0],
            serde_json::Value::String("insight:write".to_string())
        );
        assert_eq!(json["outbound_runtime"]["outbox_pending_count"], 3);
    }
}
