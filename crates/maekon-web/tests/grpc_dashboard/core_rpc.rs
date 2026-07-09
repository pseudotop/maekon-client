// Core RPC smoke tests — GetAgentInfo, local-auth gate, HealthCheck, and
// sequential-call stability. Split from `grpc_dashboard_integration.rs` by
// scenario family (#7730).

use std::time::Duration;

use maekon_web::proto::dashboard::v1::dashboard_service_client::DashboardServiceClient;
use maekon_web::proto::dashboard::v1::health_check_response::Status as HealthStatus;
use maekon_web::proto::dashboard::v1::{GetAgentInfoRequest, HealthCheckRequest};

use crate::common::{
    connect_dashboard_client, in_memory_storage, pick_free_port, test_spawn_config,
    wait_for_server_ready,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_dashboard_get_agent_info_end_to_end() {
    let port = pick_free_port();
    let storage = in_memory_storage().await;

    let server_task = tokio::spawn(maekon_web::grpc::serve_optional(test_spawn_config(
        port, storage,
    )));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let mut client = connect_dashboard_client(port).await;

    let response = client
        .get_agent_info(GetAgentInfoRequest {})
        .await
        .expect("GetAgentInfo RPC succeeds")
        .into_inner();

    assert!(
        !response.version.is_empty(),
        "version must come from CARGO_PKG_VERSION — got empty string"
    );
    assert!(
        matches!(response.build_profile.as_str(), "debug" | "release"),
        "build_profile must be 'debug' or 'release' — got '{}'",
        response.build_profile
    );
    assert!(
        matches!(
            response.platform.as_str(),
            "macos" | "windows" | "linux" | "unknown"
        ),
        "platform must be one of macos/windows/linux/unknown — got '{}'",
        response.platform
    );
    assert!(
        response.uptime_secs >= 0,
        "uptime_secs must be non-negative — got {}",
        response.uptime_secs
    );

    server_task.abort();
    let _ = server_task.await;
}

/// #6420: when a per-session local-auth token is configured, the loopback gRPC
/// dashboard must reject RPCs that omit it and accept RPCs that present it —
/// matching the REST `/api` surface (defense in depth on multi-user hosts).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_dashboard_local_auth_gate_rejects_unauthenticated_and_accepts_token() {
    let port = pick_free_port();
    let storage = in_memory_storage().await;

    let mut cfg = test_spawn_config(port, storage);
    cfg.local_auth_token = Some(std::sync::Arc::from("sess-token-xyz"));
    let server_task = tokio::spawn(maekon_web::grpc::serve_optional(cfg));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let endpoint = format!("http://127.0.0.1:{port}");
    let mut unauthenticated_client = DashboardServiceClient::connect(endpoint.clone())
        .await
        .expect("connect to dashboard gRPC server");

    // No token → Unauthenticated.
    let err = unauthenticated_client
        .get_agent_info(GetAgentInfoRequest {})
        .await
        .expect_err("RPC without the local-auth token must be rejected");
    assert_eq!(err.code(), tonic::Code::Unauthenticated);

    // Correct token via `x-local-auth` metadata → accepted.
    let mut authenticated_client = DashboardServiceClient::connect(endpoint)
        .await
        .expect("connect to dashboard gRPC server");
    let mut req = tonic::Request::new(GetAgentInfoRequest {});
    let key: tonic::metadata::MetadataKey<tonic::metadata::Ascii> =
        "x-local-auth".parse().expect("valid metadata key");
    req.metadata_mut()
        .insert(key, "sess-token-xyz".parse().expect("valid metadata value"));
    authenticated_client
        .get_agent_info(req)
        .await
        .expect("RPC with the correct local-auth token must succeed");

    server_task.abort();
    let _ = server_task.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_dashboard_health_check_end_to_end() {
    let port = pick_free_port();
    let storage = in_memory_storage().await;

    let server_task = tokio::spawn(maekon_web::grpc::serve_optional(test_spawn_config(
        port, storage,
    )));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let mut client = connect_dashboard_client(port).await;

    let response = client
        .health_check(HealthCheckRequest {})
        .await
        .expect("HealthCheck RPC succeeds")
        .into_inner();

    assert_eq!(
        response.status,
        HealthStatus::Serving as i32,
        "health status should be SERVING when server is up",
    );

    server_task.abort();
    let _ = server_task.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_dashboard_survives_multiple_sequential_calls() {
    let port = pick_free_port();
    let storage = in_memory_storage().await;

    let server_task = tokio::spawn(maekon_web::grpc::serve_optional(test_spawn_config(
        port, storage,
    )));
    wait_for_server_ready(port, Duration::from_secs(5)).await;

    let mut client = connect_dashboard_client(port).await;

    // Locks the invariant that DashboardServiceImpl handles concurrent-ish
    // traffic without panicking or leaking state between calls.
    for _ in 0..5 {
        let info = client
            .get_agent_info(GetAgentInfoRequest {})
            .await
            .expect("GetAgentInfo RPC succeeds")
            .into_inner();
        assert!(!info.version.is_empty());
    }

    server_task.abort();
    let _ = server_task.await;
}
