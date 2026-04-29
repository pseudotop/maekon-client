use chrono::{DateTime, Duration, Utc};
use oneshim_core::config::TlsConfig;
use oneshim_core::error::CoreError;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration as StdDuration;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use crate::error::NetworkError;
use crate::http_client::build_reqwest_client;

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
}

#[derive(Debug, Clone)]
struct TokenState {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct TokenManager {
    base_url: String,
    client: reqwest::Client,
    state: Arc<RwLock<Option<TokenState>>>,
}

/// `Retry-After` 헤더 파싱 — 초(integer) 형식만 지원하며 최대 60초로 제한한다.
fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<StdDuration> {
    let value = headers.get("retry-after")?.to_str().ok()?;
    if let Ok(secs) = value.parse::<u64>() {
        return Some(StdDuration::from_secs(secs.min(60)));
    }
    None
}

impl TokenManager {
    /// Legacy constructor — uses a default `reqwest::Client` with no TLS policy.
    ///
    /// Prefer [`TokenManager::new_with_tls`] in production code so that the
    /// same TLS settings (HTTPS-only, self-signed cert policy) are applied to
    /// credential requests as to all other network calls.
    ///
    /// This constructor is retained for backward compatibility and unit tests
    /// that talk to `mockito` HTTP servers.
    #[deprecated(note = "Use new_with_tls() for TLS enforcement")]
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
            state: Arc::new(RwLock::new(None)),
        }
    }

    /// Preferred production constructor — accepts a pre-built `reqwest::Client`
    /// so the caller can apply the canonical TLS policy via
    /// [`build_reqwest_client`].
    ///
    /// # Example
    /// ```no_run
    /// use oneshim_network::auth::TokenManager;
    /// use oneshim_network::http_client::build_reqwest_client;
    /// use oneshim_core::config::TlsConfig;
    ///
    /// let tls = TlsConfig::default();
    /// let client = build_reqwest_client(&tls, None).unwrap();
    /// let tm = TokenManager::new_with_client("https://api.example.com", client);
    /// ```
    pub fn new_with_client(base_url: &str, client: reqwest::Client) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
            state: Arc::new(RwLock::new(None)),
        }
    }

    /// Convenience constructor that builds a TLS-aware `reqwest::Client`
    /// internally using the canonical [`build_reqwest_client`] helper.
    ///
    /// `timeout` is applied as a per-request timeout.  Pass `None` to omit a
    /// global timeout (not recommended for auth endpoints — they should be
    /// short-lived).
    pub fn new_with_tls(
        base_url: &str,
        tls: &TlsConfig,
        timeout: Option<std::time::Duration>,
    ) -> Result<Self, NetworkError> {
        let client = build_reqwest_client(tls, timeout)?;
        Ok(Self::new_with_client(base_url, client))
    }

    /// # Arguments
    pub async fn login(&self, email: &str, password: &str) -> Result<(), CoreError> {
        let organization_id =
            std::env::var("ONESHIM_ORGANIZATION_ID").unwrap_or_else(|_| "default".to_string());
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
                code: oneshim_core::error_codes::AuthCode::Failed,
                message: format!("login request failure: {e}"),
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            let message = format!("login failure ({status}): {text}");
            // Semantic status mapping per iter-54..60. For login specifically,
            // 401/403 are definitive auth failures, but 429/503/504 indicate
            // transient auth-service issues that frontend should surface
            // differently (e.g., "try again shortly" vs "credentials wrong").
            return Err(match status.as_u16() {
                408 | 504 => CoreError::RequestTimeout {
                    code: oneshim_core::error_codes::NetworkCode::Timeout,
                    timeout_ms: 0,
                },
                429 => CoreError::RateLimit {
                    code: oneshim_core::error_codes::NetworkCode::RateLimit,
                    retry_after_secs: 60,
                },
                502 | 503 => CoreError::ServiceUnavailable {
                    code: oneshim_core::error_codes::ServiceCode::Unavailable,
                    message,
                },
                _ => CoreError::Auth {
                    code: oneshim_core::error_codes::AuthCode::Failed,
                    message,
                },
            });
        }

        let token_resp: TokenResponse = resp.json().await.map_err(|e| CoreError::Auth {
            code: oneshim_core::error_codes::AuthCode::Failed,
            message: format!("Token parsing failed: {e}"),
        })?;

        let expires_at = Utc::now() + Duration::seconds(token_resp.expires_in.unwrap_or(3600));

        let mut state = self.state.write().await;
        *state = Some(TokenState {
            access_token: token_resp.access_token,
            refresh_token: token_resp.refresh_token,
            expires_at,
        });

        debug!("login success, token: {expires_at}");
        Ok(())
    }

    pub async fn refresh(&self) -> Result<(), CoreError> {
        const MAX_RETRIES: u32 = 3;
        const INITIAL_BACKOFF_MS: u64 = 500;
        const MAX_BACKOFF_MS: u64 = 8_000;

        let current = {
            let state = self.state.read().await;
            state.clone()
        };

        let current = current.ok_or_else(|| CoreError::Auth {
            code: oneshim_core::error_codes::AuthCode::Failed,
            message: "Not authenticated".to_string(),
        })?;
        let refresh_token = current.refresh_token.ok_or_else(|| CoreError::Auth {
            code: oneshim_core::error_codes::AuthCode::Failed,
            message: "Refresh token is missing".to_string(),
        })?;

        let url = format!("{}/api/v1/auth/tokens/refresh", self.base_url);

        let mut last_err = CoreError::Auth {
            code: oneshim_core::error_codes::AuthCode::Failed,
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
                        let token_resp: TokenResponse =
                            resp.json().await.map_err(|e| CoreError::Auth {
                                code: oneshim_core::error_codes::AuthCode::Failed,
                                message: format!("refresh Token parsing failed: {e}"),
                            })?;

                        let expires_at =
                            Utc::now() + Duration::seconds(token_resp.expires_in.unwrap_or(3600));

                        let mut state = self.state.write().await;
                        *state = Some(TokenState {
                            access_token: token_resp.access_token,
                            refresh_token: token_resp.refresh_token.or(Some(refresh_token.clone())),
                            expires_at,
                        });

                        debug!("token refresh success, expires_at: {expires_at}");
                        return Ok(());
                    }

                    // 429 Too Many Requests — Retry-After 헤더 우선 적용
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
                    // with login() (lines 137-154). Previously all non-2xx
                    // statuses mapped to CoreError::Auth, conflating
                    // transient service issues (5xx, 429) with genuine
                    // auth failures (401/403). Aligning with the canonical
                    // pattern from docs/guides/http-status-error-mapping.md
                    // lets telemetry distinguish "auth provider is down"
                    // from "credentials rejected".
                    last_err = match status.as_u16() {
                        408 | 504 => CoreError::RequestTimeout {
                            code: oneshim_core::error_codes::NetworkCode::Timeout,
                            timeout_ms: 0,
                        },
                        429 => CoreError::RateLimit {
                            code: oneshim_core::error_codes::NetworkCode::RateLimit,
                            retry_after_secs: 60,
                        },
                        502 | 503 => CoreError::ServiceUnavailable {
                            code: oneshim_core::error_codes::ServiceCode::Unavailable,
                            message,
                        },
                        _ => CoreError::Auth {
                            code: oneshim_core::error_codes::AuthCode::Failed,
                            message,
                        },
                    };

                    if !is_retryable {
                        return Err(last_err);
                    }
                }
                Err(e) => {
                    // Iter-98: reqwest transport failure (pre-HTTP-status)
                    // — split timeout vs connection error per canonical
                    // pattern (same as cloud_stt.rs:107 / http_client.rs).
                    // Previously all transport errors were mis-labelled
                    // as `auth.failed`.
                    last_err = if e.is_timeout() {
                        CoreError::RequestTimeout {
                            code: oneshim_core::error_codes::NetworkCode::Timeout,
                            timeout_ms: 0,
                        }
                    } else {
                        CoreError::Network {
                            code: oneshim_core::error_codes::NetworkCode::Generic,
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
                tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
            }
        }

        Err(last_err)
    }

    pub async fn get_token(&self) -> Result<String, CoreError> {
        let needs_refresh = {
            let state = self.state.read().await;
            match &*state {
                Some(s) => Utc::now() + Duration::minutes(5) >= s.expires_at,
                None => {
                    return Err(CoreError::Auth {
                        code: oneshim_core::error_codes::AuthCode::Failed,
                        message: "Not authenticated".to_string(),
                    })
                }
            }
        };

        if needs_refresh {
            self.refresh().await.map_err(|e| {
                warn!("token refresh failure: {e}");
                CoreError::Auth {
                    code: oneshim_core::error_codes::AuthCode::Failed,
                    message: format!("Automatic token refresh failed: {e}"),
                }
            })?;
        }

        let state = self.state.read().await;
        state
            .as_ref()
            .map(|s| s.access_token.clone())
            .ok_or_else(|| CoreError::Auth {
                code: oneshim_core::error_codes::AuthCode::Failed,
                message: "Not authenticated".to_string(),
            })
    }

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
                code: oneshim_core::error_codes::AuthCode::Failed,
                message: format!("token validation request failure: {e}"),
            })?;

        Ok(resp.status().is_success())
    }

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

    pub async fn is_authenticated(&self) -> bool {
        let state = self.state.read().await;
        state.as_ref().is_some_and(|s| Utc::now() < s.expires_at)
    }
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;

    #[test]
    fn parse_retry_after_valid_seconds() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "30".parse().unwrap());
        let duration = parse_retry_after(&headers);
        assert_eq!(duration, Some(StdDuration::from_secs(30)));
    }

    #[test]
    fn parse_retry_after_caps_at_60() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "120".parse().unwrap());
        let duration = parse_retry_after(&headers);
        assert_eq!(duration, Some(StdDuration::from_secs(60)));
    }

    #[test]
    fn parse_retry_after_missing_header() {
        let headers = reqwest::header::HeaderMap::new();
        assert!(parse_retry_after(&headers).is_none());
    }

    #[test]
    fn parse_retry_after_non_integer_ignored() {
        let mut headers = reqwest::header::HeaderMap::new();
        // HTTP-date format is not supported — returns None
        headers.insert(
            "retry-after",
            "Wed, 21 Oct 2015 07:28:00 GMT".parse().unwrap(),
        );
        assert!(parse_retry_after(&headers).is_none());
    }

    #[test]
    fn token_manager_creation() {
        let tm = TokenManager::new("http://localhost:8000");
        assert_eq!(tm.base_url, "http://localhost:8000");
    }

    #[test]
    fn token_manager_trailing_slash() {
        let tm = TokenManager::new("http://localhost:8000/");
        assert_eq!(tm.base_url, "http://localhost:8000");
    }

    #[test]
    fn new_with_client_strips_trailing_slash() {
        let client = reqwest::Client::new();
        let tm = TokenManager::new_with_client("http://localhost:8000/", client);
        assert_eq!(tm.base_url, "http://localhost:8000");
    }

    #[test]
    fn new_with_tls_tls_disabled_succeeds() {
        let tls = TlsConfig {
            enabled: false,
            allow_self_signed: false,
        };
        let tm = TokenManager::new_with_tls(
            "http://localhost:8000",
            &tls,
            Some(std::time::Duration::from_secs(5)),
        );
        assert!(tm.is_ok());
        assert_eq!(tm.unwrap().base_url, "http://localhost:8000");
    }

    #[test]
    fn new_with_tls_tls_enabled_succeeds() {
        let tls = TlsConfig::default();
        let tm = TokenManager::new_with_tls(
            "https://api.example.com",
            &tls,
            Some(std::time::Duration::from_secs(5)),
        );
        assert!(tm.is_ok());
    }

    #[tokio::test]
    async fn unauthenticated_get_token_fails() {
        let tm = TokenManager::new("http://localhost:8000");
        assert!(tm.get_token().await.is_err());
    }

    #[tokio::test]
    async fn unauthenticated_state() {
        let tm = TokenManager::new("http://localhost:8000");
        assert!(!tm.is_authenticated().await);
    }

    #[tokio::test]
    async fn login_success() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/v1/auth/tokens")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"jwt_abc","refresh_token":"ref_xyz","expires_in":3600}"#)
            .create_async()
            .await;

        let tm = TokenManager::new(&server.url());
        let result = tm.login("user@test.com", "test-password-placeholder").await;
        assert!(result.is_ok());
        assert!(tm.is_authenticated().await);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn login_failure_401() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/v1/auth/tokens")
            .with_status(401)
            .with_body("Unauthorized")
            .create_async()
            .await;

        let tm = TokenManager::new(&server.url());
        let result = tm.login("bad@test.com", "wrong").await;
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("login failure"));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn login_failure_network() {
        let tm = TokenManager::new("http://127.0.0.1:1");
        let result = tm.login("user@test.com", "pass").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn refresh_without_auth_fails() {
        let tm = TokenManager::new("http://localhost:9999");
        let result = tm.refresh().await;
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("Not authenticated"));
    }

    #[tokio::test]
    async fn refresh_success() {
        let mut server = mockito::Server::new_async().await;

        let login_mock = server
            .mock("POST", "/api/v1/auth/tokens")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"old_jwt","refresh_token":"ref_tok","expires_in":3600}"#)
            .create_async()
            .await;

        let refresh_mock = server
            .mock("POST", "/api/v1/auth/tokens/refresh")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"new_jwt","refresh_token":"new_ref","expires_in":7200}"#)
            .create_async()
            .await;

        let tm = TokenManager::new(&server.url());
        tm.login("user@test.com", "pass").await.unwrap();
        login_mock.assert_async().await;

        let result = tm.refresh().await;
        assert!(result.is_ok());
        refresh_mock.assert_async().await;
    }

    #[tokio::test]
    async fn get_token_propagates_refresh_failure() {
        let mut server = mockito::Server::new_async().await;

        let login_mock = server
            .mock("POST", "/api/v1/auth/tokens")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"old_jwt","refresh_token":"ref_tok","expires_in":1}"#)
            .create_async()
            .await;

        let refresh_mock = server
            .mock("POST", "/api/v1/auth/tokens/refresh")
            .with_status(401)
            .with_body("Unauthorized")
            .create_async()
            .await;

        let tm = TokenManager::new(&server.url());
        tm.login("user@test.com", "pass").await.unwrap();
        login_mock.assert_async().await;

        let result = tm.get_token().await;
        assert!(result.is_err());
        assert!(format!("{}", result.unwrap_err()).contains("Automatic token refresh failed"));
        refresh_mock.assert_async().await;
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_retries_on_transient_failure() {
        let mut server = mockito::Server::new_async().await;

        // mockito prioritises mocks with missing hits (below their expect count),
        // then falls back to the last matching mock. Register the failure mock
        // first with expect(2) so it absorbs the first two requests, then the
        // success mock (registered second = last) handles the third.
        let failure_mock = server
            .mock("POST", "/api/v1/auth/tokens/refresh")
            .with_status(503)
            .with_body("Service Unavailable")
            .expect(2)
            .create_async()
            .await;

        let success_mock = server
            .mock("POST", "/api/v1/auth/tokens/refresh")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"new_jwt","refresh_token":"new_ref","expires_in":7200}"#)
            .expect(1)
            .create_async()
            .await;

        let tm = TokenManager::new(&server.url());
        // Seed state directly so we don't need a login mock
        {
            let mut state = tm.state.write().await;
            *state = Some(TokenState {
                access_token: "old_jwt".to_string(),
                refresh_token: Some("ref_tok".to_string()),
                expires_at: Utc::now() + Duration::hours(1),
            });
        }

        let result = tm.refresh().await;
        assert!(result.is_ok());
        failure_mock.assert_async().await;
        success_mock.assert_async().await;
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_gives_up_after_max_retries() {
        let mut server = mockito::Server::new_async().await;

        // Always return 503 — should exhaust all 4 attempts (initial + 3 retries)
        let mock = server
            .mock("POST", "/api/v1/auth/tokens/refresh")
            .with_status(503)
            .with_body("Service Unavailable")
            .expect(4)
            .create_async()
            .await;

        let tm = TokenManager::new(&server.url());
        {
            let mut state = tm.state.write().await;
            *state = Some(TokenState {
                access_token: "old_jwt".to_string(),
                refresh_token: Some("ref_tok".to_string()),
                expires_at: Utc::now() + Duration::hours(1),
            });
        }

        let result = tm.refresh().await;
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("token refresh failure"));
        mock.assert_async().await;
    }

    // iter-70 regression guards for iter-61a semantic HTTP status mapping
    // in auth.rs::login. Shared helper pattern matches iter-67..69.
    async fn run_login_status_test(status: u16) -> oneshim_core::error::CoreError {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/api/v1/auth/tokens")
            .with_status(status as usize)
            .with_body(format!("http {status}"))
            .create_async()
            .await;
        let tm = TokenManager::new(&server.url());
        tm.login("u@test.com", "p").await.unwrap_err()
    }

    #[tokio::test]
    async fn login_429_maps_to_rate_limit() {
        let err = run_login_status_test(429).await;
        assert!(
            matches!(err, oneshim_core::error::CoreError::RateLimit { .. }),
            "429 → RateLimit, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn login_503_maps_to_service_unavailable() {
        let err = run_login_status_test(503).await;
        assert!(
            matches!(
                err,
                oneshim_core::error::CoreError::ServiceUnavailable { .. }
            ),
            "503 → ServiceUnavailable, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn login_504_maps_to_timeout() {
        let err = run_login_status_test(504).await;
        assert!(
            matches!(err, oneshim_core::error::CoreError::RequestTimeout { .. }),
            "504 → RequestTimeout, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn login_401_stays_as_auth() {
        // Sanity check: 401 (the "normal" login failure) still maps to
        // CoreError::Auth so iter-61a didn't regress the common case.
        let err = run_login_status_test(401).await;
        assert!(
            matches!(err, oneshim_core::error::CoreError::Auth { .. }),
            "401 → Auth, got: {err:?}"
        );
    }

    /// iter-79: domain-fallback guard for login. 500 (not in specific arms)
    /// falls back to CoreError::Auth/Failed — the login-endpoint-appropriate
    /// wildcard. Catches regressions that broaden the transient-class arms
    /// (429/502/503/504) into the 5xx space.
    #[tokio::test]
    async fn login_500_falls_back_to_auth() {
        let err = run_login_status_test(500).await;
        assert!(
            matches!(err, oneshim_core::error::CoreError::Auth { .. }),
            "500 should fall back to Auth (domain-appropriate for login endpoint), got: {err:?}"
        );
    }

    // iter-98 regression guards: refresh() must apply canonical HTTP status
    // mapping, not blanket-label every error as CoreError::Auth. The 503
    // path in particular exercised the retry loop — prior to iter-98 all
    // 4 retry attempts emitted CoreError::Auth, masking the transient
    // service-unavailability signal.
    async fn run_refresh_status_test(status: u16) -> oneshim_core::error::CoreError {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/api/v1/auth/tokens/refresh")
            .with_status(status as usize)
            .with_body(format!("http {status}"))
            .expect_at_least(1)
            .create_async()
            .await;
        let tm = TokenManager::new(&server.url());
        {
            let mut state = tm.state.write().await;
            *state = Some(TokenState {
                access_token: "old_jwt".to_string(),
                refresh_token: Some("ref_tok".to_string()),
                expires_at: Utc::now() + Duration::hours(1),
            });
        }
        tm.refresh().await.unwrap_err()
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_503_maps_to_service_unavailable() {
        let err = run_refresh_status_test(503).await;
        assert_eq!(err.code(), "service.unavailable");
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_429_maps_to_rate_limit() {
        let err = run_refresh_status_test(429).await;
        assert_eq!(err.code(), "network.rate_limit");
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_504_maps_to_timeout() {
        let err = run_refresh_status_test(504).await;
        assert_eq!(err.code(), "network.timeout");
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_401_maps_to_auth() {
        let err = run_refresh_status_test(401).await;
        assert_eq!(err.code(), "auth.failed");
    }

    /// Domain-fallback: 500 falls back to Auth (refresh is auth-domain).
    #[tokio::test(start_paused = true)]
    async fn refresh_500_falls_back_to_auth() {
        let err = run_refresh_status_test(500).await;
        assert_eq!(err.code(), "auth.failed");
    }
}
