use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::stream::StreamExt;
use oneshim_core::config::TlsConfig;
use oneshim_core::error::CoreError;
use oneshim_core::models::suggestion::Suggestion;
use oneshim_core::ports::api_client::{SseClient, SseEvent};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tracing::{debug, info, warn};

use crate::auth::TokenManager;
use crate::http_client::build_reqwest_client;

/// SSE 활동 타임아웃 기본값 — 5분 동안 메시지가 없으면 재연결을 트리거한다.
const ACTIVITY_TIMEOUT_SECS: u64 = 300;

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
    /// `tls.allow_self_signed=true` 이면 자체 서명 인증서를 허용한다 (개발 전용).
    pub fn new_with_tls(
        base_url: &str,
        token_manager: Arc<TokenManager>,
        max_retry_secs: u64,
        tls: &TlsConfig,
    ) -> Result<Self, crate::error::NetworkError> {
        // SSE 스트림에도 HTTP 클라이언트와 동일한 TLS 정책 적용
        // 전역 타임아웃 미적용(None): SSE는 장기 스트림 연결이므로 단일 타임아웃으로 끊기면 안 됨
        let http_client = build_reqwest_client(tls, None)?;
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token_manager,
            max_retry_secs,
            http_client,
            last_event_id: Mutex::new(None),
            gap_count: Arc::new(AtomicU64::new(0)),
        })
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
}

#[async_trait]
impl SseClient for SseStreamClient {
    async fn connect(&self, session_id: &str, tx: mpsc::Sender<SseEvent>) -> Result<(), CoreError> {
        let url = format!(
            "{}/user_context/sessions/stream?session_id={}",
            self.base_url, session_id
        );
        let max_retry = self.max_retry_secs;

        info!("SSE connection started: {url}");

        let mut retry_delay = 1u64;

        loop {
            let token = self.token_manager.get_token().await?;

            let mut request = self
                .http_client
                .get(&url)
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

                    warn!("SSE reconnect waiting: {retry_delay}s");
                    tokio::time::sleep(Duration::from_secs(retry_delay)).await;
                    retry_delay = (retry_delay * 2).min(max_retry);
                    continue;
                }
            };

            if !response.status().is_success() {
                warn!(
                    "SSE connection failure (status={}): {}",
                    response.status(),
                    url
                );

                if tx.is_closed() {
                    return Ok(());
                }

                warn!("SSE reconnect waiting: {retry_delay}s");
                tokio::time::sleep(Duration::from_secs(retry_delay)).await;
                retry_delay = (retry_delay * 2).min(max_retry);
                continue;
            }

            let mut stream = response.bytes_stream().eventsource();
            debug!("SSE connection established");
            retry_delay = 1;

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

            warn!("SSE reconnect waiting: {retry_delay}s");
            tokio::time::sleep(Duration::from_secs(retry_delay)).await;
            retry_delay = (retry_delay * 2).min(max_retry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
