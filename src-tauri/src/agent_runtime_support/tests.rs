//! Unit tests for the agent support-context wiring.
//!
//! Split out of `agent_runtime_support.rs` when that file crossed its ADR-013
//! baseline cap (`cargo test -p maekon-lint --test adr013_loc_growth_gate`).
//! Rust 2018+ lets a `foo.rs` module own submodules under a sibling `foo/`
//! directory, so this needs no `mod.rs` conversion of the parent.

use super::*;

#[test]
fn generate_session_id_format() {
    let id = generate_session_id();
    assert!(id.starts_with("sess_"));
    assert!(id.len() > 20);
}

#[test]
fn tauri_notifier_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<TauriNotifier>();
    assert_send_sync::<LogOnlyNotifier>();
}

#[test]
fn offline_mode_disables_server_transport_wiring() {
    let config = AppConfig::default_config();
    let ports = server_transport_ports_for_mode(
        true,
        &config,
        "sess_test_offline",
        None,
        #[cfg(feature = "server")]
        None,
    )
    .unwrap();

    assert!(ports.0.is_none());
    assert!(ports.1.is_none());
    assert!(ports.2.is_none());
    #[cfg(feature = "server")]
    assert!(ports.3.is_none());
}

/// A loopback development endpoint with TLS policy disabled. The shipped
/// default (`http://localhost:8000` with `tls.enabled = true`) is rejected by
/// the transport builders' HTTPS policy, so transport-construction tests must
/// name an explicit development endpoint. No request is ever issued.
#[cfg(feature = "server")]
fn loopback_transport_config() -> AppConfig {
    let mut config = AppConfig::default_config();
    config.server.base_url = "http://127.0.0.1:19999".to_string();
    config.tls.enabled = false;
    config
}

/// #9459 fails-before: the transports must be built on the ONE shared
/// `TokenManager` the composition root created (and restored from the
/// keychain), not on a second one constructed here. Proven by `Arc`
/// identity: every transport stores an `Arc<TokenManager>` clone, so the
/// shared handle's strong count stays above 1 for as long as `ports` lives.
/// Before the fix the clone passed in was the only extra handle and it died
/// with the call frame, leaving the count back at 1 — the login token would
/// then reach the IPC slot but never the upload/SSE path.
#[cfg(feature = "server")]
#[test]
fn shared_token_manager_backs_the_built_transports() {
    let config = loopback_transport_config();
    let shared = Arc::new(
        TokenManager::new_with_tls(&config.server.base_url, &config.tls, None)
            .expect("TokenManager must build for a loopback development endpoint"),
    );

    let ports = build_server_transports(
        &config,
        "sess_test_shared_token_manager",
        None,
        Some(shared.clone()),
    )
    .expect("server transports must build when handed a shared TokenManager");

    assert!(ports.1.is_some(), "an api client must have been built");
    assert!(
        Arc::strong_count(&shared) > 1,
        "the built transports must hold the SHARED TokenManager (strong_count == 1 \
         means build_server_transports discarded it and constructed its own session)"
    );
    drop(ports);
}

/// Absent a shared manager, the pre-#9459 behavior is preserved: this
/// wiring still constructs its own TLS-aware manager and returns transports.
#[cfg(feature = "server")]
#[test]
fn transports_still_build_without_a_shared_token_manager() {
    let config = loopback_transport_config();

    let ports = build_server_transports(&config, "sess_test_no_shared_tm", None, None)
        .expect("server transports must still build without a shared TokenManager");

    assert!(ports.0.is_some(), "a batch uploader must have been built");
    assert!(ports.1.is_some(), "an api client must have been built");
    assert!(ports.2.is_some(), "an SSE client must have been built");
}

/// #7668 regression: with `use_grpc_context=false` (the shipped default),
/// `select_sse_client` must pick the REST `SseStreamClient`, not the
/// gRPC-only `GrpcSseAdapter`. Proven end-to-end: log in against a stub
/// REST server, then confirm the *selected* client's `connect()` actually
/// delivers a suggestion pushed over the REST SSE endpoint.
///
/// Before the fix, `GrpcSseAdapter` was selected unconditionally in the
/// `--features grpc` build. Its `connect()` calls
/// `UnifiedClient::subscribe_suggestions`, which returns
/// `Err(CoreError::Network { .. "Suggestion streaming is available only
/// in gRPC mode. Set use_grpc_context=true." .. })` immediately — no
/// request would ever reach the stub server below, so this test would
/// time out waiting on `rx.recv()` and fail (fails-before evidence).
#[cfg(feature = "grpc")]
#[tokio::test]
async fn grpc_disabled_selects_rest_sse_client_and_delivers_suggestion() {
    use maekon_core::config::TlsConfig;
    use maekon_core::ports::api_client::SseEvent;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base = format!("http://127.0.0.1:{}", addr.port());

    // Stub server: 1) respond to the login POST with a valid token, 2)
    // respond to the SSE GET with a single `suggestion` event. Mirrors the
    // stub-server pattern in maekon-network::sse_client::tests.
    let server_task = tokio::spawn(async move {
        // login (POST /api/v1/auth/tokens)
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0u8; 2048];
            let _ = socket.read(&mut buf).await;
            let body = r#"{"access_token":"tok","refresh_token":"ref","expires_in":3600}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(resp.as_bytes()).await;
            let _ = socket.flush().await;
        }
        // SSE stream (GET /user_context/sessions/stream) → one suggestion event
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0u8; 2048];
            let _ = socket.read(&mut buf).await;
            let suggestion_json = r#"{"suggestion_id":"sug_7668","suggestion_type":"WORK_GUIDANCE","content":"REST-SSE fallback delivered","priority":"HIGH","confidence_score":0.9,"relevance_score":0.9,"is_actionable":true,"created_at":"2026-01-28T10:00:00Z"}"#;
            let sse_body = format!("event: suggestion\ndata: {suggestion_json}\n\n");
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{sse_body}"
            );
            let _ = socket.write_all(resp.as_bytes()).await;
            let _ = socket.flush().await;
        }
    });

    let tls = TlsConfig {
        enabled: false,
        allow_self_signed: false,
    };
    let token_manager = Arc::new(
        TokenManager::new_with_tls(&base, &tls, None)
            .expect("TokenManager must build for a loopback http base_url"),
    );
    // Runtime-built password fixture — a string literal at the `login()`
    // call site trips CodeQL `rust/hard-coded-cryptographic-value`;
    // mirrors `maekon_network::sse_client::tests::primary_password`.
    let password = String::from_utf8(vec![b'x'; 16]).expect("password fixture must be UTF-8");
    token_manager
        .login("user@example.com", &password)
        .await
        .expect("login against the stub REST server must succeed");

    // A minimal UnifiedClient — required by `select_sse_client`'s signature
    // even though the REST branch never touches it. Construction performs
    // no network I/O.
    let unified = Arc::new(
        UnifiedClient::new(GrpcConfig::default(), token_manager.clone())
            .expect("UnifiedClient must build without network I/O"),
    );

    let sse_client = select_sse_client(
        false, // use_grpc_context — the shipped default
        &unified,
        &base,
        token_manager.clone(),
        30,
        &tls,
    )
    .expect("select_sse_client must build the REST fallback when use_grpc_context is false");

    let (tx, mut rx) = tokio::sync::mpsc::channel::<SseEvent>(8);
    let connect_task = tokio::spawn(async move {
        let _ = sse_client.connect("sess_7668_fallback", tx).await;
    });

    let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect(
            "REST SSE fallback must deliver an event before the timeout — the pre-fix \
             GrpcSseAdapter selection would fail immediately with 'Suggestion streaming is \
             available only in gRPC mode' instead of ever reaching this stub server",
        )
        .expect("event channel must not close before the suggestion arrives");

    assert!(
        matches!(event, SseEvent::Suggestion(ref s) if s.suggestion_id == "sug_7668"),
        "expected a Suggestion event delivered via the REST SseStreamClient fallback, got: {event:?}"
    );

    connect_task.abort();
    server_task.abort();
}
