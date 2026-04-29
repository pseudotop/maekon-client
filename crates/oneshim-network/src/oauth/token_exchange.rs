//! OAuth token exchange — authorization code → access + refresh tokens.

use oneshim_core::error::CoreError;
use oneshim_core::ports::oauth::OAuthErrorKind;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use super::provider_config::OAuthProviderConfig;

/// Successful token exchange result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenExchangeResult {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<u64>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub token_type: Option<String>,
}

/// Error response from the token endpoint.
#[derive(Debug, Deserialize)]
struct TokenErrorResponse {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

/// Exchange an authorization code for tokens.
pub async fn exchange_code(
    http: &reqwest::Client,
    config: &OAuthProviderConfig,
    code: &str,
    pkce_verifier: &str,
) -> Result<TokenExchangeResult, CoreError> {
    let redirect_uri = config.redirect_uri();

    debug!(
        "exchanging authorization code for tokens (provider: {})",
        config.provider_id
    );

    let resp = http
        .post(&config.token_endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri.as_str()),
            ("client_id", config.client_id.as_str()),
            ("code_verifier", pkce_verifier),
        ])
        .send()
        .await
        .map_err(|e| CoreError::OAuthError {
            code: oneshim_core::error_codes::OAuthCode::Failed,
            provider: config.provider_id.clone(),
            message: format!("token exchange request failed: {e}"),
        })?;

    let status = resp.status();
    let body = resp.text().await.map_err(|e| CoreError::OAuthError {
        code: oneshim_core::error_codes::OAuthCode::Failed,
        provider: config.provider_id.clone(),
        message: format!("failed to read token response: {e}"),
    })?;

    if !status.is_success() {
        if let Ok(err) = serde_json::from_str::<TokenErrorResponse>(&body) {
            let desc = err.error_description.unwrap_or_default();
            warn!(
                "token exchange failed: {} — {} (status: {})",
                err.error, desc, status
            );
            return Err(CoreError::OAuthError {
                code: oneshim_core::error_codes::OAuthCode::Failed,
                provider: config.provider_id.clone(),
                message: format!("{}: {desc}", err.error),
            });
        }
        return Err(CoreError::OAuthError {
            code: oneshim_core::error_codes::OAuthCode::Failed,
            provider: config.provider_id.clone(),
            message: format!("token exchange returned {status}"),
        });
    }

    let result: TokenExchangeResult =
        serde_json::from_str(&body).map_err(|e| CoreError::OAuthError {
            code: oneshim_core::error_codes::OAuthCode::Failed,
            provider: config.provider_id.clone(),
            message: format!("failed to parse token response: {e}"),
        })?;

    debug!(
        "token exchange successful (expires_in: {:?}, has_refresh: {})",
        result.expires_in,
        result.refresh_token.is_some()
    );

    Ok(result)
}

/// Classify an HTTP error response into a typed `OAuthErrorKind`.
///
/// Examines the status code first (429 → RateLimited, 5xx → ServerError),
/// then attempts to parse the JSON `error` field for OAuth-specific codes.
fn classify_error_response(status: u16, body: &str) -> OAuthErrorKind {
    // HTTP-level classification takes precedence for unambiguous status codes.
    if status == 429 {
        return OAuthErrorKind::RateLimited;
    }
    if (500..600).contains(&status) {
        return OAuthErrorKind::ServerError;
    }

    // Attempt JSON error field parsing.
    if let Ok(err) = serde_json::from_str::<TokenErrorResponse>(body) {
        return match err.error.as_str() {
            "invalid_grant" => OAuthErrorKind::InvalidGrant,
            "invalid_client" => OAuthErrorKind::InvalidClient,
            "invalid_scope" => OAuthErrorKind::InvalidScope,
            "server_error" => OAuthErrorKind::ServerError,
            other => OAuthErrorKind::Unknown(other.to_string()),
        };
    }

    OAuthErrorKind::Unknown(format!("HTTP {status}"))
}

/// Refresh an access token using a refresh token.
pub async fn refresh_token(
    http: &reqwest::Client,
    config: &OAuthProviderConfig,
    refresh_tok: &str,
) -> Result<TokenExchangeResult, CoreError> {
    debug!("refreshing access token (provider: {})", config.provider_id);

    let resp = http
        .post(&config.token_endpoint)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_tok),
            ("client_id", config.client_id.as_str()),
        ])
        .send()
        .await
        .map_err(|e| {
            let kind = if e.is_timeout() || e.is_connect() {
                OAuthErrorKind::NetworkError
            } else {
                OAuthErrorKind::Unknown(e.to_string())
            };
            CoreError::OAuthRefreshError {
                code: oneshim_core::error_codes::OAuthCode::RefreshFailed,
                provider: config.provider_id.clone(),
                kind,
                message: format!("token refresh request failed: {e}"),
            }
        })?;

    let status = resp.status().as_u16();
    let body = resp
        .text()
        .await
        .map_err(|e| CoreError::OAuthRefreshError {
            code: oneshim_core::error_codes::OAuthCode::RefreshFailed,
            provider: config.provider_id.clone(),
            kind: OAuthErrorKind::NetworkError,
            message: format!("failed to read refresh response: {e}"),
        })?;

    if !(200..300).contains(&status) {
        let kind = classify_error_response(status, &body);
        let desc = serde_json::from_str::<TokenErrorResponse>(&body)
            .ok()
            .and_then(|e| e.error_description)
            .unwrap_or_default();
        warn!(
            "token refresh failed for {}: [{:?}] {desc} (status: {status})",
            config.provider_id, kind
        );
        return Err(CoreError::OAuthRefreshError {
            code: oneshim_core::error_codes::OAuthCode::RefreshFailed,
            provider: config.provider_id.clone(),
            kind,
            message: format!("refresh failed (HTTP {status}): {desc}"),
        });
    }

    serde_json::from_str(&body).map_err(|e| CoreError::OAuthRefreshError {
        code: oneshim_core::error_codes::OAuthCode::RefreshFailed,
        provider: config.provider_id.clone(),
        kind: OAuthErrorKind::Unknown("parse_error".into()),
        message: format!("failed to parse refresh response: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_result_deserializes_minimal() {
        let json = r#"{"access_token": "at_123", "token_type": "Bearer"}"#;
        let result: TokenExchangeResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.access_token, "at_123");
        assert!(result.refresh_token.is_none());
        assert!(result.expires_in.is_none());
    }

    #[test]
    fn token_result_deserializes_full() {
        let json = r#"{
            "access_token": "at_abc",
            "refresh_token": "rt_xyz",
            "expires_in": 3600,
            "scope": "openid profile",
            "token_type": "Bearer"
        }"#;
        let result: TokenExchangeResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.access_token, "at_abc");
        assert_eq!(result.refresh_token.as_deref(), Some("rt_xyz"));
        assert_eq!(result.expires_in, Some(3600));
        assert_eq!(result.scope.as_deref(), Some("openid profile"));
    }

    #[test]
    fn error_response_deserializes() {
        let json = r#"{"error": "invalid_grant", "error_description": "code expired"}"#;
        let err: TokenErrorResponse = serde_json::from_str(json).unwrap();
        assert_eq!(err.error, "invalid_grant");
        assert_eq!(err.error_description.as_deref(), Some("code expired"));
    }

    #[test]
    fn classify_http_429_as_rate_limited() {
        let kind = classify_error_response(429, "");
        assert!(matches!(kind, OAuthErrorKind::RateLimited));
    }

    #[test]
    fn classify_http_500_as_server_error() {
        let kind = classify_error_response(500, r#"{"error":"server_error"}"#);
        assert!(matches!(kind, OAuthErrorKind::ServerError));
    }

    #[test]
    fn classify_invalid_grant_from_json() {
        let body = r#"{"error":"invalid_grant","error_description":"token revoked"}"#;
        let kind = classify_error_response(400, body);
        assert!(matches!(kind, OAuthErrorKind::InvalidGrant));
    }

    #[test]
    fn classify_invalid_client_from_json() {
        let body = r#"{"error":"invalid_client"}"#;
        let kind = classify_error_response(401, body);
        assert!(matches!(kind, OAuthErrorKind::InvalidClient));
    }

    #[test]
    fn classify_unknown_error_code() {
        let body = r#"{"error":"unsupported_grant_type"}"#;
        let kind = classify_error_response(400, body);
        assert!(matches!(kind, OAuthErrorKind::Unknown(ref s) if s == "unsupported_grant_type"));
    }

    #[test]
    fn classify_non_json_body() {
        let kind = classify_error_response(400, "Bad Request");
        assert!(matches!(kind, OAuthErrorKind::Unknown(ref s) if s == "HTTP 400"));
    }
}
