//! HTTP-based authentication for the Maekon network layer.
//!
//! Public surface: [`TokenManager`] — the sole entry-point for callers.
//!
//! # Module layout (ADR-013 split)
//!
//! | file | responsibility |
//! |------|----------------|
//! | `tokens.rs` | `TokenResponse`, `TokenState`, `TokenManager` struct, constructors, `get_token`, `is_authenticated` |
//! | `refresh.rs` | `refresh()` with exponential-backoff retry + `parse_retry_after` |
//! | `mod.rs` (this file) | `login`, `login_with_org`, `verify`, `logout`, `logout_all_sessions` |
//! | `tests.rs` | All `#[cfg(test)]` content |

mod refresh;
mod tests;
mod tokens;

// Re-export the sole public type so that `use maekon_network::auth::TokenManager` continues
// to work without any callsite changes.
pub use tokens::TokenManager;

use chrono::{Duration, Utc};
use maekon_core::error::CoreError;
use tokens::{TokenState, MAX_TOKEN_TTL_SECS};
use tracing::debug;

impl TokenManager {
    // ── Login ─────────────────────────────────────────────────────────────────

    /// Login using the `MAEKON_ORGANIZATION_ID` environment variable (falls
    /// back to `"default"` when the variable is absent).
    ///
    /// # Arguments
    pub async fn login(&self, email: &str, password: &str) -> Result<(), CoreError> {
        let organization_id =
            std::env::var("MAEKON_ORGANIZATION_ID").unwrap_or_else(|_| "default".to_string());
        self.login_with_org(email, password, &organization_id).await
    }

    pub async fn login_with_org(
        &self,
        email: &str,
        password: &str,
        organization_id: &str,
    ) -> Result<(), CoreError> {
        let url = format!("{}/api/v1/auth/tokens", self.base_url);
        let body = serde_json::json!({
            "identifier": email,
            "password": password,
            "organization_id": organization_id,
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| CoreError::Auth {
                code: maekon_core::error_codes::AuthCode::Failed,
                message: format!("login request failure: {e}"),
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            // #6949: cap the login error response body (OOM guard)
            let text =
                crate::outbound::read_text_capped(resp, crate::outbound::MAX_AUTH_RESPONSE_BYTES)
                    .await
                    .unwrap_or_default();
            let message = format!("login failure ({status}): {text}");
            // Semantic status mapping per iter-54..60. For login specifically,
            // 401/403 are definitive auth failures, but 429/503/504 indicate
            // transient auth-service issues that frontend should surface
            // differently (e.g., "try again shortly" vs "credentials wrong").
            return Err(match status.as_u16() {
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
            });
        }

        // #6949: cap the login token response body (OOM guard)
        let token_bytes =
            crate::outbound::read_body_capped(resp, crate::outbound::MAX_AUTH_RESPONSE_BYTES)
                .await
                .map_err(|e| match e {
                    crate::outbound::BodyReadError::Transport(te) => CoreError::Auth {
                        code: maekon_core::error_codes::AuthCode::Failed,
                        message: format!("Token parsing failed: {te}"),
                    },
                    crate::outbound::BodyReadError::TooLarge { len, cap } => CoreError::Auth {
                        code: maekon_core::error_codes::AuthCode::Failed,
                        message: format!(
                            "Token parsing failed: response too large ({len} > {cap})"
                        ),
                    },
                })?;
        let token_resp: tokens::TokenResponse =
            serde_json::from_slice(&token_bytes).map_err(|e| CoreError::Auth {
                code: maekon_core::error_codes::AuthCode::Failed,
                message: format!("Token parsing failed: {e}"),
            })?;

        // Clamp the server-supplied TTL before building a chrono::Duration.
        // `expires_in` is server-controlled; an adversarial/huge value (e.g.
        // i64::MAX) would otherwise panic in `Duration::seconds()` (TimeDelta
        // out-of-range) or overflow the `Utc::now() + Duration` addition. A
        // 30-day cap is trivially in range and neutralizes both panic paths.
        let ttl = token_resp
            .expires_in
            .unwrap_or(3600)
            .clamp(0, MAX_TOKEN_TTL_SECS);
        let expires_at = Utc::now() + Duration::seconds(ttl);

        let mut state = self.state.write().await;
        *state = Some(TokenState {
            access_token: token_resp.access_token,
            refresh_token: token_resp.refresh_token,
            expires_at,
        });

        debug!("login success, token: {expires_at}");
        Ok(())
    }

    // ── Verify ────────────────────────────────────────────────────────────────

    /// Validate the current token against the server's verify endpoint.
    ///
    /// Returns `Ok(true)` on a 2xx response, `Ok(false)` on any non-2xx, and
    /// `Err` only when the network call itself fails.
    pub async fn verify(&self) -> Result<bool, CoreError> {
        let token = self.get_token().await?;
        let url = format!("{}/api/v1/auth/tokens/verify", self.base_url);

        let resp = self
            .client
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| CoreError::Auth {
                code: maekon_core::error_codes::AuthCode::Failed,
                message: format!("token validation request failure: {e}"),
            })?;

        Ok(resp.status().is_success())
    }

    // ── Logout ────────────────────────────────────────────────────────────────

    /// Revoke the current session on the server and clear local token state.
    ///
    /// Server-side revocation failures are logged as warnings but do not
    /// prevent local state from being cleared — the UX contract is that a
    /// successful `logout()` call always leaves the client unauthenticated.
    pub async fn logout(&self) -> Result<(), CoreError> {
        let token = self.get_token().await.ok();

        if let Some(token) = token {
            let url = format!("{}/api/v1/auth/tokens", self.base_url);
            if let Err(e) = self.client.delete(&url).bearer_auth(&token).send().await {
                tracing::warn!("server-side token revocation failed (local state cleared): {e}");
            }
        }

        let mut state = self.state.write().await;
        *state = None;
        debug!("logout completed");
        Ok(())
    }

    /// Sign out from all devices (revoke all sessions, including the current one).
    ///
    /// OOS-N15 (2026-05-05): Server endpoint `DELETE /api/v1/auth/tokens/all` —
    /// `invalidate_all_user_sessions(except_current_session_id=None)` routes through
    /// all device token/session revocation.  UI "Sign out of all devices" entry point.
    ///
    /// Local state is cleared on both success and server failure (same policy as
    /// [`logout`]).
    ///
    /// # Errors
    ///
    /// Returns `CoreError::Auth` when no valid token is available.  Server-call
    /// failures are warn-logged and ignored; local state is always cleared.
    pub async fn logout_all_sessions(&self) -> Result<(), CoreError> {
        let token = self.get_token().await.ok();

        if let Some(token) = token {
            let url = format!("{}/api/v1/auth/tokens/all", self.base_url);
            if let Err(e) = self.client.delete(&url).bearer_auth(&token).send().await {
                tracing::warn!("server-side full logout failed (local state cleared): {e}");
            }
        }

        let mut state = self.state.write().await;
        *state = None;
        debug!("logout_all_sessions completed");
        Ok(())
    }
}
