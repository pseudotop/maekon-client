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

use oneshim_api_contracts::integration::IntegrationAckPayload;
use oneshim_core::error::CoreError;
use oneshim_core::models::integration::{IntegrationAckCursor, ProactivePrompt};

use super::cloudevents::{IntegrationCloudEvent, PromptCloudEventBatch};
use super::prompt_from_cloudevent;
use super::transport::IntegrationEgressTransportResponse;

type LiveWebSocketStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

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
}

impl WebSocketIntegrationSessionChannel {
    pub async fn connect(url: &str, headers: HeaderMap) -> Result<Self, CoreError> {
        let mut request = url
            .into_client_request()
            .map_err(|err| CoreError::Validation {
                code: oneshim_core::error_codes::ValidationCode::InvalidField,
                field: "integration.session.channel_url".to_string(),
                message: format!("invalid websocket URL: {err}"),
            })?;

        for (name, value) in headers.iter() {
            request.headers_mut().insert(name, value.clone());
        }

        let (stream, _) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|err| CoreError::Network {
                code: oneshim_core::error_codes::NetworkCode::Generic,
                message: format!("integration websocket connect failed: {err}"),
            })?;
        let (writer, reader) = stream.split();
        let inbound = Arc::new(Mutex::new(WebSocketIntegrationInboundState::default()));
        let ack_notify = Arc::new(Notify::new());
        let prompt_notify = Arc::new(Notify::new());

        tokio::spawn(Self::read_loop(
            reader,
            inbound.clone(),
            ack_notify.clone(),
            prompt_notify.clone(),
        ));

        Ok(Self {
            sender: Arc::new(Mutex::new(writer)),
            inbound,
            ack_notify,
            prompt_notify,
        })
    }

    async fn read_loop(
        mut reader: futures::stream::SplitStream<LiveWebSocketStream>,
        inbound: Arc<Mutex<WebSocketIntegrationInboundState>>,
        ack_notify: Arc<Notify>,
        prompt_notify: Arc<Notify>,
    ) {
        while let Some(message) = reader.next().await {
            match message {
                Ok(Message::Text(text)) => {
                    let mut ack_changed = false;
                    let mut prompt_changed = false;
                    if let Ok(ack) = serde_json::from_str::<IntegrationAckPayload>(&text) {
                        inbound.lock().await.outbound_acks.push_back(ack);
                        ack_changed = true;
                    } else if let Ok(event) =
                        serde_json::from_str::<IntegrationCloudEvent<ProactivePrompt>>(&text)
                    {
                        match prompt_from_cloudevent(event) {
                            Ok(prompt) => {
                                inbound.lock().await.prompts.push_back(prompt);
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
                                    inbound.lock().await.prompts.push_back(prompt);
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
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("integration websocket serialization failed: {err}"),
        })?;
        let mut sender = self.sender.lock().await;
        sender
            .send(Message::Text(text.into()))
            .await
            .map_err(|err| CoreError::Network {
                code: oneshim_core::error_codes::NetworkCode::Generic,
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
                    code: oneshim_core::error_codes::NetworkCode::Timeout,
                    timeout_ms: timeout.as_millis() as u64,
                });
            }

            tokio::time::timeout_at(deadline, self.ack_notify.notified())
                .await
                .map_err(|_| CoreError::RequestTimeout {
                    code: oneshim_core::error_codes::NetworkCode::Timeout,
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
        let mut sender = self.sender.lock().await;
        sender
            .send(Message::Close(None))
            .await
            .map_err(|err| CoreError::Network {
                code: oneshim_core::error_codes::NetworkCode::Generic,
                message: format!("integration websocket close failed: {err}"),
            })
    }
}
