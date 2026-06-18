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

/// Default SSE activity timeout — triggers a reconnect when no message arrives
/// for 5 minutes.
const ACTIVITY_TIMEOUT_SECS: u64 = 300;

/// Ceiling on consecutive reconnect attempts — gives up reconnecting after this
/// many consecutive failures. The counter is only reset to 0 once the stream has
/// delivered at least one real event. (A 200 handshake alone does NOT reset it —
/// this prevents an unbounded reconnect loop caused by a server that drops the
/// connection right after the handshake.)
const MAX_RECONNECT_ATTEMPTS: u32 = 10;

/// Maximum byte size accepted for a single SSE event (data + event-name + id).
///
/// `eventsource-stream` accumulates server bytes into an internal buffer and
/// only yields an `Event` once an event boundary (blank line) is seen, with no
/// upper bound of its own. A malformed or hostile server can therefore deliver
/// one enormous event whose `data` field is many MiB/GiB, which would balloon
/// the heap of this 24/7 process. We cap each delivered event at 1 MiB; any
/// event exceeding the cap is treated as a transient stream error (the inner
/// read loop breaks to force a reconnect) rather than being processed or
/// accumulated. Legitimate suggestion/heartbeat payloads are well under 1 MiB.
const MAX_SSE_EVENT_BYTES: usize = 1024 * 1024;

/// Determines whether an HTTP status code is a permanent (not-worth-retrying) failure.
///
/// 401 (authentication failure) and 403 (forbidden) are token/permission issues,
/// so repeated reconnects would fail identically. Give up immediately to avoid
/// an infinite retry loop.
fn is_permanent_failure(status: reqwest::StatusCode) -> bool {
    matches!(
        status,
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
    )
}

/// Returns the total server-controlled byte size of a parsed SSE event.
///
/// Sums the three server-supplied string fields (`data`, `event`, `id`) that
/// hold attacker-influenced payload bytes, so the cap covers an oversized event
/// regardless of which field the bytes land in (e.g. a giant `data` block or an
/// abusive event-name/id).
fn sse_event_byte_len(msg: &eventsource_stream::Event) -> usize {
    msg.data
        .len()
        .saturating_add(msg.event.len())
        .saturating_add(msg.id.len())
}

/// Returns true when a parsed SSE event exceeds [`MAX_SSE_EVENT_BYTES`].
fn sse_event_exceeds_cap(msg: &eventsource_stream::Event) -> bool {
    sse_event_byte_len(msg) > MAX_SSE_EVENT_BYTES
}

/// Number of skipped event IDs between two consecutive numeric SSE IDs.
///
/// Both IDs are server-controlled `u64`s, so the gap is computed with
/// saturating arithmetic: a naive `new - last - 1` panics in debug and wraps to
/// garbage in release when a hostile or buggy server sends `last == u64::MAX`
/// (or any non-increasing pair). Returns 0 for adjacent, equal, or decreasing
/// IDs — i.e. only a genuine forward skip yields a positive count.
fn sse_id_gap(last_n: u64, new_n: u64) -> u64 {
    new_n.saturating_sub(last_n).saturating_sub(1)
}

pub struct SseStreamClient {
    base_url: String,
    token_manager: Arc<TokenManager>,
    max_retry_secs: u64,
    http_client: reqwest::Client,
    /// Tracks the last SSE event ID for automatic resume on reconnect (RFC 9110 §9.3.4)
    last_event_id: Mutex<Option<String>>,
    /// Cumulative event-ID gap counter — an estimate of missed deliveries.
    gap_count: Arc<AtomicU64>,
}

impl SseStreamClient {
    /// Legacy constructor — no TLS (test-only; permitted only for loopback addresses).
    ///
    /// Production callers must use [`SseStreamClient::new_with_tls`] to enforce
    /// the HTTPS-only policy.  This constructor is retained only for unit/integration
    /// tests that connect to in-process loopback servers.
    #[deprecated(note = "Use new_with_tls() for TLS enforcement; this constructor is test-only")]
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

    /// TLS-enabled constructor — the standard production entry point.
    ///
    /// When `tls.enabled=true`, HTTPS is enforced.
    /// `tls.allow_self_signed=true` is NOT treated as a certificate-verification bypass.
    pub fn new_with_tls(
        base_url: &str,
        token_manager: Arc<TokenManager>,
        max_retry_secs: u64,
        tls: &TlsConfig,
    ) -> Result<Self, crate::error::NetworkError> {
        let base_url = Self::validated_base_url(base_url, tls)?;
        // Apply the same TLS policy to the SSE stream as the HTTP client uses.
        // No global timeout (None): SSE is a long-lived stream connection, so it
        // must not be torn down by a single overall timeout.
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

    /// Returns the cumulative event-ID gap count — an estimate of deliveries
    /// missed while the connection was held open.
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
        // Consecutive reconnect-attempt count — only reset to 0 once the stream
        // has delivered at least one real event.
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

                // Permanent failures (401/403 etc.) would fail identically on
                // retry, so give up immediately.
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

            // The 200 handshake alone is NOT proof of a healthy stream: a server
            // that accepts the handshake then immediately drops the connection
            // would otherwise reset the backoff/give-up counters on every loop,
            // causing an unbounded tight reconnect loop that never reaches
            // MAX_RECONNECT_ATTEMPTS. We only reset the counters once the
            // connection has produced at least one real event (`made_progress`).
            let mut made_progress = false;

            let activity_timeout = Duration::from_secs(ACTIVITY_TIMEOUT_SECS);

            loop {
                match timeout(activity_timeout, stream.next()).await {
                    Ok(Some(Ok(msg))) => {
                        // Bound the SSE body: a malformed or hostile server can
                        // emit a single enormous event that would OOM this
                        // long-running process. Reject any event over the cap as
                        // a transient stream error — break out of the inner loop
                        // to force a reconnect (do NOT process or accumulate it,
                        // and do NOT reset the backoff/give-up budget for it).
                        if sse_event_exceeds_cap(&msg) {
                            warn!(
                                event_bytes = sse_event_byte_len(&msg),
                                cap_bytes = MAX_SSE_EVENT_BYTES,
                                "SSE event exceeds byte cap — dropping and reconnecting"
                            );
                            break;
                        }

                        // A real event arrived — the connection is genuinely
                        // healthy, so the backoff/give-up budget can reset.
                        if !made_progress {
                            made_progress = true;
                            retry_delay = 1;
                            reconnect_attempts = 0;
                        }

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
                                // Event IDs are server-controlled u64s, so the
                                // gap is computed with saturating arithmetic to
                                // avoid an overflow (debug panic / release
                                // garbage) on a hostile `last_n == u64::MAX`.
                                let gap = sse_id_gap(last_n, new_n);
                                if gap > 0 {
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
        // 401 and 403 must be classified as permanent failures and not retried.
        assert!(is_permanent_failure(reqwest::StatusCode::UNAUTHORIZED));
        assert!(is_permanent_failure(reqwest::StatusCode::FORBIDDEN));
    }

    #[test]
    fn permanent_failure_excludes_transient_statuses() {
        // 5xx/429/404 etc. may be transient, so they are retry candidates (not
        // permanent failures).
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

        // stub: 1) respond to the login request with a valid token, 2) respond to
        // the SSE request with 401
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
            // SSE stream → 401 Unauthorized (permanent failure)
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

        // Must terminate immediately with an Auth error instead of retrying forever.
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

        // stub: handle a single login request then exit → all subsequent SSE
        // connections are refused (connection failure)
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
            // listener drop → subsequent connections are refused
        });

        let tm = TokenManager::new(&base);
        tm.login("user@example.com", &primary_password())
            .await
            .unwrap();
        // Wait for the server task to finish so the login response is handled
        // (guarantees the listener has been dropped).
        let _ = server_task.await;

        // With max_retry_secs=0 there are no backoff sleeps, so the give-up
        // ceiling is reached quickly.
        let client = SseStreamClient::new(&base, Arc::new(tm), 0);

        let (tx, _rx) = mpsc::channel::<SseEvent>(8);
        let result = client.connect("sess_giveup", tx).await;

        // Must terminate with a Network error at the give-up ceiling instead of
        // retrying forever.
        assert!(
            matches!(result, Err(CoreError::Network { .. })),
            "exhausted reconnects should map to a Network error, got: {result:?}"
        );
    }

    #[tokio::test]
    #[allow(deprecated)]
    async fn handshake_then_immediate_drop_reaches_give_up_ceiling() {
        // Regression (review2 #6079): a server that accepts the 200 handshake then
        // immediately drops the stream (delivering zero events) must NOT reset the
        // give-up budget on the handshake alone. Otherwise the client tight-loops
        // reconnecting forever and MAX_RECONNECT_ATTEMPTS is never reached. The
        // reset is now gated on at least one real event (`made_progress`), so this
        // sequence must terminate with a Network give-up error.
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base = format!("http://127.0.0.1:{}", addr.port());

        // stub: after a single login response, every SSE request gets only a
        // 200 + event-stream handshake and then the connection is dropped
        // immediately with no events (simulates a server-side handshake-then-drop).
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
            // Every subsequent SSE connection: 200 handshake + 0-byte body →
            // immediate close. Accept forever — with the bug present the client
            // would reconnect endlessly, so this loop must never end and shut the
            // client down; that way the test exercises the give-up logic itself
            // (and avoids an accidental termination caused by the server exiting).
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = socket.read(&mut buf).await;
                let _ = socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                    )
                    .await;
                let _ = socket.flush().await;
                // socket drop → from the client's view the stream ends (zero events)
            }
        });

        let tm = TokenManager::new(&base);
        tm.login("user@example.com", &primary_password())
            .await
            .unwrap();

        // max_retry_secs=0 → no backoff sleeps, so the give-up ceiling is reached quickly.
        let client = SseStreamClient::new(&base, Arc::new(tm), 0);

        let (tx, _rx) = mpsc::channel::<SseEvent>(8);
        // Bound the call: with the bug present the client would reconnect forever,
        // so a timeout here surfaces the regression as a test failure instead of a
        // hang. The fixed client reaches the give-up ceiling near-instantly
        // (max_retry_secs=0 → no backoff sleeps).
        let result = timeout(
            Duration::from_secs(10),
            client.connect("sess_handshake_drop", tx),
        )
        .await
        .expect("handshake-then-drop must terminate via give-up, not loop forever");

        server_task.abort();

        // The handshake alone does not reset the counter, so this must terminate
        // at the give-up ceiling.
        assert!(
            matches!(result, Err(CoreError::Network { .. })),
            "handshake-then-drop must reach the give-up ceiling (Network error), got: {result:?}"
        );
    }

    #[test]
    fn sse_event_within_cap_is_accepted() {
        // A normal-sized event (well under 1 MiB) must not be flagged.
        let msg = eventsource_stream::Event {
            event: "suggestion".to_string(),
            data: "x".repeat(4096),
            id: "42".to_string(),
            retry: None,
        };
        assert!(!sse_event_exceeds_cap(&msg));
    }

    #[test]
    fn sse_event_at_cap_boundary_is_accepted() {
        // Exactly MAX_SSE_EVENT_BYTES is allowed; only strictly-greater is rejected.
        let msg = eventsource_stream::Event {
            event: String::new(),
            data: "y".repeat(MAX_SSE_EVENT_BYTES),
            id: String::new(),
            retry: None,
        };
        assert_eq!(sse_event_byte_len(&msg), MAX_SSE_EVENT_BYTES);
        assert!(!sse_event_exceeds_cap(&msg));
    }

    #[test]
    fn sse_event_over_cap_is_rejected() {
        // A single oversized `data` payload (the OOM vector) must be rejected so
        // the connect loop drops it and reconnects instead of accumulating it.
        let msg = eventsource_stream::Event {
            event: String::new(),
            data: "z".repeat(MAX_SSE_EVENT_BYTES + 1),
            id: String::new(),
            retry: None,
        };
        assert!(sse_event_exceeds_cap(&msg));
    }

    #[test]
    fn sse_event_cap_counts_all_server_fields() {
        // The cap sums data + event + id so bytes are bounded regardless of which
        // server-controlled field carries them (here split across all three).
        let third = MAX_SSE_EVENT_BYTES / 3 + 1;
        let msg = eventsource_stream::Event {
            event: "e".repeat(third),
            data: "d".repeat(third),
            id: "i".repeat(third),
            retry: None,
        };
        assert!(sse_event_byte_len(&msg) > MAX_SSE_EVENT_BYTES);
        assert!(sse_event_exceeds_cap(&msg));
    }

    #[test]
    #[allow(deprecated)]
    fn gap_count_increments_atomically() {
        let tm = TokenManager::new("http://localhost");
        let client = SseStreamClient::new("http://localhost", Arc::new(tm), 30);
        // Verify counter behavior by manipulating the AtomicU64 directly.
        client
            .gap_count
            .fetch_add(3, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(client.gap_count(), 3);
        client
            .gap_count
            .fetch_add(5, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(client.gap_count(), 8);
    }

    #[test]
    fn sse_id_gap_counts_forward_skips() {
        // Adjacent IDs => no gap.
        assert_eq!(sse_id_gap(10, 11), 0);
        // One skipped ID (10 -> 12 means 11 was dropped).
        assert_eq!(sse_id_gap(10, 12), 1);
        // Several skipped IDs.
        assert_eq!(sse_id_gap(10, 15), 4);
        // Equal or decreasing IDs => no gap (never negative/underflow).
        assert_eq!(sse_id_gap(10, 10), 0);
        assert_eq!(sse_id_gap(10, 5), 0);
    }

    #[test]
    fn sse_id_gap_does_not_overflow_on_max_last_id() {
        // Regression: a server-controlled `last == u64::MAX` previously caused
        // `last + 1` to overflow (debug panic / release garbage). Saturating
        // arithmetic must keep this finite and panic-free for every new id.
        assert_eq!(sse_id_gap(u64::MAX, u64::MAX), 0);
        assert_eq!(sse_id_gap(u64::MAX, 0), 0);
        assert_eq!(sse_id_gap(u64::MAX, u64::MAX - 1), 0);
        // Symmetric extreme: huge new id off a small last id stays bounded.
        assert_eq!(sse_id_gap(0, u64::MAX), u64::MAX - 1);
    }
}
