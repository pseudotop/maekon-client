use super::*;
use crate::grpc::external::{
    cert_resolver::HotReloadCertResolver,
    ip_ban::IpBan,
    jwt_verifier::JwtVerifier,
    live_config::{LiveExternalConfig, LiveSnapshot},
    metrics::ExternalMetrics,
    test_support::{
        server_cert_pem, test_cert_pair, test_jwt_keypair, test_mint_jwt, NoopAudit, TestJwt,
    },
    tls_config::load_certified_key,
};
use maekon_core::config::{AuthMode, ExternalGrpcConfig, JwtAlgorithm};
use maekon_core::config_manager::ConfigManager;

struct Setup {
    cfg: ExternalGrpcSpawnConfig,
    manager: Arc<ConfigManager>,
    key: TestJwt,
    _dir: tempfile::TempDir,
}

fn setup() -> Setup {
    install_rustls_crypto_provider();
    let key = test_jwt_keypair().unwrap();
    let verifier = JwtVerifier::new(
        JwtAlgorithm::Es256,
        &std::fs::read(&key.pub_pem_path).unwrap(),
        "iss",
        "aud",
    )
    .unwrap();
    let (cert, private_key) = test_cert_pair().unwrap();
    let certified = load_certified_key(&cert, &private_key).unwrap();
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let (event_tx, _) = tokio::sync::broadcast::channel(16);
    let dir = tempfile::tempdir().unwrap();
    let manager = Arc::new(ConfigManager::with_path(dir.path().join("config.json")).unwrap());
    Setup {
        cfg: ExternalGrpcSpawnConfig {
            bind_addr: ([127, 0, 0, 1], 0).into(),
            config: ExternalGrpcConfig {
                enabled: true,
                auth_mode: Some(AuthMode::Jwt),
                ..Default::default()
            },
            storage: Arc::new(maekon_storage::sqlite::SqliteStorage::open_in_memory(30).unwrap()),
            system_monitor: crate::grpc::test_support::mock_system_monitor::MockSystemMonitor::new(
                20.0, 2048, 8192,
            ),
            event_tx,
            audit_port: Arc::new(NoopAudit),
            cert_resolver: Arc::new(HotReloadCertResolver::new(certified)),
            jwt_verifier: Some(Arc::new(verifier)),
            mtls_verifier: None,
            ip_ban: Arc::new(IpBan::new()),
            metrics: Arc::new(ExternalMetrics::new()),
            shutdown_rx,
            shutdown_tx: Arc::new(shutdown_tx),
            pii_sanitizer: None,
            ai_runtime_status_snapshot: None,
            live: Arc::new(LiveExternalConfig::new(LiveSnapshot {
                streaming_enabled: true,
                load_policy: Arc::new(
                    crate::grpc::LoadPolicy::try_new(Default::default()).unwrap(),
                ),
            })),
        },
        manager,
        key,
        _dir: dir,
    }
}

#[tokio::test]
async fn readiness_reports_own_ephemeral_port_and_real_authenticated_tls_response() {
    use crate::proto::dashboard::v1::{
        dashboard_service_client::DashboardServiceClient, GetAgentInfoRequest,
    };
    use tonic::transport::{Certificate, ClientTlsConfig, Endpoint};
    let setup = setup();
    let shutdown = setup.cfg.shutdown_tx.clone();
    let (server, reload, port) = spawn_server_with_config_manager(setup.cfg, setup.manager)
        .await
        .unwrap();
    assert_ne!(port, 0);
    let channel = Endpoint::from_shared(format!("https://localhost:{port}"))
        .unwrap()
        .tls_config(
            ClientTlsConfig::new()
                .domain_name("localhost")
                .ca_certificate(Certificate::from_pem(server_cert_pem().unwrap())),
        )
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut client = DashboardServiceClient::new(channel);
    let token = test_mint_jwt(&setup.key.enc_key, "client", "iss", "aud", 60).unwrap();
    let mut request = tonic::Request::new(GetAgentInfoRequest {});
    request
        .metadata_mut()
        .insert("authorization", format!("Bearer {token}").parse().unwrap());
    let info = client.get_agent_info(request).await.unwrap().into_inner();
    assert!(!info.build_profile.is_empty());
    drop(client);
    shutdown.send_replace(true);
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), reload)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn occupied_port_is_a_bind_error_even_when_another_listener_accepts_connections() {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let mut setup = setup();
    setup.cfg.bind_addr = listener.local_addr().unwrap();
    let metrics = Arc::downgrade(&setup.cfg.metrics);
    let error = spawn_server_with_config_manager(setup.cfg, setup.manager)
        .await
        .unwrap_err();
    let cause = std::error::Error::source(&error).expect("retain the server failure cause");
    assert_eq!(
        cause
            .source()
            .and_then(|source| source.downcast_ref::<std::io::Error>())
            .map(std::io::Error::kind),
        Some(std::io::ErrorKind::AddrInUse)
    );
    assert!(
        matches!(error, FixtureError::Server(ServeExternalError::Bind(ref error)) if error.kind() == std::io::ErrorKind::AddrInUse)
    );
    assert!(
        metrics.upgrade().is_none(),
        "failed startup must release its resources"
    );
}

#[tokio::test]
async fn missing_ca_is_a_tls_error_and_does_not_start_reload_work() {
    let mut setup = setup();
    setup.cfg.config.auth_mode = Some(AuthMode::Mtls);
    setup.cfg.config.mtls_ca_path = Some(setup._dir.path().join("missing-ca.pem"));
    let metrics = Arc::downgrade(&setup.cfg.metrics);
    let error = spawn_server_with_config_manager(setup.cfg, setup.manager)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        FixtureError::Server(ServeExternalError::Tls(_))
    ));
    assert!(metrics.upgrade().is_none());
}

#[tokio::test]
async fn canceling_pending_startup_releases_the_unpolled_server_task() {
    use std::future::Future;
    use std::task::{Context, Poll, Waker};
    let setup = setup();
    let metrics = Arc::downgrade(&setup.cfg.metrics);
    let mut startup = Box::pin(spawn_server_with_config_manager(setup.cfg, setup.manager));
    assert!(matches!(
        startup
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    drop(startup);
    tokio::time::timeout(Duration::from_secs(1), async {
        while metrics.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("canceled startup must release its task resources");
}

#[tokio::test]
async fn closed_readiness_receiver_stops_the_real_server_before_accepting() {
    let setup = setup();
    let resources = Arc::downgrade(&setup.cfg.metrics);
    let _shutdown_owner = setup.cfg.shutdown_tx.clone();
    let service = crate::grpc::DashboardServiceImpl::from_external_spawn_config(&setup.cfg);
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    drop(ready_rx);
    tokio::time::timeout(
        Duration::from_secs(1),
        crate::grpc::external::serve_external_inner(
            setup.cfg,
            service,
            "external_grpc(test)",
            Some(ready_tx),
        ),
    )
    .await
    .expect("canceled readiness must stop startup without a shutdown signal")
    .expect("canceled readiness is a clean server exit");
    assert!(resources.upgrade().is_none());
}

fn stalled_server() -> (
    ServerHandle,
    tokio::sync::oneshot::Receiver<std::net::SocketAddr>,
    std::sync::Weak<()>,
) {
    let resource = Arc::new(());
    let weak = Arc::downgrade(&resource);
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        // Fault injection at the ownership boundary: startup never finishes
        // and does not cooperate when the readiness receiver is dropped.
        let _owned_resources = (ready_tx, resource);
        std::future::pending::<Result<(), ServeExternalError>>().await
    });
    (task, ready_rx, weak)
}

#[tokio::test]
async fn canceling_stalled_startup_aborts_even_without_server_cooperation() {
    use std::future::Future;
    use std::task::{Context, Poll, Waker};
    let (server, ready, resource) = stalled_server();
    let mut startup = Box::pin(await_server_readiness(server, ready));
    assert!(matches!(
        startup
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    assert!(resource.upgrade().is_some());
    drop(startup);
    tokio::time::timeout(Duration::from_secs(1), async {
        while resource.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancel must reclaim a stalled task without its cooperation");
}

#[tokio::test(start_paused = true)]
async fn stalled_startup_times_out_and_reaps_owned_resources() {
    let (server, ready, resource) = stalled_server();
    let started = tokio::time::Instant::now();
    assert!(matches!(
        await_server_readiness(server, ready).await,
        Err(FixtureError::StartupTimeout)
    ));
    assert_eq!(started.elapsed(), Duration::from_secs(5));
    assert!(resource.upgrade().is_none());
}

#[tokio::test(start_paused = true)]
async fn closed_readiness_still_bounds_and_reaps_a_stalled_server() {
    let resource = Arc::new(());
    let weak = Arc::downgrade(&resource);
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let _owned_resource = resource;
        drop(ready_tx);
        std::future::pending::<Result<(), ServeExternalError>>().await
    });
    let started = tokio::time::Instant::now();
    let outcome = tokio::time::timeout(
        Duration::from_secs(6),
        await_server_readiness(server, ready_rx),
    )
    .await
    .expect("closing readiness must not remove the five-second startup deadline");
    assert!(matches!(outcome, Err(FixtureError::StartupTimeout)));
    assert_eq!(started.elapsed(), Duration::from_secs(5));
    assert!(weak.upgrade().is_none());
}

#[tokio::test]
async fn closed_readiness_preserves_completed_and_panicked_task_outcomes() {
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        drop(ready_tx);
        Ok(())
    });
    assert!(matches!(
        await_server_readiness(server, ready_rx).await,
        Err(FixtureError::StoppedBeforeReady)
    ));

    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        drop(ready_tx);
        panic!("injected startup task panic");
    });
    assert!(matches!(
        await_server_readiness(server, ready_rx).await,
        Err(FixtureError::Task(error)) if error.is_panic()
    ));
}

#[tokio::test]
async fn aborting_started_server_releases_its_owned_accept_listener() {
    let setup = setup();
    // Keep the application owners alive so this test observes explicit task
    // cancellation, rather than a clean exit after their channels close.
    let _shutdown_owner = setup.cfg.shutdown_tx.clone();
    let (server, reload, port) = spawn_server_with_config_manager(setup.cfg, setup.manager.clone())
        .await
        .unwrap();
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    reload.abort();
    assert!(reload.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(listener) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
                break listener;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("aborted server must not leave its accept task listening");
}

#[tokio::test]
async fn revoked_startup_and_unavailable_loopback_are_errors() {
    let setup = setup();
    setup.cfg.shutdown_tx.send_replace(true);
    assert!(matches!(
        spawn_server_with_config_manager(setup.cfg, setup.manager).await,
        Err(FixtureError::StoppedBeforeReady)
    ));
    // A bound but non-listening socket deterministically rejects a connection.
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind(([127, 0, 0, 1], 0).into()).unwrap();
    assert!(matches!(
        connect_loopback(socket.local_addr().unwrap().port()).await,
        Err(FixtureError::Transport(_))
    ));
}

async fn assert_pending_handshake_reaped(abort: bool) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let setup = setup();
    let resources = Arc::downgrade(&setup.cfg.metrics);
    let shutdown = setup.cfg.shutdown_tx.clone();
    let (server, reload, port) = spawn_server_with_config_manager(setup.cfg, setup.manager.clone())
        .await
        .unwrap();
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
    let mut client = rustls::ClientConnection::new(
        Arc::new(config),
        rustls::pki_types::ServerName::try_from("localhost").unwrap(),
    )
    .unwrap();
    let mut hello = Vec::new();
    client.write_tls(&mut hello).unwrap();
    let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    tcp.write_all(&hello).await.unwrap();
    let mut response = [0u8; 1];
    tokio::time::timeout(Duration::from_secs(5), tcp.read_exact(&mut response))
        .await
        .unwrap()
        .unwrap();
    // A real ServerHello proves the accept loop spawned and polled this TLS
    // handshake. Withhold ClientFinished so shutdown cannot rely on its timeout.
    assert_eq!(response[0], 22, "server must emit a TLS handshake record");
    assert!(resources.upgrade().is_some());
    if abort {
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
        reload.abort();
        assert!(reload.await.unwrap_err().is_cancelled());
    } else {
        shutdown.send_replace(true);
        tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .expect("graceful shutdown must not wait for an unfinished handshake")
            .unwrap()
            .unwrap();
        reload.await.unwrap();
    }
    tokio::time::timeout(Duration::from_secs(1), async {
        while resources.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("shutdown must release pending handshake resources");
}

#[tokio::test]
async fn server_abort_reaps_an_observed_in_flight_tls_handshake() {
    assert_pending_handshake_reaped(true).await;
}

#[tokio::test]
async fn graceful_shutdown_reaps_an_observed_in_flight_tls_handshake() {
    assert_pending_handshake_reaped(false).await;
}
