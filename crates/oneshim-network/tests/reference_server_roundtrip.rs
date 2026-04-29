//! Integration test: RemoteSyncTransport <-> reference sync server roundtrip.
//!
//! Starts the reference server, points a RemoteSyncTransport at it,
//! and exercises push / pull / discover_peers through the real HTTP stack.

use oneshim_core::config::RemoteSyncAuth;
use oneshim_core::models::sync::{ChangeSet, ChangeSetKind};
use oneshim_core::ports::sync_transport::SyncTransport;
use oneshim_core::sync::Hlc;

use oneshim_network::sync::reference_server::run_reference_server;
use oneshim_network::sync::RemoteSyncTransport;

const AUTH_TOKEN: &str = "integration-test-token";
const PASSPHRASE: &str = "integration-test-passphrase";

fn make_changeset(device_id: &str, wall_ms: u64) -> ChangeSet {
    ChangeSet {
        kind: ChangeSetKind::Data,
        origin_device_id: device_id.to_string(),
        origin_device_name: format!("Device {device_id}"),
        watermark: Hlc {
            wall_ms,
            counter: 1,
            device_id: device_id.to_string(),
        },
        segments: vec![serde_json::json!({"table": "segments", "device": device_id})],
        regimes: vec![serde_json::json!({"table": "regimes", "device": device_id})],
        ..Default::default()
    }
}

fn make_transport(base_url: &str, device_id: &str) -> RemoteSyncTransport {
    RemoteSyncTransport::new(
        base_url.to_string(),
        device_id.to_string(),
        PASSPHRASE.to_string(),
        RemoteSyncAuth::BearerToken,
        AUTH_TOKEN.to_string(),
    )
    .expect("failed to create transport")
}

#[tokio::test]
async fn push_pull_roundtrip_via_reference_server() {
    let handle = run_reference_server(0, AUTH_TOKEN, PASSPHRASE)
        .await
        .expect("failed to start reference server");
    let base_url = handle.base_url();

    // Device A pushes a changeset
    let transport_a = make_transport(&base_url, "device-a");
    let cs_a = make_changeset("device-a", 1000);
    transport_a.push(&cs_a).await.expect("device-a push failed");

    // Device B pulls -- should see device-a's data
    let transport_b = make_transport(&base_url, "device-b");
    let pulled = transport_b
        .pull(&Hlc::default())
        .await
        .expect("device-b pull failed");
    assert!(pulled.is_some(), "expected data from device-a");
    let pulled = pulled.unwrap();
    assert_eq!(pulled.segments.len(), 1);
    assert_eq!(pulled.regimes.len(), 1);

    // Device A pulls -- should get 204 (no other device's data)
    let pulled_a = transport_a
        .pull(&Hlc::default())
        .await
        .expect("device-a pull failed");
    assert!(
        pulled_a.is_none(),
        "device-a should not see its own data on pull"
    );

    handle.shutdown().await;
}

#[tokio::test]
async fn discover_peers_via_reference_server() {
    let handle = run_reference_server(0, AUTH_TOKEN, PASSPHRASE)
        .await
        .expect("failed to start reference server");
    let base_url = handle.base_url();

    let transport_a = make_transport(&base_url, "device-a");

    // Initially no peers
    let peers = transport_a
        .discover_peers()
        .await
        .expect("discover peers failed");
    assert!(peers.is_empty());

    // Push from device-a and device-b to register them as peers
    transport_a
        .push(&make_changeset("device-a", 100))
        .await
        .expect("push a failed");

    let transport_b = make_transport(&base_url, "device-b");
    transport_b
        .push(&make_changeset("device-b", 200))
        .await
        .expect("push b failed");

    // Now should see 2 peers
    let peers = transport_a
        .discover_peers()
        .await
        .expect("discover peers failed");
    assert_eq!(peers.len(), 2);

    let device_ids: Vec<&str> = peers.iter().map(|p| p.device_id.as_str()).collect();
    assert!(device_ids.contains(&"device-a"));
    assert!(device_ids.contains(&"device-b"));

    handle.shutdown().await;
}

#[tokio::test]
async fn api_key_auth_works_with_reference_server() {
    let handle = run_reference_server(0, AUTH_TOKEN, PASSPHRASE)
        .await
        .expect("failed to start reference server");
    let base_url = handle.base_url();

    // Use API key auth mode
    let transport = RemoteSyncTransport::new(
        base_url.clone(),
        "device-api".to_string(),
        PASSPHRASE.to_string(),
        RemoteSyncAuth::ApiKey,
        AUTH_TOKEN.to_string(),
    )
    .expect("failed to create transport");

    // Push should work with API key
    transport
        .push(&make_changeset("device-api", 300))
        .await
        .expect("push with API key failed");

    // Peers should work with API key
    let peers = transport
        .discover_peers()
        .await
        .expect("discover peers with API key failed");
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].device_id, "device-api");

    handle.shutdown().await;
}

#[tokio::test]
async fn watermark_filtering_across_transport() {
    let handle = run_reference_server(0, AUTH_TOKEN, PASSPHRASE)
        .await
        .expect("failed to start reference server");
    let base_url = handle.base_url();

    let transport_a = make_transport(&base_url, "device-a");
    let transport_b = make_transport(&base_url, "device-b");

    // Device A pushes two changesets at different watermarks
    transport_a
        .push(&make_changeset("device-a", 100))
        .await
        .unwrap();
    transport_a
        .push(&make_changeset("device-a", 300))
        .await
        .unwrap();

    // Device B pulls with watermark at 200 -- should only get wall_ms=300
    let since = Hlc {
        wall_ms: 200,
        counter: 0,
        device_id: "device-b".to_string(),
    };
    let pulled = transport_b.pull(&since).await.unwrap();
    assert!(pulled.is_some());
    let pulled = pulled.unwrap();
    // Only 1 segment from the wall_ms=300 changeset
    assert_eq!(pulled.segments.len(), 1);
    assert_eq!(pulled.regimes.len(), 1);

    handle.shutdown().await;
}

#[tokio::test]
async fn wrong_auth_token_rejected() {
    let handle = run_reference_server(0, AUTH_TOKEN, PASSPHRASE)
        .await
        .expect("failed to start reference server");
    let base_url = handle.base_url();

    let transport = RemoteSyncTransport::new(
        base_url,
        "device-bad".to_string(),
        PASSPHRASE.to_string(),
        RemoteSyncAuth::BearerToken,
        "wrong-token".to_string(),
    )
    .unwrap();

    let result = transport.push(&make_changeset("device-bad", 100)).await;
    assert!(result.is_err(), "push with wrong token should fail");

    let result = transport.discover_peers().await;
    assert!(
        result.is_err(),
        "discover_peers with wrong token should fail"
    );

    handle.shutdown().await;
}
