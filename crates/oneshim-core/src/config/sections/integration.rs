use crate::models::integration::{
    IntegrationAuthProfileKind, IntegrationAuthScheme, IntegrationTransportKind,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct IntegrationOidcDeviceFlowConfig {
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub device_authorization_url: Option<String>,
    #[serde(default)]
    pub token_url: Option<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IntegrationConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub auth_profile_kind: IntegrationAuthProfileKind,
    #[serde(default)]
    pub bootstrap_url: Option<String>,
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub device_label: Option<String>,
    #[serde(default)]
    pub resource_indicator: Option<String>,
    #[serde(default)]
    pub auth_token_env_var: Option<String>,
    #[serde(default)]
    pub oidc_device_flow: IntegrationOidcDeviceFlowConfig,
    #[serde(default = "default_integration_request_timeout_secs")]
    pub request_timeout_secs: u64,
    #[serde(default = "default_integration_connect_retry_secs")]
    pub connect_retry_secs: u64,
    #[serde(default = "default_integration_heartbeat_interval_secs")]
    pub heartbeat_interval_secs: u64,
    #[serde(default = "default_integration_sync_interval_secs")]
    pub sync_interval_secs: u64,
    #[serde(default = "default_integration_produce_interval_secs")]
    pub produce_interval_secs: u64,
    #[serde(default = "default_integration_inbox_refresh_interval_secs")]
    pub inbox_refresh_interval_secs: u64,
    #[serde(default = "default_integration_max_batch_size")]
    pub max_batch_size: usize,
    #[serde(default = "default_integration_max_stored_prompts")]
    pub max_stored_prompts: usize,
    #[serde(default = "default_integration_redact_completed_prompt_bodies")]
    pub redact_completed_prompt_bodies: bool,
    #[serde(default = "default_integration_preferred_transports")]
    pub preferred_transports: Vec<IntegrationTransportKind>,
    #[serde(default = "default_integration_supported_auth_schemes")]
    pub supported_auth_schemes: Vec<IntegrationAuthScheme>,
}

impl Default for IntegrationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            auth_profile_kind: IntegrationAuthProfileKind::default(),
            bootstrap_url: None,
            device_id: None,
            device_label: None,
            resource_indicator: None,
            auth_token_env_var: None,
            oidc_device_flow: IntegrationOidcDeviceFlowConfig::default(),
            request_timeout_secs: default_integration_request_timeout_secs(),
            connect_retry_secs: default_integration_connect_retry_secs(),
            heartbeat_interval_secs: default_integration_heartbeat_interval_secs(),
            sync_interval_secs: default_integration_sync_interval_secs(),
            produce_interval_secs: default_integration_produce_interval_secs(),
            inbox_refresh_interval_secs: default_integration_inbox_refresh_interval_secs(),
            max_batch_size: default_integration_max_batch_size(),
            max_stored_prompts: default_integration_max_stored_prompts(),
            redact_completed_prompt_bodies: default_integration_redact_completed_prompt_bodies(),
            preferred_transports: default_integration_preferred_transports(),
            supported_auth_schemes: default_integration_supported_auth_schemes(),
        }
    }
}

pub(crate) fn default_integration_request_timeout_secs() -> u64 {
    15
}

pub(crate) fn default_integration_connect_retry_secs() -> u64 {
    15
}

pub(crate) fn default_integration_heartbeat_interval_secs() -> u64 {
    30
}

pub(crate) fn default_integration_sync_interval_secs() -> u64 {
    15
}

pub(crate) fn default_integration_produce_interval_secs() -> u64 {
    30
}

pub(crate) fn default_integration_inbox_refresh_interval_secs() -> u64 {
    15
}

pub(crate) fn default_integration_max_batch_size() -> usize {
    50
}

pub(crate) fn default_integration_max_stored_prompts() -> usize {
    256
}

pub(crate) fn default_integration_redact_completed_prompt_bodies() -> bool {
    true
}

fn default_integration_preferred_transports() -> Vec<IntegrationTransportKind> {
    vec![
        IntegrationTransportKind::WebSocket,
        IntegrationTransportKind::HttpsSse,
        IntegrationTransportKind::HttpsLongPoll,
    ]
}

fn default_integration_supported_auth_schemes() -> Vec<IntegrationAuthScheme> {
    vec![IntegrationAuthScheme::BearerToken]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn integration_config_defaults_are_safe() {
        let config = IntegrationConfig::default();
        assert!(!config.enabled);
        assert!(config.bootstrap_url.is_none());
        assert_eq!(config.request_timeout_secs, 15);
        assert_eq!(config.connect_retry_secs, 15);
        assert_eq!(config.heartbeat_interval_secs, 30);
        assert_eq!(config.sync_interval_secs, 15);
        assert_eq!(config.produce_interval_secs, 30);
        assert_eq!(config.inbox_refresh_interval_secs, 15);
        assert_eq!(config.max_batch_size, 50);
        assert_eq!(config.max_stored_prompts, 256);
        assert!(config.redact_completed_prompt_bodies);
        assert_eq!(
            config.preferred_transports,
            vec![
                IntegrationTransportKind::WebSocket,
                IntegrationTransportKind::HttpsSse,
                IntegrationTransportKind::HttpsLongPoll,
            ]
        );
        assert_eq!(
            config.supported_auth_schemes,
            vec![IntegrationAuthScheme::BearerToken]
        );
    }

    #[test]
    fn integration_config_deserializes_custom_values() {
        let parsed: IntegrationConfig = serde_json::from_value(json!({
            "enabled": true,
            "auth_profile_kind": "oidc_device_flow",
            "bootstrap_url": "https://integration.example.com/bootstrap",
            "device_id": "device-001",
            "device_label": "workstation",
            "resource_indicator": "https://integration.example.com",
            "auth_token_env_var": "ONESHIM_INTEGRATION_TOKEN",
            "oidc_device_flow": {
                "client_id": "desktop-client",
                "device_authorization_url": "https://id.example.com/oauth/device/code",
                "token_url": "https://id.example.com/oauth/token",
                "scopes": ["openid", "profile", "offline_access"]
            },
            "request_timeout_secs": 20,
            "connect_retry_secs": 25,
            "heartbeat_interval_secs": 45,
            "sync_interval_secs": 12,
            "produce_interval_secs": 18,
            "inbox_refresh_interval_secs": 9,
            "max_batch_size": 24,
            "max_stored_prompts": 96,
            "redact_completed_prompt_bodies": false,
            "preferred_transports": ["web_socket", "https_long_poll"],
            "supported_auth_schemes": ["bearer_token"]
        }))
        .unwrap();

        assert!(parsed.enabled);
        assert_eq!(
            parsed.bootstrap_url.as_deref(),
            Some("https://integration.example.com/bootstrap")
        );
        assert_eq!(
            parsed.auth_profile_kind,
            IntegrationAuthProfileKind::OidcDeviceFlow
        );
        assert_eq!(parsed.device_id.as_deref(), Some("device-001"));
        assert_eq!(
            parsed.oidc_device_flow.client_id.as_deref(),
            Some("desktop-client")
        );
        assert_eq!(
            parsed.preferred_transports,
            vec![
                IntegrationTransportKind::WebSocket,
                IntegrationTransportKind::HttpsLongPoll
            ]
        );
        assert_eq!(parsed.connect_retry_secs, 25);
        assert_eq!(parsed.heartbeat_interval_secs, 45);
        assert_eq!(parsed.sync_interval_secs, 12);
        assert_eq!(parsed.produce_interval_secs, 18);
        assert_eq!(parsed.inbox_refresh_interval_secs, 9);
        assert_eq!(parsed.max_batch_size, 24);
        assert_eq!(parsed.max_stored_prompts, 96);
        assert!(!parsed.redact_completed_prompt_bodies);
    }
}
