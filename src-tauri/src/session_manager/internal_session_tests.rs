//! #12521: a session a use case opens for itself stays out of the Chat list and
//! the most-recent-session pickers, yet stays managed until it is closed.

use std::sync::Arc;

use maekon_core::config::AiSessionConfig;
use maekon_core::error::CoreError;
use maekon_core::models::ai_session::{SessionConfig, SessionTransport};
use maekon_core::ports::conversation_session::SessionManager;

use super::factory::LocalLlmTarget;
use super::SessionManagerImpl;

fn local_llm_config() -> SessionConfig {
    SessionConfig {
        transport: SessionTransport::LocalLlm,
        surface_id: None,
        model: Some("llama3".to_string()),
        system_prompt: Some("Be concise.".to_string()),
        tools_enabled: false,
        cwd: None,
        sandbox_policy: None,
        approval_policy: None,
    }
}

#[cfg(feature = "analysis")]
#[tokio::test]
async fn internal_session_is_managed_but_never_listed() {
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
    let mgr = SessionManagerImpl::new(
        Arc::new(AiSessionConfig {
            max_concurrent_sessions: 2,
            ..Default::default()
        }),
        Arc::new(crate::auditing_session::tests::MockAudit::default()),
        None,
    )
    .with_local_llm_target(LocalLlmTarget {
        base_url: server.url(),
        default_model: None,
    });

    let listed = mgr
        .create_session(local_llm_config())
        .await
        .expect("listed session");
    let internal = mgr
        .create_internal_session(local_llm_config())
        .await
        .expect("internal session");

    let ids: Vec<String> = mgr
        .list_sessions()
        .await
        .into_iter()
        .map(|info| info.session_id)
        .collect();
    assert_eq!(ids, vec![listed.session_id().to_string()]);
    mgr.get_session(internal.session_id())
        .await
        .expect("an internal session is still managed");
    // Hidden is not free: it still holds one of the two session slots.
    let full = match mgr.create_session(local_llm_config()).await {
        Ok(_) => panic!("the internal session must count toward the limit"),
        Err(error) => error,
    };
    assert_eq!(full.code(), "service.unavailable");

    mgr.kill_session_with_reason(internal.session_id(), "context recovery finished")
        .await
        .expect("the internal session closes");
    assert!(matches!(
        mgr.get_session(internal.session_id())
            .await
            .err()
            .expect("a closed internal session is gone"),
        CoreError::NotFound { .. }
    ));
    assert!(mgr.internal_session_ids.lock().is_empty());
}
