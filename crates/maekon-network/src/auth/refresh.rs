//! Token refresh logic: exponential backoff retry loop + `Retry-After` header parsing.

use std::time::Duration as StdDuration;

use maekon_core::error::CoreError;
use tracing::warn;

use super::tokens::{TokenManager, TokenState};

/// Parse a `Retry-After` header value (integer seconds only).
///
/// Returns `None` for absent or non-integer values (HTTP-date format is not
/// supported).  The returned duration is capped at 60 seconds to prevent an
/// abusive server from stalling the client indefinitely.
pub(super) fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<StdDuration> {
    let value = headers.get("retry-after")?.to_str().ok()?;
    if let Ok(secs) = value.parse::<u64>() {
        return Some(StdDuration::from_secs(secs.min(60)));
    }
    None
}

impl TokenManager {
    /// Refresh the access token using the stored refresh token.
    ///
    /// Retries up to `MAX_RETRIES` times with exponential back-off for
    /// transient failures (5xx, 429).  Non-retryable 4xx responses (except 429)
    /// return immediately.
    ///
    /// HTTP status mapping follows the canonical pattern from
    /// `docs/guides/http-status-error-mapping.md` so that telemetry can
    /// distinguish "auth provider is down" from "credentials rejected".
    pub async fn refresh(&self) -> Result<(), CoreError> {
        const MAX_RETRIES: u32 = 3;
        const INITIAL_BACKOFF_MS: u64 = 500;
        const MAX_BACKOFF_MS: u64 = 8_000;

        let current = {
            let state = self.state.read().await;
            state.clone()
        };

        let current = current.ok_or_else(|| CoreError::Auth {
            code: maekon_core::error_codes::AuthCode::Failed,
            message: "Not authenticated".to_string(),
        })?;
        let refresh_token = current.refresh_token.ok_or_else(|| CoreError::Auth {
            code: maekon_core::error_codes::AuthCode::Failed,
            message: "Refresh token is missing".to_string(),
        })?;

        let url = format!("{}/api/v1/auth/tokens/refresh", self.base_url);

        let mut last_err = CoreError::Auth {
            code: maekon_core::error_codes::AuthCode::Failed,
            message: "token refresh failed".to_string(),
        };

        for attempt in 0..=MAX_RETRIES {
            let body = serde_json::json!({
                "refresh_token": refresh_token,
            });

            let result = self.client.post(&url).json(&body).send().await;

            match result {
                Ok(resp) => {
                    let status = resp.status();

                    if status.is_success() {
                        let token_resp: super::tokens::TokenResponse =
                            resp.json().await.map_err(|e| CoreError::Auth {
                                code: maekon_core::error_codes::AuthCode::Failed,
                                message: format!("refresh Token parsing failed: {e}"),
                            })?;

                        let expires_at = chrono::Utc::now()
                            + chrono::Duration::seconds(token_resp.expires_in.unwrap_or(3600));

                        let mut state = self.state.write().await;
                        *state = Some(TokenState {
                            access_token: token_resp.access_token,
                            refresh_token: token_resp.refresh_token.or(Some(refresh_token.clone())),
                            expires_at,
                        });

                        tracing::debug!("token refresh success, expires_at: {expires_at}");
                        return Ok(());
                    }

                    // 429 Too Many Requests — Retry-After header takes priority
                    if status.as_u16() == 429 {
                        if let Some(retry_duration) = parse_retry_after(resp.headers()) {
                            warn!(
                                attempt = attempt + 1,
                                retry_after_secs = retry_duration.as_secs(),
                                "token refresh rate-limited, waiting Retry-After"
                            );
                            tokio::time::sleep(retry_duration).await;
                            continue;
                        }
                    }

                    // 4xx errors (except 429) are not retryable
                    let is_retryable = status.is_server_error() || status.as_u16() == 429;

                    let text = resp.text().await.unwrap_or_default();
                    let message = format!("token refresh failure ({status}): {text}");
                    // Iter-98: apply canonical HTTP status mapping consistent
                    // with login() — lets telemetry distinguish "auth provider
                    // is down" from "credentials rejected".
                    last_err = match status.as_u16() {
                        408 | 504 => CoreError::RequestTimeout {
                            code: maekon_core::error_codes::NetworkCode::Timeout,
                            timeout_ms: 0,
                        },
                        429 => CoreError::RateLimit {
                            code: maekon_core::error_codes::NetworkCode::RateLimit,
                            retry_after_secs: 60,
                        },
                        502 | 503 => CoreError::ServiceUnavailable {
                            code: maekon_core::error_codes::ServiceCode::Unavailable,
                            message,
                        },
                        _ => CoreError::Auth {
                            code: maekon_core::error_codes::AuthCode::Failed,
                            message,
                        },
                    };

                    if !is_retryable {
                        return Err(last_err);
                    }
                }
                Err(e) => {
                    // Iter-98: reqwest transport failure (pre-HTTP-status) —
                    // split timeout vs connection error per canonical pattern
                    // (same as cloud_stt.rs / http_client.rs).
                    last_err = if e.is_timeout() {
                        CoreError::RequestTimeout {
                            code: maekon_core::error_codes::NetworkCode::Timeout,
                            timeout_ms: 0,
                        }
                    } else {
                        CoreError::Network {
                            code: maekon_core::error_codes::NetworkCode::Generic,
                            message: format!("token refresh request failure: {e}"),
                        }
                    };
                }
            }

            if attempt < MAX_RETRIES {
                let backoff_ms = (INITIAL_BACKOFF_MS * 2u64.pow(attempt)).min(MAX_BACKOFF_MS);
                warn!(
                    attempt = attempt + 1,
                    max = MAX_RETRIES,
                    backoff_ms,
                    "token refresh failed, retrying"
                );
                tokio::time::sleep(StdDuration::from_millis(backoff_ms)).await;
            }
        }

        Err(last_err)
    }
}
