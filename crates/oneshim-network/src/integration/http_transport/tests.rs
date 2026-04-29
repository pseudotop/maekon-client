use super::*;
use crate::integration::transport::{
    IntegrationEgressTransportClient, IntegrationInboxTransportClient, IntegrationTransportClient,
    IntegrationTransportConnectRequest,
};
use crate::integration::IntegrationRequestProof;
use async_trait::async_trait;
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use mockito::Matcher;
use oneshim_api_contracts::integration::{
    IntegrationSessionDisconnectPayload, IntegrationSessionHeartbeatPayload,
};
use oneshim_core::error::CoreError;
use oneshim_core::models::integration::{
    InsightPacket, InsightSourceWindow, IntegrationAckCursor, IntegrationAuthContext,
    IntegrationAuthScheme, IntegrationCapabilityScope, IntegrationEnvelope, IntegrationMessageType,
    IntegrationOrigin, IntegrationOutboundPayload, IntegrationPrivacyClassification,
    IntegrationTransportKind, ProactivePromptCategory, ProactivePromptPriority,
    QueuedIntegrationEgressMessage,
};
use oneshim_core::ports::integration::IntegrationAuthPort;
use std::sync::{Arc as StdArc, Mutex as StdMutex};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::Message;

struct StaticAuthPort {
    context: IntegrationAuthContext,
}

#[async_trait]
impl IntegrationAuthPort for StaticAuthPort {
    async fn resolve_session_auth(
        &self,
        _requested_scopes: &[IntegrationCapabilityScope],
        _resource_indicator: Option<&str>,
    ) -> Result<IntegrationAuthContext, CoreError> {
        Ok(self.context.clone())
    }

    async fn current_auth_status(
        &self,
    ) -> Result<oneshim_core::models::integration::IntegrationAuthStatus, CoreError> {
        Ok(oneshim_core::models::integration::IntegrationAuthStatus {
            profile_kind: oneshim_core::models::integration::IntegrationAuthProfileKind::EnvToken,
            status: oneshim_core::models::integration::IntegrationAuthStatusKind::Ready,
            interactive: false,
            authenticated: true,
            expires_at: self.context.expires_at,
            resource_indicator: self.context.resource_indicator.clone(),
            pending_flow: None,
            message: None,
        })
    }

    async fn start_device_authorization(
        &self,
        _requested_scopes: &[IntegrationCapabilityScope],
        _resource_indicator: Option<&str>,
    ) -> Result<oneshim_core::models::integration::IntegrationDeviceAuthorizationFlow, CoreError>
    {
        Err(CoreError::InvalidArguments {
            code: oneshim_core::error_codes::ValidationCode::InvalidArguments,
            message: "static auth port does not support device authorization".to_string(),
        })
    }

    async fn poll_device_authorization(
        &self,
        _flow_id: &str,
    ) -> Result<oneshim_core::models::integration::IntegrationAuthStatus, CoreError> {
        self.current_auth_status().await
    }

    async fn cancel_device_authorization(&self, _flow_id: &str) -> Result<(), CoreError> {
        Ok(())
    }

    async fn reset_auth_state(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

struct RecordingProofFactory {
    returned: Option<IntegrationRequestProof>,
    calls: Arc<Mutex<Vec<(String, String)>>>,
}

#[async_trait]
impl IntegrationRequestProofFactory for RecordingProofFactory {
    async fn build_proof(
        &self,
        _auth: &IntegrationAuthContext,
        method: &str,
        url: &str,
    ) -> Result<Option<IntegrationRequestProof>, CoreError> {
        self.calls
            .lock()
            .await
            .push((method.to_string(), url.to_string()));
        Ok(self.returned.clone())
    }
}

fn connect_request(server_url: &str) -> IntegrationTransportConnectRequest {
    IntegrationTransportConnectRequest {
        device_id: "device-001".to_string(),
        client_version: "0.3.8".to_string(),
        device_label: Some("macbook".to_string()),
        requested_scopes: vec![
            IntegrationCapabilityScope::InsightWrite,
            IntegrationCapabilityScope::SessionManage,
        ],
        preferred_transports: vec![
            IntegrationTransportKind::WebSocket,
            IntegrationTransportKind::HttpsLongPoll,
        ],
        supported_auth_schemes: vec![
            IntegrationAuthScheme::DpopBearer,
            IntegrationAuthScheme::BearerToken,
        ],
        resource_indicator: Some(server_url.to_string()),
    }
}

#[allow(clippy::result_large_err)]
async fn start_session_ws_server(
    auto_ack_insights: bool,
) -> (
    String,
    StdArc<StdMutex<Vec<String>>>,
    StdArc<StdMutex<Vec<(String, String)>>>,
    mpsc::UnboundedSender<String>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let messages = StdArc::new(StdMutex::new(Vec::new()));
    let headers = StdArc::new(StdMutex::new(Vec::new()));
    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel::<String>();
    let messages_task = messages.clone();
    let headers_task = headers.clone();

    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let websocket = accept_hdr_async(stream, move |request: &Request, response: Response| {
            if let Some(value) = request.headers().get("authorization") {
                headers_task.lock().unwrap().push((
                    "authorization".to_string(),
                    value.to_str().unwrap_or_default().to_string(),
                ));
            }
            if let Some(value) = request.headers().get("dpop") {
                headers_task.lock().unwrap().push((
                    "dpop".to_string(),
                    value.to_str().unwrap_or_default().to_string(),
                ));
            }
            Ok(response)
        })
        .await
        .unwrap();

        let (mut writer, mut reader) = websocket.split();
        loop {
            tokio::select! {
                maybe_outbound = outbound_rx.recv() => {
                    let Some(outbound) = maybe_outbound else {
                        break;
                    };
                    writer.send(Message::Text(outbound.into())).await.unwrap();
                }
                maybe_message = reader.next() => {
                    let Some(message) = maybe_message else {
                        break;
                    };
                    match message.unwrap() {
                        Message::Text(text) => {
                            let text = text.to_string();
                            messages_task.lock().unwrap().push(text.clone());
                            if auto_ack_insights {
                                if let Ok(event) = serde_json::from_str::<crate::integration::cloudevents::IntegrationCloudEvent<InsightPacket>>(&text) {
                                    if let Some(queue_id) = event.oneshimqueueid {
                                        let ack = serde_json::json!({
                                            "session_id": event.oneshimsessionid.unwrap_or_else(|| "session-test".to_string()),
                                            "acknowledged_ids": [queue_id],
                                        });
                                        writer.send(Message::Text(ack.to_string().into())).await.unwrap();
                                    }
                                }
                            }
                        }
                        Message::Close(_) => break,
                        _ => {}
                    }
                }
            }
        }
    });

    (
        format!("ws://{address}/integration/session-control"),
        messages,
        headers,
        outbound_tx,
    )
}

#[tokio::test]
async fn connect_bootstraps_with_bearer_auth() {
    let mut server = mockito::Server::new_async().await;
    let heartbeat_url = format!(
        "{}/integration/sessions/session-001/heartbeat",
        server.url()
    );
    let disconnect_url = format!("{}/integration/sessions/session-001", server.url());

    let bootstrap = server
        .mock("POST", "/integration/bootstrap")
        .match_header("authorization", "Bearer access-token")
        .match_body(Matcher::PartialJson(serde_json::json!({
            "client_version": "0.3.8",
            "device_id": "device-001"
        })))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "schema_version": "integration.bootstrap.v1",
                "supported_scopes": ["insight:write", "session:manage"],
                "granted_scopes": ["insight:write", "session:manage"],
                "supported_transports": ["https_long_poll"],
                "selected_transport": "https_long_poll",
                "supported_auth_schemes": ["bearer_token"],
                "selected_auth_scheme": "bearer_token",
                "resource_indicator": server.url(),
                "session_required": true,
                "session": {
                    "session_id": "session-001",
                    "heartbeat_url": heartbeat_url,
                    "disconnect_url": disconnect_url
                }
            })
            .to_string(),
        )
        .create_async()
        .await;

    let client = HttpsIntegrationTransportClient::new(
        HttpsIntegrationTransportConfig::new(
            format!("{}/integration/bootstrap", server.url()),
            Duration::from_secs(5),
        ),
        Arc::new(StaticAuthPort {
            context: IntegrationAuthContext {
                access_token: "access-token".to_string(),
                scheme: IntegrationAuthScheme::BearerToken,
                expires_at: None,
                resource_indicator: None,
            },
        }),
        Arc::new(RecordingProofFactory {
            returned: None,
            calls: Arc::new(Mutex::new(Vec::new())),
        }),
    )
    .unwrap();

    let response = client
        .connect(connect_request(&server.url()))
        .await
        .unwrap();
    bootstrap.assert_async().await;
    assert_eq!(response.session_id, "session-001");
    assert_eq!(
        response.transport_kind,
        IntegrationTransportKind::HttpsLongPoll
    );
    assert_eq!(response.auth_scheme, IntegrationAuthScheme::BearerToken);
}

#[tokio::test]
async fn connect_heartbeat_and_disconnect_use_dpop_websocket_control_channel() {
    let mut server = mockito::Server::new_async().await;
    let bootstrap_url = format!("{}/integration/bootstrap", server.url());
    let proof_calls = Arc::new(Mutex::new(Vec::new()));
    let (channel_url, live_messages, live_headers, _outbound_tx) =
        start_session_ws_server(false).await;

    let bootstrap = server
        .mock("POST", "/integration/bootstrap")
        .match_header("authorization", "DPoP access-token")
        .match_header("dpop", "proof-token")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "schema_version": "integration.bootstrap.v1",
                "supported_scopes": ["insight:write", "session:manage"],
                "granted_scopes": ["insight:write", "session:manage"],
                "supported_transports": ["web_socket"],
                "selected_transport": "web_socket",
                "supported_auth_schemes": ["dpop_bearer"],
                "selected_auth_scheme": "dpop_bearer",
                "resource_indicator": server.url(),
                "session_required": true,
                "session": {
                    "session_id": "session-002",
                    "channel_url": channel_url
                }
            })
            .to_string(),
        )
        .create_async()
        .await;

    let client = HttpsIntegrationTransportClient::new(
        HttpsIntegrationTransportConfig::new(bootstrap_url.clone(), Duration::from_secs(5)),
        Arc::new(StaticAuthPort {
            context: IntegrationAuthContext {
                access_token: "access-token".to_string(),
                scheme: IntegrationAuthScheme::DpopBearer,
                expires_at: None,
                resource_indicator: Some(server.url()),
            },
        }),
        Arc::new(RecordingProofFactory {
            returned: Some(IntegrationRequestProof {
                header_name: "dpop".to_string(),
                header_value: "proof-token".to_string(),
            }),
            calls: proof_calls.clone(),
        }),
    )
    .unwrap();

    let response = client
        .connect(connect_request(&server.url()))
        .await
        .unwrap();
    client.heartbeat(&response.session_id).await.unwrap();
    client.disconnect(&response.session_id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    bootstrap.assert_async().await;

    let calls = proof_calls.lock().await;
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, "POST");
    assert_eq!(calls[1].0, "GET");

    let live_headers = live_headers.lock().unwrap().clone();
    assert!(live_headers
        .iter()
        .any(|(name, value)| { name == "authorization" && value == "DPoP access-token" }));
    assert!(live_headers
        .iter()
        .any(|(name, value)| name == "dpop" && value == "proof-token"));

    let live_messages = live_messages.lock().unwrap().clone();
    assert_eq!(live_messages.len(), 2);
    let heartbeat: IntegrationSessionHeartbeatPayload =
        serde_json::from_str(&live_messages[0]).unwrap();
    assert_eq!(heartbeat.session_id, "session-002");
    let disconnect: IntegrationSessionDisconnectPayload =
        serde_json::from_str(&live_messages[1]).unwrap();
    assert_eq!(disconnect.session_id, "session-002");
}

#[tokio::test]
async fn connect_rejects_unexpected_granted_scope() {
    let mut server = mockito::Server::new_async().await;
    let bootstrap = server
        .mock("POST", "/integration/bootstrap")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "schema_version": "integration.bootstrap.v1",
                "supported_scopes": ["insight:write", "policy:read"],
                "granted_scopes": ["policy:read"],
                "supported_transports": ["https_long_poll"],
                "selected_transport": "https_long_poll",
                "supported_auth_schemes": ["bearer_token"],
                "selected_auth_scheme": "bearer_token",
                "session_required": true,
                "session": {
                    "session_id": "session-003"
                }
            })
            .to_string(),
        )
        .create_async()
        .await;

    let client = HttpsIntegrationTransportClient::new(
        HttpsIntegrationTransportConfig::new(
            format!("{}/integration/bootstrap", server.url()),
            Duration::from_secs(5),
        ),
        Arc::new(StaticAuthPort {
            context: IntegrationAuthContext {
                access_token: "access-token".to_string(),
                scheme: IntegrationAuthScheme::BearerToken,
                expires_at: None,
                resource_indicator: None,
            },
        }),
        Arc::new(RecordingProofFactory {
            returned: None,
            calls: Arc::new(Mutex::new(Vec::new())),
        }),
    )
    .unwrap();

    let err = client
        .connect(connect_request(&server.url()))
        .await
        .expect_err("unexpected scope should fail");
    bootstrap.assert_async().await;

    match err {
        CoreError::Validation {
            code: oneshim_core::error_codes::ValidationCode::InvalidField,
            field,
            ..
        } => {
            assert_eq!(field, "integration.bootstrap.granted_scopes");
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[tokio::test]
async fn egress_transport_posts_outbound_cloudevents() {
    let mut server = mockito::Server::new_async().await;
    let events_url = format!("{}/integration/sessions/session-010/events", server.url());

    let bootstrap = server
        .mock("POST", "/integration/bootstrap")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "schema_version": "integration.bootstrap.v1",
                "supported_scopes": ["insight:write", "session:manage"],
                "granted_scopes": ["insight:write", "session:manage"],
                "supported_transports": ["https_long_poll"],
                "selected_transport": "https_long_poll",
                "supported_auth_schemes": ["bearer_token"],
                "selected_auth_scheme": "bearer_token",
                "session_required": true,
                "session": {
                    "session_id": "session-010",
                    "send_events_url": events_url
                }
            })
            .to_string(),
        )
        .create_async()
        .await;

    let egress = server
        .mock("POST", "/integration/sessions/session-010/events")
        .match_header("authorization", "Bearer access-token")
        .match_body(Matcher::PartialJson(serde_json::json!({
            "items": [{
                "queue_id": "queue-010",
                "event": {
                    "type": "io.oneshim.integration.insight.v1",
                    "oneshimscope": "insight:write",
                    "data": {
                        "packet_id": "packet-010"
                    }
                }
            }]
        })))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "accepted_ids": ["queue-010"]
            })
            .to_string(),
        )
        .create_async()
        .await;

    let client = HttpsIntegrationTransportClient::new(
        HttpsIntegrationTransportConfig::new(
            format!("{}/integration/bootstrap", server.url()),
            Duration::from_secs(5),
        ),
        Arc::new(StaticAuthPort {
            context: IntegrationAuthContext {
                access_token: "access-token".to_string(),
                scheme: IntegrationAuthScheme::BearerToken,
                expires_at: None,
                resource_indicator: None,
            },
        }),
        Arc::new(RecordingProofFactory {
            returned: None,
            calls: Arc::new(Mutex::new(Vec::new())),
        }),
    )
    .unwrap();

    let session = client
        .connect(connect_request(&server.url()))
        .await
        .unwrap();
    let response = client
        .egress_transport()
        .send_messages(
            &session.session_id,
            vec![QueuedIntegrationEgressMessage {
                queue_id: "queue-010".to_string(),
                envelope: IntegrationEnvelope {
                    envelope_id: "env-010".to_string(),
                    schema_version: "integration.envelope.v1".to_string(),
                    message_type: IntegrationMessageType::InsightPacket,
                    timestamp: Utc::now(),
                    nonce: "nonce-010".to_string(),
                    origin: IntegrationOrigin {
                        device_id: "device-001".to_string(),
                        workspace_id: None,
                        session_id: Some("session-010".to_string()),
                        source: "desktop-client".to_string(),
                    },
                    capability_scope: IntegrationCapabilityScope::InsightWrite,
                },
                payload: IntegrationOutboundPayload::Insight(InsightPacket {
                    packet_id: "packet-010".to_string(),
                    summary: "summary".to_string(),
                    derived_tags: vec!["focus".to_string()],
                    source_window: InsightSourceWindow {
                        started_at: Utc::now(),
                        ended_at: Utc::now(),
                    },
                    privacy_classification: IntegrationPrivacyClassification::DerivedSummary,
                    audit_reference_id: None,
                }),
                queued_at: Utc::now(),
            }],
        )
        .await
        .unwrap();

    bootstrap.assert_async().await;
    egress.assert_async().await;
    assert_eq!(
        response.acknowledged_queue_ids,
        vec!["queue-010".to_string()]
    );
}

#[tokio::test]
async fn egress_transport_uses_websocket_channel_with_queue_id_extension() {
    let mut server = mockito::Server::new_async().await;
    let (channel_url, live_messages, _live_headers, _outbound_tx) =
        start_session_ws_server(true).await;

    let bootstrap = server
        .mock("POST", "/integration/bootstrap")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "schema_version": "integration.bootstrap.v1",
                "supported_scopes": ["insight:write", "session:manage"],
                "granted_scopes": ["insight:write", "session:manage"],
                "supported_transports": ["web_socket"],
                "selected_transport": "web_socket",
                "supported_auth_schemes": ["bearer_token"],
                "selected_auth_scheme": "bearer_token",
                "session_required": true,
                "session": {
                    "session_id": "session-010-ws",
                    "channel_url": channel_url
                }
            })
            .to_string(),
        )
        .create_async()
        .await;

    let client = HttpsIntegrationTransportClient::new(
        HttpsIntegrationTransportConfig::new(
            format!("{}/integration/bootstrap", server.url()),
            Duration::from_secs(5),
        ),
        Arc::new(StaticAuthPort {
            context: IntegrationAuthContext {
                access_token: "access-token".to_string(),
                scheme: IntegrationAuthScheme::BearerToken,
                expires_at: None,
                resource_indicator: None,
            },
        }),
        Arc::new(RecordingProofFactory {
            returned: None,
            calls: Arc::new(Mutex::new(Vec::new())),
        }),
    )
    .unwrap();

    let session = client
        .connect(connect_request(&server.url()))
        .await
        .unwrap();
    let response = client
        .egress_transport()
        .send_messages(
            &session.session_id,
            vec![QueuedIntegrationEgressMessage {
                queue_id: "queue-010-ws".to_string(),
                envelope: IntegrationEnvelope {
                    envelope_id: "env-010-ws".to_string(),
                    schema_version: "integration.envelope.v1".to_string(),
                    message_type: IntegrationMessageType::InsightPacket,
                    timestamp: Utc::now(),
                    nonce: "nonce-010-ws".to_string(),
                    origin: IntegrationOrigin {
                        device_id: "device-001".to_string(),
                        workspace_id: None,
                        session_id: Some("session-010-ws".to_string()),
                        source: "desktop-client".to_string(),
                    },
                    capability_scope: IntegrationCapabilityScope::InsightWrite,
                },
                payload: IntegrationOutboundPayload::Insight(InsightPacket {
                    packet_id: "packet-010-ws".to_string(),
                    summary: "summary".to_string(),
                    derived_tags: vec!["focus".to_string()],
                    source_window: InsightSourceWindow {
                        started_at: Utc::now(),
                        ended_at: Utc::now(),
                    },
                    privacy_classification: IntegrationPrivacyClassification::DerivedSummary,
                    audit_reference_id: None,
                }),
                queued_at: Utc::now(),
            }],
        )
        .await
        .unwrap();

    bootstrap.assert_async().await;
    assert_eq!(
        response.acknowledged_queue_ids,
        vec!["queue-010-ws".to_string()]
    );

    let live_messages = live_messages.lock().unwrap().clone();
    let event: crate::integration::cloudevents::IntegrationCloudEvent<InsightPacket> =
        serde_json::from_str(&live_messages[0]).unwrap();
    assert_eq!(event.oneshimqueueid.as_deref(), Some("queue-010-ws"));
}

#[tokio::test]
async fn egress_transport_posts_prompt_receipt_cloudevents() {
    let mut server = mockito::Server::new_async().await;
    let events_url = format!("{}/integration/sessions/session-011/events", server.url());

    let bootstrap = server
        .mock("POST", "/integration/bootstrap")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "schema_version": "integration.bootstrap.v1",
                "supported_scopes": ["prompt:ack", "session:manage"],
                "granted_scopes": ["prompt:ack", "session:manage"],
                "supported_transports": ["https_long_poll"],
                "selected_transport": "https_long_poll",
                "supported_auth_schemes": ["bearer_token"],
                "selected_auth_scheme": "bearer_token",
                "session_required": true,
                "session": {
                    "session_id": "session-011",
                    "send_events_url": events_url
                }
            })
            .to_string(),
        )
        .create_async()
        .await;

    let egress = server
        .mock("POST", "/integration/sessions/session-011/events")
        .match_header("authorization", "Bearer access-token")
        .match_body(Matcher::PartialJson(serde_json::json!({
            "items": [{
                "queue_id": "queue-011",
                "event": {
                    "type": "io.oneshim.integration.prompt_receipt.v1",
                    "oneshimscope": "prompt:ack",
                    "data": {
                        "prompt_id": "prompt-011",
                        "action": "acknowledged"
                    }
                }
            }]
        })))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "accepted_ids": ["queue-011"]
            })
            .to_string(),
        )
        .create_async()
        .await;

    let client = HttpsIntegrationTransportClient::new(
        HttpsIntegrationTransportConfig::new(
            format!("{}/integration/bootstrap", server.url()),
            Duration::from_secs(5),
        ),
        Arc::new(StaticAuthPort {
            context: IntegrationAuthContext {
                access_token: "access-token".to_string(),
                scheme: IntegrationAuthScheme::BearerToken,
                expires_at: None,
                resource_indicator: None,
            },
        }),
        Arc::new(RecordingProofFactory {
            returned: None,
            calls: Arc::new(Mutex::new(Vec::new())),
        }),
    )
    .unwrap();

    let session = client
        .connect(IntegrationTransportConnectRequest {
            device_id: "device-001".to_string(),
            client_version: "0.3.8".to_string(),
            device_label: Some("macbook".to_string()),
            requested_scopes: vec![
                IntegrationCapabilityScope::PromptAck,
                IntegrationCapabilityScope::SessionManage,
            ],
            preferred_transports: vec![IntegrationTransportKind::HttpsLongPoll],
            supported_auth_schemes: vec![IntegrationAuthScheme::BearerToken],
            resource_indicator: Some(server.url()),
        })
        .await
        .unwrap();

    let response = client
            .egress_transport()
            .send_messages(
                &session.session_id,
                vec![QueuedIntegrationEgressMessage {
                    queue_id: "queue-011".to_string(),
                    envelope: IntegrationEnvelope {
                        envelope_id: "env-011".to_string(),
                        schema_version: "integration.prompt_receipt.v1".to_string(),
                        message_type: IntegrationMessageType::PromptReceipt,
                        timestamp: Utc::now(),
                        nonce: "nonce-011".to_string(),
                        origin: IntegrationOrigin {
                            device_id: "device-001".to_string(),
                            workspace_id: None,
                            session_id: Some("session-011".to_string()),
                            source: "desktop-client".to_string(),
                        },
                        capability_scope: IntegrationCapabilityScope::PromptAck,
                    },
                    payload: IntegrationOutboundPayload::PromptReceipt(
                        oneshim_core::models::integration::IntegrationPromptReceipt {
                            receipt_id: "receipt-011".to_string(),
                            prompt_id: "prompt-011".to_string(),
                            action:
                                oneshim_core::models::integration::IntegrationPromptReceiptAction::Acknowledged,
                            occurred_at: Utc::now(),
                            reason: None,
                        },
                    ),
                    queued_at: Utc::now(),
                }],
            )
            .await
            .unwrap();

    bootstrap.assert_async().await;
    egress.assert_async().await;
    assert_eq!(
        response.acknowledged_queue_ids,
        vec!["queue-011".to_string()]
    );
}

#[tokio::test]
async fn inbox_transport_parses_prompt_cloudevents() {
    let mut server = mockito::Server::new_async().await;
    let prompts_url = format!("{}/integration/sessions/session-011/prompts", server.url());

    let bootstrap = server
        .mock("POST", "/integration/bootstrap")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "schema_version": "integration.bootstrap.v1",
                "supported_scopes": ["prompt:read", "session:manage"],
                "granted_scopes": ["prompt:read", "session:manage"],
                "supported_transports": ["https_long_poll"],
                "selected_transport": "https_long_poll",
                "supported_auth_schemes": ["bearer_token"],
                "selected_auth_scheme": "bearer_token",
                "session_required": true,
                "session": {
                    "session_id": "session-011",
                    "receive_prompts_url": prompts_url
                }
            })
            .to_string(),
        )
        .create_async()
        .await;

    let pull = server
        .mock("POST", "/integration/sessions/session-011/prompts")
        .match_header("authorization", "Bearer access-token")
        .match_body(Matcher::PartialJson(serde_json::json!({
            "after_stream_id": "prompt",
            "after_cursor": "cursor-0",
            "limit": 10
        })))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "events": [{
                    "specversion": "1.0",
                    "id": "prompt-env-1",
                    "source": "oneshim://devices/device-001",
                    "type": "io.oneshim.integration.prompt.v1",
                    "subject": "prompt-011",
                    "time": Utc::now(),
                    "datacontenttype": "application/json",
                    "data": {
                        "prompt_id": "prompt-011",
                        "category": "task",
                        "title": "title",
                        "body": "body",
                        "priority": "medium",
                        "actions": [],
                        "provenance": {
                            "source_system": "integration"
                        }
                    },
                    "oneshimscope": "prompt:read",
                    "oneshimnonce": "nonce-011",
                    "oneshimsessionid": "session-011",
                    "oneshimpromptcategory": "task"
                }],
                "ack_cursor": {
                    "stream_id": "prompt",
                    "cursor": "cursor-1",
                    "acknowledged_at": Utc::now()
                }
            })
            .to_string(),
        )
        .create_async()
        .await;

    let client = HttpsIntegrationTransportClient::new(
        HttpsIntegrationTransportConfig::new(
            format!("{}/integration/bootstrap", server.url()),
            Duration::from_secs(5),
        ),
        Arc::new(StaticAuthPort {
            context: IntegrationAuthContext {
                access_token: "access-token".to_string(),
                scheme: IntegrationAuthScheme::BearerToken,
                expires_at: None,
                resource_indicator: None,
            },
        }),
        Arc::new(RecordingProofFactory {
            returned: None,
            calls: Arc::new(Mutex::new(Vec::new())),
        }),
    )
    .unwrap();

    let session = client
        .connect(IntegrationTransportConnectRequest {
            device_id: "device-001".to_string(),
            client_version: "0.3.8".to_string(),
            device_label: Some("macbook".to_string()),
            requested_scopes: vec![
                IntegrationCapabilityScope::PromptRead,
                IntegrationCapabilityScope::SessionManage,
            ],
            preferred_transports: vec![IntegrationTransportKind::HttpsLongPoll],
            supported_auth_schemes: vec![IntegrationAuthScheme::BearerToken],
            resource_indicator: Some(server.url()),
        })
        .await
        .unwrap();

    let response = client
        .inbox_transport()
        .receive_prompts(
            &session.session_id,
            Some(IntegrationAckCursor {
                stream_id: "prompt".to_string(),
                cursor: "cursor-0".to_string(),
                acknowledged_at: Utc::now(),
            }),
            10,
        )
        .await
        .unwrap();

    bootstrap.assert_async().await;
    pull.assert_async().await;
    assert_eq!(response.prompts.len(), 1);
    assert_eq!(response.prompts[0].prompt_id, "prompt-011");
    assert_eq!(
        response
            .ack_cursor
            .as_ref()
            .map(|cursor| cursor.cursor.as_str()),
        Some("cursor-1")
    );
}

#[tokio::test]
async fn inbox_transport_drains_websocket_prompt_events() {
    let mut server = mockito::Server::new_async().await;
    let (channel_url, _live_messages, _live_headers, outbound_tx) =
        start_session_ws_server(false).await;

    let bootstrap = server
        .mock("POST", "/integration/bootstrap")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "schema_version": "integration.bootstrap.v1",
                "supported_scopes": ["prompt:read", "session:manage"],
                "granted_scopes": ["prompt:read", "session:manage"],
                "supported_transports": ["web_socket"],
                "selected_transport": "web_socket",
                "supported_auth_schemes": ["bearer_token"],
                "selected_auth_scheme": "bearer_token",
                "session_required": true,
                "session": {
                    "session_id": "session-011-ws",
                    "channel_url": channel_url
                }
            })
            .to_string(),
        )
        .create_async()
        .await;

    let client = HttpsIntegrationTransportClient::new(
        HttpsIntegrationTransportConfig::new(
            format!("{}/integration/bootstrap", server.url()),
            Duration::from_secs(5),
        ),
        Arc::new(StaticAuthPort {
            context: IntegrationAuthContext {
                access_token: "access-token".to_string(),
                scheme: IntegrationAuthScheme::BearerToken,
                expires_at: None,
                resource_indicator: None,
            },
        }),
        Arc::new(RecordingProofFactory {
            returned: None,
            calls: Arc::new(Mutex::new(Vec::new())),
        }),
    )
    .unwrap();

    let session = client
        .connect(IntegrationTransportConnectRequest {
            device_id: "device-001".to_string(),
            client_version: "0.3.8".to_string(),
            device_label: Some("macbook".to_string()),
            requested_scopes: vec![
                IntegrationCapabilityScope::PromptRead,
                IntegrationCapabilityScope::SessionManage,
            ],
            preferred_transports: vec![IntegrationTransportKind::WebSocket],
            supported_auth_schemes: vec![IntegrationAuthScheme::BearerToken],
            resource_indicator: Some(server.url()),
        })
        .await
        .unwrap();

    outbound_tx
        .send(
            serde_json::json!({
                "events": [{
                    "specversion": "1.0",
                    "id": "prompt-env-ws-1",
                    "source": "oneshim://devices/device-001",
                    "type": "io.oneshim.integration.prompt.v1",
                    "subject": "prompt-011-ws",
                    "time": Utc::now(),
                    "datacontenttype": "application/json",
                    "data": {
                        "prompt_id": "prompt-011-ws",
                        "category": "task",
                        "title": "title",
                        "body": "body",
                        "priority": "medium",
                        "actions": [],
                        "provenance": {
                            "source_system": "integration"
                        }
                    },
                    "oneshimscope": "prompt:read",
                    "oneshimnonce": "nonce-011-ws",
                    "oneshimsessionid": "session-011-ws",
                    "oneshimpromptcategory": "task"
                }, {
                    "specversion": "1.0",
                    "id": "prompt-env-ws-2",
                    "source": "oneshim://devices/device-001",
                    "type": "io.oneshim.integration.prompt.v1",
                    "subject": "prompt-012-ws",
                    "time": Utc::now(),
                    "datacontenttype": "application/json",
                    "data": {
                        "prompt_id": "prompt-012-ws",
                        "category": "reminder",
                        "title": "title-2",
                        "body": "body-2",
                        "priority": "low",
                        "actions": [],
                        "provenance": {
                            "source_system": "integration"
                        }
                    }
                ,
                    "oneshimscope": "prompt:read",
                    "oneshimnonce": "nonce-012-ws",
                    "oneshimsessionid": "session-011-ws",
                    "oneshimpromptcategory": "reminder"
                }]
            })
            .to_string(),
        )
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let response = client
        .inbox_transport()
        .receive_prompts(&session.session_id, None, 10)
        .await
        .unwrap();

    bootstrap.assert_async().await;
    assert_eq!(response.prompts.len(), 2);
    assert_eq!(response.prompts[0].prompt_id, "prompt-011-ws");
    assert_eq!(response.prompts[0].category, ProactivePromptCategory::Task);
    assert_eq!(
        response.prompts[0].priority,
        ProactivePromptPriority::Medium
    );
    assert_eq!(response.prompts[1].prompt_id, "prompt-012-ws");
    assert!(response.ack_cursor.is_none());
}

// iter-84 regression guards for iter-55a semantic HTTP status mapping
// in integration/http_transport::check_response. Uses the existing
// StaticAuthPort + connect_request helpers; bootstrap endpoint returns
// the test status code and we assert the resulting CoreError variant.
async fn run_bootstrap_status_test(status: u16) -> CoreError {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("POST", "/integration/bootstrap")
        .with_status(status as usize)
        .with_body(format!("http {status}"))
        .create_async()
        .await;

    let client = HttpsIntegrationTransportClient::new(
        HttpsIntegrationTransportConfig::new(
            format!("{}/integration/bootstrap", server.url()),
            std::time::Duration::from_secs(5),
        ),
        Arc::new(StaticAuthPort {
            context: IntegrationAuthContext {
                access_token: "access-token".to_string(),
                scheme: IntegrationAuthScheme::BearerToken,
                expires_at: None,
                resource_indicator: None,
            },
        }),
        Arc::new(RecordingProofFactory {
            returned: None,
            calls: Arc::new(Mutex::new(Vec::new())),
        }),
    )
    .unwrap();

    client
        .connect(connect_request(&server.url()))
        .await
        .unwrap_err()
}

#[tokio::test]
async fn bootstrap_403_maps_to_auth() {
    let err = run_bootstrap_status_test(403).await;
    assert!(
        matches!(err, CoreError::Auth { .. }),
        "403 → Auth, got: {err:?}"
    );
}

#[tokio::test]
async fn bootstrap_404_maps_to_not_found() {
    let err = run_bootstrap_status_test(404).await;
    assert!(
        matches!(err, CoreError::NotFound { .. }),
        "404 → NotFound, got: {err:?}"
    );
}

#[tokio::test]
async fn bootstrap_429_maps_to_rate_limit() {
    let err = run_bootstrap_status_test(429).await;
    assert!(
        matches!(err, CoreError::RateLimit { .. }),
        "429 → RateLimit, got: {err:?}"
    );
}

#[tokio::test]
async fn bootstrap_502_maps_to_service_unavailable() {
    let err = run_bootstrap_status_test(502).await;
    assert!(
        matches!(err, CoreError::ServiceUnavailable { .. }),
        "502 → ServiceUnavailable, got: {err:?}"
    );
}

#[tokio::test]
async fn bootstrap_504_maps_to_timeout() {
    let err = run_bootstrap_status_test(504).await;
    assert!(
        matches!(err, CoreError::RequestTimeout { .. }),
        "504 → RequestTimeout, got: {err:?}"
    );
}

/// Domain fallback: 500 stays as Network (iter-55a wildcard).
#[tokio::test]
async fn bootstrap_500_falls_back_to_network() {
    let err = run_bootstrap_status_test(500).await;
    assert!(
        matches!(err, CoreError::Network { .. }),
        "500 should fall back to Network, got: {err:?}"
    );
}
