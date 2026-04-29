use super::*;
use oneshim_core::models::sync::ChangeSetKind;
use oneshim_core::sync::Hlc;

use crate::sync::{lan_crypto, sync_crypto};

fn test_changeset() -> ChangeSet {
    ChangeSet {
        kind: ChangeSetKind::Data,
        origin_device_id: "peer-1".to_string(),
        origin_device_name: "Peer Mac".to_string(),
        watermark: Hlc {
            wall_ms: 100,
            counter: 1,
            device_id: "peer-1".to_string(),
        },
        segments: vec![serde_json::json!({"id": "seg-1"})],
        ..Default::default()
    }
}

/// Helper: perform challenge-response to get a session token.
async fn authenticate(
    client: &reqwest::Client,
    base: &str,
    passphrase: &str,
    local_device_id: &str,
    server_device_id: &str,
) -> String {
    // Step 1: request challenge
    let challenge_resp = client
        .post(format!("{base}/sync/challenge"))
        .json(&ChallengeRequest {
            device_id: local_device_id.to_string(),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(challenge_resp.status(), 200);
    let challenge: ChallengeResponse = challenge_resp.json().await.unwrap();

    // Step 2: compute HMAC response
    let nonce_bytes = hex::decode(&challenge.nonce).unwrap();
    let hmac_response = lan_crypto::compute_challenge_response(
        &nonce_bytes,
        passphrase,
        local_device_id,
        server_device_id,
    )
    .unwrap();

    // Step 3: verify
    let verify_resp = client
        .post(format!("{base}/sync/verify"))
        .json(&VerifyRequest {
            device_id: local_device_id.to_string(),
            nonce: challenge.nonce.clone(),
            response: hex::encode(&hmac_response),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(verify_resp.status(), 200);
    let verify: VerifyResponse = verify_resp.json().await.unwrap();
    assert!(!verify.session_token.is_empty());
    assert!(verify.expires_in_secs > 0);

    verify.session_token
}

#[tokio::test]
async fn server_start_stop() {
    let mut server = LanPeerServer::new(
        "dev-1".to_string(),
        "Test".to_string(),
        "pass".to_string(),
        "fp123".to_string(),
    );
    assert!(!server.is_running());

    let port = server.start(b"cert", b"key", 0).await.unwrap();
    assert!(port > 0);
    assert!(server.is_running());
    assert!(!server.is_tls_enabled()); // invalid PEM -> fallback

    server.stop();
    assert!(!server.is_running());
}

#[tokio::test]
async fn info_endpoint() {
    let mut server = LanPeerServer::new(
        "dev-info".to_string(),
        "Info Test".to_string(),
        "pass".to_string(),
        "fp-info".to_string(),
    );
    let port = server.start(b"cert", b"key", 0).await.unwrap();

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/sync/info"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let info: DeviceInfoResponse = resp.json().await.unwrap();
    assert_eq!(info.device_id, "dev-info");
    assert_eq!(info.device_name, "Info Test");
    assert_eq!(info.fingerprint, "fp-info");
    assert_eq!(info.protocol_version, PROTOCOL_VERSION);

    server.stop();
}

#[tokio::test]
async fn challenge_verify_flow() {
    let passphrase = "shared-secret";
    let mut server = LanPeerServer::new(
        "server-dev".to_string(),
        "Server".to_string(),
        passphrase.to_string(),
        "fp".to_string(),
    );
    let port = server.start(b"", b"", 0).await.unwrap();
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let token = authenticate(&client, &base, passphrase, "client-dev", "server-dev").await;
    assert!(!token.is_empty());
    assert_eq!(token.len(), 64); // 32 bytes hex-encoded

    server.stop();
}

#[tokio::test]
async fn challenge_wrong_passphrase_fails() {
    let mut server = LanPeerServer::new(
        "server-dev".to_string(),
        "Server".to_string(),
        "correct-pass".to_string(),
        "fp".to_string(),
    );
    let port = server.start(b"", b"", 0).await.unwrap();
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // Get challenge
    let challenge_resp = client
        .post(format!("{base}/sync/challenge"))
        .json(&ChallengeRequest {
            device_id: "client-dev".to_string(),
        })
        .send()
        .await
        .unwrap();
    let challenge: ChallengeResponse = challenge_resp.json().await.unwrap();
    let nonce_bytes = hex::decode(&challenge.nonce).unwrap();

    // Compute HMAC with wrong passphrase
    let hmac_response = lan_crypto::compute_challenge_response(
        &nonce_bytes,
        "wrong-pass",
        "client-dev",
        "server-dev",
    )
    .unwrap();

    // Verify should fail
    let verify_resp = client
        .post(format!("{base}/sync/verify"))
        .json(&VerifyRequest {
            device_id: "client-dev".to_string(),
            nonce: challenge.nonce,
            response: hex::encode(&hmac_response),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(verify_resp.status(), 401);

    server.stop();
}

#[tokio::test]
async fn pull_push_require_auth() {
    let mut server = LanPeerServer::new(
        "dev-auth".to_string(),
        "Auth".to_string(),
        "pass".to_string(),
        "fp".to_string(),
    );
    let port = server.start(b"", b"", 0).await.unwrap();
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");

    // Pull without token -> 401
    let pull_resp = client
        .get(format!("{base}/sync/pull?since_wall_ms=0&since_counter=0"))
        .send()
        .await
        .unwrap();
    assert_eq!(pull_resp.status(), 401);

    // Push without token -> 401
    let push_resp = client
        .post(format!("{base}/sync/push"))
        .body(vec![1, 2, 3])
        .send()
        .await
        .unwrap();
    assert_eq!(push_resp.status(), 401);

    // Pull with invalid token -> 401
    let pull_resp = client
        .get(format!("{base}/sync/pull?since_wall_ms=0&since_counter=0"))
        .header("authorization", "Bearer invalid-token")
        .send()
        .await
        .unwrap();
    assert_eq!(pull_resp.status(), 401);

    server.stop();
}

#[tokio::test]
async fn pull_returns_204_when_empty() {
    let passphrase = "test-pass";
    let mut server = LanPeerServer::new(
        "dev-pull".to_string(),
        "Pull Test".to_string(),
        passphrase.to_string(),
        "fp".to_string(),
    );
    let port = server.start(b"", b"", 0).await.unwrap();
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let token = authenticate(&client, &base, passphrase, "client-dev", "dev-pull").await;

    let resp = client
        .get(format!("{base}/sync/pull?since_wall_ms=0&since_counter=0"))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 204);
    server.stop();
}

#[tokio::test]
async fn push_and_pull_roundtrip() {
    let passphrase = "test-roundtrip-pass";
    let server_id = "dev-rt";
    let client_id = "client-rt";

    let mut server = LanPeerServer::new(
        server_id.to_string(),
        "Roundtrip".to_string(),
        passphrase.to_string(),
        "fp".to_string(),
    );
    let port = server.start(b"", b"", 0).await.unwrap();
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // Authenticate
    let token = authenticate(&client, &base, passphrase, client_id, server_id).await;

    // Push an encrypted changeset
    let cs = test_changeset();
    let json = serde_json::to_vec(&cs).unwrap();
    let encrypted = sync_crypto::encrypt(passphrase, &json).unwrap();

    let push_resp = client
        .post(format!("{base}/sync/push"))
        .header("authorization", format!("Bearer {token}"))
        .body(encrypted)
        .send()
        .await
        .unwrap();
    assert_eq!(push_resp.status(), 200);

    // Verify the server received it
    let received = server.drain_received();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].origin_device_id, "peer-1");

    // Enqueue an outbound changeset and pull it
    let outbound_cs = ChangeSet {
        origin_device_id: server_id.to_string(),
        origin_device_name: "Roundtrip".to_string(),
        watermark: Hlc {
            wall_ms: 200,
            counter: 1,
            device_id: server_id.to_string(),
        },
        segments: vec![serde_json::json!({"id": "seg-out"})],
        ..Default::default()
    };
    server.enqueue_outbound(outbound_cs);

    let pull_resp = client
        .get(format!("{base}/sync/pull?since_wall_ms=0&since_counter=0"))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(pull_resp.status(), 200);

    let pull_bytes = pull_resp.bytes().await.unwrap();
    let decrypted = sync_crypto::decrypt(passphrase, &pull_bytes).unwrap();
    let pulled: Vec<ChangeSet> = serde_json::from_slice(&decrypted).unwrap();
    assert_eq!(pulled.len(), 1);
    assert_eq!(pulled[0].origin_device_id, server_id);

    server.stop();
}

#[tokio::test]
async fn push_wrong_passphrase_returns_400() {
    let server_pass = "correct-pass";
    let server_id = "dev-auth";

    let mut server = LanPeerServer::new(
        server_id.to_string(),
        "Auth Test".to_string(),
        server_pass.to_string(),
        "fp".to_string(),
    );
    let port = server.start(b"", b"", 0).await.unwrap();
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // Authenticate with correct passphrase
    let token = authenticate(&client, &base, server_pass, "client-1", server_id).await;

    // But encrypt the payload with wrong passphrase
    let cs = test_changeset();
    let json = serde_json::to_vec(&cs).unwrap();
    let encrypted = sync_crypto::encrypt("wrong-pass", &json).unwrap();

    let resp = client
        .post(format!("{base}/sync/push"))
        .header("authorization", format!("Bearer {token}"))
        .body(encrypted)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400);
    server.stop();
}

#[tokio::test]
async fn push_empty_body_returns_400() {
    let passphrase = "pass";
    let server_id = "dev-empty";

    let mut server = LanPeerServer::new(
        server_id.to_string(),
        "Empty".to_string(),
        passphrase.to_string(),
        "fp".to_string(),
    );
    let port = server.start(b"", b"", 0).await.unwrap();
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let token = authenticate(&client, &base, passphrase, "client-1", server_id).await;

    let resp = client
        .post(format!("{base}/sync/push"))
        .header("authorization", format!("Bearer {token}"))
        .body(vec![])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400);
    server.stop();
}

#[tokio::test]
async fn nonce_is_single_use() {
    let passphrase = "single-use";
    let server_id = "dev-nonce";

    let mut server = LanPeerServer::new(
        server_id.to_string(),
        "Nonce".to_string(),
        passphrase.to_string(),
        "fp".to_string(),
    );
    let port = server.start(b"", b"", 0).await.unwrap();
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // Get a challenge
    let challenge_resp = client
        .post(format!("{base}/sync/challenge"))
        .json(&ChallengeRequest {
            device_id: "client-dev".to_string(),
        })
        .send()
        .await
        .unwrap();
    let challenge: ChallengeResponse = challenge_resp.json().await.unwrap();
    let nonce_bytes = hex::decode(&challenge.nonce).unwrap();

    // First verify should succeed
    let hmac_response =
        lan_crypto::compute_challenge_response(&nonce_bytes, passphrase, "client-dev", server_id)
            .unwrap();

    let verify_resp = client
        .post(format!("{base}/sync/verify"))
        .json(&VerifyRequest {
            device_id: "client-dev".to_string(),
            nonce: challenge.nonce.clone(),
            response: hex::encode(&hmac_response),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(verify_resp.status(), 200);

    // Second verify with same nonce should fail (consumed)
    let verify_resp2 = client
        .post(format!("{base}/sync/verify"))
        .json(&VerifyRequest {
            device_id: "client-dev".to_string(),
            nonce: challenge.nonce,
            response: hex::encode(&hmac_response),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(verify_resp2.status(), 401);

    server.stop();
}

#[test]
fn fingerprint_accessible() {
    let server = LanPeerServer::new(
        "dev-1".to_string(),
        "Test".to_string(),
        "pass".to_string(),
        "abc123def".to_string(),
    );
    assert_eq!(server.fingerprint(), "abc123def");
}

#[test]
fn enqueue_and_drain() {
    let server = LanPeerServer::new(
        "dev-q".to_string(),
        "Queue".to_string(),
        "pass".to_string(),
        "fp".to_string(),
    );
    assert!(server.drain_received().is_empty());

    server.enqueue_outbound(test_changeset());
    // outbound is separate from received
    assert!(server.drain_received().is_empty());
}

#[test]
fn session_store_basics() {
    let store = SessionStore::new();

    // Create nonce
    let nonce = store.create_nonce("peer-1");
    assert_eq!(nonce.len(), 32);

    // Take nonce -- single use
    let hex = hex::encode(&nonce);
    let taken = store.take_nonce(&hex);
    assert!(taken.is_some());
    let (bytes, peer_id) = taken.unwrap();
    assert_eq!(bytes, nonce);
    assert_eq!(peer_id, "peer-1");

    // Second take fails
    assert!(store.take_nonce(&hex).is_none());

    // Create and validate session
    let token = store.create_session("peer-1");
    assert!(store.validate_token(&token));
    assert!(!store.validate_token("invalid"));
}

#[tokio::test]
async fn pull_watermark_filtering() {
    let passphrase = "wm-filter-pass";
    let server_id = "dev-wm";

    let mut server = LanPeerServer::new(
        server_id.to_string(),
        "WM Filter".to_string(),
        passphrase.to_string(),
        "fp".to_string(),
    );
    let port = server.start(b"", b"", 0).await.unwrap();
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let token = authenticate(&client, &base, passphrase, "client-wm", server_id).await;

    // Enqueue 3 changesets with watermarks 100, 200, 300
    for wm in [100u64, 200, 300] {
        server.enqueue_outbound(ChangeSet {
            origin_device_id: server_id.to_string(),
            origin_device_name: "WM Filter".to_string(),
            watermark: Hlc {
                wall_ms: wm,
                counter: 1,
                device_id: server_id.to_string(),
            },
            segments: vec![serde_json::json!({"wm": wm})],
            ..Default::default()
        });
    }

    // Pull with since_wall_ms=200, since_counter=1 -> only wm=300 returned
    // (wm=200,counter=1 does NOT pass because counter is not > 1)
    let resp = client
        .get(format!(
            "{base}/sync/pull?since_wall_ms=200&since_counter=1"
        ))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let bytes = resp.bytes().await.unwrap();
    let decrypted = sync_crypto::decrypt(passphrase, &bytes).unwrap();
    let pulled: Vec<ChangeSet> = serde_json::from_slice(&decrypted).unwrap();
    assert_eq!(pulled.len(), 1);
    assert_eq!(pulled[0].watermark.wall_ms, 300);

    // Pull with since_wall_ms=0 -> all 3 returned
    let resp = client
        .get(format!("{base}/sync/pull?since_wall_ms=0&since_counter=0"))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let bytes = resp.bytes().await.unwrap();
    let decrypted = sync_crypto::decrypt(passphrase, &bytes).unwrap();
    let pulled: Vec<ChangeSet> = serde_json::from_slice(&decrypted).unwrap();
    assert_eq!(pulled.len(), 3);

    // Pull with since_wall_ms=300, since_counter=1 -> nothing newer -> 204
    let resp = client
        .get(format!(
            "{base}/sync/pull?since_wall_ms=300&since_counter=1"
        ))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    server.stop();
}

#[tokio::test]
async fn multiple_changesets_ordering() {
    let passphrase = "ordering-pass";
    let server_id = "dev-order";

    let mut server = LanPeerServer::new(
        server_id.to_string(),
        "Ordering".to_string(),
        passphrase.to_string(),
        "fp".to_string(),
    );
    let port = server.start(b"", b"", 0).await.unwrap();
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let token = authenticate(&client, &base, passphrase, "client-ord", server_id).await;

    // Enqueue 5 changesets with increasing watermarks
    for wm in [10u64, 20, 30, 40, 50] {
        server.enqueue_outbound(ChangeSet {
            origin_device_id: server_id.to_string(),
            origin_device_name: "Ordering".to_string(),
            watermark: Hlc {
                wall_ms: wm,
                counter: 1,
                device_id: server_id.to_string(),
            },
            segments: vec![serde_json::json!({"wm": wm})],
            ..Default::default()
        });
    }

    // Pull all (since_wall_ms=0)
    let resp = client
        .get(format!("{base}/sync/pull?since_wall_ms=0&since_counter=0"))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let bytes = resp.bytes().await.unwrap();
    let decrypted = sync_crypto::decrypt(passphrase, &bytes).unwrap();
    let pulled: Vec<ChangeSet> = serde_json::from_slice(&decrypted).unwrap();

    assert_eq!(pulled.len(), 5);
    // Verify watermark ascending order
    let watermarks: Vec<u64> = pulled.iter().map(|cs| cs.watermark.wall_ms).collect();
    assert_eq!(watermarks, vec![10, 20, 30, 40, 50]);
}

#[tokio::test]
async fn server_restart_same_port() {
    let mut server = LanPeerServer::new(
        "dev-restart".to_string(),
        "Restart".to_string(),
        "pass".to_string(),
        "fp-restart".to_string(),
    );

    // Start and record the port
    let port = server.start(b"", b"", 0).await.unwrap();
    assert!(server.is_running());

    let client = reqwest::Client::new();

    // Verify it responds
    let resp = client
        .get(format!("http://127.0.0.1:{port}/sync/info"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Stop the server
    server.stop();
    assert!(!server.is_running());

    // Wait for the port to be released
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Restart on the same port
    let port2 = server.start(b"", b"", port).await.unwrap();
    assert_eq!(port2, port);
    assert!(server.is_running());

    // Verify the restarted server responds
    let resp = client
        .get(format!("http://127.0.0.1:{port2}/sync/info"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let info: DeviceInfoResponse = resp.json().await.unwrap();
    assert_eq!(info.device_id, "dev-restart");

    server.stop();
}

#[tokio::test]
async fn tls_with_real_cert() {
    use super::super::lan_tls;

    // Generate a real self-signed cert
    let (cert_pem, key_pem) = lan_tls::generate_self_signed_cert("test-tls-dev").unwrap();

    let mut server = LanPeerServer::new(
        "test-tls-dev".to_string(),
        "TLS Test".to_string(),
        "pass".to_string(),
        "fp-tls".to_string(),
    );
    let port = server.start(&cert_pem, &key_pem, 0).await.unwrap();
    assert!(port > 0);
    assert!(server.is_running());
    assert!(server.is_tls_enabled());

    // Verify TLS is active by confirming a plain HTTP connection fails.
    // Note: reqwest in this project uses native-tls, which may have protocol
    // compatibility issues with rustls on some platforms. So instead we verify
    // that plain HTTP to a TLS port gets rejected (connection reset or error).
    let plain_result = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/sync/info"))
        .send()
        .await;
    assert!(
        plain_result.is_err(),
        "plain HTTP should not succeed against TLS server"
    );

    server.stop();
}
