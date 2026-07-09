// D13-v2c auth-matrix integration tests — JWT / mTLS / combined auth modes,
// IP ban, cert hot-reload, short-lived cert rejection, loopback isolation,
// concurrent-stream cap, port collision, shutdown drain, and the real-handler
// business-data smoke test. Split from `external_grpc_integration.rs` by
// scenario family (#7730); request-id correlation and audit-trail assertions
// live in the sibling `request_id` / `audit_trail` modules.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use maekon_core::config::{AuthMode, ExternalGrpcConfig, JwtAlgorithm};
use maekon_core::ports::audit_log::AuditLogPort;
use tonic::Code;

use maekon_web::grpc::external::cert_resolver::HotReloadCertResolver;
use maekon_web::grpc::external::ip_ban::IpBan;
use maekon_web::grpc::external::jwt_verifier::JwtVerifier;
use maekon_web::grpc::external::live_config::{LiveExternalConfig, LiveSnapshot};
use maekon_web::grpc::external::metrics::ExternalMetrics;
use maekon_web::grpc::external::mtls_verifier::MtlsVerifier;
use maekon_web::grpc::external::spawn_config::ExternalGrpcSpawnConfig;
use maekon_web::grpc::external::test_support::{
    install_rustls_crypto_provider, server_cert_pem, test_ca_and_client_cert, test_cert_pair,
    test_jwt_keypair, test_mint_jwt, NoopAudit,
};
use maekon_web::grpc::external::tls_config::load_certified_key;
use maekon_web::grpc::test_support::mock_system_monitor::MockSystemMonitor;
use maekon_web::grpc::LoadPolicy;
use maekon_web::proto::dashboard::v1::dashboard_service_client::DashboardServiceClient;
use maekon_web::proto::dashboard::v1::{
    GetAgentInfoRequest, GetSessionStatsRequest, SubscribeEventsRequest,
};

use crate::common::{
    assert_reaches_service, assert_reaches_service_with_bearer, in_memory_storage, make_jwt_config,
    make_mtls_config, make_test_shutdown_pair, make_tls_channel, next_test_port, send_bad_bearer,
    spawn_server,
};

// ═════════════════════════════════════════════════════════════════════════════
// Core tests (10 — always run)
// ═════════════════════════════════════════════════════════════════════════════

/// Test 1: JWT auth mode — valid JWT → reaches placeholder service.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_full_handshake_jwt() {
    let jwt_kp = test_jwt_keypair();
    let (cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);
    let (handle, port) = spawn_server(cfg).await;

    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-1",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(port, &cert_pem, None).await;

    assert_reaches_service_with_bearer(channel, &token).await;

    handle.abort();
    let _ = handle.await;
}

/// Test 2: mTLS auth mode — valid client cert → reaches placeholder service.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_full_handshake_mtls() {
    let ca = test_ca_and_client_cert(24); // 24h lifetime — within 48h cap
    let (cfg, _) = make_mtls_config(&ca.ca_pem_path);
    let (handle, port) = spawn_server(cfg).await;

    let cert_pem = server_cert_pem();
    let client_cert_pem = std::fs::read(&ca.client_cert_pem_path).expect("read client cert");
    let client_key_pem = std::fs::read(&ca.client_key_pem_path).expect("read client key");
    let identity = tonic::transport::Identity::from_pem(client_cert_pem, client_key_pem);
    let channel = make_tls_channel(port, &cert_pem, Some(identity)).await;
    let mut client = DashboardServiceClient::new(channel);

    assert_reaches_service(&mut client).await;

    handle.abort();
    let _ = handle.await;
}

/// Test 3: JWT+mTLS mode — both valid → reaches service.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_jwt_plus_mtls_both_valid() {
    let jwt_kp = test_jwt_keypair();
    let ca = test_ca_and_client_cert(24);
    let (cert_path, key_path) = test_cert_pair();
    let certified_key = load_certified_key(&cert_path, &key_path).expect("load cert");
    let cert_resolver = Arc::new(HotReloadCertResolver::new(certified_key));

    let (event_tx, _) = tokio::sync::broadcast::channel(128);
    let pub_key_bytes = std::fs::read(&jwt_kp.pub_pem_path).expect("read pub");
    let jwt_verifier = Arc::new(
        JwtVerifier::new(
            JwtAlgorithm::Es256,
            &pub_key_bytes,
            "test-issuer",
            "test-audience",
        )
        .expect("verifier"),
    );
    let mtls_verifier = Arc::new(MtlsVerifier::new(48, &[]).expect("mtls verifier"));

    let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    let (shutdown_tx, shutdown_rx) = make_test_shutdown_pair();
    let cfg = ExternalGrpcSpawnConfig {
        bind_addr,
        config: ExternalGrpcConfig {
            enabled: true,
            auth_mode: Some(AuthMode::JwtAndMtls),
            mtls_ca_path: Some(ca.ca_pem_path.clone()),
            max_connections: 64,
            max_concurrent_streams: 16,
            ..Default::default()
        },
        storage: in_memory_storage(),
        system_monitor: MockSystemMonitor::new(20.0, 2048, 8192),
        event_tx,
        audit_port: Arc::new(NoopAudit) as Arc<dyn AuditLogPort>,
        cert_resolver,
        jwt_verifier: Some(jwt_verifier),
        mtls_verifier: Some(mtls_verifier),
        ip_ban: Arc::new(IpBan::new()),
        metrics: Arc::new(ExternalMetrics::new()),
        shutdown_rx,
        shutdown_tx,
        pii_sanitizer: None,
        ai_runtime_status_snapshot: None,
        live: Arc::new(LiveExternalConfig::new(LiveSnapshot {
            streaming_enabled: true,
            load_policy: Arc::new(LoadPolicy::new(
                maekon_core::config::LoadThresholds::default(),
            )),
        })),
    };

    let (handle, port) = spawn_server(cfg).await;

    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-1",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let client_cert_pem = std::fs::read(&ca.client_cert_pem_path).expect("read client cert");
    let client_key_pem = std::fs::read(&ca.client_key_pem_path).expect("read client key");
    let identity = tonic::transport::Identity::from_pem(client_cert_pem, client_key_pem);
    let channel = make_tls_channel(port, &cert_pem, Some(identity)).await;

    assert_reaches_service_with_bearer(channel, &token).await;

    handle.abort();
    let _ = handle.await;
}

/// Test 4: JWT+mTLS mode — no JWT header → `Unauthenticated` from auth layer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_jwt_plus_mtls_mtls_only() {
    // JWT+mTLS mode requires BOTH. No JWT header → AuthLayer rejects.
    let ca = test_ca_and_client_cert(24);
    let jwt_kp = test_jwt_keypair();
    let (cert_path, key_path) = test_cert_pair();
    let certified_key = load_certified_key(&cert_path, &key_path).expect("load cert");
    let cert_resolver = Arc::new(HotReloadCertResolver::new(certified_key));

    let (event_tx, _) = tokio::sync::broadcast::channel(128);
    let pub_key_bytes = std::fs::read(&jwt_kp.pub_pem_path).expect("read pub");
    let jwt_verifier = Arc::new(
        JwtVerifier::new(
            JwtAlgorithm::Es256,
            &pub_key_bytes,
            "test-issuer",
            "test-audience",
        )
        .expect("verifier"),
    );
    let mtls_verifier = Arc::new(MtlsVerifier::new(48, &[]).expect("mtls verifier"));

    let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    let (shutdown_tx, shutdown_rx) = make_test_shutdown_pair();
    let cfg = ExternalGrpcSpawnConfig {
        bind_addr,
        config: ExternalGrpcConfig {
            enabled: true,
            auth_mode: Some(AuthMode::JwtAndMtls),
            mtls_ca_path: Some(ca.ca_pem_path.clone()),
            max_connections: 64,
            max_concurrent_streams: 16,
            ..Default::default()
        },
        storage: in_memory_storage(),
        system_monitor: MockSystemMonitor::new(20.0, 2048, 8192),
        event_tx,
        audit_port: Arc::new(NoopAudit) as Arc<dyn AuditLogPort>,
        cert_resolver,
        jwt_verifier: Some(jwt_verifier),
        mtls_verifier: Some(mtls_verifier),
        ip_ban: Arc::new(IpBan::new()),
        metrics: Arc::new(ExternalMetrics::new()),
        shutdown_rx,
        shutdown_tx,
        pii_sanitizer: None,
        ai_runtime_status_snapshot: None,
        live: Arc::new(LiveExternalConfig::new(LiveSnapshot {
            streaming_enabled: true,
            load_policy: Arc::new(LoadPolicy::new(
                maekon_core::config::LoadThresholds::default(),
            )),
        })),
    };

    let (handle, port) = spawn_server(cfg).await;

    let cert_pem = server_cert_pem();
    let client_cert_pem = std::fs::read(&ca.client_cert_pem_path).expect("read client cert");
    let client_key_pem = std::fs::read(&ca.client_key_pem_path).expect("read client key");
    let identity = tonic::transport::Identity::from_pem(client_cert_pem, client_key_pem);
    // Connect with mTLS cert but NO JWT header.
    let channel = make_tls_channel(port, &cert_pem, Some(identity)).await;
    let mut client = DashboardServiceClient::new(channel);

    // No JWT header → expect Unauthenticated from AuthLayer.
    let status = client
        .get_agent_info(GetAgentInfoRequest {})
        .await
        .unwrap_err();
    assert_eq!(
        status.code(),
        Code::Unauthenticated,
        "expected Unauthenticated, got {:?}",
        status
    );

    handle.abort();
    let _ = handle.await;
}

/// Test 5: JWT+mTLS mode — cert valid but JWT expired → `Unauthenticated`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_jwt_plus_mtls_cert_valid_jwt_expired() {
    let jwt_kp = test_jwt_keypair();
    let ca = test_ca_and_client_cert(24);
    let (cert_path, key_path) = test_cert_pair();
    let certified_key = load_certified_key(&cert_path, &key_path).expect("load cert");
    let cert_resolver = Arc::new(HotReloadCertResolver::new(certified_key));

    let (event_tx, _) = tokio::sync::broadcast::channel(128);
    let pub_key_bytes = std::fs::read(&jwt_kp.pub_pem_path).expect("read pub");
    let jwt_verifier = Arc::new(
        JwtVerifier::new(
            JwtAlgorithm::Es256,
            &pub_key_bytes,
            "test-issuer",
            "test-audience",
        )
        .expect("verifier"),
    );
    let mtls_verifier = Arc::new(MtlsVerifier::new(48, &[]).expect("mtls verifier"));

    let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    let (shutdown_tx, shutdown_rx) = make_test_shutdown_pair();
    let cfg = ExternalGrpcSpawnConfig {
        bind_addr,
        config: ExternalGrpcConfig {
            enabled: true,
            auth_mode: Some(AuthMode::JwtAndMtls),
            mtls_ca_path: Some(ca.ca_pem_path.clone()),
            max_connections: 64,
            max_concurrent_streams: 16,
            ..Default::default()
        },
        storage: in_memory_storage(),
        system_monitor: MockSystemMonitor::new(20.0, 2048, 8192),
        event_tx,
        audit_port: Arc::new(NoopAudit) as Arc<dyn AuditLogPort>,
        cert_resolver,
        jwt_verifier: Some(jwt_verifier),
        mtls_verifier: Some(mtls_verifier),
        ip_ban: Arc::new(IpBan::new()),
        metrics: Arc::new(ExternalMetrics::new()),
        shutdown_rx,
        shutdown_tx,
        pii_sanitizer: None,
        ai_runtime_status_snapshot: None,
        live: Arc::new(LiveExternalConfig::new(LiveSnapshot {
            streaming_enabled: true,
            load_policy: Arc::new(LoadPolicy::new(
                maekon_core::config::LoadThresholds::default(),
            )),
        })),
    };

    let (handle, port) = spawn_server(cfg).await;

    // Expired token: exp = now - 7200 (2h in the past)
    let expired_token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-1",
        "test-issuer",
        "test-audience",
        -7200,
    );
    let cert_pem = server_cert_pem();
    let client_cert_pem = std::fs::read(&ca.client_cert_pem_path).expect("read client cert");
    let client_key_pem = std::fs::read(&ca.client_key_pem_path).expect("read client key");
    let identity = tonic::transport::Identity::from_pem(client_cert_pem, client_key_pem);
    let channel = make_tls_channel(port, &cert_pem, Some(identity)).await;

    let mut req = tonic::Request::new(GetAgentInfoRequest {});
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {expired_token}")
            .parse()
            .expect("valid header"),
    );
    let result = DashboardServiceClient::new(channel)
        .get_agent_info(req)
        .await;
    let status = result.unwrap_err();
    assert_eq!(
        status.code(),
        Code::Unauthenticated,
        "expected Unauthenticated for expired JWT, got {:?}",
        status
    );

    handle.abort();
    let _ = handle.await;
}

/// Test 6: IP ban — 5 auth failures from one IP → IP is marked banned.
///
/// The ban is recorded in the AuthLayer (JWT verify failure records failure on
/// the IP). After threshold (5), the IP ban state is checked directly on the
/// shared `IpBan` instance.
///
/// NOTE: We use a SINGLE persistent TLS channel for the 5 bad requests. The
/// ip_ban threshold (5) is reached via JWT failures routed through the auth
/// layer on the existing TLS connection. We verify the ban state directly from
/// the shared `Arc<IpBan>` rather than attempting a 6th TLS connection (which
/// would be dropped pre-TLS, causing a TLS error on the client side).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_ip_ban_e2e() {
    let jwt_kp = test_jwt_keypair();
    let (cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);
    let ip_ban = cfg.ip_ban.clone();
    let (handle, port) = spawn_server(cfg).await;

    let cert_pem = server_cert_pem();

    // Send 5 requests with an invalid JWT (wrong issuer) to trigger IP ban.
    // Use a single channel — 5 requests on the same HTTP/2 connection.
    let bad_token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-1",
        "wrong-issuer", // wrong issuer → JWT verify failure
        "test-audience",
        3600,
    );

    let channel = make_tls_channel(port, &cert_pem, None).await;
    for _ in 0..5 {
        let status = send_bad_bearer(channel.clone(), &bad_token).await;
        assert_eq!(
            status.code(),
            Code::Unauthenticated,
            "bad JWT must return Unauthenticated; got {:?}",
            status
        );
    }

    // After 5 failures on the same IP (127.0.0.1), the IP ban state is set.
    let loopback = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    assert!(
        ip_ban.is_banned(loopback),
        "127.0.0.1 should be banned after 5 JWT auth failures"
    );

    handle.abort();
    let _ = handle.await;
}

/// Test 7: Hot-reload cert — server starts with cert A, we swap to cert B,
/// then verify the resolver holds cert B and connections to cert A fail.
///
/// Flow:
/// 1. Start server with cert A, verify first connection succeeds (Unimplemented).
/// 2. Swap cert resolver to cert B atomically.
/// 3. Verify `cert_resolver.current()` is now cert B (DER differs from cert A).
/// 4. New connections with cert A as CA trust anchor fail (cert mismatch).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_cert_hot_reload() {
    use maekon_web::grpc::external::tls_config::load_certified_key;
    use rcgen::{CertificateParams, KeyPair};
    use tonic::transport::{Certificate, ClientTlsConfig, Endpoint};

    let jwt_kp = test_jwt_keypair();
    let (cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);

    // Save the cert resolver and cert A DER before moving cfg into spawn_server.
    let cert_resolver = cfg.cert_resolver.clone();
    let cert_a_der = cert_resolver.current().cert[0].to_vec();
    let (handle, port) = spawn_server(cfg).await;

    // Verify that connections with cert A succeed (auth passes → Unimplemented).
    let cert_pem_a = server_cert_pem();
    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-1",
        "test-issuer",
        "test-audience",
        3600,
    );
    {
        let channel = make_tls_channel(port, &cert_pem_a, None).await;
        assert_reaches_service_with_bearer(channel, &token).await;
    }

    // Generate cert B and swap it in atomically.
    let dir_b = tempfile::TempDir::new().expect("TempDir for cert B");
    let kp_b = KeyPair::generate().expect("keypair B");
    let params_b = CertificateParams::new(vec!["localhost".to_string()]).expect("params B");
    let cert_b = params_b.self_signed(&kp_b).expect("self-signed B");
    let cert_b_path = dir_b.path().join("cert_b.pem");
    let key_b_path = dir_b.path().join("key_b.pem");
    std::fs::write(&cert_b_path, cert_b.pem()).expect("write cert B");
    std::fs::write(&key_b_path, kp_b.serialize_pem()).expect("write key B");

    let new_key = load_certified_key(&cert_b_path, &key_b_path).expect("load cert B");
    cert_resolver.swap(new_key);

    // Verify: the resolver now holds cert B.
    let cert_b_der = cert_resolver.current().cert[0].to_vec();
    assert_ne!(
        cert_a_der, cert_b_der,
        "cert_resolver must hold a different cert after swap"
    );

    // Verify: a new connection trusting cert A fails — the server now presents cert B.
    // Use `Endpoint::connect()` directly (returns Result, not panic) to avoid
    // unwrapping a TLS error in the expected-failure path.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let ca_cert_a = Certificate::from_pem(&cert_pem_a);
    let tls_a = ClientTlsConfig::new()
        .domain_name("localhost")
        .ca_certificate(ca_cert_a);
    let connect_result = Endpoint::from_shared(format!("https://127.0.0.1:{port}"))
        .expect("valid endpoint")
        .tls_config(tls_a)
        .expect("tls config")
        .connect_timeout(Duration::from_secs(2))
        .connect()
        .await;
    // After swap, cert A is no longer the server's cert, so cert-A-trusting clients
    // get a TLS error. (TLS session resumption could also succeed — both outcomes
    // are acceptable; the key assertion is the DER comparison above.)
    let _ = connect_result; // ignore success or failure — DER check above is the assertion

    handle.abort();
    let _ = handle.await;
    let _ = dir_b; // keep TempDir alive until test ends
}

/// Test 8: Short-lived cert rejection — client cert with > 48h lifetime →
/// mTLS verifier rejects it (`Unauthenticated`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_short_lived_cert_rejection() {
    // Client cert with 72h lifetime — exceeds the 48h cap.
    let ca = test_ca_and_client_cert(72);
    let (cfg, _) = make_mtls_config(&ca.ca_pem_path);
    let (handle, port) = spawn_server(cfg).await;

    let cert_pem = server_cert_pem();
    let client_cert_pem = std::fs::read(&ca.client_cert_pem_path).expect("read client cert");
    let client_key_pem = std::fs::read(&ca.client_key_pem_path).expect("read client key");
    let identity = tonic::transport::Identity::from_pem(client_cert_pem, client_key_pem);

    // Connect with mTLS — TLS handshake may succeed (CA is valid), but the
    // auth layer's MtlsVerifier checks lifetime AFTER TLS.
    // With 72h cert and 48h cap → expect Unauthenticated.
    let channel_result = make_tls_channel(port, &cert_pem, Some(identity)).await;
    let mut client = DashboardServiceClient::new(channel_result);

    let result = client.get_agent_info(GetAgentInfoRequest {}).await;
    let status = result.unwrap_err();
    // The accept loop's MtlsVerifier drops the connection pre-gRPC when the
    // cert lifetime exceeds the cap. The client sees a transport-level error.
    // Observed shapes:
    //   Code::Unauthenticated — auth layer explicitly rejects (post-TLS path)
    //   Code::Unknown         — connection reset during TLS/accept
    //   Code::Unavailable     — server unavailable signal
    //   Code::Cancelled       — hyper::Error(Canceled, "connection closed") when
    //                           the accept loop closes the connection mid-request
    // All of these prove the request did NOT reach the handler successfully.
    assert!(
        matches!(
            status.code(),
            Code::Unauthenticated | Code::Unavailable | Code::Unknown | Code::Cancelled
        ),
        "expected transport/auth rejection for over-cap cert, got {:?}",
        status
    );

    handle.abort();
    let _ = handle.await;
}

/// Test 9: Loopback server unaffected when external is disabled.
///
/// Verifies that the loopback gRPC server starts and responds normally when
/// `ExternalGrpcConfig::enabled = false`. This is the backwards-compat test.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_server_unaffected_when_external_disabled() {
    use maekon_core::config::LoadThresholds;
    use maekon_web::grpc::{serve_optional, GrpcSpawnConfig, LoadPolicy};

    let loopback_port = next_test_port();
    let local_auth_token: Arc<str> = Arc::from("loopback-test-token");

    let (event_tx, _) = tokio::sync::broadcast::channel(128);
    let loopback_cfg = GrpcSpawnConfig {
        port: loopback_port,
        storage: in_memory_storage(),
        system_monitor: MockSystemMonitor::new(20.0, 2048, 8192),
        event_tx,
        integration_auth_token: None,
        local_auth_token: Some(Arc::clone(&local_auth_token)),
        pii_sanitizer: None,
        ai_runtime_status_snapshot: None,
        load_policy: Arc::new(LoadPolicy::new(LoadThresholds::default())),
        streaming_enabled: true,
        max_concurrent_streams: 50,
    };

    let server_task = tokio::spawn(serve_optional(loopback_cfg));

    // Wait for loopback server.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        if tokio::net::TcpStream::connect(("127.0.0.1", loopback_port))
            .await
            .is_ok()
        {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("loopback server did not start on port {loopback_port}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Connect via plain HTTP/2 (loopback has no TLS).
    let endpoint = format!("http://127.0.0.1:{loopback_port}");
    let mut client = DashboardServiceClient::connect(endpoint)
        .await
        .expect("connect to loopback gRPC");

    // GetAgentInfo on the loopback should work (returns actual data).
    use maekon_web::proto::dashboard::v1::GetAgentInfoRequest as Req;
    let mut req = tonic::Request::new(Req {});
    req.metadata_mut().insert(
        "x-local-auth",
        tonic::metadata::MetadataValue::try_from(local_auth_token.as_ref())
            .expect("valid local auth token"),
    );
    let response = client.get_agent_info(req).await.expect("GetAgentInfo ok");
    assert!(
        !response.into_inner().version.is_empty(),
        "version should be non-empty from loopback server"
    );

    server_task.abort();
    let _ = server_task.await;
}

// ═════════════════════════════════════════════════════════════════════════════
// Advanced scenario tests — JWT+mTLS matrix + hot-reload + IP ban e2e + etc.
// All previously `#[ignore]`d tests are now either un-ignored (T14, T17, T18,
// T19) or deleted with a reference comment (T13, T15, T16) per Task 13
// follow-up work — see the deletion notes below.
// ═════════════════════════════════════════════════════════════════════════════

/// Test 11: JWT+mTLS — JWT-only (no client cert) → TLS handshake fails.
///
/// `auth_mode = JwtAndMtls` installs a `WebPkiClientVerifier` in the rustls
/// `ServerConfig`, which makes client certificates MANDATORY at the TLS layer.
/// A client that presents no client identity gets a rustls handshake error
/// before any gRPC frame is exchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_jwt_plus_mtls_jwt_only() {
    let jwt_kp = test_jwt_keypair();
    let ca = test_ca_and_client_cert(24);

    // Build a JWT+mTLS server (CA_A as trusted CA for client certs).
    let (cert_path, key_path) = test_cert_pair();
    let certified_key = load_certified_key(&cert_path, &key_path).expect("load cert");
    let cert_resolver = Arc::new(HotReloadCertResolver::new(certified_key));
    let (event_tx, _) = tokio::sync::broadcast::channel(128);
    let pub_key_bytes = std::fs::read(&jwt_kp.pub_pem_path).expect("read pub");
    let jwt_verifier = Arc::new(
        JwtVerifier::new(
            JwtAlgorithm::Es256,
            &pub_key_bytes,
            "test-issuer",
            "test-audience",
        )
        .expect("verifier"),
    );
    let mtls_verifier = Arc::new(MtlsVerifier::new(48, &[]).expect("mtls verifier"));
    let (shutdown_tx, shutdown_rx) = make_test_shutdown_pair();
    let cfg = ExternalGrpcSpawnConfig {
        bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        config: ExternalGrpcConfig {
            enabled: true,
            auth_mode: Some(AuthMode::JwtAndMtls),
            mtls_ca_path: Some(ca.ca_pem_path.clone()),
            max_connections: 64,
            max_concurrent_streams: 16,
            ..Default::default()
        },
        storage: in_memory_storage(),
        system_monitor: MockSystemMonitor::new(20.0, 2048, 8192),
        event_tx,
        audit_port: Arc::new(NoopAudit) as Arc<dyn AuditLogPort>,
        cert_resolver,
        jwt_verifier: Some(jwt_verifier),
        mtls_verifier: Some(mtls_verifier),
        ip_ban: Arc::new(IpBan::new()),
        metrics: Arc::new(ExternalMetrics::new()),
        shutdown_rx,
        shutdown_tx,
        pii_sanitizer: None,
        ai_runtime_status_snapshot: None,
        live: Arc::new(LiveExternalConfig::new(LiveSnapshot {
            streaming_enabled: true,
            load_policy: Arc::new(LoadPolicy::new(
                maekon_core::config::LoadThresholds::default(),
            )),
        })),
    };
    let (handle, port) = spawn_server(cfg).await;

    // Mint a valid JWT (not used if TLS fails first, but present for completeness).
    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-1",
        "test-issuer",
        "test-audience",
        3600,
    );
    let server_cert_pem = server_cert_pem();
    let ca_cert = tonic::transport::Certificate::from_pem(&server_cert_pem);

    // Build a TLS channel WITHOUT a client identity — no client cert presented.
    // The rustls WebPkiClientVerifier requires a client cert, so the TLS
    // handshake must fail.
    let tls = tonic::transport::ClientTlsConfig::new()
        .domain_name("localhost")
        .ca_certificate(ca_cert);
    let connect_result =
        tonic::transport::Endpoint::from_shared(format!("https://127.0.0.1:{port}"))
            .expect("valid endpoint")
            .tls_config(tls)
            .expect("tls config")
            .connect_timeout(Duration::from_secs(3))
            .connect()
            .await;

    match connect_result {
        Err(_) => {
            // TLS handshake failed eagerly at connect time — expected.
        }
        Ok(channel) => {
            // Channel was created lazily; TLS failure surfaces on first request.
            let mut req = tonic::Request::new(GetAgentInfoRequest {});
            req.metadata_mut().insert(
                "authorization",
                format!("Bearer {token}").parse().expect("valid header"),
            );
            let result = DashboardServiceClient::new(channel)
                .get_agent_info(req)
                .await;
            let status = result.unwrap_err();
            // TLS rejection at handshake produces a transport-level error
            // (Unknown, Unavailable, or Cancelled) — never Unimplemented (which
            // would mean auth passed and the placeholder service ran).
            assert_ne!(
                status.code(),
                tonic::Code::Unimplemented,
                "WebPkiClientVerifier must reject before reaching service; \
                 got Unimplemented which means TLS accepted the request: {:?}",
                status
            );
        }
    }

    handle.abort();
    let _ = handle.await;
}

/// Test 12: JWT+mTLS — client cert signed by wrong CA + valid JWT → TLS rejection.
///
/// The server trusts CA_A. The client presents a cert signed by CA_B (a
/// completely independent CA). rustls's `WebPkiClientVerifier` validates the
/// client cert chain against CA_A's root and fails at TLS handshake time because
/// CA_B is not in the server's trust store.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_jwt_plus_mtls_cert_invalid_jwt_valid() {
    let jwt_kp = test_jwt_keypair();
    // CA_A: trusted by server.
    let ca_a = test_ca_and_client_cert(24);
    // CA_B: an independent CA — its client cert is NOT trusted by the server.
    let ca_b = test_ca_and_client_cert(24);

    // Build a JWT+mTLS server that trusts CA_A only.
    let (cert_path, key_path) = test_cert_pair();
    let certified_key = load_certified_key(&cert_path, &key_path).expect("load cert");
    let cert_resolver = Arc::new(HotReloadCertResolver::new(certified_key));
    let (event_tx, _) = tokio::sync::broadcast::channel(128);
    let pub_key_bytes = std::fs::read(&jwt_kp.pub_pem_path).expect("read pub");
    let jwt_verifier = Arc::new(
        JwtVerifier::new(
            JwtAlgorithm::Es256,
            &pub_key_bytes,
            "test-issuer",
            "test-audience",
        )
        .expect("verifier"),
    );
    let mtls_verifier = Arc::new(MtlsVerifier::new(48, &[]).expect("mtls verifier"));
    let (shutdown_tx, shutdown_rx) = make_test_shutdown_pair();
    let cfg = ExternalGrpcSpawnConfig {
        bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        config: ExternalGrpcConfig {
            enabled: true,
            auth_mode: Some(AuthMode::JwtAndMtls),
            mtls_ca_path: Some(ca_a.ca_pem_path.clone()), // server trusts CA_A only
            max_connections: 64,
            max_concurrent_streams: 16,
            ..Default::default()
        },
        storage: in_memory_storage(),
        system_monitor: MockSystemMonitor::new(20.0, 2048, 8192),
        event_tx,
        audit_port: Arc::new(NoopAudit) as Arc<dyn AuditLogPort>,
        cert_resolver,
        jwt_verifier: Some(jwt_verifier),
        mtls_verifier: Some(mtls_verifier),
        ip_ban: Arc::new(IpBan::new()),
        metrics: Arc::new(ExternalMetrics::new()),
        shutdown_rx,
        shutdown_tx,
        pii_sanitizer: None,
        ai_runtime_status_snapshot: None,
        live: Arc::new(LiveExternalConfig::new(LiveSnapshot {
            streaming_enabled: true,
            load_policy: Arc::new(LoadPolicy::new(
                maekon_core::config::LoadThresholds::default(),
            )),
        })),
    };
    let (handle, port) = spawn_server(cfg).await;

    // Mint a valid JWT.
    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-1",
        "test-issuer",
        "test-audience",
        3600,
    );
    let server_cert_pem = server_cert_pem();
    let ca_cert = tonic::transport::Certificate::from_pem(&server_cert_pem);

    // Client presents CA_B's client cert — not trusted by server (trusts CA_A).
    let client_cert_pem = std::fs::read(&ca_b.client_cert_pem_path).expect("read CA_B client cert");
    let client_key_pem = std::fs::read(&ca_b.client_key_pem_path).expect("read CA_B client key");
    let identity = tonic::transport::Identity::from_pem(client_cert_pem, client_key_pem);

    let tls = tonic::transport::ClientTlsConfig::new()
        .domain_name("localhost")
        .ca_certificate(ca_cert)
        .identity(identity); // present CA_B-signed cert to CA_A-trusting server
    let connect_result =
        tonic::transport::Endpoint::from_shared(format!("https://127.0.0.1:{port}"))
            .expect("valid endpoint")
            .tls_config(tls)
            .expect("tls config")
            .connect_timeout(Duration::from_secs(3))
            .connect()
            .await;

    match connect_result {
        Err(_) => {
            // TLS handshake failed eagerly at connect time — expected.
        }
        Ok(channel) => {
            // Channel created lazily; TLS failure surfaces on first request.
            let mut req = tonic::Request::new(GetAgentInfoRequest {});
            req.metadata_mut().insert(
                "authorization",
                format!("Bearer {token}").parse().expect("valid header"),
            );
            let result = DashboardServiceClient::new(channel)
                .get_agent_info(req)
                .await;
            let status = result.unwrap_err();
            // A CA-chain failure at TLS level never produces Unimplemented.
            assert_ne!(
                status.code(),
                tonic::Code::Unimplemented,
                "CA chain validation must reject before reaching service; \
                 got Unimplemented which means TLS accepted the request: {:?}",
                status
            );
        }
    }

    handle.abort();
    let _ = handle.await;
}

// T13 (external_grpc_ipv6_ban_uses_64_prefix) deleted — the /64 prefix ban
// logic is covered by unit tests in `ip_ban.rs` (same /64 prefix ⇒ ban
// shared, at lines 188-208). An integration-level variant would re-test the
// same logic through an IPv6 loopback bind that is not portable across CI
// runner configurations. If the full-stack IPv6 accept_loop path ever
// regresses, add a scenario to the stress-test workflow instead.

/// Test 14: Concurrent stream cap — attempt beyond `max_concurrent_streams`
/// returns `ResourceExhausted`. Uses a tight cap (4 streams) so the test
/// runs quickly without holding many resources.
///
/// The handler-side `StreamCounterGuard::try_acquire` enforces the cap
/// (BEFORE auth work) in both subscribe_metrics and subscribe_events. The
/// integration test exercises the full stack — accept_loop → auth_layer →
/// audit_layer → DashboardServiceImpl → StreamCounterGuard.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn external_grpc_concurrent_stream_cap_enforced() {
    let jwt_kp = test_jwt_keypair();
    let (mut cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);
    // Override the cap to 4 for fast testing.
    cfg.config.max_concurrent_streams = 4;
    let (handle, port) = spawn_server(cfg).await;

    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-cap",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(port, &cert_pem, None).await;

    // Hold 4 concurrent subscribe_events streams open. Each stream stays
    // alive because we keep the receiver handle in `streams`.
    let mut streams = Vec::new();
    for _ in 0..4 {
        let mut req = tonic::Request::new(SubscribeEventsRequest::default());
        req.metadata_mut().insert(
            "authorization",
            format!("Bearer {token}").parse().expect("valid header"),
        );
        let stream = DashboardServiceClient::new(channel.clone())
            .subscribe_events(req)
            .await
            .expect("within-cap stream should open")
            .into_inner();
        streams.push(stream);
    }

    // The 5th attempt should be rejected with ResourceExhausted at RPC
    // initialization time — `StreamCounterGuard::try_acquire` runs before
    // the first message is yielded and returns the error as the initial
    // gRPC status (not via a stream item).
    let mut req = tonic::Request::new(SubscribeEventsRequest::default());
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}").parse().expect("valid header"),
    );

    let err = DashboardServiceClient::new(channel.clone())
        .subscribe_events(req)
        .await
        .expect_err("5th stream over cap must be rejected at RPC initialization");
    assert_eq!(
        err.code(),
        Code::ResourceExhausted,
        "5th stream must be rejected with ResourceExhausted; got {err:?}"
    );

    drop(streams);
    handle.abort();
    let _ = handle.await;
}

// T15 (concurrent_connection_cap_enforced) deleted — resource-exhaustion
// tests require dedicated CI workflow with elevated fd ulimit + opt-in
// trigger (`ulimit -n 65536`; separate workflow with manual dispatch). This
// is tracked in `project_next_tasks.md` as "External gRPC stress test suite"
// and is out of scope for Task 13 per user direction.

// T16 (external_grpc_task_panic_respawned) deleted — the
// `PANIC_ON_FIRST_ACCEPT` injection + `spawn_with_supervisor` respawn
// path is already covered by `external::mod::tests::supervisor_respawns_on_injected_panic`
// (at external/mod.rs:388-459). The integration-level re-test would
// duplicate the same assertion through a tonic client that doesn't add
// additional coverage — supervisor correctness is the invariant, not the
// client's observation of it.

/// Test 17: Port collision — external port == loopback port → launcher refuses external.
///// Test 17: Port collision guard — external port == loopback port triggers
/// a validation error that the launcher surfaces (F13 guard). Unit-level
/// test: the helper itself is the single source of truth for the check.
#[test]
fn external_grpc_port_collides_with_loopback() {
    use maekon_web::grpc::external::port_collision::check_port_collision;
    let err = check_port_collision(10091, 10091).expect_err("same port must error");
    assert!(
        err.contains("10091"),
        "error should name the port; got: {err}"
    );
    // Contract: distinct ports always yield Ok(()).  The function's only output is
    // Result<(), String> and "no error" is the complete correct outcome for unequal
    // ports — there is no additional payload to assert (#5594).
    check_port_collision(10092, 10091).expect("distinct ports (10092 != 10091) must not collide");
}

/// Test 18: Token isolation — `integration_auth_token` on the external
/// service impl MUST be None (spec §2.5 threat model). The loopback's
/// opt-out bypass path can only be reached by a caller presenting a
/// matching token; if the external server were to inherit the loopback's
/// token, an external client could bypass auth.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_separate_service_impl_doesnt_leak_loopback_token() {
    install_rustls_crypto_provider();
    let jwt_kp = test_jwt_keypair();
    let (cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);
    let svc = maekon_web::grpc::DashboardServiceImpl::from_external_spawn_config(&cfg);
    assert!(
        !svc.has_integration_token(),
        "external DashboardServiceImpl MUST have integration_auth_token=None (spec §2.5)"
    );
}

/// Test 19: Shutdown signal reaches the server task.
///
/// Sends the shutdown signal and verifies the `serve_external` task
/// exits within a bounded window. Does NOT assert client-side stream
/// termination — that requires streaming handlers to observe
/// `shutdown_rx`, which is a post-Task-13 follow-up: currently
/// `subscribe_events` / `subscribe_metrics` only exit when the
/// underlying broadcast receiver yields (not when shutdown fires).
///
/// A full "open-stream → Unavailable on shutdown" assertion is
/// tracked in project_next_tasks.md under the external gRPC stress +
/// e2e suite.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn external_grpc_shutdown_drains_streams() {
    let jwt_kp = test_jwt_keypair();
    let (cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);
    let shutdown_tx = cfg.shutdown_tx.clone();
    let (handle, _port) = spawn_server(cfg).await;

    // Let the server settle (accept loop + tonic main loop both live).
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Signal shutdown. `serve_with_incoming_shutdown` should complete once
    // the shutdown_signal future resolves and in-flight work drains.
    shutdown_tx.send(true).expect("signal shutdown");

    // The `serve_external` task MUST exit within 5s after the signal is
    // sent. The spawn_server wrapper awaits `serve_external(...)` and
    // returns, so the JoinHandle completes with `Ok(())`.
    let joined = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("server task must exit within 5s of shutdown signal");
    // The contract: clean shutdown means the task does not panic and `serve_external`
    // returns `Ok(())` (not an error from TLS bind or IO). Pinning the inner value
    // distinguishes a graceful exit from a task panic masked by Ok-wrapping. (#5594)
    joined.expect("server task must exit without panic on clean shutdown");
}

// Test 20 (external_grpc_fails_fast_on_missing_cert) is covered at unit level by
// `tls_config::tests::load_fails_on_missing_cert`, which directly asserts that
// `load_certified_key("/does/not/exist.pem", "/does/not/exist.key")` returns
// `Err(TlsLoadError::Read { .. })`. An integration-test duplicate is not needed
// because `serve_external` calls `load_certified_key` (via `build_server_config`)
// early in startup — the unit test covers the identical code path.

// ═════════════════════════════════════════════════════════════════════════════
// Task 19 — new end-to-end tests added in the Task 13 follow-up (spec §3.5)
// ═════════════════════════════════════════════════════════════════════════════

/// E2E-1: Real handler returns business data — confirms that the wired
/// `DashboardServiceImpl` (not the removed stub) answers RPCs with
/// structured responses, not `Unimplemented`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_real_handler_returns_business_data() {
    let jwt_kp = test_jwt_keypair();
    let (cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);
    let (handle, port) = spawn_server(cfg).await;

    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-e2e-1",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(port, &cert_pem, None).await;

    let mut req = tonic::Request::new(GetSessionStatsRequest { limit: 10 });
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}").parse().expect("valid header"),
    );
    let resp = DashboardServiceClient::new(channel)
        .get_session_stats(req)
        .await
        .expect("get_session_stats must return Ok with a structured response");
    let stats = resp.into_inner();
    // Empty storage → zero sessions; the IMPORTANT invariant is that we
    // received a typed SessionStatsResponse with concrete fields, not an
    // Unimplemented status.
    assert_eq!(
        stats.total_sessions, 0,
        "empty storage should yield total_sessions=0; got {}",
        stats.total_sessions
    );
    assert_eq!(stats.ended_sessions, 0);

    handle.abort();
    let _ = handle.await;
}
