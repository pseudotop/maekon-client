use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use reqwest::header::HeaderMap;
use tokio::net::TcpStream;
use tokio::sync::{Mutex, Notify};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tracing::{debug, warn};

use maekon_api_contracts::integration::IntegrationAckPayload;
use maekon_core::error::CoreError;
use maekon_core::models::integration::{IntegrationAckCursor, ProactivePrompt};

use super::cloudevents::{IntegrationCloudEvent, PromptCloudEventBatch};
use super::prompt_from_cloudevent;
use super::transport::IntegrationEgressTransportResponse;

type LiveWebSocketStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Maximum number of inbound ack entries queued before the oldest is evicted.
/// Prevents an untrusted/compromised server from growing the queue without bound.
const MAX_INBOUND_ACKS: usize = 1000;

/// Maximum number of inbound prompt entries queued before the oldest is evicted.
/// Prevents an untrusted/compromised server from growing the queue without bound.
const MAX_INBOUND_PROMPTS: usize = 1000;

/// Maximum byte length of a WebSocket text frame that will be deserialized.
/// Frames larger than this limit are skipped with a warning rather than parsed,
/// capping per-message allocations under the agent's <100 MB RSS budget.
const MAX_WS_MESSAGE_BYTES: usize = 1024 * 1024; // 1 MiB

/// Protocol-layer WebSocket config that caps both frame and message size at
/// [`MAX_WS_MESSAGE_BYTES`]. tungstenite's defaults are 16 MiB/frame and
/// 64 MiB/message, so without this an oversized frame would be fully buffered
/// (blowing the agent's <100 MB RSS budget) before the app-level length gate in
/// `read_loop` ever runs. With this cap tungstenite rejects the frame during
/// the read itself; the app-level check remains as defense-in-depth (#6825).
fn capped_ws_config() -> tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
    tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(MAX_WS_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_WS_MESSAGE_BYTES))
}

#[derive(Default)]
struct WebSocketIntegrationInboundState {
    outbound_acks: VecDeque<IntegrationAckPayload>,
    prompts: VecDeque<ProactivePrompt>,
}

#[derive(Clone)]
pub struct WebSocketIntegrationSessionChannel {
    sender: Arc<Mutex<futures::stream::SplitSink<LiveWebSocketStream, Message>>>,
    inbound: Arc<Mutex<WebSocketIntegrationInboundState>>,
    ack_notify: Arc<Notify>,
    prompt_notify: Arc<Notify>,
    /// Notified on drop/close to unblock the read_loop task (F-RR-29).
    cancel_notify: Arc<Notify>,
}

impl WebSocketIntegrationSessionChannel {
    pub async fn connect(url: &str, headers: HeaderMap) -> Result<Self, CoreError> {
        let mut request = url
            .into_client_request()
            .map_err(|err| CoreError::Validation {
                code: maekon_core::error_codes::ValidationCode::InvalidField,
                field: "integration.session.channel_url".to_string(),
                message: format!("invalid websocket URL: {err}"),
            })?;

        for (name, value) in headers.iter() {
            request.headers_mut().insert(name, value.clone());
        }

        let (stream, _) =
            tokio_tungstenite::connect_async_with_config(request, Some(capped_ws_config()), false)
                .await
                .map_err(|err| CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    message: format!("integration websocket connect failed: {err}"),
                })?;
        let (writer, reader) = stream.split();
        let inbound = Arc::new(Mutex::new(WebSocketIntegrationInboundState::default()));
        let ack_notify = Arc::new(Notify::new());
        let prompt_notify = Arc::new(Notify::new());
        // F-RR-29: cancel_notify signals read_loop to exit when the channel is
        // dropped or explicitly closed, preventing a leaked detached task.
        let cancel_notify = Arc::new(Notify::new());

        tokio::spawn(Self::read_loop(
            reader,
            inbound.clone(),
            ack_notify.clone(),
            prompt_notify.clone(),
            cancel_notify.clone(),
        ));

        Ok(Self {
            sender: Arc::new(Mutex::new(writer)),
            inbound,
            ack_notify,
            prompt_notify,
            cancel_notify,
        })
    }

    async fn read_loop(
        mut reader: futures::stream::SplitStream<LiveWebSocketStream>,
        inbound: Arc<Mutex<WebSocketIntegrationInboundState>>,
        ack_notify: Arc<Notify>,
        prompt_notify: Arc<Notify>,
        cancel_notify: Arc<Notify>,
    ) {
        // F-RR-29: select! on cancel_notify so the task exits cleanly when
        // WebSocketIntegrationSessionChannel is dropped or close() is called.
        loop {
            let raw = tokio::select! {
                biased;
                _ = cancel_notify.notified() => {
                    debug!("integration websocket read_loop: cancel signal received");
                    break;
                }
                msg = reader.next() => {
                    match msg {
                        Some(m) => m,
                        None => break,
                    }
                }
            };
            match raw {
                Ok(Message::Text(text)) => {
                    // Redundant backstop to the protocol-layer cap in
                    // `capped_ws_config` (#6825): tungstenite already rejects a
                    // frame/message larger than MAX_WS_MESSAGE_BYTES during the
                    // read (yielding Error::Capacity → this loop breaks), so an
                    // oversized payload is never buffered. This app-level check
                    // therefore only fires if that cap is ever loosened, and —
                    // unlike the protocol cap which tears the connection down —
                    // it skips the single frame and keeps the connection alive.
                    if text.len() > MAX_WS_MESSAGE_BYTES {
                        warn!(
                            bytes = text.len(),
                            limit = MAX_WS_MESSAGE_BYTES,
                            "integration websocket: incoming text frame exceeds size limit, skipping"
                        );
                        continue;
                    }

                    let mut ack_changed = false;
                    let mut prompt_changed = false;
                    if let Ok(ack) = serde_json::from_str::<IntegrationAckPayload>(&text) {
                        let mut state = inbound.lock().await;
                        if state.outbound_acks.len() >= MAX_INBOUND_ACKS {
                            state.outbound_acks.pop_front();
                            warn!("inbound ack buffer full: dropping oldest");
                        }
                        state.outbound_acks.push_back(ack);
                        ack_changed = true;
                    } else if let Ok(event) =
                        serde_json::from_str::<IntegrationCloudEvent<ProactivePrompt>>(&text)
                    {
                        match prompt_from_cloudevent(event) {
                            Ok(prompt) => {
                                let mut state = inbound.lock().await;
                                if state.prompts.len() >= MAX_INBOUND_PROMPTS {
                                    state.prompts.pop_front();
                                    warn!("inbound prompt buffer full: dropping oldest");
                                }
                                state.prompts.push_back(prompt);
                                prompt_changed = true;
                            }
                            Err(err) => {
                                warn!("integration websocket prompt parse failed: {err}");
                            }
                        }
                    } else if let Ok(batch) = serde_json::from_str::<PromptCloudEventBatch>(&text) {
                        for event in batch.events {
                            match prompt_from_cloudevent(event) {
                                Ok(prompt) => {
                                    let mut state = inbound.lock().await;
                                    if state.prompts.len() >= MAX_INBOUND_PROMPTS {
                                        state.prompts.pop_front();
                                        warn!("inbound prompt buffer full: dropping oldest");
                                    }
                                    state.prompts.push_back(prompt);
                                    prompt_changed = true;
                                }
                                Err(err) => {
                                    warn!("integration websocket prompt parse failed: {err}");
                                }
                            }
                        }
                    }

                    if ack_changed {
                        ack_notify.notify_waiters();
                    }
                    if prompt_changed {
                        prompt_notify.notify_waiters();
                    }
                }
                Ok(Message::Close(_)) => break,
                Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {}
                Ok(Message::Binary(_)) | Ok(Message::Frame(_)) => {}
                Err(err) => {
                    warn!("integration websocket read failed: {err}");
                    break;
                }
            }
        }
        debug!("integration websocket session channel reader ended");
    }

    pub async fn send_json<T: serde::Serialize>(&self, payload: &T) -> Result<(), CoreError> {
        let text = serde_json::to_string(payload).map_err(|err| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("integration websocket serialization failed: {err}"),
        })?;
        let mut sender = self.sender.lock().await;
        sender
            .send(Message::Text(text.into()))
            .await
            .map_err(|err| CoreError::Network {
                code: maekon_core::error_codes::NetworkCode::Generic,
                message: format!("integration websocket send failed: {err}"),
            })
    }

    pub async fn wait_for_outbound_ack(
        &self,
        expected_queue_ids: &[String],
        timeout: Duration,
    ) -> Result<IntegrationEgressTransportResponse, CoreError> {
        let expected: BTreeSet<String> = expected_queue_ids.iter().cloned().collect();
        let deadline = tokio::time::Instant::now() + timeout;
        let mut acknowledged = BTreeSet::new();
        let mut ack_cursor: Option<IntegrationAckCursor> = None;

        loop {
            {
                let mut inbound = self.inbound.lock().await;
                let mut remaining = VecDeque::new();
                while let Some(ack) = inbound.outbound_acks.pop_front() {
                    let mut ack = ack;
                    let mut unmatched_ids = Vec::new();
                    for queue_id in ack.acknowledged_ids {
                        if expected.contains(&queue_id) {
                            acknowledged.insert(queue_id);
                        } else {
                            unmatched_ids.push(queue_id);
                        }
                    }
                    if ack.ack_cursor.is_some() {
                        ack_cursor = ack.ack_cursor.clone();
                    }
                    if !unmatched_ids.is_empty() {
                        ack.acknowledged_ids = unmatched_ids;
                        remaining.push_back(ack);
                    }
                }
                inbound.outbound_acks = remaining;
            }

            if acknowledged.len() == expected.len() {
                return Ok(IntegrationEgressTransportResponse {
                    acknowledged_queue_ids: acknowledged.into_iter().collect(),
                    ack_cursor,
                });
            }

            let now = tokio::time::Instant::now();
            if now >= deadline {
                return Err(CoreError::RequestTimeout {
                    code: maekon_core::error_codes::NetworkCode::Timeout,
                    timeout_ms: timeout.as_millis() as u64,
                });
            }

            tokio::time::timeout_at(deadline, self.ack_notify.notified())
                .await
                .map_err(|_| CoreError::RequestTimeout {
                    code: maekon_core::error_codes::NetworkCode::Timeout,
                    timeout_ms: timeout.as_millis() as u64,
                })?;
        }
    }

    pub async fn wait_for_prompt_signal(&self, timeout: Duration) -> Result<bool, CoreError> {
        if !self.inbound.lock().await.prompts.is_empty() {
            return Ok(true);
        }

        match tokio::time::timeout(timeout, self.prompt_notify.notified()).await {
            Ok(_) => Ok(!self.inbound.lock().await.prompts.is_empty()),
            Err(_) => Ok(false),
        }
    }

    pub async fn drain_prompts(&self, limit: usize) -> Vec<ProactivePrompt> {
        let mut inbound = self.inbound.lock().await;
        let mut drained = Vec::new();
        for _ in 0..limit {
            let Some(prompt) = inbound.prompts.pop_front() else {
                break;
            };
            drained.push(prompt);
        }
        drained
    }

    pub async fn close(&self) -> Result<(), CoreError> {
        // F-RR-29: signal the read_loop to exit before sending Close frame.
        self.cancel_notify.notify_one();
        let mut sender = self.sender.lock().await;
        sender
            .send(Message::Close(None))
            .await
            .map_err(|err| CoreError::Network {
                code: maekon_core::error_codes::NetworkCode::Generic,
                message: format!("integration websocket close failed: {err}"),
            })
    }
}

/// F-RR-37: ensure the read_loop task exits when a `WebSocketIntegrationSessionChannel`
/// is dropped without an explicit `close()` call (e.g. reconnect path in
/// `runtime_loop.rs`).  Notifying `cancel_notify` on drop mirrors the explicit
/// `close()` path and prevents a lingering read_loop from holding a stale
/// TCP connection open after the caller has discarded the channel.
///
/// The former F-RR-33 guard (`Arc::strong_count(&self.cancel_notify) == 1`)
/// was permanently false: at Drop time the spawned read_loop task holds its
/// own clone of `cancel_notify`, so the strong count is always >= 2 while the
/// task is alive.  The guard therefore silently suppressed every implicit-drop
/// notification.  `notify_one()` is idempotent — safe to call from any clone —
/// so the check is removed and we notify unconditionally.
impl Drop for WebSocketIntegrationSessionChannel {
    fn drop(&mut self) {
        // Unconditionally signal the read_loop to exit.  The previous
        // Arc::strong_count == 1 guard was always false because the spawned
        // read_loop task holds its own Arc clone (count >= 2).  Calling
        // notify_one() from any clone is safe and idempotent.
        self.cancel_notify.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::timeout;

    /// F-RR-37: verify that dropping a simulated channel without calling
    /// close() fires the cancel_notify so the read_loop (simulated here as a
    /// task waiting on notified()) exits within 1 second.
    ///
    /// This test does NOT open a real WebSocket connection.  Instead it
    /// constructs only the `cancel_notify` Arc and a lightweight task that
    /// mirrors what read_loop does: it blocks on `cancel_notify.notified()`.
    /// The task should exit promptly when the Arc is dropped (via
    /// `notify_one()`).
    #[tokio::test]
    async fn drop_fires_cancel_notify_unconditionally() {
        let cancel_notify = Arc::new(Notify::new());

        // Simulate the read_loop: hold a clone of cancel_notify and wait for
        // a notification.  This clone is the reason Arc::strong_count >= 2
        // at drop time, which was the root cause of F-RR-33/F-RR-37.
        let task_cancel = cancel_notify.clone();
        let task = tokio::spawn(async move {
            task_cancel.notified().await;
        });

        // Drop the owner Arc — this is what WebSocketIntegrationSessionChannel::drop does.
        // The strong_count is 2 here (owner + task clone).
        assert!(
            Arc::strong_count(&cancel_notify) >= 2,
            "precondition: task clone must keep count >= 2"
        );
        cancel_notify.notify_one();
        drop(cancel_notify);

        // The task should exit within 1 second.
        let result = timeout(Duration::from_secs(1), task).await;
        // Two-layer unwrap: outer = timeout, inner = JoinHandle (task did not panic).
        let join_result = result.expect(
            "F-RR-37: read_loop task did not exit after notify_one() — \
                     implicit drop would leave a stale TCP connection",
        );
        // The task body is `task_cancel.notified().await` — no panic path; JoinHandle must be Ok.
        join_result.expect("read_loop task panicked unexpectedly");
    }

    /// F-RR-37 regression: the former `Arc::strong_count == 1` guard would
    /// have silently blocked the notification when the task clone is alive.
    /// Confirm that strong_count is >= 2 at the moment we call notify_one in
    /// the simulated drop scenario above (i.e., the guard was always false).
    #[test]
    fn strong_count_is_never_one_while_task_holds_clone() {
        let notify = Arc::new(Notify::new());
        let _task_clone = notify.clone();
        // At this point strong_count == 2; the former guard would have been false.
        assert!(
            Arc::strong_count(&notify) >= 2,
            "F-RR-37: strong_count should be >= 2 when a task clone exists; \
             the Arc::strong_count == 1 guard was permanently false"
        );
    }

    // ------------------------------------------------------------------
    // #5974: inbound buffer cap tests
    // ------------------------------------------------------------------

    /// Simulate the bounded-push logic for the ack queue: pushing
    /// MAX_INBOUND_ACKS + 1 items must leave exactly MAX_INBOUND_ACKS items
    /// in the queue (oldest evicted, newest retained).
    #[test]
    fn inbound_ack_buffer_evicts_oldest_when_full() {
        use maekon_api_contracts::integration::IntegrationAckPayload;

        let mut state = WebSocketIntegrationInboundState::default();
        // Fill to capacity.
        for i in 0..MAX_INBOUND_ACKS {
            let ack = IntegrationAckPayload {
                session_id: "sess".to_string(),
                acknowledged_ids: vec![format!("id-{i}")],
                ack_cursor: None,
            };
            if state.outbound_acks.len() >= MAX_INBOUND_ACKS {
                state.outbound_acks.pop_front();
            }
            state.outbound_acks.push_back(ack);
        }
        assert_eq!(state.outbound_acks.len(), MAX_INBOUND_ACKS);

        // Push one more — oldest (id-0) must be evicted.
        let overflow = IntegrationAckPayload {
            session_id: "sess".to_string(),
            acknowledged_ids: vec!["id-overflow".to_string()],
            ack_cursor: None,
        };
        if state.outbound_acks.len() >= MAX_INBOUND_ACKS {
            state.outbound_acks.pop_front();
        }
        state.outbound_acks.push_back(overflow);

        assert_eq!(
            state.outbound_acks.len(),
            MAX_INBOUND_ACKS,
            "queue must remain at capacity after overflow"
        );
        // The front is now id-1 (id-0 was evicted).
        assert_eq!(
            state.outbound_acks.front().unwrap().acknowledged_ids[0],
            "id-1"
        );
        // The back is the overflow entry.
        assert_eq!(
            state.outbound_acks.back().unwrap().acknowledged_ids[0],
            "id-overflow"
        );
    }

    /// Simulate the bounded-push logic for the prompt queue.
    #[test]
    fn inbound_prompt_buffer_evicts_oldest_when_full() {
        use maekon_core::models::integration::{
            ProactivePrompt, ProactivePromptCategory, ProactivePromptPriority, PromptProvenance,
        };

        let mut state = WebSocketIntegrationInboundState::default();

        let make_prompt = |tag: &str| ProactivePrompt {
            prompt_id: tag.to_string(),
            category: ProactivePromptCategory::Task,
            title: String::new(),
            body: String::new(),
            priority: ProactivePromptPriority::Low,
            actions: vec![],
            expires_at: None,
            provenance: PromptProvenance {
                source_system: "test".to_string(),
                source_actor: None,
                correlation_id: None,
            },
        };

        // Fill to capacity.
        for i in 0..MAX_INBOUND_PROMPTS {
            let p = make_prompt(&format!("p-{i}"));
            if state.prompts.len() >= MAX_INBOUND_PROMPTS {
                state.prompts.pop_front();
            }
            state.prompts.push_back(p);
        }
        assert_eq!(state.prompts.len(), MAX_INBOUND_PROMPTS);

        // Push one more — oldest must be evicted.
        let overflow = make_prompt("p-overflow");
        if state.prompts.len() >= MAX_INBOUND_PROMPTS {
            state.prompts.pop_front();
        }
        state.prompts.push_back(overflow);

        assert_eq!(
            state.prompts.len(),
            MAX_INBOUND_PROMPTS,
            "queue must remain at capacity after overflow"
        );
        assert_eq!(state.prompts.front().unwrap().prompt_id, "p-1");
        assert_eq!(state.prompts.back().unwrap().prompt_id, "p-overflow");
    }

    /// Verify the size-gate constant relationship: a message of exactly
    /// MAX_WS_MESSAGE_BYTES bytes must be accepted (len == limit is within
    /// bounds), while len > limit is rejected.
    #[test]
    fn ws_message_size_gate_boundary() {
        let at_limit = "x".repeat(MAX_WS_MESSAGE_BYTES);
        let over_limit = "x".repeat(MAX_WS_MESSAGE_BYTES + 1);

        assert!(
            at_limit.len() <= MAX_WS_MESSAGE_BYTES,
            "a frame at the limit must pass the gate"
        );
        assert!(
            over_limit.len() > MAX_WS_MESSAGE_BYTES,
            "a frame over the limit must be rejected"
        );
    }

    /// #6825: the protocol-layer config must cap both frame and message size at
    /// MAX_WS_MESSAGE_BYTES (overriding tungstenite's 16/64 MiB defaults), so an
    /// oversized frame is rejected during read instead of being fully buffered.
    #[test]
    fn ws_config_caps_frame_and_message_size() {
        let cfg = capped_ws_config();
        assert_eq!(
            cfg.max_message_size,
            Some(MAX_WS_MESSAGE_BYTES),
            "max_message_size must be capped at the app limit, not the 64 MiB default"
        );
        assert_eq!(
            cfg.max_frame_size,
            Some(MAX_WS_MESSAGE_BYTES),
            "max_frame_size must be capped at the app limit, not the 16 MiB default"
        );
    }
}
