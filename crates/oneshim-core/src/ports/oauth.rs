//! OAuth port — runtime abstraction for provider-managed credential flows.
//!
//! The OAuthPort trait defines the contract for starting, polling, cancelling,
//! and revoking OAuth flows against external providers.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;

/// Handle returned when an OAuth flow is initiated.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthFlowHandle {
    pub flow_id: String,
    pub auth_url: String,
}

/// Status of a pending or completed OAuth flow.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum OAuthFlowStatus {
    Pending,
    Completed,
    Failed { error: String },
    Cancelled,
}

/// Connection status for a provider's managed credentials.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthConnectionStatus {
    pub provider_id: String,
    pub connected: bool,
    pub expires_at: Option<String>,
    pub scopes: Vec<String>,
    /// API base URL for authenticated requests (e.g., `chatgpt.com/backend-api/codex`).
    #[serde(default)]
    pub api_base_url: Option<String>,
    /// Whether a refresh token is stored for this provider.
    #[serde(default)]
    pub has_refresh_token: bool,
}

/// Classification of OAuth error responses.
///
/// Typed replacement for fragile substring matching on error messages.
/// Terminal kinds (`InvalidGrant`, `InvalidClient`) require re-authentication;
/// transient kinds can be retried with backoff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OAuthErrorKind {
    InvalidGrant,
    InvalidClient,
    InvalidScope,
    ServerError,
    NetworkError,
    RateLimited,
    Unknown(String),
}

impl OAuthErrorKind {
    /// Returns `true` for error kinds that cannot be recovered by retrying.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::InvalidGrant | Self::InvalidClient)
    }
}

/// Result of an access-token refresh attempt.
#[derive(Debug, Clone)]
pub enum RefreshResult {
    AlreadyFresh {
        expires_at: String,
    },
    Refreshed {
        expires_at: String,
    },
    NotAuthenticated,
    ReauthRequired {
        kind: OAuthErrorKind,
        reason: String,
    },
    TransientFailure {
        kind: OAuthErrorKind,
        message: String,
    },
}

/// OAuth runtime port.
///
/// Implementations coordinate PKCE, callback server, token exchange,
/// and secure storage.
///
/// # Errors
/// - `CoreError::OAuthError` (wire: `oauth.failed`) for provider-side
///   token exchange / PKCE verification failures, including
///   RFC 6749 error classification (`invalid_grant`, `invalid_client`,
///   `server_error`). See `oneshim-network::oauth::token_exchange`.
/// - `CoreError::OAuthRefreshError` (wire: `oauth.refresh_failed`)
///   for refresh-specific failures. The variant carries a typed
///   `OAuthErrorKind` (InvalidGrant/InvalidClient/InvalidScope/
///   ServerError/NetworkError/RateLimited/Unknown) classified from
///   the HTTP response per RFC 6749. See
///   `oneshim-network/src/oauth/token_exchange.rs::classify_error_response`
///   for the full status-to-kind mapping.
/// - `CoreError::Auth` (wire: `auth.failed`) for session-token-level
///   auth failures distinct from the OAuth flow itself.
/// - `CoreError::SecretStoreError` (wire: `secret.failed`) when
///   secret-store read/write for OAuth tokens fails.
/// - Note: HTTP-status arms NOT applied to token exchange — RFC 6749
///   semantic classification is the source of truth.
#[async_trait]
pub trait OAuthPort: Send + Sync {
    /// Start an OAuth authorization flow. Returns a handle with the auth URL
    /// that should be opened in the user's browser.
    async fn start_flow(&self, provider_id: &str) -> Result<OAuthFlowHandle, CoreError>;

    /// Check the status of a pending flow.
    async fn flow_status(&self, flow_id: &str) -> Result<OAuthFlowStatus, CoreError>;

    /// Cancel a pending flow and shut down the callback server.
    async fn cancel_flow(&self, flow_id: &str) -> Result<(), CoreError>;

    /// Get an active access token for a provider, refreshing if needed.
    /// Returns `None` if not authenticated.
    async fn get_access_token(&self, provider_id: &str) -> Result<Option<String>, CoreError>;

    /// Revoke stored credentials and disconnect.
    async fn revoke(&self, provider_id: &str) -> Result<(), CoreError>;

    /// Get connection status for a provider.
    async fn connection_status(
        &self,
        provider_id: &str,
    ) -> Result<OAuthConnectionStatus, CoreError>;

    /// Refresh the access token for a provider if it is expiring soon.
    ///
    /// `min_valid_for_secs` specifies how many seconds of remaining validity
    /// are required — if the token expires sooner, a refresh is attempted.
    async fn refresh_access_token(
        &self,
        provider_id: &str,
        min_valid_for_secs: i64,
    ) -> Result<RefreshResult, CoreError>;
}

/// Events emitted during token lifecycle operations.
#[derive(Clone, Debug)]
pub enum TokenEvent {
    Refreshed {
        provider_id: String,
        expires_at: String,
    },
    RefreshFailed {
        provider_id: String,
        attempt: u8,
        max_attempts: u8,
    },
    ReauthRequired {
        provider_id: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_handle_serialization() {
        let handle = OAuthFlowHandle {
            flow_id: "abc-123".to_string(),
            auth_url: "https://auth.openai.com/oauth/authorize?foo=bar".to_string(),
        };
        let json = serde_json::to_string(&handle).unwrap();
        let parsed: OAuthFlowHandle = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.flow_id, "abc-123");
    }

    #[test]
    fn flow_status_serialization() {
        let statuses = vec![
            OAuthFlowStatus::Pending,
            OAuthFlowStatus::Completed,
            OAuthFlowStatus::Failed {
                error: "timeout".to_string(),
            },
            OAuthFlowStatus::Cancelled,
        ];
        for status in &statuses {
            let json = serde_json::to_string(status).unwrap();
            let parsed: OAuthFlowStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(&parsed, status);
        }
    }

    #[test]
    fn connection_status_serialization() {
        let status = OAuthConnectionStatus {
            provider_id: "openai".to_string(),
            connected: true,
            expires_at: Some("2026-03-14T00:00:00Z".to_string()),
            scopes: vec!["openid".to_string(), "offline_access".to_string()],
            api_base_url: Some("https://chatgpt.com/backend-api/codex".to_string()),
            has_refresh_token: true,
        };
        let json = serde_json::to_string(&status).unwrap();
        let parsed: OAuthConnectionStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.provider_id, "openai");
        assert!(parsed.connected);
        assert_eq!(parsed.scopes.len(), 2);
        assert!(parsed.has_refresh_token);
    }

    #[test]
    fn connection_status_deserializes_without_has_refresh_token() {
        let json = r#"{"provider_id":"openai","connected":false,"expires_at":null,"scopes":[]}"#;
        let parsed: OAuthConnectionStatus = serde_json::from_str(json).unwrap();
        assert!(!parsed.has_refresh_token);
    }

    #[test]
    fn error_kind_is_terminal() {
        assert!(OAuthErrorKind::InvalidGrant.is_terminal());
        assert!(OAuthErrorKind::InvalidClient.is_terminal());
        assert!(!OAuthErrorKind::InvalidScope.is_terminal());
        assert!(!OAuthErrorKind::ServerError.is_terminal());
        assert!(!OAuthErrorKind::NetworkError.is_terminal());
        assert!(!OAuthErrorKind::RateLimited.is_terminal());
        assert!(!OAuthErrorKind::Unknown("foo".into()).is_terminal());
    }

    #[test]
    fn error_kind_serialization_roundtrip() {
        let kinds = vec![
            OAuthErrorKind::InvalidGrant,
            OAuthErrorKind::InvalidClient,
            OAuthErrorKind::InvalidScope,
            OAuthErrorKind::ServerError,
            OAuthErrorKind::NetworkError,
            OAuthErrorKind::RateLimited,
            OAuthErrorKind::Unknown("custom_error".into()),
        ];
        for kind in &kinds {
            let json = serde_json::to_string(kind).unwrap();
            let parsed: OAuthErrorKind = serde_json::from_str(&json).unwrap();
            assert_eq!(format!("{:?}", kind), format!("{:?}", parsed));
        }
    }
}
