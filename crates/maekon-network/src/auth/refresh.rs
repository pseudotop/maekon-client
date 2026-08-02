//! Token refresh logic: exponential backoff retry loop + `Retry-After` header parsing.

use std::sync::atomic::Ordering;
use std::time::Duration as StdDuration;

use maekon_core::error::CoreError;
use tracing::warn;

use super::tokens::{TokenManager, TokenState, MAX_TOKEN_TTL_SECS};
use crate::resilience::jittered_backoff_delay;

/// Retry ceiling for [`TokenManager::refresh`]. Hoisted to module scope
/// (rather than a `fn`-local `const`) so tests can pin the backoff envelope
/// against the exact tuning `refresh()` uses.
pub(super) const REFRESH_MAX_RETRIES: u32 = 3;

/// Base backoff delay fed to [`jittered_backoff_delay`] by [`TokenManager::refresh`].
pub(super) const REFRESH_INITIAL_BACKOFF_MS: u64 = 500;

/// Cap on the refresh backoff delay.
pub(super) const REFRESH_MAX_BACKOFF_MS: u64 = 8_000;

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
        const MAX_RETRIES: u32 = REFRESH_MAX_RETRIES;
        const INITIAL_BACKOFF_MS: u64 = REFRESH_INITIAL_BACKOFF_MS;
        const MAX_BACKOFF_MS: u64 = REFRESH_MAX_BACKOFF_MS;

        // #9491: capture the session generation under the SAME read lock as the
        // snapshot, so the two always describe one session. Every state
        // transition bumps the counter under the state write lock, so a
        // different value at commit time means this rotation belongs to a
        // session that no longer exists.
        let (current, entry_generation) = {
            let state = self.state.read().await;
            (
                state.clone(),
                self.session_generation.load(Ordering::SeqCst),
            )
        };

        let current = current.ok_or_else(|| CoreError::Auth {
            code: maekon_core::error_codes::AuthCode::Failed,
            message: "Not authenticated".to_string(),
        })?;
        let refresh_token = current.refresh_token.ok_or_else(|| CoreError::Auth {
            code: maekon_core::error_codes::AuthCode::Failed,
            message: "Refresh token is missing".to_string(),
        })?;
        // The refresh response carries token material only, so who the session
        // belongs to has to survive the swap from the previous state.
        let identifier = current.identifier.clone();
        let organization_id = current.organization_id.clone();

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
                        // #6949: cap the token-refresh success response body (OOM guard)
                        let token_bytes = crate::outbound::read_body_capped(
                            resp,
                            crate::outbound::MAX_AUTH_RESPONSE_BYTES,
                        )
                        .await
                        .map_err(|e| match e {
                            crate::outbound::BodyReadError::Transport(te) => CoreError::Auth {
                                code: maekon_core::error_codes::AuthCode::Failed,
                                message: format!("refresh Token parsing failed: {te}"),
                            },
                            crate::outbound::BodyReadError::TooLarge { len, cap } => {
                                CoreError::Auth {
                                    code: maekon_core::error_codes::AuthCode::Failed,
                                    message: format!(
                                        "refresh Token parsing failed: response too large ({len} > {cap})"
                                    ),
                                }
                            }
                        })?;
                        let token_resp: super::tokens::TokenResponse =
                            serde_json::from_slice(&token_bytes).map_err(|e| CoreError::Auth {
                                code: maekon_core::error_codes::AuthCode::Failed,
                                message: format!("refresh Token parsing failed: {e}"),
                            })?;

                        // Clamp the server-supplied TTL before building a
                        // chrono::Duration — see MAX_TOKEN_TTL_SECS. An
                        // adversarial/huge `expires_in` would otherwise panic in
                        // `Duration::seconds()` or overflow the `Utc::now() +
                        // Duration` addition.
                        let ttl = token_resp
                            .expires_in
                            .unwrap_or(3600)
                            .clamp(0, MAX_TOKEN_TTL_SECS);
                        let expires_at = chrono::Utc::now() + chrono::Duration::seconds(ttl);

                        let mut state = self.state.write().await;
                        // A session change can land while this refresh is in
                        // flight: neither `logout()` / `logout_all_sessions()`
                        // nor `login_with_org()` holds `refresh_lock`, so the
                        // state can be cleared (and the persisted namespace
                        // wiped) — and then re-populated by a fresh login,
                        // possibly for a *different* account — between the
                        // request above and this write. Committing the rotation
                        // now would repopulate memory AND re-persist the
                        // previous session's tokens, identifier, and
                        // organization: after a plain logout the next launch
                        // would greet a signed-out user, and after a re-login
                        // the live session would silently revert to the account
                        // the user just left (#9491).
                        //
                        // The generation captured at entry is the discriminator,
                        // and it is re-read HERE, under the write lock that
                        // every transition also holds — so the comparison
                        // cannot race a concurrent login/logout. It subsumes the
                        // old bare `state.is_none()` post-logout check (a logout
                        // bumps the generation too); `is_none()` is retained
                        // only as a backstop for a state written directly,
                        // outside the transition helpers.
                        let superseded =
                            self.session_generation.load(Ordering::SeqCst) != entry_generation;
                        if superseded || state.is_none() {
                            drop(state);
                            tracing::debug!(
                                superseded,
                                "token refresh completed after the session changed; \
                                 rotated tokens discarded"
                            );
                            return Err(CoreError::Auth {
                                code: maekon_core::error_codes::AuthCode::Failed,
                                message: "Not authenticated (session ended during refresh)"
                                    .to_string(),
                            });
                        }
                        *state = Some(TokenState {
                            access_token: token_resp.access_token,
                            refresh_token: token_resp.refresh_token.or(Some(refresh_token.clone())),
                            expires_at,
                            identifier: identifier.clone(),
                            organization_id: organization_id.clone(),
                        });
                        drop(state);
                        self.persist_current_state().await;

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

                    // #6949: cap the token-refresh error response body (OOM guard)
                    let text = crate::outbound::read_text_capped(
                        resp,
                        crate::outbound::MAX_AUTH_RESPONSE_BYTES,
                    )
                    .await
                    .unwrap_or_default();
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
                // #7725: adopted `resilience::jittered_backoff_delay` in place of
                // the hand-rolled `(INITIAL_BACKOFF_MS * 2u64.pow(attempt)).min(MAX_BACKOFF_MS)`
                // doubling (found during the #7725 fix-class completeness sweep,
                // beyond the originally-flagged sites — same crate as the shared
                // helper, so the adoption is mechanical).
                let delay = jittered_backoff_delay(
                    attempt,
                    StdDuration::from_millis(INITIAL_BACKOFF_MS),
                    StdDuration::from_millis(MAX_BACKOFF_MS),
                );
                warn!(
                    attempt = attempt + 1,
                    max = MAX_RETRIES,
                    backoff_ms = delay.as_millis() as u64,
                    "token refresh failed, retrying"
                );
                tokio::time::sleep(delay).await;
            }
        }

        Err(last_err)
    }
}
