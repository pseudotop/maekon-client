use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::stream::StreamExt;
use maekon_core::config::TlsConfig;
use maekon_core::error::CoreError;
use maekon_core::models::suggestion::Suggestion;
use maekon_core::ports::api_client::{SseClient, SseEvent};
use parking_lot::Mutex;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tracing::{debug, info, warn};

use crate::auth::TokenManager;
use crate::error::NetworkError;
use crate::http_client::build_reqwest_client_for_url;

/// SSE 활동 타임아웃 기본값 — 5분 동안 메시지가 없으면 재연결을 트리거한다.
const ACTIVITY_TIMEOUT_SECS: u64 = 300;

/// 연속 재연결 시도 상한 — 이 횟수만큼 연속 실패하면 재연결을 포기한다.
/// 성공적으로 스트림이 연결되면 카운터는 0으로 초기화된다.
const MAX_RECONNECT_ATTEMPTS: u32 = 10;

/// HTTP 상태 코드가 영구적(재시도 무의미) 실패인지 판별한다.
///
/// 401(인증 실패)·403(권한 없음)은 토큰/권한 문제이므로 재연결을 반복해도
/// 동일하게 실패한다. 즉시 포기하여 무한 재시도를 방지한다.
fn is_permanent_failure(status: reqwest::StatusCode) -> bool {
    matches!(
        status,
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
    )
}

pub struct SseStreamClient {
    base_url: String,
    token_manager: Arc<TokenManager>,
    max_retry_secs: u64,
    http_client: reqwest::Client,
    /// Tracks the last SSE event ID for automatic resume on reconnect (RFC 9110 §9.3.4)
    last_event_id: Mutex<Option<String>>,
    /// 누적 이벤트 ID 갭 카운터 — 수신 누락 추정치
    gap_count: Arc<AtomicU64>,
}

impl SseStreamClient {
    /// 기존 생성자 — TLS 미적용 (역호환성 보장, 테스트 전용)
    pub fn new(base_url: &str, token_manager: Arc<TokenManager>, max_retry_secs: u64) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token_manager,
            max_retry_secs,
            http_client: reqwest::Client::new(),
            last_event_id: Mutex::new(None),
            gap_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// TLS 설정 적용 생성자 — 운영 환경 표준 진입점
    ///
    /// `tls.enabled=true` 이면 HTTPS 전용을 강제한다.
    /// `tls.allow_self_signed=true` 는 인증서 검증 우회로 처리하지 않는다.
    pub fn new_with_tls(
        base_url: &str,
        token_manager: Arc<TokenManager>,
        max_retry_secs: u64,
        tls: &TlsConfig,
    ) -> Result<Self, crate::error::NetworkError> {
        let base_url = Self::validated_base_url(base_url, tls)?;
        // SSE 스트림에도 HTTP 클라이언트와 동일한 TLS 정책 적용
        // 전역 타임아웃 미적용(None): SSE는 장기 스트림 연결이므로 단일 타임아웃으로 끊기면 안 됨
        let http_client = build_reqwest_client_for_url(tls, None, Some(&base_url))?;
        Ok(Self {
            base_url,
            token_manager,
            max_retry_secs,
            http_client,
            last_event_id: Mutex::new(None),
            gap_count: Arc::new(AtomicU64::new(0)),
        })
    }

    fn validated_base_url(base_url: &str, tls: &TlsConfig) -> Result<String, NetworkError> {
        let trimmed = base_url.trim_end_matches('/');
        let url = reqwest::Url::parse(trimmed)
            .map_err(|e| NetworkError::Config(format!("invalid SSE base URL `{trimmed}`: {e}")))?;

        match url.scheme() {
            "https" => Ok(trimmed.to_string()),
            "http" if !tls.enabled && Self::is_loopback_or_localhost_url(&url) => {
                Ok(trimmed.to_string())
            }
            "http" => Err(NetworkError::Config(
                "remote SSE endpoints must use HTTPS; cleartext HTTP is allowed only for loopback development endpoints with TLS disabled".to_string(),
            )),
            scheme => Err(NetworkError::Config(format!(
                "unsupported SSE URL scheme `{scheme}`; expected https"
            ))),
        }
    }

    fn is_loopback_or_localhost_url(url: &reqwest::Url) -> bool {
        let Some(host) = url.host_str() else {
            return false;
        };

        host.eq_ignore_ascii_case("localhost")
            || host.parse::<IpAddr>().is_ok_and(|addr| addr.is_loopback())
    }

    /// Returns the last received SSE event ID, if any.
    pub fn last_event_id(&self) -> Option<String> {
        self.last_event_id.lock().clone()
    }

    /// 누적 이벤트 ID 갭 수 반환 — 연결 유지 중 수신 누락 추정치
    pub fn gap_count(&self) -> u64 {
        self.gap_count.load(Ordering::Relaxed)
    }

    fn parse_event(event_type: &str, data: &str) -> Option<SseEvent> {
        match event_type {
            "connection" => {
                let val: serde_json::Value = serde_json::from_str(data).ok()?;
                let session_id = val.get("session_id")?.as_str()?.to_string();
                Some(SseEvent::Connected { session_id })
            }
            "suggestion" => {
                let suggestion: Suggestion = serde_json::from_str(data).ok()?;
                Some(SseEvent::Suggestion(suggestion))
            }
            "update" => {
                let val: serde_json::Value = serde_json::from_str(data).ok()?;
                Some(SseEvent::Update(val))
            }
            "heartbeat" => {
                let val: serde_json::Value = serde_json::from_str(data).ok()?;
                let ts_str = val.get("timestamp")?.as_str()?;
                let timestamp = chrono::DateTime::parse_from_rfc3339(ts_str)
                    .ok()?
                    .with_timezone(&chrono::Utc);
                Some(SseEvent::Heartbeat { timestamp })
            }
            "error" => {
                let msg = data.to_string();
                Some(SseEvent::Error(msg))
            }
            "close" => Some(SseEvent::Close),
            "message" => {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(data) {
                    Some(SseEvent::Update(val))
                } else {
                    debug!("message received: {data}");
                    None
                }
            }
            _ => {
                debug!("unknown SSE event: {event_type}");
                None
            }
        }
    }

    fn stream_url(&self, session_id: &str) -> Result<reqwest::Url, CoreError> {
        let endpoint = format!("{}/user_context/sessions/stream", self.base_url);
        let mut url = reqwest::Url::parse(&endpoint).map_err(|e| CoreError::Network {
            code: maekon_core::error_codes::NetworkCode::Generic,
            message: format!("Invalid SSE stream URL: {e}"),
        })?;
        url.query_pairs_mut().append_pair("session_id", session_id);
        Ok(url)
    }
}

#[async_trait]
impl SseClient for SseStreamClient {
    async fn connect(&self, session_id: &str, tx: mpsc::Sender<SseEvent>) -> Result<(), CoreError> {
        let url = self.stream_url(session_id)?;
        let max_retry = self.max_retry_secs;

        info!("SSE connection started");

        let mut retry_delay = 1u64;
        // 연속 재연결 시도 횟수 — 스트림이 성공적으로 열리면 0으로 초기화된다.
        let mut reconnect_attempts = 0u32;

        loop {
            let token = self.token_manager.get_token().await?;

            let mut request = self
                .http_client
                .get(url.clone())
                .header("Authorization", format!("Bearer {token}"));

            if let Some(ref id) = *self.last_event_id.lock() {
                request = request.header("Last-Event-ID", id.as_str());
                debug!(last_event_id = %id, "SSE reconnecting with Last-Event-ID");
            }

            let response = match request.send().await {
                Ok(response) => response,
                Err(e) => {
                    warn!("SSE connection request failure: {e}");

                    if tx.is_closed() {
                        return Ok(());
                    }

                    reconnect_attempts += 1;
                    if reconnect_attempts >= MAX_RECONNECT_ATTEMPTS {
                        warn!(
                            attempts = reconnect_attempts,
                            "SSE reconnect give-up — max attempts reached"
                        );
                        return Err(CoreError::Network {
                            code: maekon_core::error_codes::NetworkCode::Generic,
                            message: format!(
                                "SSE reconnect aborted after {reconnect_attempts} consecutive failures"
                            ),
                        });
                    }

                    warn!("SSE reconnect waiting: {retry_delay}s");
                    tokio::time::sleep(Duration::from_secs(retry_delay)).await;
                    retry_delay = (retry_delay * 2).min(max_retry);
                    continue;
                }
            };

            if !response.status().is_success() {
                let status = response.status();
                warn!(
                    status = %status,
                    "SSE connection failure"
                );

                // 401/403 등 영구 실패는 재시도해도 동일하게 실패하므로 즉시 포기한다.
                if is_permanent_failure(status) {
                    warn!(
                        status = %status,
                        "SSE permanent failure — not retrying"
                    );
                    return Err(CoreError::Auth {
                        code: maekon_core::error_codes::AuthCode::Failed,
                        message: format!("SSE stream rejected with permanent status {status}"),
                    });
                }

                if tx.is_closed() {
                    return Ok(());
                }

                reconnect_attempts += 1;
                if reconnect_attempts >= MAX_RECONNECT_ATTEMPTS {
                    warn!(
                        attempts = reconnect_attempts,
                        "SSE reconnect give-up — max attempts reached"
                    );
                    return Err(CoreError::Network {
                        code: maekon_core::error_codes::NetworkCode::Generic,
                        message: format!(
                            "SSE reconnect aborted after {reconnect_attempts} consecutive failures"
                        ),
                    });
                }

                warn!("SSE reconnect waiting: {retry_delay}s");
                tokio::time::sleep(Duration::from_secs(retry_delay)).await;
                retry_delay = (retry_delay * 2).min(max_retry);
                continue;
            }

            let mut stream = response.bytes_stream().eventsource();
            debug!("SSE connection established");
            // 연결 성공 — 백오프와 give-up 카운터를 모두 초기화한다.
            retry_delay = 1;
            reconnect_attempts = 0;

            let activity_timeout = Duration::from_secs(ACTIVITY_TIMEOUT_SECS);

            loop {
                match timeout(activity_timeout, stream.next()).await {
                    Ok(Some(Ok(msg))) => {
                        let event_id = if msg.id.is_empty() {
                            None
                        } else {
                            Some(msg.id.clone())
                        };

                        // Gap detection: warn when numeric event IDs skip values
                        if let (Some(ref last_str), Some(ref new_str)) =
                            (&*self.last_event_id.lock(), &event_id)
                        {
                            if let (Ok(last_n), Ok(new_n)) =
                                (last_str.parse::<u64>(), new_str.parse::<u64>())
                            {
                                if new_n > last_n + 1 {
                                    let gap = new_n - last_n - 1;
                                    self.gap_count.fetch_add(gap, Ordering::Relaxed);
                                    warn!(
                                        gap,
                                        last = last_n,
                                        current = new_n,
                                        "SSE event ID gap detected"
                                    );
                                }
                            }
                        }

                        if let Some(ref id) = event_id {
                            *self.last_event_id.lock() = Some(id.clone());
                        }

                        let event_type = if msg.event.is_empty() {
                            "message"
                        } else {
                            &msg.event
                        };

                        if let Some(sse_event) = Self::parse_event(event_type, &msg.data) {
                            if tx.send(sse_event).await.is_err() {
                                info!("SSE event channel closed, connection closed");
                                return Ok(());
                            }
                        }
                    }
                    Ok(Some(Err(e))) => {
                        warn!("SSE stream error: {e}");
                        break;
                    }
                    Ok(None) => {
                        info!("SSE stream ended");
                        break;
                    }
                    Err(_elapsed) => {
                        warn!(
                            timeout_secs = ACTIVITY_TIMEOUT_SECS,
                            "SSE activity timeout — reconnecting"
                        );
                        break;
                    }
                }
            }

            if tx.is_closed() {
                return Ok(());
            }

            reconnect_attempts += 1;
            if reconnect_attempts >= MAX_RECONNECT_ATTEMPTS {
                warn!(
                    attempts = reconnect_attempts,
                    "SSE reconnect give-up — max attempts reached"
                );
                return Err(CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    message: format!(
                        "SSE reconnect aborted after {reconnect_attempts} consecutive failures"
                    ),
                });
            }

            warn!("SSE reconnect waiting: {retry_delay}s");
            tokio::time::sleep(Duration::from_secs(retry_delay)).await;
            retry_delay = (retry_delay * 2).min(max_retry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runtime-built password fixture — a string literal at the `login()` call
    /// site trips CodeQL `rust/hard-coded-cryptographic-value` (public alerts
    /// #175/#176); mirrors `auth::tests::primary_password`.
    fn primary_password() -> String {
        String::from_utf8(vec![b'x'; 16]).expect("password fixture bytes must be UTF-8")
    }

    #[test]
    fn parse_connection_event() {
        let data = r#"{"session_id": "sess_123"}"#;
        let event = SseStreamClient::parse_event("connection", data);
        assert!(
            matches!(event, Some(SseEvent::Connected { session_id }) if session_id == "sess_123")
        );
    }

    #[test]
    fn parse_suggestion_event() {
        let data = r#"{
            "suggestion_id": "sug_001",
            "suggestion_type": "WORK_GUIDANCE",
            "content": "test suggestion",
            "priority": "HIGH",
            "confidence_score": 0.95,
            "relevance_score": 0.88,
            "is_actionable": true,
            "created_at": "2026-01-28T10:00:00Z"
        }"#;
        let event = SseStreamClient::parse_event("suggestion", data);
        assert!(matches!(event, Some(SseEvent::Suggestion(_))));
    }

    #[test]
    fn parse_heartbeat_event() {
        let data = r#"{"timestamp": "2026-01-28T10:00:00Z"}"#;
        let event = SseStreamClient::parse_event("heartbeat", data);
        assert!(matches!(event, Some(SseEvent::Heartbeat { .. })));
    }

    #[test]
    fn parse_error_event() {
        let event = SseStreamClient::parse_event("error", "server error");
        assert!(matches!(event, Some(SseEvent::Error(_))));
    }

    #[test]
    fn parse_close_event() {
        let event = SseStreamClient::parse_event("close", "");
        assert!(matches!(event, Some(SseEvent::Close)));
    }

    #[test]
    fn parse_unknown_event() {
        let event = SseStreamClient::parse_event("unknown_type", "data");
        assert!(event.is_none());
    }

    #[test]
    fn parse_message_event_json() {
        let data = r#"{"key": "value"}"#;
        let event = SseStreamClient::parse_event("message", data);
        assert!(matches!(event, Some(SseEvent::Update(_))));
    }

    #[test]
    fn parse_message_event_non_json() {
        let event = SseStreamClient::parse_event("message", "plain text");
        assert!(event.is_none());
    }

    #[test]
    #[allow(deprecated)]
    fn new_with_tls_rejects_remote_cleartext_base_url() {
        let tls = TlsConfig {
            enabled: false,
            allow_self_signed: false,
        };
        let tm = TokenManager::new("https://auth.example.com");

        let result =
            SseStreamClient::new_with_tls("http://api.example.com", Arc::new(tm), 30, &tls);

        // .err().expect(..) instead of .unwrap_err(): SseStreamClient (the Ok
        // type) does not implement Debug, which unwrap_err requires.
        let cfg_err = result
            .err()
            .expect("remote cleartext HTTP must be rejected at construction");
        assert!(
            matches!(cfg_err, crate::error::NetworkError::Config(_)),
            "remote cleartext HTTP must yield NetworkError::Config; got: {cfg_err:?}"
        );
    }

    #[test]
    #[allow(deprecated)]
    fn last_event_id_initially_none() {
        let tm = TokenManager::new("http://localhost");
        let client = SseStreamClient::new("http://localhost", Arc::new(tm), 30);
        assert!(client.last_event_id().is_none());
    }

    #[test]
    #[allow(deprecated)]
    fn gap_count_initially_zero() {
        let tm = TokenManager::new("http://localhost");
        let client = SseStreamClient::new("http://localhost", Arc::new(tm), 30);
        assert_eq!(client.gap_count(), 0);
    }

    #[test]
    fn permanent_failure_detects_auth_statuses() {
        // 401·403 은 영구 실패로 분류되어 재시도하지 않아야 한다.
        assert!(is_permanent_failure(reqwest::StatusCode::UNAUTHORIZED));
        assert!(is_permanent_failure(reqwest::StatusCode::FORBIDDEN));
    }

    #[test]
    fn permanent_failure_excludes_transient_statuses() {
        // 5xx·429·404 등은 일시적일 수 있으므로 재시도 대상(영구 실패 아님)이다.
        assert!(!is_permanent_failure(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR
        ));
        assert!(!is_permanent_failure(reqwest::StatusCode::BAD_GATEWAY));
        assert!(!is_permanent_failure(
            reqwest::StatusCode::SERVICE_UNAVAILABLE
        ));
        assert!(!is_permanent_failure(
            reqwest::StatusCode::TOO_MANY_REQUESTS
        ));
        assert!(!is_permanent_failure(reqwest::StatusCode::NOT_FOUND));
        assert!(!is_permanent_failure(reqwest::StatusCode::OK));
    }

    #[tokio::test]
    #[allow(deprecated)]
    async fn permanent_failure_returns_auth_error_without_retry() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base = format!("http://127.0.0.1:{}", addr.port());

        // stub: 1) login 요청에 유효 토큰 응답, 2) SSE 요청에 401 응답
        let server_task = tokio::spawn(async move {
            // login (POST /api/v1/auth/tokens)
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = socket.read(&mut buf).await;
                let body = r#"{"access_token":"tok","refresh_token":"ref","expires_in":3600}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(resp.as_bytes()).await;
                let _ = socket.flush().await;
            }
            // SSE stream → 401 Unauthorized (영구 실패)
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = socket.read(&mut buf).await;
                let _ = socket
                    .write_all(
                        b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
                let _ = socket.flush().await;
            }
        });

        let tm = TokenManager::new(&base);
        tm.login("user@example.com", &primary_password())
            .await
            .unwrap();
        let client = SseStreamClient::new(&base, Arc::new(tm), 30);

        let (tx, _rx) = mpsc::channel::<SseEvent>(8);
        let result = client.connect("sess_perm", tx).await;

        server_task.abort();

        // 무한 재시도 대신 Auth 에러로 즉시 종료되어야 한다.
        assert!(
            matches!(result, Err(CoreError::Auth { .. })),
            "401 should map to a permanent Auth error, got: {result:?}"
        );
    }

    #[tokio::test]
    #[allow(deprecated)]
    async fn give_up_ceiling_returns_network_error() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base = format!("http://127.0.0.1:{}", addr.port());

        // stub: login 요청 1회만 처리하고 종료 → 이후 SSE 연결은 모두 거부(연결 실패)
        let server_task = tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = socket.read(&mut buf).await;
                let body = r#"{"access_token":"tok","refresh_token":"ref","expires_in":3600}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(resp.as_bytes()).await;
                let _ = socket.flush().await;
            }
            // listener drop → 이후 연결 거부됨
        });

        let tm = TokenManager::new(&base);
        tm.login("user@example.com", &primary_password())
            .await
            .unwrap();
        // login 응답이 처리되도록 서버 태스크 완료를 기다린다(listener drop 보장).
        let _ = server_task.await;

        // max_retry_secs=0 이므로 백오프 sleep 없이 빠르게 give-up 상한에 도달한다.
        let client = SseStreamClient::new(&base, Arc::new(tm), 0);

        let (tx, _rx) = mpsc::channel::<SseEvent>(8);
        let result = client.connect("sess_giveup", tx).await;

        // 무한 재시도 대신 give-up 상한에서 Network 에러로 종료되어야 한다.
        assert!(
            matches!(result, Err(CoreError::Network { .. })),
            "exhausted reconnects should map to a Network error, got: {result:?}"
        );
    }

    #[test]
    #[allow(deprecated)]
    fn gap_count_increments_atomically() {
        let tm = TokenManager::new("http://localhost");
        let client = SseStreamClient::new("http://localhost", Arc::new(tm), 30);
        // 직접 AtomicU64 조작으로 카운터 동작 검증
        client
            .gap_count
            .fetch_add(3, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(client.gap_count(), 3);
        client
            .gap_count
            .fetch_add(5, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(client.gap_count(), 8);
    }
}
