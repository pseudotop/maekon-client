use async_trait::async_trait;
use chrono::{DateTime, Utc};
use oneshim_core::error::CoreError;
use oneshim_core::models::integration::{
    IntegrationAckCursor, IntegrationAuthContext, IntegrationAuthScheme,
    IntegrationCapabilityScope, IntegrationTransportKind, ProactivePrompt,
    QueuedIntegrationEgressMessage,
};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct IntegrationTransportConnectRequest {
    pub device_id: String,
    pub client_version: String,
    pub device_label: Option<String>,
    pub requested_scopes: Vec<IntegrationCapabilityScope>,
    pub preferred_transports: Vec<IntegrationTransportKind>,
    pub supported_auth_schemes: Vec<IntegrationAuthScheme>,
    pub resource_indicator: Option<String>,
}

#[derive(Debug, Clone)]
pub struct IntegrationTransportConnectResponse {
    pub session_id: String,
    pub connected_at: DateTime<Utc>,
    pub granted_scopes: Vec<IntegrationCapabilityScope>,
    pub transport_kind: IntegrationTransportKind,
    pub auth_scheme: IntegrationAuthScheme,
}

#[async_trait]
pub trait IntegrationTransportClient: Send + Sync {
    async fn connect(
        &self,
        request: IntegrationTransportConnectRequest,
    ) -> Result<IntegrationTransportConnectResponse, CoreError>;

    async fn heartbeat(&self, session_id: &str) -> Result<DateTime<Utc>, CoreError>;

    async fn disconnect(&self, session_id: &str) -> Result<(), CoreError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationRequestProof {
    pub header_name: String,
    pub header_value: String,
}

#[async_trait]
pub trait IntegrationRequestProofFactory: Send + Sync {
    async fn build_proof(
        &self,
        auth: &IntegrationAuthContext,
        method: &str,
        url: &str,
    ) -> Result<Option<IntegrationRequestProof>, CoreError>;
}

#[derive(Debug, Clone)]
pub struct IntegrationEgressTransportResponse {
    pub acknowledged_queue_ids: Vec<String>,
    pub ack_cursor: Option<IntegrationAckCursor>,
}

impl IntegrationEgressTransportResponse {
    pub fn accepted_count(&self) -> usize {
        self.acknowledged_queue_ids.len()
    }
}

#[async_trait]
pub trait IntegrationEgressTransportClient: Send + Sync {
    async fn send_messages(
        &self,
        session_id: &str,
        items: Vec<QueuedIntegrationEgressMessage>,
    ) -> Result<IntegrationEgressTransportResponse, CoreError>;
}

#[derive(Debug, Clone)]
pub struct IntegrationInboxTransportResponse {
    pub prompts: Vec<ProactivePrompt>,
    pub ack_cursor: Option<IntegrationAckCursor>,
}

#[async_trait]
pub trait IntegrationInboxTransportClient: Send + Sync {
    async fn receive_prompts(
        &self,
        session_id: &str,
        after_cursor: Option<IntegrationAckCursor>,
        limit: usize,
    ) -> Result<IntegrationInboxTransportResponse, CoreError>;

    async fn wait_for_remote_signal(
        &self,
        _session_id: &str,
        timeout: Duration,
    ) -> Result<bool, CoreError> {
        tokio::time::sleep(timeout).await;
        Ok(false)
    }
}
