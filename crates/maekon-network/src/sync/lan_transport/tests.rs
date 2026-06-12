use super::*;
use maekon_core::models::sync::{ChangeSetKind, Tombstone};
use maekon_core::ports::sync_transport::SyncTransport;
use maekon_core::sync::Hlc;

use super::super::lan_discovery::LanPeerInfo;

fn test_pin_store() -> std::sync::Arc<dyn maekon_core::ports::lan_pin_store::LanPinStorePort> {
    std::sync::Arc::new(
        maekon_storage::lan_pin_store_adapter::LanPinStoreAdapter::new(std::sync::Arc::new(
            maekon_storage::sqlite::SqliteStorage::open_in_memory(30).unwrap(),
        )),
    )
}

/// Generate a real self-signed cert + matching SHA-256 fingerprint for a
/// transport whose server is actually connected to over TLS in a roundtrip
/// test. The fingerprint must be re-used as the injected peer's advertised
/// fingerprint so the client's first-contact TOFU cross-check passes.
fn real_tls(device_id: &str) -> (Vec<u8>, Vec<u8>, String) {
    let (cert_pem, key_pem) = crate::sync::lan_tls::generate_self_signed_cert(device_id).unwrap();
    let fingerprint = crate::sync::lan_tls::compute_cert_fingerprint(&cert_pem).unwrap();
    (cert_pem, key_pem, fingerprint)
}

fn test_changeset() -> ChangeSet {
    ChangeSet {
        kind: ChangeSetKind::Data,
        origin_device_id: "dev-a".to_string(),
        origin_device_name: "Test A".to_string(),
        watermark: Hlc {
            wall_ms: 100,
            counter: 1,
            device_id: "dev-a".to_string(),
        },
        segments: vec![serde_json::json!({"id": "seg-1", "origin_device_id": "dev-a"})],
        ..Default::default()
    }
}

#[tokio::test]
async fn transport_start_and_discover_empty() {
    let transport = LanSyncTransport::start(
        "dev-1".to_string(),
        "Test Mac".to_string(),
        "passphrase".to_string(),
        b"cert".to_vec(),
        b"key".to_vec(),
        "fp123".to_string(),
        0,
        false, // don't advertise in test
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();

    assert!(transport.server_port() > 0);

    let peers = transport.discover_peers().await.unwrap();
    assert!(peers.is_empty());

    transport.stop();
}

#[tokio::test]
async fn push_to_no_peers_is_noop() {
    let transport = LanSyncTransport::start(
        "dev-1".to_string(),
        "Test".to_string(),
        "pass".to_string(),
        b"cert".to_vec(),
        b"key".to_vec(),
        "fp".to_string(),
        0,
        false,
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();

    let cs = ChangeSet::default();
    // No peers → best-effort fanout returns Ok(0): no bytes left the device.
    let delivered = transport
        .push(&cs)
        .await
        .expect("push with no peers must return Ok, not Err");
    assert_eq!(
        delivered, 0,
        "push to no peers must report 0 confirmed deliveries"
    );

    transport.stop();
}

#[tokio::test]
async fn pull_from_no_peers_returns_none() {
    let transport = LanSyncTransport::start(
        "dev-1".to_string(),
        "Test".to_string(),
        "pass".to_string(),
        b"cert".to_vec(),
        b"key".to_vec(),
        "fp".to_string(),
        0,
        false,
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();

    let result = transport.pull(&Hlc::default()).await.unwrap();
    assert!(result.is_none());

    transport.stop();
}

#[tokio::test]
async fn push_to_local_server_roundtrip() {
    // Start two transports: "sender" pushes to "receiver"'s server.
    let passphrase = "shared-secret-123";

    // Start receiver with a real TLS cert (sender connects to it over https)
    let (recv_cert, recv_key, recv_fp) = real_tls("receiver");
    let receiver = LanSyncTransport::start(
        "receiver".to_string(),
        "Receiver".to_string(),
        passphrase.to_string(),
        recv_cert,
        recv_key,
        recv_fp.clone(),
        0,
        false,
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();

    let receiver_port = receiver.server_port();

    // Manually inject receiver as a verified peer in sender's peer map
    let sender = LanSyncTransport::start(
        "sender".to_string(),
        "Sender".to_string(),
        passphrase.to_string(),
        b"cert".to_vec(),
        b"key".to_vec(),
        "fp-send".to_string(),
        0,
        false,
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();

    // Inject receiver as a known peer (advertised fingerprint == real cert fp
    // so the first-contact TOFU cross-check passes)
    sender.verified_peers.write().insert(
        "receiver".to_string(),
        LanPeerInfo {
            device_id: "receiver".to_string(),
            device_name: "Receiver".to_string(),
            host: "127.0.0.1".to_string(),
            port: receiver_port,
            fingerprint: recv_fp.clone(),
            version: "1".to_string(),
        },
    );

    // Yield to let the server task start accepting connections
    tokio::task::yield_now().await;

    // Push a changeset (sender will auto-authenticate with receiver). #5211: origin must
    // be the pushing device ("sender"), bound to the authenticated peer.
    let cs = changeset_with_watermark("sender", 100, 1);
    sender.push(&cs).await.unwrap();

    // Verify receiver got it
    let received = receiver.drain_received();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].origin_device_id, "sender");

    sender.stop();
    receiver.stop();
}

#[tokio::test]
async fn pull_from_peer_server() {
    let passphrase = "pull-test-pass";

    // Start a server with an outbound changeset (consumer connects to it over https)
    let (prov_cert, prov_key, prov_fp) = real_tls("provider");
    let provider = LanSyncTransport::start(
        "provider".to_string(),
        "Provider".to_string(),
        passphrase.to_string(),
        prov_cert,
        prov_key,
        prov_fp.clone(),
        0,
        false,
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();

    let provider_port = provider.server_port();
    provider.enqueue_outbound(test_changeset());

    // Start a consumer and inject provider as a peer
    let consumer = LanSyncTransport::start(
        "consumer".to_string(),
        "Consumer".to_string(),
        passphrase.to_string(),
        b"cert".to_vec(),
        b"key".to_vec(),
        "fp-cons".to_string(),
        0,
        false,
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();

    consumer.verified_peers.write().insert(
        "provider".to_string(),
        LanPeerInfo {
            device_id: "provider".to_string(),
            device_name: "Provider".to_string(),
            host: "127.0.0.1".to_string(),
            port: provider_port,
            fingerprint: prov_fp.clone(),
            version: "1".to_string(),
        },
    );

    // Yield to let the server task start accepting connections
    tokio::task::yield_now().await;

    // Pull from provider (consumer will auto-authenticate)
    let pulled = consumer.pull(&Hlc::default()).await.unwrap();

    assert!(pulled.is_some());
    let cs = pulled.unwrap();
    assert_eq!(cs.origin_device_id, "dev-a");
    assert_eq!(cs.segments.len(), 1);

    provider.stop();
    consumer.stop();
}

#[tokio::test]
async fn pull_wrong_passphrase_returns_none() {
    // Server uses one passphrase, client uses another
    let provider = LanSyncTransport::start(
        "provider".to_string(),
        "Provider".to_string(),
        "server-pass".to_string(),
        b"cert".to_vec(),
        b"key".to_vec(),
        "fp".to_string(),
        0,
        false,
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();

    provider.enqueue_outbound(test_changeset());
    let provider_port = provider.server_port();

    let consumer = LanSyncTransport::start(
        "consumer".to_string(),
        "Consumer".to_string(),
        "wrong-pass".to_string(),
        b"cert".to_vec(),
        b"key".to_vec(),
        "fp".to_string(),
        0,
        false,
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();

    consumer.verified_peers.write().insert(
        "provider".to_string(),
        LanPeerInfo {
            device_id: "provider".to_string(),
            device_name: "Provider".to_string(),
            host: "127.0.0.1".to_string(),
            port: provider_port,
            fingerprint: "fp".to_string(),
            version: "1".to_string(),
        },
    );

    tokio::task::yield_now().await;

    // Pull should fail auth and return None (graceful degradation)
    let pulled = consumer.pull(&Hlc::default()).await;
    match pulled {
        Ok(None) => {} // graceful: auth failed, no data
        Err(_) => {}   // also acceptable
        Ok(Some(_)) => panic!("should not succeed with wrong passphrase"),
    }

    provider.stop();
    consumer.stop();
}

#[tokio::test]
async fn token_cache_is_used() {
    let passphrase = "cache-test";

    // Receiver gets a real TLS cert (sender connects to it over https)
    let (recv_cert, recv_key, recv_fp) = real_tls("receiver");
    let receiver = LanSyncTransport::start(
        "receiver".to_string(),
        "Receiver".to_string(),
        passphrase.to_string(),
        recv_cert,
        recv_key,
        recv_fp.clone(),
        0,
        false,
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();

    let receiver_port = receiver.server_port();

    let sender = LanSyncTransport::start(
        "sender".to_string(),
        "Sender".to_string(),
        passphrase.to_string(),
        b"".to_vec(),
        b"".to_vec(),
        "fp".to_string(),
        0,
        false,
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();

    sender.verified_peers.write().insert(
        "receiver".to_string(),
        LanPeerInfo {
            device_id: "receiver".to_string(),
            device_name: "Receiver".to_string(),
            host: "127.0.0.1".to_string(),
            port: receiver_port,
            fingerprint: recv_fp.clone(),
            version: "1".to_string(),
        },
    );

    tokio::task::yield_now().await;

    // First push: authenticates fresh. #5211: origin = the pushing device ("sender").
    let cs1 = changeset_with_watermark("sender", 100, 1);
    sender.push(&cs1).await.unwrap();
    assert_eq!(receiver.drain_received().len(), 1);

    // Verify token is cached
    assert!(sender.token_cache.get("receiver").is_some());

    // Second push: uses cached token (no re-auth)
    let cs2 = changeset_with_watermark("sender", 101, 1);
    sender.push(&cs2).await.unwrap();
    assert_eq!(receiver.drain_received().len(), 1);

    sender.stop();
    receiver.stop();
}

#[tokio::test]
async fn push_to_offline_peer_is_graceful() {
    let transport = LanSyncTransport::start(
        "dev-offline-push".to_string(),
        "Offline Push".to_string(),
        "pass".to_string(),
        b"cert".to_vec(),
        b"key".to_vec(),
        "fp".to_string(),
        0,
        false,
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();

    // Inject a ghost peer at a port where nobody is listening
    transport.verified_peers.write().insert(
        "ghost".to_string(),
        LanPeerInfo {
            device_id: "ghost".to_string(),
            device_name: "Ghost".to_string(),
            host: "127.0.0.1".to_string(),
            port: 19999,
            fingerprint: "fp-ghost".to_string(),
            version: "1".to_string(),
        },
    );

    // Best-effort fanout: connection to ghost peer fails but the overall push
    // returns Ok. The failed peer increments fail_count, not the overall result,
    // so delivered count is 0 (nothing actually left the device).
    let cs = test_changeset();
    let delivered = transport
        .push(&cs)
        .await
        .expect("push must return Ok even when all peers are unreachable");
    assert_eq!(
        delivered, 0,
        "no bytes reached any peer — delivered count must be 0"
    );

    transport.stop();
}

#[tokio::test]
async fn pull_from_offline_peer_returns_none() {
    let transport = LanSyncTransport::start(
        "dev-offline-pull".to_string(),
        "Offline Pull".to_string(),
        "pass".to_string(),
        b"cert".to_vec(),
        b"key".to_vec(),
        "fp".to_string(),
        0,
        false,
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();

    // Inject a ghost peer at a port where nobody is listening
    transport.verified_peers.write().insert(
        "ghost".to_string(),
        LanPeerInfo {
            device_id: "ghost".to_string(),
            device_name: "Ghost".to_string(),
            host: "127.0.0.1".to_string(),
            port: 19999,
            fingerprint: "fp-ghost".to_string(),
            version: "1".to_string(),
        },
    );

    // Pull from offline peer: Ok(None) or Err are both acceptable
    let result = transport.pull(&Hlc::default()).await;
    match result {
        Ok(None) => {} // graceful: no data from unreachable peer
        Err(_) => {}   // also acceptable: connection failed
        Ok(Some(_)) => panic!("should not get data from offline peer"),
    }

    transport.stop();
}

// -----------------------------------------------------------------------
// Task 7: Sync Verification Tests
// -----------------------------------------------------------------------

/// Helper to create a changeset with a specific origin and watermark.
fn changeset_with_watermark(origin: &str, wall_ms: u64, counter: u32) -> ChangeSet {
    ChangeSet {
        kind: ChangeSetKind::Data,
        origin_device_id: origin.to_string(),
        origin_device_name: format!("Device {origin}"),
        watermark: Hlc {
            wall_ms,
            counter,
            device_id: origin.to_string(),
        },
        segments: vec![serde_json::json!({
            "id": format!("seg-{origin}"),
            "origin_device_id": origin
        })],
        ..Default::default()
    }
}

/// Helper to create a transport and inject a peer, returning (transport, peer_port).
///
/// Both transports get real TLS certs and the injected peer fingerprints are
/// set to the matching real values, because bidirectional/concurrent tests
/// connect in BOTH directions (each transport's server is connected to).
async fn start_pair(
    id_a: &str,
    id_b: &str,
    passphrase: &str,
) -> (LanSyncTransport, LanSyncTransport) {
    let (a_cert, a_key, a_fp) = real_tls(id_a);
    let (b_cert, b_key, b_fp) = real_tls(id_b);

    let a = LanSyncTransport::start(
        id_a.to_string(),
        format!("Peer {id_a}"),
        passphrase.to_string(),
        a_cert,
        a_key,
        a_fp.clone(),
        0,
        false,
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();

    let b = LanSyncTransport::start(
        id_b.to_string(),
        format!("Peer {id_b}"),
        passphrase.to_string(),
        b_cert,
        b_key,
        b_fp.clone(),
        0,
        false,
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();

    let a_port = a.server_port();
    let b_port = b.server_port();

    // A knows about B (advertised fingerprint == B's real cert fp)
    a.verified_peers.write().insert(
        id_b.to_string(),
        LanPeerInfo {
            device_id: id_b.to_string(),
            device_name: format!("Peer {id_b}"),
            host: "127.0.0.1".to_string(),
            port: b_port,
            fingerprint: b_fp.clone(),
            version: "1".to_string(),
        },
    );

    // B knows about A (advertised fingerprint == A's real cert fp)
    b.verified_peers.write().insert(
        id_a.to_string(),
        LanPeerInfo {
            device_id: id_a.to_string(),
            device_name: format!("Peer {id_a}"),
            host: "127.0.0.1".to_string(),
            port: a_port,
            fingerprint: a_fp.clone(),
            version: "1".to_string(),
        },
    );

    // Yield to let both servers start accepting connections
    tokio::task::yield_now().await;

    (a, b)
}

#[tokio::test]
async fn bidirectional_sync_roundtrip() {
    let (a, b) = start_pair("bi-a", "bi-b", "bidir-pass").await;

    // A pushes a changeset to B
    let cs_a = changeset_with_watermark("bi-a", 1000, 1);
    a.push(&cs_a).await.unwrap();

    // B should have received A's data
    let received_by_b = b.drain_received();
    assert_eq!(
        received_by_b.len(),
        1,
        "B should receive 1 changeset from A"
    );
    assert_eq!(received_by_b[0].origin_device_id, "bi-a");

    // B pushes a different changeset to A
    let cs_b = changeset_with_watermark("bi-b", 2000, 1);
    b.push(&cs_b).await.unwrap();

    // A should have received B's data
    let received_by_a = a.drain_received();
    assert_eq!(
        received_by_a.len(),
        1,
        "A should receive 1 changeset from B"
    );
    assert_eq!(received_by_a[0].origin_device_id, "bi-b");

    a.stop();
    b.stop();
}

#[tokio::test]
async fn watermark_filtering_skips_old_data() {
    let passphrase = "watermark-test-pass";

    // Provider enqueues a changeset at T=1000 (consumer connects over https)
    let (prov_cert, prov_key, prov_fp) = real_tls("wm-provider");
    let provider = LanSyncTransport::start(
        "wm-provider".to_string(),
        "Provider".to_string(),
        passphrase.to_string(),
        prov_cert,
        prov_key,
        prov_fp.clone(),
        0,
        false,
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();

    let cs = changeset_with_watermark("wm-provider", 1000, 1);
    provider.enqueue_outbound(cs);
    let provider_port = provider.server_port();

    // Consumer connects to the provider
    let consumer = LanSyncTransport::start(
        "wm-consumer".to_string(),
        "Consumer".to_string(),
        passphrase.to_string(),
        b"cert".to_vec(),
        b"key".to_vec(),
        "fp-cons".to_string(),
        0,
        false,
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();

    consumer.verified_peers.write().insert(
        "wm-provider".to_string(),
        LanPeerInfo {
            device_id: "wm-provider".to_string(),
            device_name: "Provider".to_string(),
            host: "127.0.0.1".to_string(),
            port: provider_port,
            fingerprint: prov_fp.clone(),
            version: "1".to_string(),
        },
    );

    tokio::task::yield_now().await;

    // Pull with since before T=1000 should return the changeset
    let since_before = Hlc {
        wall_ms: 500,
        counter: 0,
        device_id: String::new(),
    };
    let pulled = consumer.pull(&since_before).await.unwrap();
    assert!(
        pulled.is_some(),
        "pull with since before the watermark should return data"
    );
    assert_eq!(pulled.unwrap().origin_device_id, "wm-provider");

    // Pull with since after T=1000 should return no data (204)
    let since_after = Hlc {
        wall_ms: 1001,
        counter: 0,
        device_id: String::new(),
    };
    let pulled_none = consumer.pull(&since_after).await.unwrap();
    assert!(
        pulled_none.is_none(),
        "pull with since after the watermark should return no data"
    );

    provider.stop();
    consumer.stop();
}

#[tokio::test]
async fn concurrent_push_pull_no_data_loss() {
    let (a, b) = start_pair("conc-a", "conc-b", "concurrent-pass").await;

    // A and B push simultaneously
    let cs_a = changeset_with_watermark("conc-a", 3000, 1);
    let cs_b = changeset_with_watermark("conc-b", 3000, 2);

    let (res_a, res_b) = tokio::join!(a.push(&cs_a), b.push(&cs_b));
    res_a.unwrap();
    res_b.unwrap();

    // Both should have received each other's data
    let received_by_a = a.drain_received();
    let received_by_b = b.drain_received();

    assert_eq!(
        received_by_a.len(),
        1,
        "A should receive 1 changeset from B"
    );
    assert_eq!(received_by_a[0].origin_device_id, "conc-b");

    assert_eq!(
        received_by_b.len(),
        1,
        "B should receive 1 changeset from A"
    );
    assert_eq!(received_by_b[0].origin_device_id, "conc-a");

    a.stop();
    b.stop();
}

// -----------------------------------------------------------------------
// Conflict Resolution Tests
// -----------------------------------------------------------------------

/// Helper to create a changeset carrying a specific record in a named table.
///
/// The record payload includes a `record_id` for identification and a `value`
/// field to distinguish versions from different devices.
fn changeset_with_record(
    origin: &str,
    wall_ms: u64,
    counter: u32,
    record_id: &str,
    value: &str,
) -> ChangeSet {
    ChangeSet {
        kind: ChangeSetKind::Data,
        origin_device_id: origin.to_string(),
        origin_device_name: format!("Device {origin}"),
        watermark: Hlc {
            wall_ms,
            counter,
            device_id: origin.to_string(),
        },
        segments: vec![serde_json::json!({
            "record_id": record_id,
            "value": value,
            "hlc_wall_ms": wall_ms,
            "hlc_counter": counter,
            "origin_device_id": origin,
        })],
        ..Default::default()
    }
}

/// Two devices push conflicting versions of the same record to a shared
/// server. A puller then reads back the merged result and verifies that
/// the higher-HLC version wins.
///
/// Flow:
///   1. Start a "hub" server and two transports (A, B) that both know the hub.
///   2. Device A pushes record X with HLC (wall_ms=100, counter=1).
///   3. Device B pushes record X with HLC (wall_ms=200, counter=1).
///   4. The hub now holds both changesets in its received queue.
///   5. Hub enqueues both as outbound for a consumer to pull.
///   6. Consumer pulls and gets a merged changeset whose watermark is
///      the higher of the two (wall_ms=200). Both record versions are
///      present in the merged segments so the application-layer merger
///      (ChangeMerger) can apply LWW.
#[tokio::test]
async fn conflict_resolution_higher_hlc_wins_on_pull() {
    let passphrase = "conflict-test-pass";

    // Start a hub server that both devices will push to (clients connect over https)
    let (hub_cert, hub_key, hub_fp) = real_tls("hub");
    let hub = LanSyncTransport::start(
        "hub".to_string(),
        "Hub".to_string(),
        passphrase.to_string(),
        hub_cert,
        hub_key,
        hub_fp.clone(),
        0,
        false,
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();
    let hub_port = hub.server_port();

    let hub_peer = LanPeerInfo {
        device_id: "hub".to_string(),
        device_name: "Hub".to_string(),
        host: "127.0.0.1".to_string(),
        port: hub_port,
        fingerprint: hub_fp.clone(),
        version: "1".to_string(),
    };

    // Start device A and inject hub as a peer
    let dev_a = LanSyncTransport::start(
        "dev-a".to_string(),
        "Device A".to_string(),
        passphrase.to_string(),
        b"cert".to_vec(),
        b"key".to_vec(),
        "fp-a".to_string(),
        0,
        false,
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();
    dev_a
        .verified_peers
        .write()
        .insert("hub".to_string(), hub_peer.clone());

    // Start device B and inject hub as a peer
    let dev_b = LanSyncTransport::start(
        "dev-b".to_string(),
        "Device B".to_string(),
        passphrase.to_string(),
        b"cert".to_vec(),
        b"key".to_vec(),
        "fp-b".to_string(),
        0,
        false,
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();
    dev_b
        .verified_peers
        .write()
        .insert("hub".to_string(), hub_peer.clone());

    tokio::task::yield_now().await;

    // Device A pushes record X at HLC(100, 1)
    let cs_a = changeset_with_record("dev-a", 100, 1, "record-X", "value-from-A");
    dev_a.push(&cs_a).await.unwrap();

    // Device B pushes record X at HLC(200, 1) -- higher wall_ms wins
    let cs_b = changeset_with_record("dev-b", 200, 1, "record-X", "value-from-B");
    dev_b.push(&cs_b).await.unwrap();

    // Hub received both pushes
    let received = hub.drain_received();
    assert_eq!(received.len(), 2, "hub should have received 2 changesets");

    // Verify one is from A (HLC=100) and the other from B (HLC=200)
    let from_a = received
        .iter()
        .find(|cs| cs.origin_device_id == "dev-a")
        .expect("should find changeset from dev-a");
    let from_b = received
        .iter()
        .find(|cs| cs.origin_device_id == "dev-b")
        .expect("should find changeset from dev-b");
    assert_eq!(from_a.watermark.wall_ms, 100);
    assert_eq!(from_b.watermark.wall_ms, 200);

    // Hub enqueues both as outbound for a consumer to pull.
    // The one with the lower HLC goes first to simulate natural order.
    hub.enqueue_outbound(from_a.clone());
    hub.enqueue_outbound(from_b.clone());

    // Start a consumer that pulls from the hub
    let consumer = LanSyncTransport::start(
        "consumer".to_string(),
        "Consumer".to_string(),
        passphrase.to_string(),
        b"cert".to_vec(),
        b"key".to_vec(),
        "fp-cons".to_string(),
        0,
        false,
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();
    consumer
        .verified_peers
        .write()
        .insert("hub".to_string(), hub_peer);

    tokio::task::yield_now().await;

    // Pull everything from the hub (since HLC epoch)
    let pulled = consumer.pull(&Hlc::default()).await.unwrap();
    assert!(pulled.is_some(), "consumer should receive merged changeset");

    let merged = pulled.unwrap();

    // The merged changeset's watermark should be the higher HLC (B's: 200)
    assert_eq!(
        merged.watermark.wall_ms, 200,
        "merged watermark should adopt the higher HLC (200 from dev-b)"
    );

    // Both record versions should be present in segments so the
    // application-layer ChangeMerger can apply LWW resolution.
    assert_eq!(
        merged.segments.len(),
        2,
        "merged segments should contain both record versions"
    );

    // Verify both values are present
    let values: Vec<&str> = merged
        .segments
        .iter()
        .filter_map(|s| s.get("value").and_then(|v| v.as_str()))
        .collect();
    assert!(
        values.contains(&"value-from-A"),
        "should contain A's record version"
    );
    assert!(
        values.contains(&"value-from-B"),
        "should contain B's record version"
    );

    // The consumer can now apply LWW: for records with the same record_id,
    // the version with the higher HLC wins. We verify the HLC ordering holds.
    let hlc_a = Hlc {
        wall_ms: 100,
        counter: 1,
        device_id: "dev-a".to_string(),
    };
    let hlc_b = Hlc {
        wall_ms: 200,
        counter: 1,
        device_id: "dev-b".to_string(),
    };
    assert!(
        hlc_b.is_after(&hlc_a),
        "HLC(200) should be causally after HLC(100)"
    );

    dev_a.stop();
    dev_b.stop();
    hub.stop();
    consumer.stop();
}

/// Same record pushed by two devices with identical wall_ms but different
/// counters. Verifies that the counter acts as the tiebreaker.
#[tokio::test]
async fn conflict_resolution_counter_tiebreaker() {
    let passphrase = "counter-tie-pass";

    let (hub_cert, hub_key, hub_fp) = real_tls("hub-ctr");
    let hub = LanSyncTransport::start(
        "hub-ctr".to_string(),
        "Hub".to_string(),
        passphrase.to_string(),
        hub_cert,
        hub_key,
        hub_fp.clone(),
        0,
        false,
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();
    let hub_port = hub.server_port();

    let hub_peer = LanPeerInfo {
        device_id: "hub-ctr".to_string(),
        device_name: "Hub".to_string(),
        host: "127.0.0.1".to_string(),
        port: hub_port,
        fingerprint: hub_fp.clone(),
        version: "1".to_string(),
    };

    // Device A: record X at HLC(500, 3)
    let dev_a = LanSyncTransport::start(
        "ctr-a".to_string(),
        "Device A".to_string(),
        passphrase.to_string(),
        b"cert".to_vec(),
        b"key".to_vec(),
        "fp-ctr-a".to_string(),
        0,
        false,
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();
    dev_a
        .verified_peers
        .write()
        .insert("hub-ctr".to_string(), hub_peer.clone());

    // Device B: record X at HLC(500, 7) -- same wall_ms, higher counter
    let dev_b = LanSyncTransport::start(
        "ctr-b".to_string(),
        "Device B".to_string(),
        passphrase.to_string(),
        b"cert".to_vec(),
        b"key".to_vec(),
        "fp-ctr-b".to_string(),
        0,
        false,
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();
    dev_b
        .verified_peers
        .write()
        .insert("hub-ctr".to_string(), hub_peer.clone());

    tokio::task::yield_now().await;

    let cs_a = changeset_with_record("ctr-a", 500, 3, "record-Y", "version-A");
    dev_a.push(&cs_a).await.unwrap();

    let cs_b = changeset_with_record("ctr-b", 500, 7, "record-Y", "version-B");
    dev_b.push(&cs_b).await.unwrap();

    let received = hub.drain_received();
    assert_eq!(received.len(), 2);

    // Enqueue both as outbound (lower counter first)
    let lower_first = if received[0].watermark.counter <= received[1].watermark.counter {
        vec![received[0].clone(), received[1].clone()]
    } else {
        vec![received[1].clone(), received[0].clone()]
    };
    for cs in lower_first {
        hub.enqueue_outbound(cs);
    }

    // Consumer pulls the merged result
    let consumer = LanSyncTransport::start(
        "cons-ctr".to_string(),
        "Consumer".to_string(),
        passphrase.to_string(),
        b"cert".to_vec(),
        b"key".to_vec(),
        "fp-cons-ctr".to_string(),
        0,
        false,
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();
    consumer
        .verified_peers
        .write()
        .insert("hub-ctr".to_string(), hub_peer);

    tokio::task::yield_now().await;

    let pulled = consumer.pull(&Hlc::default()).await.unwrap();
    assert!(pulled.is_some());

    let merged = pulled.unwrap();

    // Merged watermark should have the higher counter (7)
    assert_eq!(merged.watermark.wall_ms, 500);
    assert_eq!(
        merged.watermark.counter, 7,
        "merged watermark counter should be 7 (higher counter wins)"
    );

    // Both versions present for LWW resolution
    assert_eq!(merged.segments.len(), 2);

    // Verify HLC ordering: counter=7 > counter=3 when wall_ms is equal
    let hlc_low = Hlc {
        wall_ms: 500,
        counter: 3,
        device_id: "ctr-a".to_string(),
    };
    let hlc_high = Hlc {
        wall_ms: 500,
        counter: 7,
        device_id: "ctr-b".to_string(),
    };
    assert!(
        hlc_high.is_after(&hlc_low),
        "HLC(500,7) should be causally after HLC(500,3)"
    );

    dev_a.stop();
    dev_b.stop();
    hub.stop();
    consumer.stop();
}

/// Same record pushed with identical wall_ms and counter. The device_id
/// string acts as the final lexicographic tiebreaker.
#[tokio::test]
async fn conflict_resolution_device_id_tiebreaker() {
    let passphrase = "devid-tie-pass";

    let (hub_cert, hub_key, hub_fp) = real_tls("hub-did");
    let hub = LanSyncTransport::start(
        "hub-did".to_string(),
        "Hub".to_string(),
        passphrase.to_string(),
        hub_cert,
        hub_key,
        hub_fp.clone(),
        0,
        false,
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();
    let hub_port = hub.server_port();

    let hub_peer = LanPeerInfo {
        device_id: "hub-did".to_string(),
        device_name: "Hub".to_string(),
        host: "127.0.0.1".to_string(),
        port: hub_port,
        fingerprint: hub_fp.clone(),
        version: "1".to_string(),
    };

    // Device "aaa": record X at HLC(300, 1, "aaa")
    let dev_aaa = LanSyncTransport::start(
        "aaa".to_string(),
        "Device AAA".to_string(),
        passphrase.to_string(),
        b"cert".to_vec(),
        b"key".to_vec(),
        "fp-aaa".to_string(),
        0,
        false,
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();
    dev_aaa
        .verified_peers
        .write()
        .insert("hub-did".to_string(), hub_peer.clone());

    // Device "zzz": record X at HLC(300, 1, "zzz") -- same wall+counter, higher device_id
    let dev_zzz = LanSyncTransport::start(
        "zzz".to_string(),
        "Device ZZZ".to_string(),
        passphrase.to_string(),
        b"cert".to_vec(),
        b"key".to_vec(),
        "fp-zzz".to_string(),
        0,
        false,
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();
    dev_zzz
        .verified_peers
        .write()
        .insert("hub-did".to_string(), hub_peer.clone());

    tokio::task::yield_now().await;

    let cs_aaa = changeset_with_record("aaa", 300, 1, "record-Z", "from-aaa");
    dev_aaa.push(&cs_aaa).await.unwrap();

    let cs_zzz = changeset_with_record("zzz", 300, 1, "record-Z", "from-zzz");
    dev_zzz.push(&cs_zzz).await.unwrap();

    let received = hub.drain_received();
    assert_eq!(received.len(), 2);

    for cs in &received {
        hub.enqueue_outbound(cs.clone());
    }

    let consumer = LanSyncTransport::start(
        "cons-did".to_string(),
        "Consumer".to_string(),
        passphrase.to_string(),
        b"cert".to_vec(),
        b"key".to_vec(),
        "fp-cons-did".to_string(),
        0,
        false,
        test_pin_store(),
        None, // extractor: buffer-mode (transport-level test)
        None, // merger
        None, // consent_manager
    )
    .await
    .unwrap();
    consumer
        .verified_peers
        .write()
        .insert("hub-did".to_string(), hub_peer);

    tokio::task::yield_now().await;

    let pulled = consumer.pull(&Hlc::default()).await.unwrap();
    assert!(pulled.is_some());
    let merged = pulled.unwrap();

    // Both segments present
    assert_eq!(merged.segments.len(), 2);

    // Verify HLC ordering: "zzz" > "aaa" when wall_ms and counter are equal
    let hlc_aaa = Hlc {
        wall_ms: 300,
        counter: 1,
        device_id: "aaa".to_string(),
    };
    let hlc_zzz = Hlc {
        wall_ms: 300,
        counter: 1,
        device_id: "zzz".to_string(),
    };
    assert!(
        hlc_zzz.is_after(&hlc_aaa),
        "HLC with device_id 'zzz' should be after 'aaa' (lexicographic tiebreaker)"
    );

    // The merged watermark uses wall_ms/counter comparison only (no device_id),
    // so it picks whichever was later in iteration order. Both are valid since
    // the application layer will compare full HLCs per record. The important
    // invariant is that both records are present.
    assert_eq!(merged.watermark.wall_ms, 300);
    assert_eq!(merged.watermark.counter, 1);

    dev_aaa.stop();
    dev_zzz.stop();
    hub.stop();
    consumer.stop();
}

#[tokio::test]
async fn forget_peer_removes_verified_entry() {
    let (a, _b) = start_pair("fp-a", "fp-b", "pw").await;
    // start_pair pre-seeds fp-b into a.verified_peers.
    assert!(a.verified_peers.read().contains_key("fp-b"));
    a.forget_peer("fp-b").await.unwrap();
    assert!(!a.verified_peers.read().contains_key("fp-b"));
    a.stop();
    _b.stop();
}

#[tokio::test]
async fn forget_peer_invalidates_token_cache() {
    let (a, _b) = start_pair("fp2-a", "fp2-b", "pw").await;
    a.token_cache.put("fp2-b", "cached-token".to_string());
    assert!(a.token_cache.get("fp2-b").is_some());
    a.forget_peer("fp2-b").await.unwrap();
    assert!(a.token_cache.get("fp2-b").is_none());
    a.stop();
    _b.stop();
}

#[tokio::test]
async fn forget_peer_unknown_device_is_idempotent() {
    let (a, _b) = start_pair("fp3-a", "fp3-b", "pw").await;
    let before: Vec<String> = a.verified_peers.read().keys().cloned().collect();
    a.forget_peer("never-seen").await.unwrap();
    let after: Vec<String> = a.verified_peers.read().keys().cloned().collect();
    assert_eq!(before, after, "verified_peers unchanged on unknown forget");
    a.stop();
    _b.stop();
}

#[tokio::test]
async fn forget_peer_clears_pin() {
    let pin_store = test_pin_store();
    pin_store.upsert_pin("peer-x", "fp-x").await.unwrap();
    let transport = LanSyncTransport::start(
        "dev-self".into(),
        "self".into(),
        "passphrase-1234".into(),
        Vec::new(),
        Vec::new(),
        "fp-self".into(),
        0,
        false,
        pin_store.clone(),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    transport.forget_peer("peer-x").await.unwrap();
    assert_eq!(pin_store.get_pin("peer-x").await.unwrap(), None);
}

// ── #5174 LAN storage-backed convergence (production path) ────────────────
// These exercise the REAL storage-backed server (Some(extractor)/Some(merger)) over
// the full TLS + auth + HTTP path between two SqliteStorage-backed transports — the
// path build_sync_engine wires in production. mDNS is off (manual peer injection), so
// they are deterministic and CI-safe; mDNS discovery + real multi-device stay QA-only.

fn full_segment_changeset(origin: &str, seg_id: &str, wall: u64, counter: u32) -> ChangeSet {
    ChangeSet {
        origin_device_id: origin.to_string(),
        origin_device_name: format!("Device {origin}"),
        watermark: Hlc {
            wall_ms: wall,
            counter,
            device_id: origin.to_string(),
        },
        segments: vec![serde_json::json!({
            "id": seg_id, "start_time": "2026-01-01", "end_time": "2026-01-01",
            "duration_secs": 3600, "trigger_reason": "timer", "dominant_category": "Dev",
            "hlc_wall_ms": wall, "hlc_counter": counter, "origin_device_id": origin
        })],
        ..Default::default()
    }
}

#[allow(clippy::type_complexity)]
async fn start_storage_pair(
    id_a: &str,
    id_b: &str,
    passphrase: &str,
    consent_b: Option<Arc<dyn ConsentManagerPort>>,
) -> (
    LanSyncTransport,
    LanSyncTransport,
    Arc<maekon_storage::sqlite::SqliteStorage>,
    Arc<maekon_storage::sqlite::SqliteStorage>,
) {
    use maekon_storage::sqlite::SqliteStorage;
    use maekon_storage::sync_extractor::SqliteSyncExtractor;
    use maekon_storage::sync_merger::SqliteSyncMerger;

    let storage_a = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
    let storage_b = Arc::new(SqliteStorage::open_in_memory(30).unwrap());

    let ex_a: Arc<dyn ChangeExtractor> = Arc::new(SqliteSyncExtractor::new(
        storage_a.connection_arc(),
        id_a.to_string(),
        format!("Peer {id_a}"),
        maekon_core::config::SyncConfig::default(),
    ));
    let mg_a: Arc<dyn ChangeMerger> = Arc::new(SqliteSyncMerger::new(
        storage_a.connection_arc(),
        id_a.to_string(),
    ));
    let ex_b: Arc<dyn ChangeExtractor> = Arc::new(SqliteSyncExtractor::new(
        storage_b.connection_arc(),
        id_b.to_string(),
        format!("Peer {id_b}"),
        maekon_core::config::SyncConfig::default(),
    ));
    let mg_b: Arc<dyn ChangeMerger> = Arc::new(SqliteSyncMerger::new(
        storage_b.connection_arc(),
        id_b.to_string(),
    ));

    let (a_cert, a_key, a_fp) = real_tls(id_a);
    let (b_cert, b_key, b_fp) = real_tls(id_b);

    let a = LanSyncTransport::start(
        id_a.to_string(),
        format!("Peer {id_a}"),
        passphrase.to_string(),
        a_cert,
        a_key,
        a_fp.clone(),
        0,
        false,
        test_pin_store(),
        Some(ex_a),
        Some(mg_a),
        None, // consent_manager: unmanaged in the convergence test (consent gate tested separately)
    )
    .await
    .unwrap();
    let b = LanSyncTransport::start(
        id_b.to_string(),
        format!("Peer {id_b}"),
        passphrase.to_string(),
        b_cert,
        b_key,
        b_fp.clone(),
        0,
        false,
        test_pin_store(),
        Some(ex_b),
        Some(mg_b),
        consent_b, // B's server consent gate (None = unmanaged/permitted)
    )
    .await
    .unwrap();

    let a_port = a.server_port();
    let b_port = b.server_port();
    a.verified_peers.write().insert(
        id_b.to_string(),
        LanPeerInfo {
            device_id: id_b.to_string(),
            device_name: format!("Peer {id_b}"),
            host: "127.0.0.1".to_string(),
            port: b_port,
            fingerprint: b_fp.clone(),
            version: "1".to_string(),
        },
    );
    b.verified_peers.write().insert(
        id_a.to_string(),
        LanPeerInfo {
            device_id: id_a.to_string(),
            device_name: format!("Peer {id_a}"),
            host: "127.0.0.1".to_string(),
            port: a_port,
            fingerprint: a_fp.clone(),
            version: "1".to_string(),
        },
    );
    tokio::task::yield_now().await;
    (a, b, storage_a, storage_b)
}

#[tokio::test]
async fn storage_backed_push_then_pull_use_real_db() {
    // The full production loop over real storage:
    //  (1) PUSH: A pushes a changeset → B's server applies it via B's MERGER → the row
    //      lands in B's SQLite. push() awaits B's 200, sent only after the merge commits,
    //      so the assert is race-free.
    //  (2) PULL: A pulls from B → B's EXTRACTOR serves that same row from B's SQLite
    //      (not an in-memory buffer).
    let (a, _b, _sa, sb) = start_storage_pair("conv-a", "conv-b", "conv-pass", None).await;

    // (1) push → merger → B's DB
    // #5211: a push's origin = the pushing device (conv-a), bound to the authenticated peer.
    let cs = full_segment_changeset("conv-a", "seg-x", 100, 1);
    assert_eq!(a.push(&cs).await.unwrap(), 1, "delivered to the one peer");
    let count: i64 = {
        let conn = sb.connection_arc();
        let read = conn.read_lock();
        read.conn()
            .query_row(
                "SELECT COUNT(*) FROM activity_segments WHERE id='seg-x'",
                [],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert_eq!(count, 1, "B's storage converged via the merger (push path)");

    // (2) pull → extractor serves B's real row
    let pulled = a
        .pull(&Hlc::default())
        .await
        .unwrap()
        .expect("B served its real extractor data");
    assert_eq!(
        pulled.segments.len(),
        1,
        "B's extractor served the row (pull path)"
    );
    assert_eq!(pulled.segments[0]["id"], "seg-x");
}

/// A consent manager that denies everything (GDPR default — nothing granted).
struct DenyingConsent;
impl maekon_core::ports::consent_manager::ConsentManagerPort for DenyingConsent {
    fn check_consent(&self) -> maekon_core::consent::ConsentStatus {
        maekon_core::consent::ConsentStatus::NotGranted
    }
    fn current_consent(&self) -> Option<maekon_core::consent::ConsentRecord> {
        None
    }
    fn effective_permissions(&self) -> maekon_core::consent::ConsentPermissions {
        maekon_core::consent::ConsentPermissions::default() // all false
    }
    fn status_and_permissions(
        &self,
    ) -> (
        maekon_core::consent::ConsentStatus,
        maekon_core::consent::ConsentPermissions,
    ) {
        (
            maekon_core::consent::ConsentStatus::NotGranted,
            maekon_core::consent::ConsentPermissions::default(),
        )
    }
    fn grant_consent(
        &self,
        _: maekon_core::consent::ConsentPermissions,
        _: u32,
    ) -> Result<(), maekon_core::error::CoreError> {
        Ok(())
    }
    fn revoke_consent(&self) -> Result<(), maekon_core::error::CoreError> {
        Ok(())
    }
    fn has_pending_deletion(&self) -> bool {
        false
    }
    fn pending_erasure_id(&self) -> Option<String> {
        None
    }
    fn clear_pending_deletion(&self) {}
    fn deletion_flag(&self) -> Arc<std::sync::atomic::AtomicBool> {
        Arc::new(std::sync::atomic::AtomicBool::new(false))
    }
    fn erasing(&self) -> Arc<std::sync::atomic::AtomicBool> {
        Arc::new(std::sync::atomic::AtomicBool::new(false))
    }
}

#[tokio::test]
async fn server_refuses_push_without_cross_device_sync_consent() {
    // #5174 consent gate: B's server has NO cross_device_sync consent → it refuses an
    // authenticated peer's push (403). Nothing lands in B's SQLite. (The storage-backed
    // server is a real data channel, so auth alone must NOT be sufficient.)
    let deny: Arc<dyn ConsentManagerPort> = Arc::new(DenyingConsent);
    let (a, _b, _sa, sb) = start_storage_pair("deny-a", "deny-b", "deny-pass", Some(deny)).await;

    let cs = full_segment_changeset("deny-a", "seg-deny", 100, 1);
    let delivered = a.push(&cs).await.unwrap();
    assert_eq!(
        delivered, 0,
        "B refused the push without consent (no delivery)"
    );

    let count: i64 = {
        let conn = sb.connection_arc();
        let read = conn.read_lock();
        read.conn()
            .query_row(
                "SELECT COUNT(*) FROM activity_segments WHERE id='seg-deny'",
                [],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert_eq!(count, 0, "nothing landed in B's storage without consent");

    // And a pull from B is refused too (403 → no data).
    assert!(
        a.pull(&Hlc::default()).await.unwrap().is_none(),
        "B refused to serve its data without cross_device_sync consent"
    );
}

#[tokio::test]
async fn server_rejects_push_with_spoofed_origin() {
    // #5211: an authenticated peer may only push ITS OWN changeset. A changeset claiming a
    // THIRD device's origin (e.g. a spoofed device-wide DeletionEvent) is rejected — auth
    // proves who is pushing, and a peer cannot forge another device's origin_device_id.
    let (a, _b, _sa, sb) = start_storage_pair("spoof-a", "spoof-b", "spoof-pass", None).await;
    // origin = "victim-c", NOT the pushing device (spoof-a) → must be rejected.
    let cs = full_segment_changeset("victim-c", "seg-spoof", 100, 1);
    let delivered = a.push(&cs).await.unwrap();
    assert_eq!(delivered, 0, "B rejected the spoofed-origin push");

    let count: i64 = {
        let conn = sb.connection_arc();
        let read = conn.read_lock();
        read.conn()
            .query_row(
                "SELECT COUNT(*) FROM activity_segments WHERE id='seg-spoof'",
                [],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert_eq!(count, 0, "nothing landed under a forged origin");
}

#[tokio::test]
async fn server_rejects_push_with_spoofed_row_origin() {
    // #5211: top-level origin binding is not enough. A peer that authenticates as itself
    // must not smuggle rows attributed to a third device inside an otherwise valid push.
    let (a, _b, _sa, sb) =
        start_storage_pair("row-spoof-a", "row-spoof-b", "row-spoof-pass", None).await;

    let mut cs = full_segment_changeset("row-spoof-a", "seg-row-spoof", 100, 1);
    cs.segments[0]["origin_device_id"] = serde_json::json!("victim-c");

    let delivered = a.push(&cs).await.unwrap();
    assert_eq!(delivered, 0, "B rejected the spoofed row-origin push");

    let count: i64 = {
        let conn = sb.connection_arc();
        let read = conn.read_lock();
        read.conn()
            .query_row(
                "SELECT COUNT(*) FROM activity_segments WHERE id='seg-row-spoof'",
                [],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert_eq!(count, 0, "nothing landed under a forged row origin");
}

#[tokio::test]
async fn server_accepts_relayed_tombstone_for_original_row_origin() {
    // #5174/#5211: tombstones are content-free erasure carriers. They keep the erased
    // row's original origin so a relay peer can help an offline receiver converge.
    let (relay, _receiver, _sa, sb) =
        start_storage_pair("relay-a", "relay-b", "relay-pass", None).await;

    {
        let conn = sb.connection_arc();
        let inserted = conn
            .write_lock()
            .run(0, |conn| {
                conn.execute(
                    "INSERT INTO activity_segments (id, start_time, end_time, duration_secs, \
                     trigger_reason, dominant_category, hlc_wall_ms, hlc_counter, origin_device_id) \
                     VALUES ('seg-relayed-delete', '2026-01-01', '2026-01-01', 3600, 'timer', 'Dev', 100, 1, 'origin-a')",
                    [],
                )
            })
            .unwrap();
        assert_eq!(inserted, 1, "receiver fixture row inserted");
    }

    let cs = ChangeSet {
        origin_device_id: "relay-a".to_string(),
        origin_device_name: "Peer relay-a".to_string(),
        watermark: Hlc {
            wall_ms: 200,
            counter: 0,
            device_id: "relay-a".to_string(),
        },
        tombstones: vec![Tombstone {
            table_name: "activity_segments".to_string(),
            row_id: "seg-relayed-delete".to_string(),
            origin_device_id: "origin-a".to_string(),
            hlc_wall_ms: 200,
            hlc_counter: 0,
            deleted_at: "2026-06-07T00:00:00Z".to_string(),
        }],
        ..Default::default()
    };

    let delivered = relay.push(&cs).await.unwrap();
    assert_eq!(delivered, 1, "relay tombstone reached the receiver");

    let count: i64 = {
        let conn = sb.connection_arc();
        let read = conn.read_lock();
        read.conn()
            .query_row(
                "SELECT COUNT(*) FROM activity_segments WHERE id='seg-relayed-delete'",
                [],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert_eq!(count, 0, "receiver converged to the relayed erasure");
}
