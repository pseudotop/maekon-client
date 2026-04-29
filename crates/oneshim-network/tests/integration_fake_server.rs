mod fake_integration_server;

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use fake_integration_server::{sample_prompt, FakeIntegrationServer};
use oneshim_core::error::CoreError;
use oneshim_core::models::integration::{
    InsightPacket, InsightSourceWindow, IntegrationAuthContext, IntegrationAuthScheme,
    IntegrationCapabilityScope, IntegrationEnvelope, IntegrationMessageType, IntegrationOrigin,
    IntegrationOutboundPayload, IntegrationPrivacyClassification, IntegrationPromptReceipt,
    IntegrationPromptReceiptAction, IntegrationTransportKind, QueuedIntegrationEgressMessage,
};
use oneshim_core::ports::integration::{
    IntegrationEgressPort, IntegrationInboxPort, IntegrationOutboxPort,
    IntegrationRuntimeTelemetryPort, IntegrationSessionPort,
};
use oneshim_network::integration::{
    HttpsIntegrationTransportClient, HttpsIntegrationTransportConfig, IntegrationEgressCoordinator,
    IntegrationEgressTransportClient, IntegrationInboxCoordinator, IntegrationInboxTransportClient,
    IntegrationRuntimeLoop, IntegrationRuntimeLoopProfile, IntegrationRuntimeTelemetryHandle,
    IntegrationSessionCoordinator, IntegrationTransportClient, IntegrationTransportConnectRequest,
    NoopIntegrationRequestProofFactory, StaticIntegrationAuthPort,
    StaticIntegrationRequestProofFactory,
};
use oneshim_storage::integration_state_store::FileIntegrationStateStore;
use tempfile::TempDir;

#[tokio::test]
async fn fake_server_roundtrip_covers_bootstrap_egress_inbox_and_session_lifecycle() {
    let server = FakeIntegrationServer::start().await;
    server.push_prompt(sample_prompt("prompt-001"));

    let client = build_client(&server);

    let session = client
        .connect(connect_request(&server))
        .await
        .expect("bootstrap should succeed");

    assert_eq!(session.session_id, "session-fake-001");
    assert_eq!(
        session.transport_kind,
        IntegrationTransportKind::HttpsLongPoll
    );

    client
        .heartbeat(&session.session_id)
        .await
        .expect("heartbeat should succeed");

    let egress_response = client
        .egress_transport()
        .send_messages(
            &session.session_id,
            vec![sample_insight_message(
                &session.session_id,
                "queue-001",
                "packet-001",
            )],
        )
        .await
        .expect("egress should succeed");

    assert_eq!(egress_response.acknowledged_queue_ids, vec!["queue-001"]);
    assert_eq!(
        egress_response
            .ack_cursor
            .as_ref()
            .map(|cursor| cursor.stream_id.as_str()),
        Some("integration.egress")
    );

    let inbox_response = client
        .inbox_transport()
        .receive_prompts(&session.session_id, None, 10)
        .await
        .expect("prompt pull should succeed");

    assert_eq!(inbox_response.prompts.len(), 1);
    assert_eq!(inbox_response.prompts[0].prompt_id, "prompt-001");
    assert_eq!(
        inbox_response
            .ack_cursor
            .as_ref()
            .map(|cursor| cursor.stream_id.as_str()),
        Some("integration.prompts")
    );

    client
        .disconnect(&session.session_id)
        .await
        .expect("disconnect should succeed");

    let snapshot = server.snapshot();
    assert_eq!(snapshot.bootstrap_requests.len(), 1);
    assert_eq!(snapshot.egress_batches.len(), 1);
    assert_eq!(snapshot.egress_batches[0].items.len(), 1);
    assert_eq!(snapshot.egress_batches[0].items[0].queue_id, "queue-001");
    assert_eq!(
        snapshot.heartbeat_session_ids,
        vec!["session-fake-001".to_string()]
    );
    assert_eq!(
        snapshot.disconnect_session_ids,
        vec!["session-fake-001".to_string()]
    );
}

#[tokio::test]
async fn fake_server_can_return_partial_acknowledgements() {
    let server = FakeIntegrationServer::start().await;
    server.set_egress_partial_ack_limit(1);

    let client = build_client(&server);
    let session = client
        .connect(connect_request(&server))
        .await
        .expect("bootstrap should succeed");

    let response = client
        .egress_transport()
        .send_messages(
            &session.session_id,
            vec![
                sample_insight_message(&session.session_id, "queue-001", "packet-001"),
                sample_insight_message(&session.session_id, "queue-002", "packet-002"),
            ],
        )
        .await
        .expect("partial ack should still return response");

    assert_eq!(response.acknowledged_queue_ids, vec!["queue-001"]);

    let snapshot = server.snapshot();
    assert_eq!(snapshot.egress_batches.len(), 1);
    assert_eq!(snapshot.egress_batches[0].items.len(), 2);
}

#[tokio::test]
async fn fake_server_propagates_retry_after_rate_limits() {
    let server = FakeIntegrationServer::start().await;
    server.set_egress_rate_limit_once(7);

    let client = build_client(&server);
    let session = client
        .connect(connect_request(&server))
        .await
        .expect("bootstrap should succeed");

    let error = client
        .egress_transport()
        .send_messages(
            &session.session_id,
            vec![sample_insight_message(
                &session.session_id,
                "queue-001",
                "packet-001",
            )],
        )
        .await
        .expect_err("rate-limited egress should fail");

    match error {
        CoreError::RateLimit {
            code: oneshim_core::error_codes::NetworkCode::RateLimit,
            retry_after_secs,
        } => assert_eq!(retry_after_secs, 7),
        other => panic!("unexpected error: {other}"),
    }
}

#[tokio::test]
async fn fake_server_records_prompt_receipt_message_shape() {
    let server = FakeIntegrationServer::start().await;
    let client = build_client(&server);
    let session = client
        .connect(connect_request(&server))
        .await
        .expect("bootstrap should succeed");

    let response = client
        .egress_transport()
        .send_messages(
            &session.session_id,
            vec![sample_prompt_receipt_message(
                &session.session_id,
                "queue-receipt-001",
                "prompt-001",
            )],
        )
        .await
        .expect("prompt receipt should succeed");

    assert_eq!(response.acknowledged_queue_ids, vec!["queue-receipt-001"]);

    let snapshot = server.snapshot();
    let event = &snapshot.egress_batches[0].items[0].event;
    assert_eq!(event.event_type, "io.oneshim.integration.prompt_receipt.v1");
    assert_eq!(event.subject, "prompt-001");
    assert_eq!(event.oneshimscope, "prompt:ack");
    assert_eq!(
        event.data.get("prompt_id").and_then(|value| value.as_str()),
        Some("prompt-001")
    );
}

#[tokio::test]
async fn fake_server_supports_reconnect_after_disconnect() {
    let server = FakeIntegrationServer::start().await;
    let client = build_client(&server);

    let first_session = client
        .connect(connect_request(&server))
        .await
        .expect("first bootstrap should succeed");
    client
        .disconnect(&first_session.session_id)
        .await
        .expect("first disconnect should succeed");

    let second_session = client
        .connect(connect_request(&server))
        .await
        .expect("second bootstrap should succeed");

    assert_eq!(second_session.session_id, "session-fake-001");

    let snapshot = server.snapshot();
    assert_eq!(snapshot.bootstrap_requests.len(), 2);
    assert_eq!(snapshot.disconnect_session_ids.len(), 1);
}

#[tokio::test]
async fn fake_server_websocket_channel_covers_heartbeat_disconnect_and_outbound_ack() {
    let server = FakeIntegrationServer::start().await;
    server.enable_websocket_transport(true);

    let client = build_client(&server);
    let session = client
        .connect(websocket_connect_request(
            &server,
            vec![
                IntegrationCapabilityScope::InsightWrite,
                IntegrationCapabilityScope::SessionManage,
            ],
        ))
        .await
        .expect("websocket bootstrap should succeed");

    let response = client
        .egress_transport()
        .send_messages(
            &session.session_id,
            vec![sample_insight_message(
                &session.session_id,
                "queue-live-001",
                "packet-live-001",
            )],
        )
        .await
        .expect("live websocket egress should succeed");

    client
        .heartbeat(&session.session_id)
        .await
        .expect("websocket heartbeat should succeed");
    tokio::time::sleep(Duration::from_millis(25)).await;
    client
        .disconnect(&session.session_id)
        .await
        .expect("websocket disconnect should succeed");
    tokio::time::sleep(Duration::from_millis(25)).await;

    assert_eq!(response.acknowledged_queue_ids, vec!["queue-live-001"]);
    assert_eq!(
        response
            .ack_cursor
            .as_ref()
            .map(|cursor| cursor.cursor.as_str()),
        Some("ack-egress-live-001")
    );

    let snapshot = server.snapshot();
    assert!(snapshot
        .live_messages
        .iter()
        .any(|message| message.contains("\"packet_id\":\"packet-live-001\"")));
    assert!(snapshot
        .live_headers
        .iter()
        .any(|(name, value)| name == "authorization" && value == "Bearer access-token"));
    assert!(snapshot
        .live_messages
        .iter()
        .any(|message| message.contains("\"session_id\":\"session-fake-001\"")));
}

#[tokio::test]
async fn fake_server_websocket_channel_delivers_prompt_signals() {
    let server = FakeIntegrationServer::start().await;
    server.enable_websocket_transport(false);

    let client = build_client(&server);
    let session = client
        .connect(websocket_connect_request(
            &server,
            vec![
                IntegrationCapabilityScope::PromptRead,
                IntegrationCapabilityScope::SessionManage,
            ],
        ))
        .await
        .expect("websocket bootstrap should succeed");

    server.push_live_prompt(sample_prompt("prompt-live-001"));

    assert!(client
        .inbox_transport()
        .wait_for_remote_signal(&session.session_id, Duration::from_millis(250))
        .await
        .expect("websocket prompt signal wait"));

    let prompts = client
        .inbox_transport()
        .receive_prompts(&session.session_id, None, 10)
        .await
        .expect("websocket prompt drain");

    assert_eq!(prompts.prompts.len(), 1);
    assert_eq!(prompts.prompts[0].prompt_id, "prompt-live-001");
    assert!(prompts.ack_cursor.is_none());
}

#[tokio::test]
async fn fake_server_websocket_channel_delivers_prompt_batches() {
    let server = FakeIntegrationServer::start().await;
    server.enable_websocket_transport(false);

    let client = build_client(&server);
    let session = client
        .connect(websocket_connect_request(
            &server,
            vec![
                IntegrationCapabilityScope::PromptRead,
                IntegrationCapabilityScope::SessionManage,
            ],
        ))
        .await
        .expect("websocket bootstrap should succeed");

    server.push_live_prompt_batch(vec![
        sample_prompt("prompt-batch-001"),
        sample_prompt("prompt-batch-002"),
    ]);

    assert!(client
        .inbox_transport()
        .wait_for_remote_signal(&session.session_id, Duration::from_millis(250))
        .await
        .expect("websocket prompt batch signal wait"));

    let prompts = client
        .inbox_transport()
        .receive_prompts(&session.session_id, None, 10)
        .await
        .expect("websocket prompt batch drain");

    assert_eq!(prompts.prompts.len(), 2);
    assert_eq!(prompts.prompts[0].prompt_id, "prompt-batch-001");
    assert_eq!(prompts.prompts[1].prompt_id, "prompt-batch-002");
}

#[tokio::test]
async fn fake_server_websocket_channel_handles_large_prompt_batches() {
    let server = FakeIntegrationServer::start().await;
    server.enable_websocket_transport(false);

    let client = build_client(&server);
    let session = client
        .connect(websocket_connect_request(
            &server,
            vec![
                IntegrationCapabilityScope::PromptRead,
                IntegrationCapabilityScope::SessionManage,
            ],
        ))
        .await
        .expect("websocket bootstrap should succeed");

    let prompt_ids = (1..=64)
        .map(|index| format!("prompt-batch-large-{index:03}"))
        .collect::<Vec<_>>();
    server.push_live_prompt_batch(
        prompt_ids
            .iter()
            .map(|prompt_id| sample_prompt(prompt_id))
            .collect(),
    );

    assert!(client
        .inbox_transport()
        .wait_for_remote_signal(&session.session_id, Duration::from_millis(250))
        .await
        .expect("websocket large prompt batch signal wait"));

    let prompts = client
        .inbox_transport()
        .receive_prompts(&session.session_id, None, 128)
        .await
        .expect("websocket large prompt batch drain");
    let received_ids = prompts
        .prompts
        .iter()
        .map(|prompt| prompt.prompt_id.clone())
        .collect::<Vec<_>>();

    assert_eq!(received_ids, prompt_ids);
}

#[tokio::test]
async fn fake_server_websocket_channel_ignores_malformed_payloads_and_recovers() {
    let server = FakeIntegrationServer::start().await;
    server.enable_websocket_transport(false);

    let client = build_client(&server);
    let session = client
        .connect(websocket_connect_request(
            &server,
            vec![
                IntegrationCapabilityScope::PromptRead,
                IntegrationCapabilityScope::SessionManage,
            ],
        ))
        .await
        .expect("websocket bootstrap should succeed");

    server.push_live_raw("{\"broken\":");
    server.push_live_prompt(sample_prompt("prompt-after-malformed-001"));

    assert!(client
        .inbox_transport()
        .wait_for_remote_signal(&session.session_id, Duration::from_millis(250))
        .await
        .expect("websocket malformed payload recovery wait"));

    let prompts = client
        .inbox_transport()
        .receive_prompts(&session.session_id, None, 10)
        .await
        .expect("websocket malformed payload recovery drain");

    assert_eq!(prompts.prompts.len(), 1);
    assert_eq!(prompts.prompts[0].prompt_id, "prompt-after-malformed-001");
}

#[tokio::test]
async fn fake_server_websocket_channel_ignores_unsupported_prompt_events_and_recovers() {
    let server = FakeIntegrationServer::start().await;
    server.enable_websocket_transport(false);

    let client = build_client(&server);
    let session = client
        .connect(websocket_connect_request(
            &server,
            vec![
                IntegrationCapabilityScope::PromptRead,
                IntegrationCapabilityScope::SessionManage,
            ],
        ))
        .await
        .expect("websocket bootstrap should succeed");

    server.push_live_raw(
        serde_json::json!({
            "specversion": "1.0",
            "id": "env-unsupported-001",
            "source": "oneshim://systems/fake-integration-server",
            "type": "io.oneshim.integration.unsupported_prompt.v1",
            "subject": "prompt-unsupported-001",
            "time": Utc::now(),
            "datacontenttype": "application/json",
            "data": sample_prompt("prompt-unsupported-001"),
            "dataschema": "integration.prompt.v1",
            "oneshimscope": "prompt:read",
            "oneshimnonce": "nonce-prompt-unsupported-001",
            "oneshimsessionid": "session-fake-001"
        })
        .to_string(),
    );
    server.push_live_prompt(sample_prompt("prompt-after-unsupported-001"));

    assert!(client
        .inbox_transport()
        .wait_for_remote_signal(&session.session_id, Duration::from_millis(250))
        .await
        .expect("websocket unsupported prompt recovery wait"));

    let prompts = client
        .inbox_transport()
        .receive_prompts(&session.session_id, None, 10)
        .await
        .expect("websocket unsupported prompt recovery drain");

    assert_eq!(prompts.prompts.len(), 1);
    assert_eq!(prompts.prompts[0].prompt_id, "prompt-after-unsupported-001");
}

#[tokio::test]
async fn fake_server_websocket_channel_times_out_on_incomplete_ack() {
    let server = FakeIntegrationServer::start().await;
    server.enable_websocket_transport(true);
    server.set_websocket_ack_limit(1);

    let client = build_client_with_timeout(&server, Duration::from_millis(200));
    let session = client
        .connect(websocket_connect_request(
            &server,
            vec![
                IntegrationCapabilityScope::InsightWrite,
                IntegrationCapabilityScope::SessionManage,
            ],
        ))
        .await
        .expect("websocket bootstrap should succeed");

    let error = client
        .egress_transport()
        .send_messages(
            &session.session_id,
            vec![
                sample_insight_message(&session.session_id, "queue-live-101", "packet-live-101"),
                sample_insight_message(&session.session_id, "queue-live-102", "packet-live-102"),
            ],
        )
        .await
        .expect_err("incomplete websocket ack should time out");

    match error {
        CoreError::RequestTimeout {
            code: oneshim_core::error_codes::NetworkCode::Timeout,
            timeout_ms,
        } => assert_eq!(timeout_ms, 200),
        other => panic!("unexpected error: {other}"),
    }

    let snapshot = server.snapshot();
    assert!(snapshot
        .live_messages
        .iter()
        .any(|message| message.contains("\"packet_id\":\"packet-live-101\"")));
    assert!(snapshot
        .live_messages
        .iter()
        .any(|message| message.contains("\"packet_id\":\"packet-live-102\"")));
}

#[tokio::test]
async fn fake_server_websocket_channel_acknowledges_large_outbound_batches() {
    let server = FakeIntegrationServer::start().await;
    server.enable_websocket_transport(true);

    let client = build_client(&server);
    let session = client
        .connect(websocket_connect_request(
            &server,
            vec![
                IntegrationCapabilityScope::InsightWrite,
                IntegrationCapabilityScope::SessionManage,
            ],
        ))
        .await
        .expect("websocket bootstrap should succeed");

    let expected_queue_ids = (1..=32)
        .map(|index| format!("queue-live-bulk-{index:03}"))
        .collect::<Vec<_>>();
    let messages = expected_queue_ids
        .iter()
        .enumerate()
        .map(|(index, queue_id)| {
            sample_insight_message(
                &session.session_id,
                queue_id,
                &format!("packet-live-bulk-{:03}", index + 1),
            )
        })
        .collect::<Vec<_>>();

    let response = client
        .egress_transport()
        .send_messages(&session.session_id, messages)
        .await
        .expect("large websocket egress batch should succeed");

    assert_eq!(response.acknowledged_queue_ids, expected_queue_ids);

    let snapshot = server.snapshot();
    assert!(snapshot
        .live_messages
        .iter()
        .any(|message| message.contains("\"packet_id\":\"packet-live-bulk-001\"")));
    assert!(snapshot
        .live_messages
        .iter()
        .any(|message| message.contains("\"packet_id\":\"packet-live-bulk-032\"")));
}

#[tokio::test]
async fn fake_server_websocket_channel_captures_dpop_headers() {
    let server = FakeIntegrationServer::start().await;
    server.enable_websocket_transport(true);
    server.set_selected_auth_scheme(IntegrationAuthScheme::DpopBearer);

    let client = build_dpop_client(&server);
    let session = client
        .connect(websocket_connect_request_with_auth(
            &server,
            vec![
                IntegrationCapabilityScope::InsightWrite,
                IntegrationCapabilityScope::SessionManage,
            ],
            vec![IntegrationAuthScheme::DpopBearer],
        ))
        .await
        .expect("websocket dpop bootstrap should succeed");

    client
        .disconnect(&session.session_id)
        .await
        .expect("websocket disconnect should succeed");
    tokio::time::sleep(Duration::from_millis(25)).await;

    let snapshot = server.snapshot();
    assert!(snapshot
        .live_headers
        .iter()
        .any(|(name, value)| name == "authorization" && value == "DPoP access-token"));
    assert!(snapshot
        .live_headers
        .iter()
        .any(|(name, value)| name == "dpop" && value == "proof-token"));
}

#[tokio::test]
async fn websocket_session_coordinator_reconnects_after_live_channel_drop() {
    let server = FakeIntegrationServer::start().await;
    server.enable_websocket_transport(false);
    server.set_websocket_close_after_messages(1);

    let transport = Arc::new(build_client(&server));
    let coordinator = IntegrationSessionCoordinator::new("device-compat-001", transport);
    let requested_scopes = vec![
        IntegrationCapabilityScope::InsightWrite,
        IntegrationCapabilityScope::SessionManage,
    ];

    let first_session = coordinator
        .connect(requested_scopes.clone())
        .await
        .expect("first websocket bootstrap should succeed");
    coordinator
        .heartbeat(&first_session.session_id)
        .await
        .expect("first heartbeat should succeed");
    tokio::time::sleep(Duration::from_millis(50)).await;

    let heartbeat_error = coordinator
        .heartbeat(&first_session.session_id)
        .await
        .expect_err("second heartbeat should fail after channel drop");
    assert!(matches!(heartbeat_error, CoreError::Network { .. }));

    let resumed = coordinator
        .connect(requested_scopes)
        .await
        .expect("session coordinator should reconnect after live channel drop");

    assert_eq!(resumed.session_id, "session-fake-001");
    assert_eq!(resumed.transport_kind, IntegrationTransportKind::WebSocket);

    let snapshot = server.snapshot();
    assert_eq!(snapshot.bootstrap_requests.len(), 2);
}

#[tokio::test]
async fn websocket_session_coordinator_handles_repeated_live_channel_churn() {
    let server = FakeIntegrationServer::start().await;
    server.enable_websocket_transport(false);
    server.set_websocket_close_after_messages(1);

    let transport = Arc::new(build_client(&server));
    let coordinator = IntegrationSessionCoordinator::new("device-compat-001", transport);
    let requested_scopes = vec![IntegrationCapabilityScope::SessionManage];
    let mut session = coordinator
        .connect(requested_scopes.clone())
        .await
        .expect("initial websocket bootstrap should succeed");

    for cycle in 1..=3 {
        coordinator
            .heartbeat(&session.session_id)
            .await
            .unwrap_or_else(|err| panic!("cycle {cycle} first heartbeat should succeed: {err}"));
        tokio::time::sleep(Duration::from_millis(50)).await;

        let error = coordinator
            .heartbeat(&session.session_id)
            .await
            .expect_err("subsequent heartbeat should fail after live channel drop");
        assert!(
            matches!(error, CoreError::Network { .. }),
            "cycle {cycle} should surface network error, got {error}"
        );

        session = coordinator
            .connect(requested_scopes.clone())
            .await
            .unwrap_or_else(|err| panic!("cycle {cycle} reconnect should succeed: {err}"));
        assert_eq!(session.transport_kind, IntegrationTransportKind::WebSocket);
    }

    let snapshot = server.snapshot();
    assert_eq!(snapshot.bootstrap_requests.len(), 4);
}

#[tokio::test]
async fn fake_server_websocket_channel_supports_multiple_live_connections() {
    let server = FakeIntegrationServer::start().await;
    server.enable_websocket_transport(false);

    let first_client = build_client(&server);
    let second_client = build_client(&server);
    let first_session = first_client
        .connect(websocket_connect_request(
            &server,
            vec![
                IntegrationCapabilityScope::PromptRead,
                IntegrationCapabilityScope::SessionManage,
            ],
        ))
        .await
        .expect("first websocket bootstrap should succeed");
    let second_session = second_client
        .connect(websocket_connect_request(
            &server,
            vec![
                IntegrationCapabilityScope::PromptRead,
                IntegrationCapabilityScope::SessionManage,
            ],
        ))
        .await
        .expect("second websocket bootstrap should succeed");

    server.push_live_prompt(sample_prompt("prompt-shared-001"));

    assert!(first_client
        .inbox_transport()
        .wait_for_remote_signal(&first_session.session_id, Duration::from_millis(250))
        .await
        .expect("first websocket prompt signal wait"));
    assert!(second_client
        .inbox_transport()
        .wait_for_remote_signal(&second_session.session_id, Duration::from_millis(250))
        .await
        .expect("second websocket prompt signal wait"));

    let first_prompts = first_client
        .inbox_transport()
        .receive_prompts(&first_session.session_id, None, 10)
        .await
        .expect("first websocket prompt drain");
    let second_prompts = second_client
        .inbox_transport()
        .receive_prompts(&second_session.session_id, None, 10)
        .await
        .expect("second websocket prompt drain");

    assert_eq!(first_prompts.prompts.len(), 1);
    assert_eq!(second_prompts.prompts.len(), 1);
    assert_eq!(first_prompts.prompts[0].prompt_id, "prompt-shared-001");
    assert_eq!(second_prompts.prompts[0].prompt_id, "prompt-shared-001");
}

#[tokio::test]
async fn duplicate_prompt_delivery_does_not_resurrect_dismissed_inbox_items() {
    let server = FakeIntegrationServer::start().await;
    server.push_prompt(sample_prompt("prompt-duplicate-001"));

    let harness = build_connected_runtime(&server).await;

    let refreshed = harness.inbox.refresh().await.unwrap();
    assert_eq!(refreshed, 1);
    assert_eq!(harness.inbox.list_pending().await.unwrap().len(), 1);

    harness
        .inbox
        .dismiss(
            "prompt-duplicate-001",
            Some("already handled locally".to_string()),
        )
        .await
        .unwrap();
    assert!(harness.inbox.list_pending().await.unwrap().is_empty());

    let duplicate_refresh = harness.inbox.refresh().await.unwrap();
    assert_eq!(duplicate_refresh, 1);
    assert!(harness.inbox.list_pending().await.unwrap().is_empty());

    let queued = harness.store.outbox_store().list_pending(10).await.unwrap();
    assert_eq!(queued.len(), 1);
    match &queued[0].payload {
        IntegrationOutboundPayload::PromptReceipt(receipt) => {
            assert_eq!(receipt.prompt_id, "prompt-duplicate-001");
            assert_eq!(receipt.action, IntegrationPromptReceiptAction::Dismissed);
            assert_eq!(receipt.reason.as_deref(), Some("already handled locally"));
        }
        IntegrationOutboundPayload::Insight(_) => panic!("expected prompt receipt payload"),
    }
}

#[tokio::test]
async fn prompt_receipt_roundtrip_flushes_ack_and_dismiss_events() {
    let server = FakeIntegrationServer::start().await;
    server.push_prompt(sample_prompt("prompt-ack-001"));
    server.push_prompt(sample_prompt("prompt-dismiss-001"));

    let harness = build_connected_runtime(&server).await;

    let refreshed = harness.inbox.refresh().await.unwrap();
    assert_eq!(refreshed, 2);

    harness
        .inbox
        .acknowledge("prompt-ack-001")
        .await
        .expect("acknowledge should enqueue receipt");
    harness
        .inbox
        .dismiss("prompt-dismiss-001", Some("user dismissed".to_string()))
        .await
        .expect("dismiss should enqueue receipt");

    let pending_before_flush = harness.store.outbox_store().pending_count().await.unwrap();
    assert_eq!(pending_before_flush, 2);

    let flushed = harness.egress.flush().await.unwrap();
    assert_eq!(flushed, 2);
    assert_eq!(
        harness.store.outbox_store().pending_count().await.unwrap(),
        0
    );

    let snapshot = server.snapshot();
    assert_eq!(snapshot.egress_batches.len(), 1);
    assert_eq!(snapshot.egress_batches[0].items.len(), 2);

    let ack_event = snapshot.egress_batches[0]
        .items
        .iter()
        .find(|item| item.event.subject == "prompt-ack-001")
        .expect("ack prompt receipt event");
    assert_eq!(
        ack_event.event.event_type,
        "io.oneshim.integration.prompt_receipt.v1"
    );
    assert_eq!(ack_event.event.oneshimscope, "prompt:ack");
    assert_eq!(
        ack_event
            .event
            .data
            .get("action")
            .and_then(|value| value.as_str()),
        Some("acknowledged")
    );

    let dismiss_event = snapshot.egress_batches[0]
        .items
        .iter()
        .find(|item| item.event.subject == "prompt-dismiss-001")
        .expect("dismiss prompt receipt event");
    assert_eq!(
        dismiss_event
            .event
            .data
            .get("action")
            .and_then(|value| value.as_str()),
        Some("dismissed")
    );
    assert_eq!(
        dismiss_event
            .event
            .data
            .get("reason")
            .and_then(|value| value.as_str()),
        Some("user dismissed")
    );

    let session = harness.session.current_session().await.unwrap().unwrap();
    assert_eq!(
        session
            .ack_cursors
            .iter()
            .find(|cursor| cursor.stream_id == "integration.egress")
            .map(|cursor| cursor.cursor.as_str()),
        Some("ack-egress-001")
    );
}

#[tokio::test]
async fn runtime_loop_recovers_after_retry_after_rate_limit() {
    let server = FakeIntegrationServer::start().await;
    server.set_egress_rate_limit_once(1);

    let harness = build_connected_runtime_with_scopes(
        &server,
        vec![
            IntegrationCapabilityScope::InsightWrite,
            IntegrationCapabilityScope::PromptRead,
            IntegrationCapabilityScope::PromptAck,
            IntegrationCapabilityScope::SessionManage,
        ],
    )
    .await;
    let telemetry = IntegrationRuntimeTelemetryHandle::default();
    let session_id = harness
        .session
        .current_session()
        .await
        .unwrap()
        .unwrap()
        .session_id;
    let message = sample_insight_message(&session_id, "queue-runtime-001", "packet-runtime-001");
    harness
        .egress
        .enqueue_message(message.envelope, message.payload)
        .await
        .expect("enqueue runtime egress message");

    let runtime = IntegrationRuntimeLoop::new(
        harness.session.clone(),
        harness.egress.clone(),
        harness.inbox.clone(),
        None,
        None,
        Some(telemetry.clone()),
        IntegrationRuntimeLoopProfile {
            requested_scopes: vec![
                IntegrationCapabilityScope::InsightWrite,
                IntegrationCapabilityScope::PromptRead,
                IntegrationCapabilityScope::PromptAck,
                IntegrationCapabilityScope::SessionManage,
            ],
            connect_retry_interval: Duration::from_millis(100),
            heartbeat_interval: Duration::from_secs(2),
            egress_interval: Duration::from_millis(50),
            inbox_refresh_interval: Duration::from_secs(2),
        },
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(async move { runtime.run(shutdown_rx).await });

    tokio::time::sleep(Duration::from_millis(1_700)).await;
    shutdown_tx.send(true).unwrap();
    task.await.unwrap();

    let snapshot = telemetry.snapshot().await.unwrap();
    assert!(snapshot.egress.last_failure_at.is_some());
    assert!(snapshot.egress.last_success_at.is_some());
    assert_eq!(snapshot.egress.consecutive_failures, 0);
    assert!(snapshot.egress.backoff_until.is_none());

    assert_eq!(
        harness.store.outbox_store().pending_count().await.unwrap(),
        0
    );

    let server_snapshot = server.snapshot();
    assert_eq!(server_snapshot.egress_batches.len(), 1);
    assert_eq!(server_snapshot.egress_batches[0].items.len(), 1);
    assert_eq!(
        server_snapshot.egress_batches[0].items[0].event.subject,
        "packet-runtime-001"
    );
}

struct ConnectedIntegrationRuntimeHarness {
    _temp_dir: TempDir,
    store: FileIntegrationStateStore,
    session: Arc<dyn IntegrationSessionPort>,
    inbox: Arc<IntegrationInboxCoordinator>,
    egress: Arc<IntegrationEgressCoordinator>,
}

async fn build_connected_runtime(
    server: &FakeIntegrationServer,
) -> ConnectedIntegrationRuntimeHarness {
    build_connected_runtime_with_scopes(
        server,
        vec![
            IntegrationCapabilityScope::PromptRead,
            IntegrationCapabilityScope::PromptAck,
        ],
    )
    .await
}

async fn build_connected_runtime_with_scopes(
    server: &FakeIntegrationServer,
    requested_scopes: Vec<IntegrationCapabilityScope>,
) -> ConnectedIntegrationRuntimeHarness {
    let temp_dir = tempfile::tempdir().expect("integration runtime tempdir");
    let store = FileIntegrationStateStore::new(temp_dir.path().join("integration.json"))
        .expect("integration state store");
    let transport = Arc::new(build_client(server));
    let session: Arc<dyn IntegrationSessionPort> =
        Arc::new(IntegrationSessionCoordinator::new_with_profile_and_store(
            "device-compat-001",
            transport.clone(),
            Default::default(),
            Some(Arc::new(store.session_store())),
        ));
    session
        .connect(requested_scopes)
        .await
        .expect("bootstrap integration session");

    let inbox_store = Arc::new(store.inbox_store());
    let outbox_store = Arc::new(store.outbox_store());
    let inbox = Arc::new(IntegrationInboxCoordinator::new(
        "device-compat-001",
        session.clone(),
        inbox_store.clone(),
        inbox_store,
        Arc::new(transport.inbox_transport()),
        10,
    ));
    let egress = Arc::new(IntegrationEgressCoordinator::new(
        session.clone(),
        outbox_store,
        Arc::new(transport.egress_transport()),
        10,
    ));

    ConnectedIntegrationRuntimeHarness {
        _temp_dir: temp_dir,
        store,
        session,
        inbox,
        egress,
    }
}

fn build_client(server: &FakeIntegrationServer) -> HttpsIntegrationTransportClient {
    build_bearer_client(server, Duration::from_secs(5))
}

fn build_client_with_timeout(
    server: &FakeIntegrationServer,
    request_timeout: Duration,
) -> HttpsIntegrationTransportClient {
    build_bearer_client(server, request_timeout)
}

fn build_bearer_client(
    server: &FakeIntegrationServer,
    request_timeout: Duration,
) -> HttpsIntegrationTransportClient {
    HttpsIntegrationTransportClient::new(
        HttpsIntegrationTransportConfig::new(server.bootstrap_url(), request_timeout),
        Arc::new(StaticIntegrationAuthPort::new(IntegrationAuthContext {
            access_token: "access-token".to_string(),
            scheme: IntegrationAuthScheme::BearerToken,
            expires_at: None,
            resource_indicator: Some(server.bootstrap_url()),
        })),
        Arc::new(NoopIntegrationRequestProofFactory),
    )
    .expect("fake integration transport client")
}

fn build_dpop_client(server: &FakeIntegrationServer) -> HttpsIntegrationTransportClient {
    HttpsIntegrationTransportClient::new(
        HttpsIntegrationTransportConfig::new(server.bootstrap_url(), Duration::from_secs(5)),
        Arc::new(StaticIntegrationAuthPort::new(IntegrationAuthContext {
            access_token: "access-token".to_string(),
            scheme: IntegrationAuthScheme::DpopBearer,
            expires_at: None,
            resource_indicator: Some(server.bootstrap_url()),
        })),
        Arc::new(StaticIntegrationRequestProofFactory::new(
            "dpop",
            "proof-token",
        )),
    )
    .expect("fake dpop integration transport client")
}

fn connect_request(server: &FakeIntegrationServer) -> IntegrationTransportConnectRequest {
    IntegrationTransportConnectRequest {
        device_id: "device-compat-001".to_string(),
        client_version: "0.3.8".to_string(),
        device_label: Some("compat-runner".to_string()),
        requested_scopes: vec![
            IntegrationCapabilityScope::InsightWrite,
            IntegrationCapabilityScope::PromptRead,
            IntegrationCapabilityScope::PromptAck,
            IntegrationCapabilityScope::SessionManage,
        ],
        preferred_transports: vec![IntegrationTransportKind::HttpsLongPoll],
        supported_auth_schemes: vec![IntegrationAuthScheme::BearerToken],
        resource_indicator: Some(server.bootstrap_url()),
    }
}

fn websocket_connect_request(
    server: &FakeIntegrationServer,
    requested_scopes: Vec<IntegrationCapabilityScope>,
) -> IntegrationTransportConnectRequest {
    websocket_connect_request_with_auth(
        server,
        requested_scopes,
        vec![IntegrationAuthScheme::BearerToken],
    )
}

fn websocket_connect_request_with_auth(
    server: &FakeIntegrationServer,
    requested_scopes: Vec<IntegrationCapabilityScope>,
    supported_auth_schemes: Vec<IntegrationAuthScheme>,
) -> IntegrationTransportConnectRequest {
    IntegrationTransportConnectRequest {
        device_id: "device-compat-001".to_string(),
        client_version: "0.3.8".to_string(),
        device_label: Some("compat-runner".to_string()),
        requested_scopes,
        preferred_transports: vec![IntegrationTransportKind::WebSocket],
        supported_auth_schemes,
        resource_indicator: Some(server.bootstrap_url()),
    }
}

fn sample_insight_message(
    session_id: &str,
    queue_id: &str,
    packet_id: &str,
) -> QueuedIntegrationEgressMessage {
    QueuedIntegrationEgressMessage {
        queue_id: queue_id.to_string(),
        envelope: IntegrationEnvelope {
            envelope_id: format!("env-{packet_id}"),
            schema_version: "integration.envelope.v1".to_string(),
            message_type: IntegrationMessageType::InsightPacket,
            timestamp: Utc::now(),
            nonce: format!("nonce-{packet_id}"),
            origin: IntegrationOrigin {
                device_id: "device-compat-001".to_string(),
                workspace_id: None,
                session_id: Some(session_id.to_string()),
                source: "compatibility-test".to_string(),
            },
            capability_scope: IntegrationCapabilityScope::InsightWrite,
        },
        payload: IntegrationOutboundPayload::Insight(InsightPacket {
            packet_id: packet_id.to_string(),
            summary: "Compatibility harness packet".to_string(),
            derived_tags: vec!["compatibility".to_string()],
            source_window: InsightSourceWindow {
                started_at: Utc::now(),
                ended_at: Utc::now(),
            },
            privacy_classification: IntegrationPrivacyClassification::DerivedSummary,
            audit_reference_id: None,
        }),
        queued_at: Utc::now(),
    }
}

fn sample_prompt_receipt_message(
    session_id: &str,
    queue_id: &str,
    prompt_id: &str,
) -> QueuedIntegrationEgressMessage {
    QueuedIntegrationEgressMessage {
        queue_id: queue_id.to_string(),
        envelope: IntegrationEnvelope {
            envelope_id: format!("env-receipt-{prompt_id}"),
            schema_version: "integration.prompt_receipt.v1".to_string(),
            message_type: IntegrationMessageType::PromptReceipt,
            timestamp: Utc::now(),
            nonce: format!("nonce-receipt-{prompt_id}"),
            origin: IntegrationOrigin {
                device_id: "device-compat-001".to_string(),
                workspace_id: None,
                session_id: Some(session_id.to_string()),
                source: "compatibility-test".to_string(),
            },
            capability_scope: IntegrationCapabilityScope::PromptAck,
        },
        payload: IntegrationOutboundPayload::PromptReceipt(IntegrationPromptReceipt {
            receipt_id: format!("receipt-{prompt_id}"),
            prompt_id: prompt_id.to_string(),
            action: IntegrationPromptReceiptAction::Acknowledged,
            occurred_at: Utc::now(),
            reason: None,
        }),
        queued_at: Utc::now(),
    }
}
