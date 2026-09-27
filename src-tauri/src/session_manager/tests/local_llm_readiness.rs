use super::*;
#[cfg(feature = "analysis")]
use futures::StreamExt;
#[cfg(feature = "analysis")]
use maekon_core::models::ai_session::{MessageRole, OutboundMessage, SessionMessage};

#[cfg(feature = "analysis")]
fn test_manager_with_local_llm(
    base_url: String,
    default_model: Option<&str>,
) -> SessionManagerImpl {
    test_manager().with_local_llm_target(crate::session_manager::factory::LocalLlmTarget {
        base_url,
        default_model: default_model.map(str::to_string),
    })
}

#[tokio::test]
async fn create_local_llm_session_succeeds() {
    let mut server = mockito::Server::new_async().await;
    let _version = server
        .mock("GET", "/api/version")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"version":"0.11.0"}"#)
        .create_async()
        .await;
    let _models = server
        .mock("GET", "/api/tags")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"models":[{"name":"llama3"}]}"#)
        .create_async()
        .await;
    let _chat = server
        .mock("POST", "/api/chat")
        .with_status(200)
        .with_header("content-type", "application/x-ndjson")
        .with_body(concat!(
            "{\"message\":{\"role\":\"assistant\",\"content\":\"READY\"},\"done\":false}\n",
            "{\"message\":{\"role\":\"assistant\",\"content\":\"\"},\"done\":true,\"eval_count\":1,\"prompt_eval_count\":4}\n"
        ))
        .create_async()
        .await;
    let mgr = test_manager_with_local_llm(server.url(), None);
    let config = SessionConfig {
        transport: SessionTransport::LocalLlm,
        surface_id: None,
        model: Some("llama3".to_string()),
        system_prompt: Some("Be concise.".to_string()),
        tools_enabled: false,
        cwd: None,
        sandbox_policy: None,
        approval_policy: None,
    };
    let session = mgr
        .create_session(config)
        .await
        .expect("should create LocalLlm session");
    assert_eq!(session.provider_name(), "ollama");
    assert!(!session.session_id().is_empty());

    let retrieved = mgr
        .get_session(session.session_id())
        .await
        .expect("LocalLlm session must be stored and retrievable after creation");
    assert_eq!(retrieved.session_id(), session.session_id());
    assert_eq!(mgr.list_sessions().await.len(), 1);

    let message = SessionMessage {
        screen_derived: false,
        role: MessageRole::User,
        content: "Reply with READY.".to_string(),
        attachments: vec![],
        tools: None,
        context: None,
        response_format: None,
    };
    let mut stream = session
        .send_message(&message)
        .await
        .expect("verified loopback session should accept its first turn");
    let mut terminal_results = Vec::new();
    while let Some(item) = stream.next().await {
        if let OutboundMessage::Result {
            content,
            done: true,
            ..
        } = item.expect("loopback stream item should succeed")
        {
            terminal_results.push(content);
        }
    }
    assert_eq!(terminal_results, vec!["READY"]);
}

/// C2 #5722: the default model MUST align with the provider-surface catalog
/// (qwen3:8b), not the stale "llama3" literal that was hardcoded pre-C2.
#[tokio::test]
async fn create_local_llm_session_uses_default_model() {
    let mut server = mockito::Server::new_async().await;
    let _version = server
        .mock("GET", "/api/version")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"version":"0.11.0"}"#)
        .create_async()
        .await;
    let _models = server
        .mock("GET", "/api/tags")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"models":[{"name":"qwen3:8b"}]}"#)
        .create_async()
        .await;
    let mgr = test_manager_with_local_llm(server.url(), None);
    let config = SessionConfig {
        transport: SessionTransport::LocalLlm,
        surface_id: None,
        model: None,
        system_prompt: None,
        tools_enabled: false,
        cwd: None,
        sandbox_policy: None,
        approval_policy: None,
    };
    let session = mgr
        .create_session(config)
        .await
        .expect("should create LocalLlm session");
    assert_eq!(session.info().model, "qwen3:8b");
}

#[cfg(feature = "analysis")]
#[tokio::test]
async fn create_local_llm_session_fails_before_admission_when_daemon_is_unreachable() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral port");
    let address = listener.local_addr().expect("listener address");
    drop(listener);
    let mgr = test_manager_with_local_llm(format!("http://{address}"), None);
    assert_preflight_rejects(mgr, "service.unavailable").await;
}

#[cfg(feature = "analysis")]
#[tokio::test]
async fn create_local_llm_session_fails_before_admission_when_endpoint_is_not_ollama() {
    let mut server = mockito::Server::new_async().await;
    let _version = server
        .mock("GET", "/api/version")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"status":"ok"}"#)
        .create_async()
        .await;
    let mgr = test_manager_with_local_llm(server.url(), None);
    assert_preflight_rejects(mgr, "config.invalid").await;
}

#[cfg(feature = "analysis")]
#[tokio::test]
async fn create_local_llm_session_fails_before_admission_when_model_is_missing() {
    let mut server = mockito::Server::new_async().await;
    let _version = server
        .mock("GET", "/api/version")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"version":"0.11.0"}"#)
        .create_async()
        .await;
    let _models = server
        .mock("GET", "/api/tags")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"models":[{"name":"mistral:7b"}]}"#)
        .create_async()
        .await;
    let mgr = test_manager_with_local_llm(server.url(), None);
    assert_preflight_rejects(mgr, "not_found.resource_missing").await;
}

#[cfg(feature = "analysis")]
async fn assert_preflight_rejects(mgr: SessionManagerImpl, expected_code: &str) {
    let config = SessionConfig {
        transport: SessionTransport::LocalLlm,
        surface_id: None,
        model: Some("llama3".to_string()),
        system_prompt: None,
        tools_enabled: false,
        cwd: None,
        sandbox_policy: None,
        approval_policy: None,
    };
    let Err(error) = mgr.create_session(config).await else {
        panic!("invalid Ollama readiness must fail before session admission");
    };
    assert_eq!(error.code(), expected_code);
    assert!(mgr.list_sessions().await.is_empty());
}
