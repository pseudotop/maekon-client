#![cfg(feature = "server")]
#![allow(deprecated)] // Tests use TokenManager::new / HttpApiClient::new (non-TLS ok in tests)
//! ```
//! cargo test -p oneshim-app --test server_integration_test -- --nocapture
//! ```

mod mock_server;

use mock_server::MockServer;
use oneshim_core::models::event::{Event, EventBatch, UserEvent, UserEventType};
use oneshim_core::models::frame::{ContextUpload, FrameMetadata};
use oneshim_core::ports::api_client::ApiClient;
use oneshim_network::auth::TokenManager;
use oneshim_network::http_client::HttpApiClient;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

#[tokio::test]
async fn test_token_manager_login() {
    let server = MockServer::start().await;
    println!("Mock server started: {}", server.url());

    let token_manager = TokenManager::new(server.url());

    let result = token_manager
        .login("test@example.com", "test-password-placeholder")
        .await;
    assert!(result.is_ok(), "login failure: {:?}", result.err());

    let token = token_manager.get_token().await;
    assert!(token.is_ok(), "failed to acquire token: {:?}", token.err());

    let token_str = token.unwrap();
    assert!(
        token_str.starts_with("mock_access_"),
        "unexpected token format: {}",
        token_str
    );

    println!("[OK] login success, token: {}...", &token_str[..20]);
}

#[tokio::test]
async fn test_api_client_create_session() {
    let server = MockServer::start().await;
    println!("Mock server started: {}", server.url());

    let token_manager = Arc::new(TokenManager::new(server.url()));
    token_manager
        .login("test@example.com", "test-password-placeholder")
        .await
        .expect("login failure");

    let api_client =
        HttpApiClient::new(server.url(), token_manager, Duration::from_secs(30)).unwrap();

    let client_id = format!("client_{}", Uuid::new_v4());
    let result = api_client.create_session(&client_id).await;

    assert!(result.is_ok(), "session create failure: {:?}", result.err());

    let session = result.unwrap();
    assert!(session.session_id.starts_with("session_"));
    assert_eq!(session.client_id, client_id);

    println!("[OK] session create success: {}", session.session_id);
    println!("   - User ID: {}", session.user_id);
    println!("   - Capabilities: {:?}", session.capabilities);
}

#[tokio::test]
async fn test_api_client_upload_context() {
    let server = MockServer::start().await;
    println!("Mock server started: {}", server.url());

    let token_manager = Arc::new(TokenManager::new(server.url()));
    token_manager
        .login("test@example.com", "test-password-placeholder")
        .await
        .expect("login failure");

    let api_client =
        HttpApiClient::new(server.url(), token_manager, Duration::from_secs(30)).unwrap();

    let context = ContextUpload {
        session_id: "test_session_123".to_string(),
        timestamp: chrono::Utc::now(),
        metadata: FrameMetadata {
            timestamp: chrono::Utc::now(),
            trigger_type: "AppSwitch".to_string(),
            app_name: "Visual Studio Code".to_string(),
            window_title: "server_integration_test.rs - maekon-client".to_string(),
            resolution: (1920, 1080),
            importance: 0.8,
        },
        ocr_text: Some("test text".to_string()),
        image: None,
    };

    let result = api_client.upload_context(&context).await;
    assert!(result.is_ok(), "context upload failure: {:?}", result.err());

    assert_eq!(server.context_count(), 1);

    println!("[OK] context upload success");
    println!("   - App: {}", context.metadata.app_name);
    println!("   - Window: {}", context.metadata.window_title);
}

#[tokio::test]
async fn test_api_client_upload_batch() {
    let server = MockServer::start().await;
    println!("Mock server started: {}", server.url());

    let token_manager = Arc::new(TokenManager::new(server.url()));
    token_manager
        .login("test@example.com", "test-password-placeholder")
        .await
        .expect("login failure");

    let api_client =
        HttpApiClient::new(server.url(), token_manager, Duration::from_secs(30)).unwrap();

    let events: Vec<Event> = (0..5)
        .map(|i| {
            Event::User(UserEvent {
                event_id: Uuid::new_v4(),
                event_type: UserEventType::WindowChange,
                timestamp: chrono::Utc::now(),
                app_name: format!("App{}", i),
                window_title: format!("Window {}", i),
            })
        })
        .collect();

    let batch = EventBatch {
        session_id: "test_session_batch".to_string(),
        events,
        created_at: chrono::Utc::now(),
    };

    let result = api_client.upload_batch(&batch).await;
    assert!(result.is_ok(), "batch upload failure: {:?}", result.err());

    println!("[OK] batch event upload success");
    println!("   - Session ID: {}", batch.session_id);
    println!("- Events: {} items", batch.events.len());
}

#[tokio::test]
async fn test_api_client_health_check() {
    let server = MockServer::start().await;
    println!("Mock server started: {}", server.url());

    let token_manager = Arc::new(TokenManager::new(server.url()));
    token_manager
        .login("test@example.com", "test-password-placeholder")
        .await
        .expect("login failure");

    let api_client =
        HttpApiClient::new(server.url(), token_manager, Duration::from_secs(30)).unwrap();

    let session = api_client
        .create_session("test_client")
        .await
        .expect("session create failure");

    let result = api_client.send_heartbeat(&session.session_id).await;
    assert!(result.is_ok(), "health check failed: {:?}", result.err());

    println!("[OK] success");
}

#[tokio::test]
async fn test_token_refresh() {
    let server = MockServer::start().await;
    println!("Mock server started: {}", server.url());

    let token_manager = TokenManager::new(server.url());

    token_manager
        .login("test@example.com", "test-password-placeholder")
        .await
        .expect("login failure");

    let token1 = token_manager
        .get_token()
        .await
        .expect("failed to acquire token");

    token_manager
        .refresh()
        .await
        .expect("token refresh failure");

    let token2 = token_manager
        .get_token()
        .await
        .expect("failed to acquire token");

    assert_ne!(token1, token2, "token was not refreshed");

    println!("[OK] token refresh success");
    println!("   - Old: {}...", &token1[..20]);
    println!("   - New: {}...", &token2[..20]);
}

#[tokio::test]
async fn test_full_client_workflow() {
    let server = MockServer::start().await;
    println!("Mock server started: {}", server.url());

    let token_manager = Arc::new(TokenManager::new(server.url()));
    token_manager
        .login("user@example.com", "test-password-placeholder")
        .await
        .expect("Step 1: login failure");
    println!("step 1: login completed");

    let api_client =
        HttpApiClient::new(server.url(), token_manager.clone(), Duration::from_secs(30)).unwrap();

    let session = api_client
        .create_session("oneshim_client_v1")
        .await
        .expect("Step 3: session create failure");
    println!("step 2: session created: {}", session.session_id);

    for i in 0..3 {
        let context = ContextUpload {
            session_id: session.session_id.clone(),
            timestamp: chrono::Utc::now(),
            metadata: FrameMetadata {
                timestamp: chrono::Utc::now(),
                trigger_type: "Timer".to_string(),
                app_name: format!("App{}", i),
                window_title: format!("Document {}", i),
                resolution: (1920, 1080),
                importance: 0.5,
            },
            ocr_text: None,
            image: None,
        };
        api_client
            .upload_context(&context)
            .await
            .unwrap_or_else(|_| panic!("Step 4-{}: context upload failure", i));
    }
    println!("step 3: uploaded 3 context entries");

    let events: Vec<Event> = (0..10)
        .map(|i| {
            Event::User(UserEvent {
                event_id: Uuid::new_v4(),
                event_type: UserEventType::WindowChange,
                timestamp: chrono::Utc::now(),
                app_name: format!("App{}", i % 3),
                window_title: format!("Window {}", i),
            })
        })
        .collect();

    let batch = EventBatch {
        session_id: session.session_id.clone(),
        events,
        created_at: chrono::Utc::now(),
    };

    api_client
        .upload_batch(&batch)
        .await
        .expect("Step 5: batch upload failure");
    println!("step 4: event batch upload completed (10 items)");

    api_client
        .send_heartbeat(&session.session_id)
        .await
        .expect("Step 6: health check failed");
    println!("step 5: completed");

    assert!(server.request_count() >= 7, "insufficient request count");
    assert_eq!(server.context_count(), 3, "unexpected context count");
    assert_eq!(server.session_count(), 1, "unexpected session count");

    println!("\n[OK] success!");
    println!("- request: {}", server.request_count());
    println!("- context: {} items", server.context_count());
    println!("- session: {} items", server.session_count());
}

#[tokio::test]
async fn test_invalid_credentials() {
    let server = MockServer::start().await;
    println!("Mock server started: {}", server.url());

    let token_manager = TokenManager::new(server.url());

    let result = token_manager.login("", "").await;

    assert!(result.is_err(), "login should fail with empty credentials");
    println!("[OK] deny check");
}

#[tokio::test]
async fn test_concurrent_requests() {
    let server = MockServer::start().await;
    println!("Mock server started: {}", server.url());

    let token_manager = Arc::new(TokenManager::new(server.url()));
    token_manager
        .login("test@example.com", "test-password-placeholder")
        .await
        .expect("login failure");

    let api_client =
        Arc::new(HttpApiClient::new(server.url(), token_manager, Duration::from_secs(30)).unwrap());

    let session = api_client
        .create_session("concurrent_test")
        .await
        .expect("session create failure");
    let session_id = session.session_id.clone();

    let mut handles = Vec::new();
    for _ in 0..10 {
        let client = api_client.clone();
        let sid = session_id.clone();
        handles.push(tokio::spawn(
            async move { client.send_heartbeat(&sid).await },
        ));
    }

    let results: Vec<_> = futures::future::join_all(handles).await;

    let success_count = results
        .iter()
        .filter(|r| r.is_ok() && r.as_ref().unwrap().is_ok())
        .count();
    assert_eq!(success_count, 10, "some concurrent requests failed");

    println!("[OK] 10 concurrent requests succeeded");
    println!("- request: {}", server.request_count());
}
