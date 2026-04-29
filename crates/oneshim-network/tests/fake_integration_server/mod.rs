use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{header, HeaderValue, StatusCode},
    response::IntoResponse,
    routing::{delete, post},
    Json, Router,
};
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use oneshim_api_contracts::integration::{
    IntegrationAckPayload, IntegrationBootstrapRequest, IntegrationBootstrapResponse,
};
use oneshim_core::models::integration::{
    IntegrationAckCursor, IntegrationAuthScheme, IntegrationTransportKind, ProactivePrompt,
    ProactivePromptCategory, ProactivePromptPriority, PromptProvenance,
};
use oneshim_network::integration::{
    IntegrationCloudEvent, IntegrationOutboundCloudEventBatch, PromptCloudEventBatch,
};
use parking_lot::Mutex;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::Message;
use tracing::debug;

const SESSION_ID: &str = "session-fake-001";
const PROMPT_EVENT_TYPE: &str = "io.oneshim.integration.prompt.v1";

#[derive(Debug, Clone)]
pub struct FakeIntegrationServerSnapshot {
    pub bootstrap_requests: Vec<IntegrationBootstrapRequest>,
    pub egress_batches: Vec<IntegrationOutboundCloudEventBatch>,
    pub heartbeat_session_ids: Vec<String>,
    pub disconnect_session_ids: Vec<String>,
    pub live_messages: Vec<String>,
    pub live_headers: Vec<(String, String)>,
}

#[derive(Default)]
struct FakeIntegrationServerState {
    base_url: Mutex<String>,
    live_channel_url: Mutex<Option<String>>,
    bootstrap_requests: Mutex<Vec<IntegrationBootstrapRequest>>,
    egress_batches: Mutex<Vec<IntegrationOutboundCloudEventBatch>>,
    heartbeat_session_ids: Mutex<Vec<String>>,
    disconnect_session_ids: Mutex<Vec<String>>,
    prompt_events: Mutex<Vec<IntegrationCloudEvent<ProactivePrompt>>>,
    selected_transport: Mutex<IntegrationTransportKind>,
    selected_auth_scheme: Mutex<IntegrationAuthScheme>,
    websocket_auto_ack_outbound: Mutex<bool>,
    websocket_ack_limit: Mutex<Option<usize>>,
    websocket_close_after_messages: Mutex<Option<usize>>,
    websocket_seen_messages: Mutex<usize>,
    live_messages: Mutex<Vec<String>>,
    live_headers: Mutex<Vec<(String, String)>>,
    live_pending_outbound: Mutex<Vec<String>>,
    live_outbound_txs: Mutex<Vec<mpsc::UnboundedSender<String>>>,
    egress_partial_ack_limit: Mutex<Option<usize>>,
    egress_rate_limit_once_secs: Mutex<Option<u64>>,
}

pub struct FakeIntegrationServer {
    base_url: String,
    state: Arc<FakeIntegrationServerState>,
    http_shutdown_tx: Option<oneshot::Sender<()>>,
    websocket_shutdown_tx: Option<oneshot::Sender<()>>,
}

impl FakeIntegrationServer {
    pub async fn start() -> Self {
        let state = Arc::new(FakeIntegrationServerState::default());
        *state.selected_transport.lock() = IntegrationTransportKind::HttpsLongPoll;
        *state.selected_auth_scheme.lock() = IntegrationAuthScheme::BearerToken;
        let http_listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("failed to bind fake integration server");
        let http_address = http_listener.local_addr().expect("fake server local addr");
        let base_url = format!("http://{}", http_address);
        *state.base_url.lock() = base_url.clone();

        let websocket_listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("failed to bind fake integration websocket server");
        let websocket_address = websocket_listener
            .local_addr()
            .expect("fake websocket local addr");
        *state.live_channel_url.lock() = Some(format!(
            "ws://{websocket_address}/integration/session-control"
        ));

        let router = create_router(state.clone());
        let (http_shutdown_tx, http_shutdown_rx) = oneshot::channel();
        let (websocket_shutdown_tx, mut websocket_shutdown_rx) = oneshot::channel();

        tokio::spawn(async move {
            axum::serve(http_listener, router)
                .with_graceful_shutdown(async {
                    if let Err(e) = http_shutdown_rx.await {
                        debug!("operation failed: {e}");
                    }
                })
                .await
                .expect("fake integration server run failed");
        });

        let websocket_state = state.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut websocket_shutdown_rx => break,
                    accept = websocket_listener.accept() => {
                        let Ok((stream, _)) = accept else {
                            break;
                        };
                        let state = websocket_state.clone();
                        tokio::spawn(async move {
                            handle_websocket_connection(stream, state).await;
                        });
                    }
                }
            }
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(30)).await;

        Self {
            base_url,
            state,
            http_shutdown_tx: Some(http_shutdown_tx),
            websocket_shutdown_tx: Some(websocket_shutdown_tx),
        }
    }

    pub fn bootstrap_url(&self) -> String {
        format!("{}/integration/bootstrap", self.base_url)
    }

    pub fn push_prompt(&self, prompt: ProactivePrompt) {
        self.state
            .prompt_events
            .lock()
            .push(prompt_to_cloudevent(prompt));
    }

    pub fn enable_websocket_transport(&self, auto_ack_outbound: bool) {
        *self.state.selected_transport.lock() = IntegrationTransportKind::WebSocket;
        *self.state.websocket_auto_ack_outbound.lock() = auto_ack_outbound;
    }

    pub fn set_selected_auth_scheme(&self, scheme: IntegrationAuthScheme) {
        *self.state.selected_auth_scheme.lock() = scheme;
    }

    pub fn set_websocket_ack_limit(&self, limit: usize) {
        *self.state.websocket_ack_limit.lock() = Some(limit);
    }

    pub fn set_websocket_close_after_messages(&self, count: usize) {
        *self.state.websocket_close_after_messages.lock() = Some(count);
    }

    pub fn push_live_prompt(&self, prompt: ProactivePrompt) {
        self.push_live_json(
            serde_json::to_string(&prompt_to_cloudevent(prompt))
                .expect("serialize fake integration live prompt"),
        );
    }

    pub fn push_live_prompt_batch(&self, prompts: Vec<ProactivePrompt>) {
        let batch = PromptCloudEventBatch {
            events: prompts.into_iter().map(prompt_to_cloudevent).collect(),
        };
        self.push_live_json(
            serde_json::to_string(&batch).expect("serialize fake integration live prompt batch"),
        );
    }

    pub fn push_live_raw(&self, payload: impl Into<String>) {
        self.push_live_json(payload.into());
    }

    pub fn set_egress_partial_ack_limit(&self, limit: usize) {
        *self.state.egress_partial_ack_limit.lock() = Some(limit);
    }

    pub fn set_egress_rate_limit_once(&self, retry_after_secs: u64) {
        *self.state.egress_rate_limit_once_secs.lock() = Some(retry_after_secs);
    }

    pub fn snapshot(&self) -> FakeIntegrationServerSnapshot {
        FakeIntegrationServerSnapshot {
            bootstrap_requests: self.state.bootstrap_requests.lock().clone(),
            egress_batches: self.state.egress_batches.lock().clone(),
            heartbeat_session_ids: self.state.heartbeat_session_ids.lock().clone(),
            disconnect_session_ids: self.state.disconnect_session_ids.lock().clone(),
            live_messages: self.state.live_messages.lock().clone(),
            live_headers: self.state.live_headers.lock().clone(),
        }
    }

    fn push_live_json(&self, payload: String) {
        let mut senders = self.state.live_outbound_txs.lock();
        if senders.is_empty() {
            self.state.live_pending_outbound.lock().push(payload);
            return;
        }

        let mut delivered = false;
        senders.retain(|tx| {
            if tx.send(payload.clone()).is_ok() {
                delivered = true;
                true
            } else {
                false
            }
        });

        if !delivered {
            self.state.live_pending_outbound.lock().push(payload);
        }
    }
}

impl Drop for FakeIntegrationServer {
    fn drop(&mut self) {
        if let Some(tx) = self.http_shutdown_tx.take() {
            if let Err(e) = tx.send(()) {
                debug!("channel send failed: {e:?}");
            }
        }
        if let Some(tx) = self.websocket_shutdown_tx.take() {
            if let Err(e) = tx.send(()) {
                debug!("channel send failed: {e:?}");
            }
        }
    }
}

#[derive(serde::Deserialize)]
struct PromptPullRequest {
    #[serde(default)]
    limit: usize,
}

#[derive(serde::Serialize)]
struct PromptPullResponse {
    events: Vec<IntegrationCloudEvent<ProactivePrompt>>,
    ack_cursor: Option<IntegrationAckCursor>,
}

fn create_router(state: Arc<FakeIntegrationServerState>) -> Router {
    Router::new()
        .route("/integration/bootstrap", post(handle_bootstrap))
        .route(
            "/integration/sessions/{session_id}/heartbeat",
            post(handle_heartbeat),
        )
        .route(
            "/integration/sessions/{session_id}",
            delete(handle_disconnect),
        )
        .route(
            "/integration/sessions/{session_id}/events",
            post(handle_events),
        )
        .route(
            "/integration/sessions/{session_id}/prompts",
            post(handle_prompts),
        )
        .with_state(state)
}

async fn handle_bootstrap(
    State(state): State<Arc<FakeIntegrationServerState>>,
    Json(request): Json<IntegrationBootstrapRequest>,
) -> Json<IntegrationBootstrapResponse> {
    state.bootstrap_requests.lock().push(request.clone());
    let selected_transport = state.selected_transport.lock().clone();
    let selected_auth_scheme = state.selected_auth_scheme.lock().clone();
    let base_url = state.base_url.lock().clone();
    let channel_url = state.live_channel_url.lock().clone();
    Json(IntegrationBootstrapResponse {
        schema_version: "integration.bootstrap.v1".to_string(),
        supported_scopes: request.requested_scopes.clone(),
        granted_scopes: request.requested_scopes.clone(),
        supported_transports: vec![selected_transport.clone()],
        selected_transport: Some(selected_transport.clone()),
        supported_auth_schemes: vec![selected_auth_scheme.clone()],
        selected_auth_scheme: Some(selected_auth_scheme),
        resource_indicator: request.resource_indicator.clone(),
        session_required: true,
        session: Some(
            oneshim_api_contracts::integration::IntegrationBootstrapSessionBinding {
                session_id: SESSION_ID.to_string(),
                channel_url: if selected_transport == IntegrationTransportKind::WebSocket {
                    Some(channel_url.expect("fake integration websocket URL"))
                } else {
                    None
                },
                heartbeat_url: (selected_transport == IntegrationTransportKind::HttpsLongPoll)
                    .then_some(format!(
                        "{base_url}/integration/sessions/{SESSION_ID}/heartbeat"
                    )),
                disconnect_url: (selected_transport == IntegrationTransportKind::HttpsLongPoll)
                    .then_some(format!("{base_url}/integration/sessions/{SESSION_ID}")),
                send_events_url: (selected_transport == IntegrationTransportKind::HttpsLongPoll)
                    .then_some(format!(
                        "{base_url}/integration/sessions/{SESSION_ID}/events"
                    )),
                receive_prompts_url: (selected_transport
                    == IntegrationTransportKind::HttpsLongPoll)
                    .then_some(format!(
                        "{base_url}/integration/sessions/{SESSION_ID}/prompts"
                    )),
            },
        ),
    })
}

async fn handle_heartbeat(
    State(state): State<Arc<FakeIntegrationServerState>>,
    Path(session_id): Path<String>,
) -> StatusCode {
    state.heartbeat_session_ids.lock().push(session_id);
    StatusCode::NO_CONTENT
}

async fn handle_disconnect(
    State(state): State<Arc<FakeIntegrationServerState>>,
    Path(session_id): Path<String>,
) -> StatusCode {
    state.disconnect_session_ids.lock().push(session_id);
    StatusCode::NO_CONTENT
}

async fn handle_events(
    State(state): State<Arc<FakeIntegrationServerState>>,
    Path(_session_id): Path<String>,
    Json(batch): Json<IntegrationOutboundCloudEventBatch>,
) -> impl IntoResponse {
    if let Some(retry_after_secs) = state.egress_rate_limit_once_secs.lock().take() {
        let headers = [(
            header::RETRY_AFTER,
            HeaderValue::from_str(&retry_after_secs.to_string())
                .unwrap_or_else(|_| HeaderValue::from_static("1")),
        )];
        return (
            StatusCode::TOO_MANY_REQUESTS,
            headers,
            Json(serde_json::json!({
                "error": "rate_limited",
                "retry_after_secs": retry_after_secs
            })),
        )
            .into_response();
    }

    let partial_ack_limit = *state.egress_partial_ack_limit.lock();
    let accepted_ids = batch
        .items
        .iter()
        .take(partial_ack_limit.unwrap_or(batch.items.len()))
        .map(|item| item.queue_id.clone())
        .collect::<Vec<_>>();
    state.egress_batches.lock().push(batch);
    Json(serde_json::json!({
        "accepted_ids": accepted_ids,
        "ack_cursor": {
            "stream_id": "integration.egress",
            "cursor": "ack-egress-001",
            "acknowledged_at": Utc::now(),
        }
    }))
    .into_response()
}

async fn handle_prompts(
    State(state): State<Arc<FakeIntegrationServerState>>,
    Path(_session_id): Path<String>,
    Json(request): Json<PromptPullRequest>,
) -> Json<PromptPullResponse> {
    let limit = if request.limit == 0 {
        usize::MAX
    } else {
        request.limit
    };
    let events = state
        .prompt_events
        .lock()
        .iter()
        .take(limit)
        .cloned()
        .collect::<Vec<_>>();
    Json(PromptPullResponse {
        events,
        ack_cursor: Some(IntegrationAckCursor {
            stream_id: "integration.prompts".to_string(),
            cursor: "ack-prompts-001".to_string(),
            acknowledged_at: Utc::now(),
        }),
    })
}

#[allow(clippy::result_large_err)]
async fn handle_websocket_connection(
    stream: tokio::net::TcpStream,
    state: Arc<FakeIntegrationServerState>,
) {
    let state_for_headers = state.clone();
    let websocket = accept_hdr_async(stream, move |request: &Request, response: Response| {
        if let Some(value) = request.headers().get("authorization") {
            state_for_headers.live_headers.lock().push((
                "authorization".to_string(),
                value.to_str().unwrap_or_default().to_string(),
            ));
        }
        if let Some(value) = request.headers().get("dpop") {
            state_for_headers.live_headers.lock().push((
                "dpop".to_string(),
                value.to_str().unwrap_or_default().to_string(),
            ));
        }
        Ok(response)
    })
    .await
    .expect("accept fake integration websocket");

    let (mut writer, mut reader) = websocket.split();
    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel::<String>();
    state.live_outbound_txs.lock().push(outbound_tx);
    *state.websocket_seen_messages.lock() = 0;

    let pending_outbound = state
        .live_pending_outbound
        .lock()
        .drain(..)
        .collect::<Vec<_>>();
    for payload in pending_outbound {
        writer
            .send(Message::Text(payload.into()))
            .await
            .expect("send pending fake integration websocket payload");
    }

    loop {
        let maybe_message = tokio::select! {
            maybe_outbound = outbound_rx.recv() => {
                match maybe_outbound {
                    Some(payload) => {
                        writer
                            .send(Message::Text(payload.into()))
                            .await
                            .expect("send fake integration websocket payload");
                        continue;
                    }
                    None => None,
                }
            }
            maybe_message = reader.next() => maybe_message,
        };

        let Some(message) = maybe_message else {
            break;
        };

        match message.expect("read fake integration websocket message") {
            Message::Text(text) => {
                let text = text.to_string();
                state.live_messages.lock().push(text.clone());
                let seen = {
                    let mut seen = state.websocket_seen_messages.lock();
                    *seen += 1;
                    *seen
                };

                if *state.websocket_auto_ack_outbound.lock() {
                    if let Ok(event) =
                        serde_json::from_str::<IntegrationCloudEvent<serde_json::Value>>(&text)
                    {
                        if let Some(queue_id) = event.oneshimqueueid {
                            let ack_limit = *state.websocket_ack_limit.lock();
                            if ack_limit.map(|limit| seen <= limit).unwrap_or(true) {
                                let ack = IntegrationAckPayload {
                                    session_id: event
                                        .oneshimsessionid
                                        .clone()
                                        .unwrap_or_else(|| SESSION_ID.to_string()),
                                    acknowledged_ids: vec![queue_id],
                                    ack_cursor: Some(IntegrationAckCursor {
                                        stream_id: "integration.egress".to_string(),
                                        cursor: "ack-egress-live-001".to_string(),
                                        acknowledged_at: Utc::now(),
                                    }),
                                };
                                writer
                                    .send(Message::Text(
                                        serde_json::to_string(&ack)
                                            .expect("serialize fake websocket ack")
                                            .into(),
                                    ))
                                    .await
                                    .expect("send fake websocket ack");
                            }
                        }
                    }
                }

                if state
                    .websocket_close_after_messages
                    .lock()
                    .is_some_and(|count| seen >= count)
                {
                    writer.send(Message::Close(None)).await.ok();
                    break;
                }
            }
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) => {}
            Message::Binary(_) | Message::Frame(_) => {}
        }
    }

    state.live_outbound_txs.lock().retain(|tx| !tx.is_closed());
}

fn prompt_to_cloudevent(prompt: ProactivePrompt) -> IntegrationCloudEvent<ProactivePrompt> {
    IntegrationCloudEvent {
        specversion: "1.0".to_string(),
        id: format!("env-{}", prompt.prompt_id),
        source: "oneshim://systems/fake-integration-server".to_string(),
        event_type: PROMPT_EVENT_TYPE.to_string(),
        subject: prompt.prompt_id.clone(),
        time: Utc::now(),
        datacontenttype: "application/json".to_string(),
        data: prompt.clone(),
        dataschema: Some("integration.prompt.v1".to_string()),
        oneshimscope: "prompt:read".to_string(),
        oneshimnonce: format!("nonce-{}", prompt.prompt_id),
        oneshimsessionid: Some(SESSION_ID.to_string()),
        oneshimworkspaceid: None,
        oneshimprivacy: None,
        oneshimpromptcategory: Some(prompt_category_slug(&prompt.category).to_string()),
        oneshimqueueid: None,
    }
}

fn prompt_category_slug(category: &ProactivePromptCategory) -> &'static str {
    match category {
        ProactivePromptCategory::Reminder => "reminder",
        ProactivePromptCategory::Task => "task",
        ProactivePromptCategory::Insight => "insight",
        ProactivePromptCategory::Escalation => "escalation",
    }
}

pub fn sample_prompt(prompt_id: &str) -> ProactivePrompt {
    ProactivePrompt {
        prompt_id: prompt_id.to_string(),
        category: ProactivePromptCategory::Task,
        title: "Review integration contract".to_string(),
        body: "Validate the fake server compatibility flow.".to_string(),
        priority: ProactivePromptPriority::Medium,
        actions: Vec::new(),
        expires_at: None,
        provenance: PromptProvenance {
            source_system: "fake-integration-server".to_string(),
            source_actor: Some("compatibility-suite".to_string()),
            correlation_id: Some("corr-compat-001".to_string()),
        },
    }
}
