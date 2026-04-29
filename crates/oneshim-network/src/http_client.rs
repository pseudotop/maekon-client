use async_trait::async_trait;
use oneshim_core::config::TlsConfig;
use oneshim_core::error::CoreError;
use oneshim_core::models::event::EventBatch;
use oneshim_core::models::frame::ContextUpload;
use oneshim_core::models::suggestion::SuggestionFeedback;
use oneshim_core::ports::api_client::{ApiClient, SessionCreateResponse};
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
/// `tls.allow_self_signed=true` 이면 자체 서명 인증서를 허용한다 (개발 전용).
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

pub fn build_reqwest_client(
    tls: &TlsConfig,
    timeout: Option<Duration>,
) -> Result<reqwest::Client, NetworkError> {
    build_reqwest_client_for_url(tls, timeout, None)
}

/// Build a `reqwest::Client` with TLS policy. When `base_url` is provided,
/// emits a warning if `allow_self_signed` is used against a non-localhost host.
pub fn build_reqwest_client_for_url(
    tls: &TlsConfig,
    timeout: Option<Duration>,
    base_url: Option<&str>,
) -> Result<reqwest::Client, NetworkError> {
    let mut builder = reqwest::Client::builder();
    if let Some(t) = timeout {
        builder = builder.timeout(t);
    }

    if tls.enabled {
        // 운영 환경: HTTPS 전용 강제
        builder = builder.https_only(true);
    }

    if tls.allow_self_signed {
        if base_url.is_some_and(|u| !is_localhost(u)) {
            warn!(
                "allow_self_signed is enabled for non-localhost URL — \
                 this disables all certificate validation"
            );
        }
        tracing::warn!(
            "TLS: allow_self_signed=true — 자체 서명 인증서 허용됨. 운영 환경에서 사용 금지!"
        );
        // 개발 전용: 자체 서명 인증서 허용 (운영에서는 사용 금지)
        builder = builder.danger_accept_invalid_certs(true);
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
            // 502 Bad Gateway is a transient upstream failure — retryable just
            // like 503 Service Unavailable. (iter-54)
            502 | 503 => Err(NetworkError::ServiceUnavailable(text)),
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

    async fn upload_context(&self, upload: &ContextUpload) -> Result<(), CoreError> {
        debug!("context upload: {}", upload.metadata.app_name);

        self.execute_with_retry(|| async {
            let req = self
                .authorized_request(reqwest::Method::POST, "/user_context/contexts")
                .await?;

            let resp = req
                .json(upload)
                .send()
                .await
                .map_err(|e| map_reqwest_error(e, "context upload failure", self.timeout_ms))?;

            self.check_response(resp).await?;
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

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;
    use crate::error::NetworkError;

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
    fn build_reqwest_client_tls_disabled_succeeds() {
        // TLS 비활성화 시 http:// 요청 허용 — 개발/테스트 환경
        let tls = TlsConfig {
            enabled: false,
            allow_self_signed: false,
        };
        let result = build_reqwest_client(&tls, Some(Duration::from_secs(5)));
        assert!(result.is_ok(), "TLS 비활성화 클라이언트 생성 성공");
    }

    #[test]
    fn build_reqwest_client_tls_enabled_succeeds() {
        // TLS 활성화 시 클라이언트 생성 자체는 성공 (요청 시점에 https 강제)
        let tls = TlsConfig::default();
        let result = build_reqwest_client(&tls, Some(Duration::from_secs(5)));
        assert!(result.is_ok(), "TLS 활성화 클라이언트 생성 성공");
    }

    #[test]
    fn new_with_tls_returns_client() {
        let tls = TlsConfig {
            enabled: false, // 테스트: http:// URL 허용
            allow_self_signed: false,
        };
        let tm = Arc::new(TokenManager::new("http://localhost:8000"));
        let client =
            HttpApiClient::new_with_tls("http://localhost:8000", tm, Duration::from_secs(5), &tls);
        assert!(client.is_ok());
        assert_eq!(client.unwrap().base_url, "http://localhost:8000");
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
        tm.login("test@test.com", "pass").await.unwrap();

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

        let result = client.create_session("cli_1").await;
        assert!(result.is_ok());
        let session = result.unwrap();
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

        let result = client.end_session("sess_123").await;
        assert!(result.is_ok());
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn upload_context_success() {
        let mut server = mockito::Server::new_async().await;
        let (client, _login_mock) = setup_authed_client(&mut server).await;

        let mock = server
            .mock("POST", "/user_context/contexts")
            .with_status(200)
            .create_async()
            .await;

        let upload = oneshim_core::models::frame::ContextUpload {
            session_id: "sess_1".to_string(),
            timestamp: chrono::Utc::now(),
            metadata: oneshim_core::models::frame::FrameMetadata {
                timestamp: chrono::Utc::now(),
                trigger_type: "test".to_string(),
                app_name: "Test".to_string(),
                window_title: "Test Window".to_string(),
                resolution: (1920, 1080),
                importance: 0.5,
            },
            ocr_text: None,
            image: None,
        };

        let result = client.upload_context(&upload).await;
        assert!(result.is_ok());
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn upload_context_server_error() {
        let mut server = mockito::Server::new_async().await;
        let (client, _login_mock) = setup_authed_client(&mut server).await;

        let mock = server
            .mock("POST", "/user_context/contexts")
            .with_status(500)
            .with_body("Internal Server Error")
            .create_async()
            .await;

        let upload = oneshim_core::models::frame::ContextUpload {
            session_id: "sess_1".to_string(),
            timestamp: chrono::Utc::now(),
            metadata: oneshim_core::models::frame::FrameMetadata {
                timestamp: chrono::Utc::now(),
                trigger_type: "test".to_string(),
                app_name: "Test".to_string(),
                window_title: "Test Window".to_string(),
                resolution: (1920, 1080),
                importance: 0.5,
            },
            ocr_text: None,
            image: None,
        };

        let result = client.upload_context(&upload).await;
        assert!(result.is_err());
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

        let batch = oneshim_core::models::event::EventBatch {
            session_id: "sess_1".to_string(),
            events: vec![],
            created_at: chrono::Utc::now(),
        };

        let result = client.upload_batch(&batch).await;
        assert!(result.is_err());
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
            .mock("POST", "/user_context/contexts")
            .with_status(429)
            .with_body("Too Many Requests")
            .create_async()
            .await;

        let upload = oneshim_core::models::frame::ContextUpload {
            session_id: "sess_1".to_string(),
            timestamp: chrono::Utc::now(),
            metadata: oneshim_core::models::frame::FrameMetadata {
                timestamp: chrono::Utc::now(),
                trigger_type: "test".to_string(),
                app_name: "Test".to_string(),
                window_title: "Test Window".to_string(),
                resolution: (1920, 1080),
                importance: 0.5,
            },
            ocr_text: None,
            image: None,
        };

        let result = client.upload_context(&upload).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            err,
            CoreError::RateLimit {
                code: oneshim_core::error_codes::NetworkCode::RateLimit,
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

        let result = client.send_heartbeat("sess_1").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
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

        let result = client.send_heartbeat("sess_test").await;
        assert!(result.is_ok());
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
}
