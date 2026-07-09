// Shared helpers for the `grpc_dashboard_integration.rs` scenario-family
// split (#7730). Included via `#[path = "grpc_dashboard/common.rs"] mod
// common;` from the crate-root test binary — cargo does NOT auto-discover
// this file as its own test target (mirrors the existing `tests/support/`
// pattern already used by this same test binary for `FailingStorage`).

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use maekon_storage::sqlite::SqliteStorage;
use maekon_web::proto::dashboard::v1::dashboard_service_client::DashboardServiceClient;
use maekon_web::storage_port::WebStorage;

pub(crate) const TEST_LOCAL_AUTH_TOKEN: &str = "grpc-test-local-auth-token";

type AuthedDashboardClient = DashboardServiceClient<
    tonic::service::interceptor::InterceptedService<
        tonic::transport::Channel,
        fn(tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status>,
    >,
>;

/// Pick a free ephemeral port by binding + immediately dropping a listener.
/// Tiny race window between drop + server bind, acceptable for test use.
pub(crate) fn pick_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().expect("local_addr").port();
    drop(listener);
    port
}

/// Poll until the server accepts TCP connections (up to `timeout`).
pub(crate) async fn wait_for_server_ready(port: u16, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "gRPC dashboard server did not accept connections on port {port} within {timeout:?}"
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Build an in-memory SqliteStorage behind the `WebStorage` trait.
pub(crate) async fn in_memory_storage() -> Arc<dyn WebStorage> {
    let storage = SqliteStorage::open_in_memory(30).expect("open in-memory SqliteStorage");
    Arc::new(storage) as Arc<dyn WebStorage>
}

fn add_local_auth(mut req: tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> {
    req.metadata_mut().insert(
        "x-local-auth",
        TEST_LOCAL_AUTH_TOKEN
            .parse()
            .expect("valid local-auth metadata value"),
    );
    Ok(req)
}

pub(crate) async fn connect_dashboard_client(port: u16) -> AuthedDashboardClient {
    let endpoint = format!("http://127.0.0.1:{port}");
    let channel = tonic::transport::Channel::from_shared(endpoint)
        .expect("valid endpoint")
        .connect()
        .await
        .expect("connect to dashboard gRPC server");
    DashboardServiceClient::with_interceptor(
        channel,
        add_local_auth as fn(tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status>,
    )
}

// Identity PII sanitizer for tests (#6421): production wiring always provides a
// sanitizer, so a `None` would (correctly, per the fail-closed fix) redact dashboard
// text. Tests that assert on real streamed frame content use this pass-through so the
// redaction does not mask the value under test.
pub(crate) struct PassthroughPiiSanitizer;
impl maekon_core::ports::pii_sanitizer::PiiSanitizer for PassthroughPiiSanitizer {
    fn sanitize_text(&self, text: &str, _level: maekon_core::config::PiiFilterLevel) -> String {
        text.to_string()
    }
}

/// Build a `GrpcSpawnConfig` with sensible test defaults (deterministic
/// `MockSystemMonitor`, 16-slot broadcast, local-auth token, default
/// `LoadThresholds`, streaming enabled, cap 50). Callers that need to
/// override fields can destructure and rebuild.
pub(crate) fn test_spawn_config(
    port: u16,
    storage: Arc<dyn WebStorage>,
) -> maekon_web::grpc::GrpcSpawnConfig {
    use maekon_core::config::LoadThresholds;
    use maekon_web::grpc::{test_support::mock_system_monitor::MockSystemMonitor, LoadPolicy};

    let (event_tx, _) = tokio::sync::broadcast::channel(16);
    maekon_web::grpc::GrpcSpawnConfig {
        port,
        storage,
        system_monitor: MockSystemMonitor::new(30.0, 4096, 16384),
        event_tx,
        integration_auth_token: None,
        local_auth_token: Some(Arc::from(TEST_LOCAL_AUTH_TOKEN)),
        pii_sanitizer: Some(Arc::new(PassthroughPiiSanitizer)
            as Arc<dyn maekon_core::ports::pii_sanitizer::PiiSanitizer>),
        ai_runtime_status_snapshot: None,
        load_policy: Arc::new(LoadPolicy::new(LoadThresholds::default())),
        streaming_enabled: true,
        max_concurrent_streams: 50,
    }
}
