use async_trait::async_trait;
use maekon_core::config::TlsConfig;
use maekon_core::error::CoreError;
use maekon_core::models::event::EventBatch;
use maekon_core::models::suggestion::SuggestionFeedback;
use maekon_core::ports::api_client::{ApiClient, SessionCreateResponse};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};

use crate::auth::TokenManager;
use crate::error::NetworkError;
use crate::resilience::{extract_retry_after, jittered_backoff_delay};

const DEFAULT_MAX_RETRIES: u32 = 3;

fn is_retryable(error: &NetworkError) -> bool {
    matches!(
        error,
        NetworkError::Http(_)
            | NetworkError::Timeout { .. }
            | NetworkError::ServiceUnavailable(_)
            | NetworkError::RateLimited { .. }
    )
}

fn map_reqwest_error(e: reqwest::Error, context: &str, timeout_ms: u64) -> NetworkError {
    if e.is_timeout() {
        NetworkError::Timeout { timeout_ms }
    } else {
        NetworkError::Http(format!("{context}: {e}"))
    }
}

pub struct HttpApiClient {
    client: reqwest::Client,
    base_url: String,
    token_manager: Arc<TokenManager>,
    max_retries: u32,
    timeout_ms: u64,
}

/// TLS 설정을 적용하여 reqwest 클라이언트를 생성하는 헬퍼 함수
///
/// `tls.enabled=true` 이면 HTTPS 전용 모드(`https_only`)를 강제한다.
/// `tls.allow_self_signed=true` 는 더 이상 인증서 검증 우회로 처리하지 않는다.
/// `timeout=None` 이면 전역 타임아웃 미적용 — SSE 등 장기 스트림 연결에 사용.
/// Check if a URL refers to a loopback / localhost address.
fn is_localhost(url: &str) -> bool {
    // Strip scheme if present to get the host portion
    let host_part = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);

    // Handle IPv6 bracket notation: [::1]:port
    let host = if host_part.starts_with('[') {
        host_part
            .find(']')
            .map(|end| &host_part[..=end])
            .unwrap_or(host_part)
    } else {
        host_part.split(':').next().unwrap_or(host_part)
    };
    host.eq_ignore_ascii_case("localhost")
        || host == "127.0.0.1"
        || host == "[::1]"
        || host == "::1"
}

/// Strict loopback check for the ADR-023 MG-PII-02 enrichment egress gate.
///
/// Parses the URL and accepts ONLY `localhost` or an IP that is loopback (the
/// full `127.0.0.0/8` range + `::1`, via `IpAddr::is_loopback`). Stronger than
/// [`is_localhost`]'s literal set. **Fail-closed**: an unparseable URL, a missing
/// host, or a non-loopback host returns `false`. Note: the IPv4-mapped IPv6
/// loopback (`::ffff:127.0.0.1`) is NOT loopback per `IpAddr::is_loopback` and is
/// therefore refused (safe default).
pub fn host_is_loopback(url: &str) -> bool {
    match reqwest::Url::parse(url) {
        Ok(parsed) => match parsed.host_str() {
            Some(host) => {
                // `host_str()` serializes IPv6 WITH brackets (e.g. "[::1]"); strip
                // them so the address parses.
                let host = host
                    .strip_prefix('[')
                    .and_then(|h| h.strip_suffix(']'))
                    .unwrap_or(host);
                host.eq_ignore_ascii_case("localhost")
                    || host
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|addr| addr.is_loopback())
            }
            None => false,
        },
        Err(_) => false,
    }
}

pub fn build_reqwest_client(
    tls: &TlsConfig,
    timeout: Option<Duration>,
) -> Result<reqwest::Client, NetworkError> {
    build_reqwest_client_for_url(tls, timeout, None)
}

/// Build a `reqwest::Client` with TLS policy.
pub fn build_reqwest_client_for_url(
    tls: &TlsConfig,
    timeout: Option<Duration>,
    base_url: Option<&str>,
) -> Result<reqwest::Client, NetworkError> {
    let mut builder = reqwest::Client::builder().use_native_tls();
    if let Some(t) = timeout {
        builder = builder.timeout(t);
    }

    if tls.enabled {
        // 운영 환경: HTTPS 전용 강제
        builder = builder.https_only(true);
    }

    if tls.allow_self_signed {
        let target = base_url.unwrap_or("unknown URL");
        let target_is_localhost = base_url.is_some_and(is_localhost);
        warn!(
            target,
            target_is_localhost,
            "TLS: allow_self_signed=true is configured but certificate validation bypass is disabled"
        );
        return Err(NetworkError::Config(format!(
            "allow_self_signed is no longer supported for {target}; install a trusted development CA or disable TLS in local-only test configuration"
        )));
    }

    builder
        .build()
        .map_err(|e| NetworkError::Http(format!("Failed to build HTTP client: {}", e)))
}

impl HttpApiClient {
    /// 기존 생성자 — TLS 미적용 (역호환성 보장, 테스트 전용)
    #[deprecated(note = "Use new_with_tls() for TLS enforcement")]
    pub fn new(
        base_url: &str,
        token_manager: Arc<TokenManager>,
        timeout: Duration,
    ) -> Result<Self, NetworkError> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| NetworkError::Http(format!("Failed to build HTTP client: {}", e)))?;

        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            token_manager,
            max_retries: DEFAULT_MAX_RETRIES,
            timeout_ms: timeout.as_millis() as u64,
        })
    }

    /// TLS 설정 적용 생성자 — 운영 환경 표준 진입점
    ///
    /// `tls.enabled=true` 이면 HTTPS 전용을 강제한다.
    pub fn new_with_tls(
        base_url: &str,
        token_manager: Arc<TokenManager>,
        timeout: Duration,
        tls: &TlsConfig,
    ) -> Result<Self, NetworkError> {
        let client = build_reqwest_client_for_url(tls, Some(timeout), Some(base_url))?;
        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            token_manager,
            max_retries: DEFAULT_MAX_RETRIES,
            timeout_ms: timeout.as_millis() as u64,
        })
    }

    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    async fn authorized_request(
        &self,
        method: reqwest::Method,
        path: &str,
    ) -> Result<reqwest::RequestBuilder, NetworkError> {
        let token = self
            .token_manager
            .get_token()
            .await
            .map_err(NetworkError::Core)?;
        let url = format!("{}{}", self.base_url, path);
        Ok(self.client.request(method, &url).bearer_auth(token))
    }

    async fn check_response(
        &self,
        resp: reqwest::Response,
    ) -> Result<reqwest::Response, NetworkError> {
        let status = resp.status();

        if status.is_success() {
            return Ok(resp);
        }

        let status_code = status.as_u16();
        let retry_after = extract_retry_after(&resp);
        let text = resp.text().await.unwrap_or_else(|e| {
            tracing::warn!("response read failure: {e}");
            String::new()
        });

        match status_code {
            401 | 403 => Err(NetworkError::Auth(format!("Authentication failed: {text}"))),
            404 => Err(NetworkError::NotFound {
                resource_type: "API".to_string(),
                id: text,
            }),
            // 408 Request Timeout and 504 Gateway Timeout are both timeout-class
            // failures — map to NetworkError::Timeout so is_retryable returns true
            // and telemetry emits `network.timeout`. (iter-54)
            408 | 504 => Err(NetworkError::Timeout {
                timeout_ms: 0, // sentinel: server returned timeout, we don't know the server's timeout budget
            }),
            429 => Err(NetworkError::RateLimited {
                retry_after_secs: retry_after,
            }),
            // All 5xx are transient server-side faults — retryable. 504 is
            // consumed by the timeout arm above; this catches 500/501/502/503/
            // 505 and proxy-specific codes (520-526). Previously only 502/503
            // were mapped, so a bare 500 fell through to `Internal` and was NOT
            // retried — a contract §5 gap for at-least-once callers like the
            // #5069 feature-perf emitter (which then dropped the batch instead
            // of backing off). (iter-54 / #5069)
            500..=599 => Err(NetworkError::ServiceUnavailable(text)),
            _ => Err(NetworkError::Internal(format!(
                "API error ({status}): {text}"
            ))),
        }
    }

    async fn execute_with_retry<F, Fut, T>(&self, operation: F) -> Result<T, NetworkError>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<T, NetworkError>>,
    {
        let mut last_error = NetworkError::Internal("request failure".to_string());
        for attempt in 0..=self.max_retries {
            match operation().await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    if !is_retryable(&e) || attempt == self.max_retries {
                        return Err(e);
                    }

                    let delay = match &e {
                        NetworkError::RateLimited { retry_after_secs } => {
                            Duration::from_secs(*retry_after_secs)
                        }
                        _ => jittered_backoff_delay(
                            attempt,
                            Duration::from_secs(1),
                            Duration::from_secs(30),
                        ),
                    };

                    warn!(
                        "request failed (attempt {}/{}): {e}, retrying in {delay:?}",
                        attempt + 1,
                        self.max_retries + 1
                    );

                    last_error = e;
                    tokio::time::sleep(delay).await;
                }
            }
        }

        Err(last_error)
    }
}

#[async_trait]
impl ApiClient for HttpApiClient {
    async fn create_session(&self, client_id: &str) -> Result<SessionCreateResponse, CoreError> {
        debug!("session create request: client_id={client_id}");

        self.execute_with_retry(|| async {
            let req = self
                .authorized_request(reqwest::Method::POST, "/user_context/sessions/")
                .await?;

            let body = serde_json::json!({ "client_id": client_id });
            let resp = req.json(&body).send().await.map_err(|e| {
                map_reqwest_error(e, "session create request failure", self.timeout_ms)
            })?;

            let resp = self.check_response(resp).await?;
            let session: SessionCreateResponse = resp.json().await.map_err(|e| {
                NetworkError::Internal(format!("Failed to parse session response: {e}"))
            })?;

            debug!("session create success: session_id={}", session.session_id);
            Ok(session)
        })
        .await
        .map_err(CoreError::from)
    }

    async fn end_session(&self, session_id: &str) -> Result<(), CoreError> {
        debug!("session ended request: session_id={session_id}");

        self.execute_with_retry(|| async {
            let path = format!("/user_context/sessions/{session_id}");
            let req = self
                .authorized_request(reqwest::Method::DELETE, &path)
                .await?;

            let resp = req.send().await.map_err(|e| {
                map_reqwest_error(e, "session ended request failure", self.timeout_ms)
            })?;

            self.check_response(resp).await?;
            debug!("session ended success");
            Ok(())
        })
        .await
        .map_err(CoreError::from)
    }

    async fn upload_batch(&self, batch: &EventBatch) -> Result<(), CoreError> {
        debug!("batch upload: {} event", batch.events.len());

        self.execute_with_retry(|| async {
            let req = self
                .authorized_request(reqwest::Method::POST, "/user_context/batches")
                .await?;

            let resp = req.json(batch).send().await.map_err(|e| {
                map_reqwest_error(e, "batch upload request failure", self.timeout_ms)
            })?;

            self.check_response(resp).await?;
            debug!("batch upload success");
            Ok(())
        })
        .await
        .map_err(CoreError::from)
    }

    async fn send_feedback(&self, feedback: &SuggestionFeedback) -> Result<(), CoreError> {
        debug!(
            "feedback sent: {} → {:?}",
            feedback.suggestion_id, feedback.feedback_type
        );

        self.execute_with_retry(|| async {
            let req = self
                .authorized_request(reqwest::Method::POST, "/user_context/suggestions/feedback")
                .await?;

            let resp = req
                .json(feedback)
                .send()
                .await
                .map_err(|e| map_reqwest_error(e, "feedback sent failure", self.timeout_ms))?;

            self.check_response(resp).await?;
            Ok(())
        })
        .await
        .map_err(CoreError::from)
    }

    async fn send_heartbeat(&self, session_id: &str) -> Result<(), CoreError> {
        debug!("heartbeat sent: {session_id}");

        self.execute_with_retry(|| async {
            let path = format!("/user_context/sessions/{}/heartbeat", session_id);
            let req = self
                .authorized_request(reqwest::Method::POST, &path)
                .await?;

            let resp = req
                .send()
                .await
                .map_err(|e| map_reqwest_error(e, "heartbeat sent failure", self.timeout_ms))?;

            self.check_response(resp).await?;
            Ok(())
        })
        .await
        .map_err(CoreError::from)
    }
}

#[async_trait]
impl maekon_core::ports::feature_perf::FeaturePerfSink for HttpApiClient {
    /// `POST /api/v1/system/features/{feature_key}/performance` with body
    /// `{"samples": [...]}` (#5069). Bearer JWT via `authorized_request`; the org
    /// is derived by the server from the token (never sent in the body).
    /// `execute_with_retry` provides the contract §5 5xx backoff; permanent 4xx
    /// (404/403/422) are non-retryable and returned for the caller to drop.
    async fn upload_feature_performance(
        &self,
        feature_key: &str,
        samples: &[maekon_core::models::feature_performance::FeaturePerfSample],
    ) -> Result<(), CoreError> {
        // `feature_key` comes from the closed canonical registry (kebab-case, no
        // path/query chars), so direct interpolation is URL-safe.
        let path = format!("/api/v1/system/features/{feature_key}/performance");
        let body = crate::feature_perf_uploader::PerfUploadBody { samples };

        self.execute_with_retry(|| async {
            let req = self
                .authorized_request(reqwest::Method::POST, &path)
                .await?;

            let resp = req.json(&body).send().await.map_err(|e| {
                map_reqwest_error(e, "feature performance upload failure", self.timeout_ms)
            })?;

            self.check_response(resp).await?;
            Ok(())
        })
        .await
        .map_err(CoreError::from)
    }
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;
    use crate::error::NetworkError;

    fn primary_password() -> String {
        String::from_utf8(vec![b'x'; 16]).expect("password fixture bytes must be UTF-8")
    }

    #[test]
    fn is_localhost_detects_loopback_variants() {
        assert!(is_localhost("http://localhost:8000"));
        assert!(is_localhost("https://localhost"));
        assert!(is_localhost("http://127.0.0.1:9090"));
        assert!(is_localhost("127.0.0.1"));
        assert!(is_localhost("http://[::1]:50051"));
        assert!(!is_localhost("http://api.example.com"));
        assert!(!is_localhost("https://10.0.0.1:443"));
    }

    #[test]
    fn host_is_loopback_strict_table() {
        // Accepted: literal localhost + any loopback IP (full 127.0.0.0/8 + ::1).
        assert!(host_is_loopback("http://localhost:11434"));
        assert!(host_is_loopback("http://127.0.0.1:8000"));
        assert!(host_is_loopback("http://127.0.0.5:1"));
        assert!(host_is_loopback("http://[::1]:50051"));
        // Refused (fail-closed):
        assert!(!host_is_loopback("https://api.example.com"));
        assert!(!host_is_loopback("http://localhost.evil.com")); // not literal localhost
        assert!(!host_is_loopback("http://10.0.0.5:11434")); // private but remote
        assert!(!host_is_loopback("http://[::ffff:127.0.0.1]:1")); // mapped-loopback refused
        assert!(!host_is_loopback("not a url")); // unparseable
        assert!(!host_is_loopback("http://")); // no host
    }

    #[test]
    fn build_reqwest_client_tls_disabled_succeeds() {
        // TLS 비활성화 시 http:// 요청 허용 — 개발/테스트 환경
        let tls = TlsConfig {
            enabled: false,
            allow_self_signed: false,
        };
        let client = build_reqwest_client(&tls, Some(Duration::from_secs(5)))
            .expect("TLS disabled + http:// allowed: client construction must succeed");
        // Verify the client is usable by confirming it has a default User-Agent header
        // (reqwest::Client::builder().build() always produces a valid client).
        // The meaningful contract here is successful construction — no allow_self_signed rejection.
        // Justified: reqwest::Client carries no observable fields; construction success is the contract.
        // #5594: ok-only justified — Client has no public accessors to probe.
        let _ = client;
    }

    #[test]
    fn build_reqwest_client_tls_enabled_succeeds() {
        // TLS 활성화 시 클라이언트 생성 자체는 성공 (요청 시점에 https 강제)
        let tls = TlsConfig::default();
        let client = build_reqwest_client(&tls, Some(Duration::from_secs(5)))
            .expect("TLS enabled: client construction succeeds (https_only enforcement deferred to request time)");
        // Construction succeeds; https_only is enforced only at request dispatch, not at build time.
        // reqwest::Client has no public field accessors — construction success is the full contract.
        // #5594: ok-only justified — Client internals are opaque; only Err path is observable.
        let _ = client;
    }

    #[test]
    fn build_reqwest_client_rejects_allow_self_signed() {
        let tls = TlsConfig {
            enabled: true,
            allow_self_signed: true,
        };
        let result = build_reqwest_client_for_url(
            &tls,
            Some(Duration::from_secs(5)),
            Some("https://localhost"),
        );
        assert!(matches!(result, Err(NetworkError::Config(_))));
    }

    #[test]
    fn new_with_tls_returns_client() {
        let tls = TlsConfig {
            enabled: false, // 테스트: http:// URL 허용
            allow_self_signed: false,
        };
        let tm = Arc::new(TokenManager::new("http://localhost:8000"));
        let client =
            HttpApiClient::new_with_tls("http://localhost:8000", tm, Duration::from_secs(5), &tls)
                .expect("TLS disabled + http:// URL must construct without error");
        assert_eq!(client.base_url, "http://localhost:8000");
        assert_eq!(client.max_retries, DEFAULT_MAX_RETRIES);
    }

    #[test]
    fn http_client_creation() {
        let tm = Arc::new(TokenManager::new("http://localhost:8000"));
        let client =
            HttpApiClient::new("http://localhost:8000", tm, Duration::from_secs(30)).unwrap();
        assert_eq!(client.base_url, "http://localhost:8000");
        assert_eq!(client.max_retries, DEFAULT_MAX_RETRIES);
    }

    #[test]
    fn with_max_retries() {
        let tm = Arc::new(TokenManager::new("http://localhost:8000"));
        let client = HttpApiClient::new("http://localhost:8000", tm, Duration::from_secs(30))
            .unwrap()
            .with_max_retries(5);
        assert_eq!(client.max_retries, 5);
    }

    #[test]
    fn is_retryable_errors() {
        assert!(is_retryable(&NetworkError::Http("test".to_string())));
        assert!(is_retryable(&NetworkError::ServiceUnavailable(
            "test".to_string()
        )));
        assert!(is_retryable(&NetworkError::RateLimited {
            retry_after_secs: 60
        }));
        assert!(!is_retryable(&NetworkError::Auth("test".to_string())));
        assert!(!is_retryable(&NetworkError::Internal("test".to_string())));
    }

    async fn setup_authed_client(
        server: &mut mockito::ServerGuard,
    ) -> (HttpApiClient, mockito::Mock) {
        let login_mock = server
            .mock("POST", "/api/v1/auth/tokens")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"test_jwt","refresh_token":"ref","expires_in":3600}"#)
            .create_async()
            .await;

        let tm = Arc::new(TokenManager::new(&server.url()));
        let password = primary_password();
        tm.login("test@test.com", &password).await.unwrap();

        let client = HttpApiClient::new(&server.url(), tm, Duration::from_secs(5)).unwrap();
        (client, login_mock)
    }

    #[tokio::test]
    async fn create_session_success() {
        let mut server = mockito::Server::new_async().await;
        let (client, _login_mock) = setup_authed_client(&mut server).await;

        let mock = server
            .mock("POST", "/user_context/sessions/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"session_id":"sess_123","user_id":"user_1","client_id":"cli_1","capabilities":["streaming"]}"#)
            .create_async()
            .await;

        let session = client
            .create_session("cli_1")
            .await
            .expect("create_session must succeed on 200 with valid JSON");
        assert_eq!(session.session_id, "sess_123");
        assert_eq!(session.user_id, "user_1");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn end_session_success() {
        let mut server = mockito::Server::new_async().await;
        let (client, _login_mock) = setup_authed_client(&mut server).await;

        let mock = server
            .mock("DELETE", "/user_context/sessions/sess_123")
            .with_status(200)
            .create_async()
            .await;

        // end_session returns () on success; the only contract to pin is that
        // the DELETE request was accepted (200) and returned Ok(()).
        client
            .end_session("sess_123")
            .await
            .expect("end_session must succeed on 200 response");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn upload_batch_401() {
        let mut server = mockito::Server::new_async().await;
        let (client, _login_mock) = setup_authed_client(&mut server).await;

        let mock = server
            .mock("POST", "/user_context/batches")
            .with_status(401)
            .with_body("Unauthorized")
            .create_async()
            .await;

        let batch = maekon_core::models::event::EventBatch {
            session_id: "sess_1".to_string(),
            events: vec![],
            created_at: chrono::Utc::now(),
        };

        let result = client.upload_batch(&batch).await;
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("Authentication"));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn rate_limit_429() {
        let mut server = mockito::Server::new_async().await;
        let (client, _login_mock) = setup_authed_client(&mut server).await;
        let client = client.with_max_retries(0); // attempt failure
        let mock = server
            .mock("POST", "/user_context/batches")
            .with_status(429)
            .with_body("Too Many Requests")
            .create_async()
            .await;

        let batch = maekon_core::models::event::EventBatch {
            session_id: "sess_1".to_string(),
            events: vec![],
            created_at: chrono::Utc::now(),
        };

        let err = client.upload_batch(&batch).await.unwrap_err();
        assert!(matches!(
            err,
            CoreError::RateLimit {
                code: maekon_core::error_codes::NetworkCode::RateLimit,
                ..
            }
        ));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn service_unavailable_503() {
        let mut server = mockito::Server::new_async().await;
        let (client, _login_mock) = setup_authed_client(&mut server).await;
        let client = client.with_max_retries(0);

        let mock = server
            .mock("POST", "/user_context/sessions/sess_1/heartbeat")
            .with_status(503)
            .with_body("Service Unavailable")
            .create_async()
            .await;

        let err = client.send_heartbeat("sess_1").await.unwrap_err();
        assert!(
            matches!(err, CoreError::ServiceUnavailable { .. }),
            "err: {err:?}"
        );
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn heartbeat_success() {
        let mut server = mockito::Server::new_async().await;
        let (client, _login_mock) = setup_authed_client(&mut server).await;

        let mock = server
            .mock("POST", "/user_context/sessions/sess_test/heartbeat")
            .with_status(200)
            .create_async()
            .await;

        // send_heartbeat returns () on 200; pin that the call succeeds and the
        // mock endpoint was actually reached.
        client
            .send_heartbeat("sess_test")
            .await
            .expect("send_heartbeat must succeed on 200 response");
        mock.assert_async().await;
    }

    /// iter-54 regression guards: HTTP status codes added to check_response
    /// with semantic mappings. Each asserts the error variant matches the
    /// contract documented in the match block.
    #[tokio::test]
    async fn forbidden_403_maps_to_auth() {
        let mut server = mockito::Server::new_async().await;
        let (client, _login_mock) = setup_authed_client(&mut server).await;
        let client = client.with_max_retries(0);

        let _mock = server
            .mock("POST", "/user_context/sessions/sess_test/heartbeat")
            .with_status(403)
            .with_body("Forbidden")
            .create_async()
            .await;

        let err = client.send_heartbeat("sess_test").await.unwrap_err();
        assert!(
            matches!(err, CoreError::Auth { .. }),
            "403 must map to CoreError::Auth, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn request_timeout_408_maps_to_timeout() {
        let mut server = mockito::Server::new_async().await;
        let (client, _login_mock) = setup_authed_client(&mut server).await;
        let client = client.with_max_retries(0);

        let _mock = server
            .mock("POST", "/user_context/sessions/sess_test/heartbeat")
            .with_status(408)
            .with_body("Request Timeout")
            .create_async()
            .await;

        let err = client.send_heartbeat("sess_test").await.unwrap_err();
        assert!(
            matches!(err, CoreError::RequestTimeout { .. }),
            "408 must map to CoreError::RequestTimeout, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn bad_gateway_502_maps_to_service_unavailable() {
        let mut server = mockito::Server::new_async().await;
        let (client, _login_mock) = setup_authed_client(&mut server).await;
        let client = client.with_max_retries(0);

        let _mock = server
            .mock("POST", "/user_context/sessions/sess_test/heartbeat")
            .with_status(502)
            .with_body("Bad Gateway")
            .create_async()
            .await;

        let err = client.send_heartbeat("sess_test").await.unwrap_err();
        assert!(
            matches!(err, CoreError::ServiceUnavailable { .. }),
            "502 must map to CoreError::ServiceUnavailable, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn gateway_timeout_504_maps_to_timeout() {
        let mut server = mockito::Server::new_async().await;
        let (client, _login_mock) = setup_authed_client(&mut server).await;
        let client = client.with_max_retries(0);

        let _mock = server
            .mock("POST", "/user_context/sessions/sess_test/heartbeat")
            .with_status(504)
            .with_body("Gateway Timeout")
            .create_async()
            .await;

        let err = client.send_heartbeat("sess_test").await.unwrap_err();
        assert!(
            matches!(err, CoreError::RequestTimeout { .. }),
            "504 must map to CoreError::RequestTimeout, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn internal_server_error_500_maps_to_service_unavailable() {
        // #5069 review (devils F5): a bare 500 must be transient/retryable, not
        // dropped as Internal — else at-least-once callers lose the batch.
        let mut server = mockito::Server::new_async().await;
        let (client, _login_mock) = setup_authed_client(&mut server).await;
        let client = client.with_max_retries(0);

        let _mock = server
            .mock("POST", "/user_context/sessions/sess_test/heartbeat")
            .with_status(500)
            .with_body("Internal Server Error")
            .create_async()
            .await;

        let err = client.send_heartbeat("sess_test").await.unwrap_err();
        assert!(
            matches!(err, CoreError::ServiceUnavailable { .. }),
            "500 must map to CoreError::ServiceUnavailable (transient/retryable), got: {err:?}"
        );
    }

    fn perf_sample() -> maekon_core::models::feature_performance::FeaturePerfSample {
        maekon_core::models::feature_performance::FeaturePerfSample {
            response_time_ms: 12.5,
            timestamp: None,
            total_requests: Some(1),
            error_count: Some(0),
        }
    }

    #[tokio::test]
    async fn feature_performance_posts_to_canonical_path_with_samples_envelope() {
        use maekon_core::ports::feature_perf::FeaturePerfSink;
        let mut server = mockito::Server::new_async().await;
        let (client, _login_mock) = setup_authed_client(&mut server).await;

        // Exact contract path (full /api/v1 prefix) + POST + `{"samples":[...]}`.
        let mock = server
            .mock("POST", "/api/v1/system/features/screen-capture/performance")
            .match_body(mockito::Matcher::Regex(r#""samples""#.to_string()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"feature_key":"screen-capture","recorded_count":2,"status":"recorded"}"#)
            .create_async()
            .await;

        let samples = vec![perf_sample(), perf_sample()];
        let result = client
            .upload_feature_performance("screen-capture", &samples)
            .await;
        // 200 response with {"recorded_count":2,"status":"recorded"} body → Ok(()).
        // The port method returns Result<(), CoreError>; Ok(()) carries no richer value.
        // #5594: ok-only justified — upload_feature_performance returns unit on success; mock.assert verifies the request shape.
        result.expect("200 recorded → Ok: upload_feature_performance must succeed on 200");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn feature_performance_404_maps_to_notfound_for_drop() {
        use maekon_core::ports::feature_perf::FeaturePerfSink;
        let mut server = mockito::Server::new_async().await;
        let (client, _login_mock) = setup_authed_client(&mut server).await;

        // Unknown feature_key → server 404 → NotFound (permanent ⇒ caller drops).
        let _mock = server
            .mock("POST", "/api/v1/system/features/unknown-key/performance")
            .with_status(404)
            .with_body("feature not found")
            .create_async()
            .await;

        let err = client
            .upload_feature_performance("unknown-key", &[perf_sample()])
            .await
            .unwrap_err();
        assert!(
            matches!(err, CoreError::NotFound { .. }),
            "404 must map to CoreError::NotFound (permanent/drop), got: {err:?}"
        );
    }
}
