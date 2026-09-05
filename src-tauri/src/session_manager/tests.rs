use super::*;
use maekon_core::models::ai_session::SessionTransport;

#[cfg(feature = "analysis")]
mod local_llm_readiness;

fn test_config() -> Arc<AiSessionConfig> {
    Arc::new(AiSessionConfig {
        max_concurrent_sessions: 2,
        idle_timeout_secs: 1,
        ..Default::default()
    })
}

fn test_manager() -> SessionManagerImpl {
    // Production always wires a privacy guard (session_wiring.rs:83). Mirror that
    // here so external-session creation isn't refused by the #4869 fail-closed
    // invariant; the bare-manager (no guard) path is exercised explicitly by
    // `decorate_session_refuses_external_session_without_guard`.
    test_manager_without_guard().with_privacy_guard(Arc::new(PassthroughGuard))
}

/// Build a manager whose local transport satisfies the same bounded Ollama
/// preflight as production. Keep every returned guard alive for the duration of
/// the test so lifecycle tests do not accidentally depend on a developer's
/// machine-wide Ollama daemon or model catalog.
macro_rules! ready_local_llm_manager {
    ($manager:expr) => {{
        let mut server = mockito::Server::new_async().await;
        let version = server
            .mock("GET", "/api/version")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"version":"0.11.0"}"#)
            .create_async()
            .await;
        let models = server
            .mock("GET", "/api/tags")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"models":[{"name":"llama3"}]}"#)
            .create_async()
            .await;
        let manager =
            ($manager).with_local_llm_target(crate::session_manager::factory::LocalLlmTarget {
                base_url: server.url(),
                default_model: None,
            });
        (manager, server, version, models)
    }};
}

fn test_manager_without_guard() -> SessionManagerImpl {
    SessionManagerImpl::new(
        test_config(),
        Arc::new(crate::auditing_session::tests::MockAudit::default()),
        None,
    )
}

/// Helper: extract error message from a Result whose Ok type is not Debug.
fn expect_err_msg(result: Result<Arc<dyn ConversationSession>, CoreError>) -> String {
    match result {
        Err(e) => format!("{e}"),
        Ok(_) => panic!("expected Err, got Ok"),
    }
}

fn has_any_subprocess_cli() -> bool {
    !crate::subprocess_provider::detect_known_cli_surfaces().is_empty()
}

#[tokio::test]
async fn list_sessions_empty() {
    let mgr = test_manager();
    assert!(mgr.list_sessions().await.is_empty());
}

/// D7 (#4812 / E20-20): spot-prove that `with_breaker_registry` stores the exact
/// SAME `Arc<CircuitBreakerRegistry>` the composition root threads in — not a
/// fresh `CircuitBreakerRegistry::new()`. This is the load-bearing guarantee that
/// `HttpApiSession`s created by this manager converge on the one shared breaker.
#[test]
fn with_breaker_registry_shares_the_exact_arc() {
    // #7947: reference the crate-local alias, not `maekon_network` directly, so
    // this test compiles in the `--no-default-features` cell too. Under
    // `analysis` the alias IS `maekon_http_core::circuit_breaker::CircuitBreakerRegistry` (a
    // re-export), so this is byte-for-byte identical there while also keeping
    // the Arc-identity coverage alive when the network dep is absent.
    let shared = crate::breaker_registry::CircuitBreakerRegistry::new();
    // Default-constructed manager gets its OWN fresh registry...
    let default_mgr = test_manager_without_guard();
    assert!(
        !Arc::ptr_eq(&default_mgr.breaker_registry, &shared),
        "a default manager must NOT already point at the shared registry"
    );
    // ...and `with_breaker_registry` replaces it with the shared Arc identity.
    let wired = test_manager_without_guard().with_breaker_registry(shared.clone());
    assert!(
        Arc::ptr_eq(&wired.breaker_registry, &shared),
        "with_breaker_registry must store the SAME Arc (shared breaker), not a clone of a new registry"
    );
}

#[tokio::test]
async fn kill_nonexistent_session_returns_error() {
    let mgr = test_manager();
    assert!(
        matches!(
            mgr.kill_session("nonexistent").await.unwrap_err(),
            CoreError::NotFound { .. }
        ),
        "killing a non-existent session must yield CoreError::NotFound"
    );
}

#[tokio::test]
async fn get_session_not_found() {
    let mgr = test_manager();
    let err_msg = expect_err_msg(mgr.get_session("no-such-id").await);
    assert!(err_msg.contains("session not found"));
}

#[tokio::test]
async fn create_subprocess_session_uses_detected_surface() {
    // probe_known_cli_surfaces checks the filesystem for installed CLIs.
    // If no supported CLI is installed (e.g. CI), the test gracefully verifies
    // the corresponding detection error instead.
    let mgr = test_manager();
    let config = SessionConfig {
        transport: SessionTransport::Subprocess,
        surface_id: None,
        model: None,
        system_prompt: Some("You are a test assistant.".to_string()),
        tools_enabled: false,
        cwd: None,
        sandbox_policy: None,
        approval_policy: None,
    };
    let result = mgr.create_session(config).await;

    if has_any_subprocess_cli() {
        let session = match result {
            Ok(session) => session,
            Err(e) => panic!("should create session when a supported CLI is present: {e}"),
        };
        assert!(!session.session_id().is_empty());
        assert!(!session.provider_name().is_empty());

        // Verify it was stored and is retrievable by the same session id.
        let retrieved = mgr
            .get_session(session.session_id())
            .await
            .expect("created session must be retrievable by its own session_id");
        assert_eq!(
            retrieved.session_id(),
            session.session_id(),
            "retrieved session id must match the created one"
        );

        // Verify it appears in list
        let list = mgr.list_sessions().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].session_id, session.session_id());
    } else {
        // Iter-94: error type refactored from Internal to NotFound. The
        // `subprocess_cli_surface` resource-type identifier and the
        // `not_found.resource_missing` wire code are the stable contract
        // for this detection-miss scenario going forward.
        let err_msg = expect_err_msg(result);
        assert!(
            err_msg.contains("subprocess_cli_surface"),
            "expected `subprocess_cli_surface` in err: {err_msg}",
        );
        assert!(
            err_msg.contains("not_found.resource_missing"),
            "expected wire code in err: {err_msg}",
        );
    }
}

#[tokio::test]
async fn create_http_api_session_requires_surface_id() {
    let mgr = test_manager();
    let config = SessionConfig {
        transport: SessionTransport::HttpApi,
        surface_id: None,
        model: None,
        system_prompt: None,
        tools_enabled: false,
        cwd: None,
        sandbox_policy: None,
        approval_policy: None,
    };
    let err_msg = expect_err_msg(mgr.create_session(config).await);
    assert!(
        err_msg.contains("surface_id is required"),
        "expected surface_id error, got: {err_msg}",
    );
}

// ── C2 #5722: resolve_local_llm_target resolver tests ─────────────────────────

#[test]
fn resolver_ollama_llm_api_strips_v1_responses_suffix() {
    use crate::session_manager::factory::resolve_local_llm_target;
    use maekon_core::config::{AiProviderConfig, AiProviderType, ExternalApiEndpoint};

    let config = AiProviderConfig {
        llm_api: Some(ExternalApiEndpoint {
            endpoint: "http://192.168.0.10:11434/v1/responses".to_string(),
            api_key: String::new(),
            model: None,
            timeout_secs: 30,
            provider_type: AiProviderType::Ollama,
            surface_id: None,
            credential: None,
        }),
        ..Default::default()
    };
    let target = resolve_local_llm_target(&config);
    assert_eq!(
        target.base_url, "http://192.168.0.10:11434",
        "custom Ollama host must be preserved after suffix strip"
    );
}

#[test]
fn resolver_ollama_llm_api_preserves_custom_path_prefix() {
    use crate::session_manager::factory::resolve_local_llm_target;
    use maekon_core::config::{AiProviderConfig, AiProviderType, ExternalApiEndpoint};

    let config = AiProviderConfig {
        llm_api: Some(ExternalApiEndpoint {
            endpoint: "http://127.0.0.1:11434/edge/ollama/v1/responses".to_string(),
            api_key: String::new(),
            model: None,
            timeout_secs: 30,
            provider_type: AiProviderType::Ollama,
            surface_id: None,
            credential: None,
        }),
        ..Default::default()
    };
    let target = resolve_local_llm_target(&config);
    assert_eq!(
        target.base_url, "http://127.0.0.1:11434/edge/ollama",
        "custom path prefix must be preserved, known suffix stripped"
    );
}

#[test]
fn resolver_ollama_llm_api_unknown_suffix_falls_back_to_origin() {
    use crate::session_manager::factory::resolve_local_llm_target;
    use maekon_core::config::{AiProviderConfig, AiProviderType, ExternalApiEndpoint};

    // Per Decision 5: unrecognized suffix → origin-only (scheme://host:port).
    let config = AiProviderConfig {
        llm_api: Some(ExternalApiEndpoint {
            endpoint: "http://127.0.0.1:11434/custom-path".to_string(),
            api_key: String::new(),
            model: None,
            timeout_secs: 30,
            provider_type: AiProviderType::Ollama,
            surface_id: None,
            credential: None,
        }),
        ..Default::default()
    };
    let target = resolve_local_llm_target(&config);
    assert_eq!(
        target.base_url, "http://127.0.0.1:11434",
        "unknown suffix must fall back to origin-only (scheme://host:port)"
    );
}

#[test]
fn resolver_non_ollama_llm_api_uses_catalog_default() {
    use crate::session_manager::factory::resolve_local_llm_target;
    use maekon_core::config::{AiProviderConfig, AiProviderType, ExternalApiEndpoint};

    // Non-Ollama llm_api must be ignored; catalog default is loopback Ollama.
    let config = AiProviderConfig {
        llm_api: Some(ExternalApiEndpoint {
            endpoint: "https://api.openai.com/v1/chat/completions".to_string(),
            api_key: "sk-test".to_string(),
            model: Some("gpt-4".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::OpenAi,
            surface_id: None,
            credential: None,
        }),
        ..Default::default()
    };
    let target = resolve_local_llm_target(&config);
    assert!(
        target.base_url.starts_with("http://"),
        "non-Ollama llm_api must fall back to catalog default, got: {}",
        target.base_url
    );
    assert!(
        target.base_url.contains("11434") || target.base_url.contains("localhost"),
        "catalog default must be a loopback Ollama base, got: {}",
        target.base_url
    );
}

#[test]
fn resolver_no_llm_api_uses_catalog_default() {
    use crate::session_manager::factory::resolve_local_llm_target;
    use maekon_core::config::AiProviderConfig;

    let config = AiProviderConfig::default();
    let target = resolve_local_llm_target(&config);
    // Catalog default = http://localhost:11434 (loopback).
    assert!(
        target.base_url.contains("localhost") || target.base_url.contains("127.0.0.1"),
        "no llm_api → catalog loopback default, got: {}",
        target.base_url
    );
    assert!(
        target.default_model.is_none(),
        "no llm_api → no user-configured model (catalog resolver used downstream)"
    );
}

/// Decision 2 / missing_hop 2: IPv6 loopback [::1] must be treated as local
/// (is_external() == false) by LocalLlmSession.
#[cfg(feature = "analysis")]
#[test]
fn local_llm_session_ipv6_loopback_is_not_external() {
    use maekon_core::ports::conversation_session::ConversationSession;
    use maekon_network::local_llm_session::LocalLlmSession;

    let session = LocalLlmSession::new(
        "test-id".to_string(),
        "qwen3:8b".to_string(),
        "http://[::1]:11434".to_string(),
        None,
        Arc::new(AiSessionConfig::default()),
    );
    assert!(
        !session.is_external(),
        "[::1] (IPv6 loopback) must be treated as local, not external"
    );
}

/// Decision 2: a LAN host (non-loopback) must trigger is_external == true.
#[cfg(feature = "analysis")]
#[test]
fn local_llm_session_lan_host_is_external() {
    use maekon_core::ports::conversation_session::ConversationSession;
    use maekon_network::local_llm_session::LocalLlmSession;

    let session = LocalLlmSession::new(
        "test-id".to_string(),
        "qwen3:8b".to_string(),
        "http://192.168.0.10:11434".to_string(),
        None,
        Arc::new(AiSessionConfig::default()),
    );
    assert!(
        session.is_external(),
        "LAN host (192.168.0.x) must be treated as external (engages PII guard)"
    );
}

#[tokio::test]
async fn create_session_enforces_max_concurrent_limit() {
    if !has_any_subprocess_cli() {
        return; // skip in environments without a supported subprocess CLI
    }

    let mgr = test_manager(); // max_concurrent_sessions = 2
    let make_config = || SessionConfig {
        transport: SessionTransport::Subprocess,
        surface_id: None,
        model: None,
        system_prompt: None,
        tools_enabled: false,
        cwd: None,
        sandbox_policy: None,
        approval_policy: None,
    };

    let _s1 = mgr.create_session(make_config()).await.expect("session 1");
    let _s2 = mgr.create_session(make_config()).await.expect("session 2");
    // Iter-97: capacity limit emits CoreError::ServiceUnavailable with wire
    // code `service.unavailable` (was Internal.Generic pre-iter-97).
    let result = mgr.create_session(make_config()).await;
    let err = match result {
        Ok(_) => panic!("third session should fail when at capacity"),
        Err(e) => e,
    };
    assert_eq!(err.code(), "service.unavailable");
    assert!(
        err.to_string().contains("max concurrent sessions"),
        "err should reference capacity limit, got: {err}"
    );
}

#[tokio::test]
async fn kill_session_removes_from_map() {
    if !has_any_subprocess_cli() {
        return;
    }

    let mgr = test_manager();
    let config = SessionConfig {
        transport: SessionTransport::Subprocess,
        surface_id: None,
        model: None,
        system_prompt: None,
        tools_enabled: false,
        cwd: None,
        sandbox_policy: None,
        approval_policy: None,
    };
    let session = mgr.create_session(config).await.expect("create session");
    let id = session.session_id().to_string();

    // Confirm the session exists before killing it.
    mgr.get_session(&id)
        .await
        .expect("session must be present before kill");
    mgr.kill_session(&id).await.unwrap();
    assert!(
        matches!(
            mgr.get_session(&id)
                .await
                .err()
                .expect("get_session after kill must Err"),
            CoreError::NotFound { .. }
        ),
        "get_session after kill must yield CoreError::NotFound"
    );
    assert!(mgr.list_sessions().await.is_empty());
}

#[tokio::test]
async fn touch_session_resets_state_to_active() {
    let (mgr, _server, _version, _models) = ready_local_llm_manager!(test_manager());

    // Create a LocalLlm session (no CLI dependency).
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
    let session = mgr.create_session(config).await.expect("create session");
    let id = session.session_id().to_string();

    // Manually mark the session as Idle to simulate idle timeout.
    {
        let mut sessions = mgr.sessions.write().await;
        let managed = sessions.get_mut(&id).unwrap();
        managed.state = SessionState::Idle;
        assert_eq!(managed.state, SessionState::Idle);
    }

    // touch_session should reset state to Active.
    mgr.touch_session(&id).await;

    {
        let sessions = mgr.sessions.read().await;
        let managed = sessions.get(&id).unwrap();
        assert_eq!(managed.state, SessionState::Active);
    }
}

#[tokio::test]
async fn reap_marks_idle_then_terminates() {
    // Use a very short idle timeout (1 second from test_config).
    let (mgr, _server, _version, _models) = ready_local_llm_manager!(test_manager());

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
    let session = mgr.create_session(config).await.expect("create session");
    let id = session.session_id().to_string();

    // Force last_active to be in the past (beyond idle_timeout_secs=1).
    {
        let mut sessions = mgr.sessions.write().await;
        let managed = sessions.get_mut(&id).unwrap();
        managed.last_active = Instant::now() - std::time::Duration::from_secs(5);
    }

    // First reap: Active → Idle (should NOT remove from map).
    mgr.reap_idle_sessions().await;
    {
        let sessions = mgr.sessions.read().await;
        let managed = sessions
            .get(&id)
            .expect("session should still exist after first reap");
        assert_eq!(managed.state, SessionState::Idle);
    }

    // Force last_active again so the Idle session also exceeds timeout.
    {
        let mut sessions = mgr.sessions.write().await;
        let managed = sessions.get_mut(&id).unwrap();
        managed.last_active = Instant::now() - std::time::Duration::from_secs(5);
    }

    // Second reap: Idle → Terminated (removed from map).
    mgr.reap_idle_sessions().await;
    assert!(
        matches!(
            mgr.get_session(&id)
                .await
                .err()
                .expect("session must be removed after second reap"),
            CoreError::NotFound { .. }
        ),
        "session should be removed after second reap with CoreError::NotFound"
    );
    assert!(mgr.list_sessions().await.is_empty());
}

#[tokio::test]
async fn create_session_uses_context_assembler() {
    use crate::scheduler::shared_regime_state::SharedRegimeState;
    use maekon_core::config::AppConfig;
    use maekon_storage::sqlite::SqliteStorage;

    let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
    let app_config = Arc::new(AppConfig::default_config());
    let regime_state = Arc::new(SharedRegimeState::new());
    let assembler = Arc::new(SessionContextAssembler::new(
        storage,
        app_config,
        regime_state,
    ));

    let (mgr, _server, _version, _models) = ready_local_llm_manager!(SessionManagerImpl::new(
        test_config(),
        Arc::new(crate::auditing_session::tests::MockAudit::default()),
        Some(assembler),
    ));

    // Create a LocalLlm session with system_prompt = None.
    // The context assembler should inject a system prompt automatically.
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

    let session = mgr
        .create_session(config)
        .await
        .expect("should create session with assembled context");

    // The session should have been created successfully.
    assert!(!session.session_id().is_empty());

    // Verify the session is stored and retrievable by the same session id.
    let retrieved = mgr
        .get_session(session.session_id())
        .await
        .expect("assembled-context session must be retrievable after creation");
    assert_eq!(
        retrieved.session_id(),
        session.session_id(),
        "retrieved session id must match the created one"
    );
}

#[tokio::test]
async fn create_session_preserves_explicit_system_prompt() {
    use crate::scheduler::shared_regime_state::SharedRegimeState;
    use maekon_core::config::AppConfig;
    use maekon_storage::sqlite::SqliteStorage;

    let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
    let app_config = Arc::new(AppConfig::default_config());
    let regime_state = Arc::new(SharedRegimeState::new());
    let assembler = Arc::new(SessionContextAssembler::new(
        storage,
        app_config,
        regime_state,
    ));

    let (mgr, _server, _version, _models) = ready_local_llm_manager!(SessionManagerImpl::new(
        test_config(),
        Arc::new(crate::auditing_session::tests::MockAudit::default()),
        Some(assembler),
    ));

    // Create a LocalLlm session with an explicit system prompt.
    // The context assembler should NOT override it.
    let config = SessionConfig {
        transport: SessionTransport::LocalLlm,
        surface_id: None,
        model: Some("llama3".to_string()),
        system_prompt: Some("Custom prompt".to_string()),
        tools_enabled: false,
        cwd: None,
        sandbox_policy: None,
        approval_policy: None,
    };

    let session = mgr
        .create_session(config)
        .await
        .expect("should create session with explicit prompt");

    assert!(!session.session_id().is_empty());
}

#[tokio::test]
async fn recover_session_increments_retry_count() {
    if !has_any_subprocess_cli() {
        return; // skip in environments without a supported subprocess CLI
    }

    let mgr = test_manager();
    let config = SessionConfig {
        transport: SessionTransport::Subprocess,
        surface_id: None,
        model: None,
        system_prompt: None,
        tools_enabled: false,
        cwd: None,
        sandbox_policy: None,
        approval_policy: None,
    };
    let session = mgr.create_session(config).await.expect("create session");
    let id = session.session_id().to_string();

    // First recovery should succeed with retry_count = 1 and return the
    // refreshed session Arc (the caller may re-use it without re-fetching).
    let recovered = mgr
        .recover_session(&id)
        .await
        .expect("first recovery must succeed");
    assert_eq!(
        recovered.session_id(),
        id,
        "recovered session must carry the same session id"
    );

    {
        let sessions = mgr.sessions.read().await;
        let managed = sessions.get(&id).unwrap();
        assert_eq!(managed.retry_count, 1);
        assert_eq!(managed.state, SessionState::Active);
    }

    // Second recovery should succeed with retry_count = 2.
    let _ = mgr.recover_session(&id).await.expect("second recovery");
    {
        let sessions = mgr.sessions.read().await;
        let managed = sessions.get(&id).unwrap();
        assert_eq!(managed.retry_count, 2);
    }
}

#[tokio::test]
async fn recover_session_fails_after_max_retries() {
    if !has_any_subprocess_cli() {
        return; // skip in environments without a supported subprocess CLI
    }

    let config = Arc::new(AiSessionConfig {
        max_concurrent_sessions: 2,
        idle_timeout_secs: 1,
        max_retries: 2,
        ..Default::default()
    });
    let mgr = SessionManagerImpl::new(
        config,
        Arc::new(crate::auditing_session::tests::MockAudit::default()),
        None,
    )
    .with_privacy_guard(Arc::new(PassthroughGuard));

    let session_config = SessionConfig {
        transport: SessionTransport::Subprocess,
        surface_id: None,
        model: None,
        system_prompt: None,
        tools_enabled: false,
        cwd: None,
        sandbox_policy: None,
        approval_policy: None,
    };
    let session = mgr
        .create_session(session_config)
        .await
        .expect("create session");
    let id = session.session_id().to_string();

    // Exhaust max_retries (2).
    let _ = mgr.recover_session(&id).await.expect("recovery 1");
    let _ = mgr.recover_session(&id).await.expect("recovery 2");

    // Third attempt should fail.
    // Iter-97: retry exhaustion emits CoreError::ServiceUnavailable with wire
    // code `service.unavailable` (was Internal.Generic pre-iter-97).
    let result = mgr.recover_session(&id).await;
    let err = match result {
        Ok(_) => panic!("third recover should fail after exhausting retries"),
        Err(e) => e,
    };
    assert_eq!(err.code(), "service.unavailable");
    assert!(
        err.to_string().contains("max retries exceeded"),
        "err should reference retry exhaustion, got: {err}"
    );

    // Session state should be Failed.
    {
        let sessions = mgr.sessions.read().await;
        let managed = sessions.get(&id).unwrap();
        assert_eq!(managed.state, SessionState::Failed);
    }
}

/// A session that cannot self-heal in place (#8057 P3) — mirrors the Codex
/// app-server backend whose single long-lived process is dead after a failure.
struct NonHealingSession;

#[async_trait]
impl ConversationSession for NonHealingSession {
    async fn send_message(
        &self,
        _: &maekon_core::models::ai_session::SessionMessage,
    ) -> Result<maekon_core::ports::conversation_session::ResponseStream, CoreError> {
        Ok(Box::pin(futures::stream::empty()))
    }
    fn info(&self) -> ConversationSessionInfo {
        ConversationSessionInfo {
            session_id: "non-healing".to_string(),
            provider_name: "codex".to_string(),
            model: "m".to_string(),
            state: SessionState::Failed,
            transport: SessionTransport::Subprocess,
            created_at: chrono::Utc::now(),
            last_active: chrono::Utc::now(),
            turn_count: 0,
            title: None,
        }
    }
    fn session_id(&self) -> &str {
        "non-healing"
    }
    fn provider_name(&self) -> &str {
        "codex"
    }
    fn can_self_heal_on_retry(&self) -> bool {
        false
    }
}

#[tokio::test]
async fn recover_session_rejects_non_self_healing_backend() {
    // #8057 (P3, #4872): a backend that cannot re-establish itself in place must
    // surface an honest `service.unavailable` error (guiding re-creation) rather
    // than a misleading "recovered" (Active) transition that re-fails next send.
    let mgr = test_manager();
    let id = "non-healing".to_string();
    mgr.sessions.write().await.insert(
        id.clone(),
        ManagedSession {
            session: Arc::new(NonHealingSession),
            state: SessionState::Failed,
            created_at: Instant::now(),
            last_active: Instant::now(),
            retry_count: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
        },
    );

    let err = match mgr.recover_session(&id).await {
        Ok(_) => panic!("a non-self-healing backend must not be recovered in place"),
        Err(e) => e,
    };
    assert_eq!(err.code(), "service.unavailable");
    assert!(
        err.to_string().contains("re-create"),
        "error must guide re-creation, got: {err}"
    );

    // State stays Failed (never flipped to Active) — retry_count untouched.
    let sessions = mgr.sessions.read().await;
    let managed = sessions.get(&id).unwrap();
    assert_eq!(managed.state, SessionState::Failed);
    assert_eq!(managed.retry_count, 0);
}

// ── report_failure tests ───────────────────────────────────

#[tokio::test]
async fn report_failure_transient_auto_recovers() {
    let (mgr, _server, _version, _models) = ready_local_llm_manager!(test_manager());
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
    let session = mgr.create_session(config).await.expect("create session");
    let id = session.session_id().to_string();

    let err = CoreError::Network {
        code: maekon_core::error_codes::NetworkCode::Generic,
        message: "connection reset".into(),
    };
    let result = mgr.report_failure(&id, &err).await;
    assert_eq!(result, SessionState::Active);

    let sessions = mgr.sessions.read().await;
    let managed = sessions.get(&id).unwrap();
    assert_eq!(managed.retry_count, 1);
    assert_eq!(managed.state, SessionState::Active);
}

#[tokio::test]
async fn record_success_resets_retry_count() {
    let mgr = test_manager_without_guard();
    let id = "fake-success-session".to_string();

    {
        let mut sessions = mgr.sessions.write().await;
        sessions.insert(
            id.clone(),
            ManagedSession {
                session: Arc::new(FakeConvSession { external: false }),
                state: SessionState::Active,
                created_at: std::time::Instant::now(),
                last_active: std::time::Instant::now(),
                retry_count: 2,
                total_input_tokens: 0,
                total_output_tokens: 0,
            },
        );
    }

    mgr.record_success(&id).await;

    let sessions = mgr.sessions.read().await;
    let managed = sessions.get(&id).unwrap();
    assert_eq!(managed.retry_count, 0);
    assert_eq!(managed.state, SessionState::Active);
}

#[tokio::test]
async fn report_failure_permanent_sets_failed() {
    let (mgr, _server, _version, _models) = ready_local_llm_manager!(test_manager());
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
    let session = mgr.create_session(config).await.expect("create session");
    let id = session.session_id().to_string();

    let err = CoreError::Auth {
        code: maekon_core::error_codes::AuthCode::Failed,
        message: "invalid API key".into(),
    };
    let result = mgr.report_failure(&id, &err).await;
    assert_eq!(result, SessionState::Failed);

    let sessions = mgr.sessions.read().await;
    let managed = sessions.get(&id).unwrap();
    assert_eq!(managed.state, SessionState::Failed);
}

#[tokio::test]
async fn report_failure_exhausts_retries() {
    let config = Arc::new(AiSessionConfig {
        max_concurrent_sessions: 2,
        idle_timeout_secs: 300,
        max_retries: 3,
        ..Default::default()
    });
    let (mgr, _server, _version, _models) = ready_local_llm_manager!(SessionManagerImpl::new(
        config,
        Arc::new(crate::auditing_session::tests::MockAudit::default()),
        None,
    ));

    let session_config = SessionConfig {
        transport: SessionTransport::LocalLlm,
        surface_id: None,
        model: Some("llama3".to_string()),
        system_prompt: None,
        tools_enabled: false,
        cwd: None,
        sandbox_policy: None,
        approval_policy: None,
    };
    let session = mgr.create_session(session_config).await.expect("create");
    let id = session.session_id().to_string();

    let err = CoreError::Network {
        code: maekon_core::error_codes::NetworkCode::Generic,
        message: "timeout".into(),
    };
    // First 3 should auto-recover.
    for i in 1..=3 {
        let result = mgr.report_failure(&id, &err).await;
        assert_eq!(result, SessionState::Active, "retry {i} should recover");
    }
    // 4th should fail.
    let result = mgr.report_failure(&id, &err).await;
    assert_eq!(result, SessionState::Failed);
}

#[tokio::test]
async fn report_failure_nonexistent_session() {
    let mgr = test_manager();
    let err = CoreError::Network {
        code: maekon_core::error_codes::NetworkCode::Generic,
        message: "test".into(),
    };
    let result = mgr.report_failure("no-such-id", &err).await;
    assert_eq!(result, SessionState::Terminated);
}

// ── absolute timeout tests ─────────────────────────────────

#[tokio::test]
async fn reap_enforces_absolute_timeout() {
    let config = Arc::new(AiSessionConfig {
        max_concurrent_sessions: 2,
        idle_timeout_secs: 300,
        session_timeout_secs: 2,
        ..Default::default()
    });
    let (mgr, _server, _version, _models) = ready_local_llm_manager!(SessionManagerImpl::new(
        config,
        Arc::new(crate::auditing_session::tests::MockAudit::default()),
        None,
    ));

    let session_config = SessionConfig {
        transport: SessionTransport::LocalLlm,
        surface_id: None,
        model: Some("llama3".to_string()),
        system_prompt: None,
        tools_enabled: false,
        cwd: None,
        sandbox_policy: None,
        approval_policy: None,
    };
    let session = mgr.create_session(session_config).await.expect("create");
    let id = session.session_id().to_string();

    // Set created_at to past (beyond session_timeout_secs=2).
    {
        let mut sessions = mgr.sessions.write().await;
        let managed = sessions.get_mut(&id).unwrap();
        managed.created_at = Instant::now() - std::time::Duration::from_secs(10);
    }

    mgr.reap_idle_sessions().await;

    assert!(
        matches!(
            mgr.get_session(&id)
                .await
                .err()
                .expect("session must be removed after absolute timeout"),
            CoreError::NotFound { .. }
        ),
        "session should be removed after absolute timeout with CoreError::NotFound"
    );
}

#[tokio::test]
async fn reap_absolute_timeout_with_recent_activity() {
    let config = Arc::new(AiSessionConfig {
        max_concurrent_sessions: 2,
        idle_timeout_secs: 300,
        session_timeout_secs: 2,
        ..Default::default()
    });
    let (mgr, _server, _version, _models) = ready_local_llm_manager!(SessionManagerImpl::new(
        config,
        Arc::new(crate::auditing_session::tests::MockAudit::default()),
        None,
    ));

    let session_config = SessionConfig {
        transport: SessionTransport::LocalLlm,
        surface_id: None,
        model: Some("llama3".to_string()),
        system_prompt: None,
        tools_enabled: false,
        cwd: None,
        sandbox_policy: None,
        approval_policy: None,
    };
    let session = mgr.create_session(session_config).await.expect("create");
    let id = session.session_id().to_string();

    // created_at far in the past, but last_active is NOW.
    {
        let mut sessions = mgr.sessions.write().await;
        let managed = sessions.get_mut(&id).unwrap();
        managed.created_at = Instant::now() - std::time::Duration::from_secs(10);
        managed.last_active = Instant::now();
    }

    mgr.reap_idle_sessions().await;

    assert!(
        matches!(
            mgr.get_session(&id).await.err().expect("session must be reaped despite recent activity"),
            CoreError::NotFound { .. }
        ),
        "session should be reaped despite recent activity (absolute timeout) — CoreError::NotFound expected"
    );
}

#[tokio::test]
async fn emit_state_change_no_panic_without_handle() {
    let mgr = test_manager();
    // app_handle is None — should not panic.
    mgr.emit_state_change(
        "test-id",
        SessionState::Active,
        SessionState::Failed,
        "test",
    );
}

/// Iter-94 regression guard: requesting a subprocess surface that isn't
/// installed must produce a NotFound error with wire code
/// `not_found.resource_missing` and resource_type `subprocess_cli_surface`.
/// Pre-iter-94 this was CoreError::Internal with `internal.generic`, which
/// conflated missing-CLI scenarios with genuine runtime failures in
/// telemetry.
#[tokio::test]
async fn missing_subprocess_cli_surface_maps_to_not_found() {
    let mgr = test_manager();
    let config = SessionConfig {
        transport: SessionTransport::Subprocess,
        surface_id: Some("provider_surface.definitely_not_real".to_string()),
        model: None,
        system_prompt: None,
        tools_enabled: false,
        cwd: None,
        sandbox_policy: None,
        approval_policy: None,
    };
    let result = mgr.create_session(config).await;
    let err = match result {
        Ok(_) => panic!("unknown surface id must error"),
        Err(e) => e,
    };
    assert_eq!(err.code(), "not_found.resource_missing");
    assert!(
        err.to_string().contains("subprocess_cli_surface"),
        "err should reference the resource_type, got: {err}"
    );
    assert!(
        err.to_string().contains("definitely_not_real"),
        "err should carry the requested surface id, got: {err}"
    );
}

// ── #4869: decorate_session fail-closed invariant ─────────────────────────────

/// Minimal `ConversationSession` whose `is_external()` is configurable, used to
/// drive the guard fail-closed branch in `decorate_session`.
struct FakeConvSession {
    external: bool,
}

#[async_trait]
impl ConversationSession for FakeConvSession {
    async fn send_message(
        &self,
        _: &maekon_core::models::ai_session::SessionMessage,
    ) -> Result<maekon_core::ports::conversation_session::ResponseStream, CoreError> {
        Ok(Box::pin(futures::stream::empty()))
    }
    fn info(&self) -> ConversationSessionInfo {
        ConversationSessionInfo {
            session_id: "fake".to_string(),
            provider_name: "fake-provider".to_string(),
            model: "m".to_string(),
            state: SessionState::Active,
            transport: SessionTransport::Subprocess,
            created_at: chrono::Utc::now(),
            last_active: chrono::Utc::now(),
            turn_count: 0,
            title: None,
        }
    }
    fn session_id(&self) -> &str {
        "fake"
    }
    fn provider_name(&self) -> &str {
        "fake-provider"
    }
    fn is_external(&self) -> bool {
        self.external
    }
}

/// Passthrough guard: proves the "guard present" branch wraps successfully.
struct PassthroughGuard;

#[async_trait]
impl ConversationContentGuard for PassthroughGuard {
    async fn sanitize_outbound(
        &self,
        message: &maekon_core::models::ai_session::SessionMessage,
        _correlation_id: &str,
    ) -> Result<maekon_core::models::ai_session::SessionMessage, CoreError> {
        Ok(message.clone())
    }
}

#[test]
fn decorate_session_refuses_external_session_without_guard() {
    // Explicitly build a manager WITHOUT a privacy guard (test_manager() now
    // wires a passthrough guard to mirror production).
    let mgr = test_manager_without_guard();
    let result = mgr.decorate_session(Arc::new(FakeConvSession { external: true }));
    let err = expect_err_msg(result);
    assert!(
        err.contains("privacy guard not configured") && err.contains("fake-provider"),
        "external session must be refused fail-closed when no guard is configured, got: {err}"
    );
}

#[test]
fn decorate_session_allows_local_session_without_guard() {
    // Local sessions keep data on-device, so they pass through even without a guard.
    let mgr = test_manager_without_guard();
    let wrapped = mgr
        .decorate_session(Arc::new(FakeConvSession { external: false }))
        .expect("local session must be allowed without a guard");
    assert!(!wrapped.is_external());
}

#[test]
fn decorate_session_allows_external_session_when_guard_present() {
    let mgr = test_manager().with_privacy_guard(Arc::new(PassthroughGuard));
    let wrapped = mgr
        .decorate_session(Arc::new(FakeConvSession { external: true }))
        .expect("external session must be allowed once a guard is configured");
    // The guard wraps the inner external session; `is_external()` is preserved
    // through the GuardedConversationSession + AuditingSession decorators.
    assert!(wrapped.is_external());
}

// ── #4871: Codex app-server rollout flag + exec fallback path selection ───────

#[test]
fn codex_rollout_flag_selects_transport_per_stage() {
    use super::factory::codex_should_attempt_app_server;
    use maekon_core::config::CodexAppServerRollout;

    // Off (default) → never attempt app-server; use codex exec (AC1: exec preserved).
    assert!(!codex_should_attempt_app_server(CodexAppServerRollout::Off));
    assert!(!codex_should_attempt_app_server(
        CodexAppServerRollout::default()
    ));
    // opt-in / default → attempt app-server (with exec fallback on failure).
    assert!(codex_should_attempt_app_server(
        CodexAppServerRollout::OptIn
    ));
    assert!(codex_should_attempt_app_server(
        CodexAppServerRollout::Default
    ));
}

#[test]
fn codex_exec_surface_is_the_fallback_target() {
    use super::factory::is_codex_exec_surface;

    // The exec sibling is the graceful fallback for the app-server transport.
    assert!(
        is_codex_exec_surface("provider_surface.openai.subprocess_cli"),
        "codex exec surface must be recognized as the fallback target"
    );
    // The app-server surface itself is NOT an exec fallback target.
    assert!(!is_codex_exec_surface(
        "provider_surface.openai.codex_app_server"
    ));
    // Unrelated / unknown surfaces are not exec targets.
    assert!(!is_codex_exec_surface(
        "provider_surface.anthropic.subprocess_cli"
    ));
    assert!(!is_codex_exec_surface("provider_surface.does_not_exist"));
}

#[test]
fn codex_exec_fallback_builds_session_from_exec_sibling() {
    use crate::subprocess_provider::{
        DetectedSubprocessCli, ProbedSubprocessCli, SubprocessCliAuthStatus,
    };

    let mgr = test_manager();
    let cfg = SessionConfig {
        transport: SessionTransport::Subprocess,
        surface_id: None,
        model: None,
        system_prompt: None,
        tools_enabled: false,
        cwd: None,
        sandbox_policy: None,
        approval_policy: None,
    };

    // Probed CLIs containing the codex exec sibling → fallback resolves it and
    // constructs a session (proves the exec-fallback build path, #4871 AC2).
    let with_exec = vec![ProbedSubprocessCli {
        detected: DetectedSubprocessCli {
            surface_id: "provider_surface.openai.subprocess_cli".to_string(),
            executable_path: std::path::PathBuf::from("/usr/bin/codex"),
        },
        auth_status: SubprocessCliAuthStatus::Authenticated,
        auth_detail: None,
    }];
    let session = mgr
        .build_codex_exec_session(&with_exec, &cfg, &None, "test")
        .expect("exec fallback must build a session when the exec sibling is present");
    // A real session was constructed on the exec sibling surface.
    assert!(!session.provider_name().is_empty());

    // No exec sibling detected → fallback fails closed with a clear NotFound.
    let without_exec = vec![ProbedSubprocessCli {
        detected: DetectedSubprocessCli {
            surface_id: "provider_surface.openai.codex_app_server".to_string(),
            executable_path: std::path::PathBuf::from("/usr/bin/codex"),
        },
        auth_status: SubprocessCliAuthStatus::Authenticated,
        auth_detail: None,
    }];
    let err = expect_err_msg(mgr.build_codex_exec_session(&without_exec, &cfg, &None, "test"));
    assert!(
        err.contains("codex_exec_surface"),
        "missing exec sibling must surface a clear NotFound, got: {err}"
    );
}

// ── E21 #4863 R7 + version-negotiation WIRED tests (#[cfg(unix)]) ─────────────
//
// These drive the REAL `connect_codex_app_server` spawn chokepoint through an
// on-disk fake `codex` binary that branches on `$1`:
//   * `--version`  → prints a version line (or exits 1 to model an unprobeable
//                    binary);
//   * `app-server` → runs a minimal JSONL `initialize`/`thread/start`/`turn/start`
//                    dialog (the same `sh` dialect proven by the inline fakes).
//
// They PROVE the trust + `--version` probe + userAgent-version extraction all run
// ON the spawn path (dead-writer ban), and pin the graceful-degrade invariant:
// version drift / out-of-allowlist path WARN-AND-PROCEED, never gate (#4871).
//
// #7947: gated on `analysis` as well as `unix` — `connect_codex_app_server` (the
// chokepoint these drive) is itself `#[cfg(feature = "analysis")]`, so without
// this gate the whole module failed to compile in the `--no-default-features`
// test cell (E0599). No default-cell coverage is lost: `analysis` is on for the
// default, `server`, and `grpc` builds where this path exists.
#[cfg(all(unix, feature = "analysis"))]
mod codex_app_server_wired {
    use super::*;
    use crate::subprocess_provider::{
        DetectedSubprocessCli, ProbedSubprocessCli, SubprocessCliAuthStatus,
    };
    use maekon_core::config::CodexAppServerRollout;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    const APP_SERVER_SURFACE: &str = "provider_surface.openai.codex_app_server";

    /// Write a fake `codex` executable at `dir/codex` that answers `--version`
    /// (per `version_line`) and runs a minimal app-server JSONL dialog for
    /// `app-server`. Chmod +x. The dialog returns userAgent `user_agent` and
    /// thread id `thr_42`, a one-line answer, and `turn/completed`.
    fn write_fake_codex(dir: &Path, version_line: Option<&str>, user_agent: &str) -> PathBuf {
        let path = dir.join("codex");
        let version_branch = match version_line {
            Some(line) => format!("printf '%s\\n' '{line}'; exit 0"),
            None => "exit 1".to_string(),
        };
        // Minimal JSONL read-loop: extract `id`, branch on the quoted method.
        let dialog = format!(
            r#"while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*) printf '{{"id":%s,"result":{{"userAgent":"{user_agent}"}}}}\n' "$id" ;;
    *'"thread/start"'*) printf '{{"id":%s,"result":{{"threadId":"thr_42"}}}}\n' "$id" ;;
    *'"turn/start"'*) printf '{{"id":%s,"result":{{}}}}\n' "$id"; printf '{{"method":"item/agentMessage/delta","params":{{"text":"Hi"}}}}\n'; printf '{{"method":"turn/completed","params":{{}}}}\n' ;;
  esac
done"#
        );
        let script = format!(
            "#!/bin/sh\ncase \"$1\" in\n  --version) {version_branch} ;;\n  app-server) {dialog} ;;\n  *) {dialog} ;;\nesac\n"
        );
        let mut file = std::fs::File::create(&path).expect("create fake codex");
        file.write_all(script.as_bytes()).expect("write fake codex");
        file.flush().expect("flush fake codex");
        let mut perms = std::fs::metadata(&path).expect("meta").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).expect("chmod");
        path
    }

    fn app_server_surface(exe: PathBuf) -> DetectedSubprocessCli {
        DetectedSubprocessCli {
            surface_id: APP_SERVER_SURFACE.to_string(),
            executable_path: exe,
        }
    }

    fn subprocess_config() -> SessionConfig {
        subprocess_config_with_model(None)
    }

    fn subprocess_config_with_model(model: Option<&str>) -> SessionConfig {
        SessionConfig {
            transport: SessionTransport::Subprocess,
            surface_id: Some(APP_SERVER_SURFACE.to_string()),
            model: model.map(str::to_string),
            system_prompt: None,
            tools_enabled: false,
            cwd: None,
            sandbox_policy: None,
            approval_policy: None,
        }
    }

    /// T1 (happy): trusted exe path + good `--version` + good handshake →
    /// `connect_codex_app_server` returns the app-server-backed session
    /// (session_id == the fake thread id "thr_42"). Proves probe + trust +
    /// userAgent extraction all RUN on the spawn path.
    #[tokio::test]
    #[serial_test::serial(maekon_codex_allowed_dirs_env)]
    async fn t1_trusted_good_version_good_handshake_yields_app_server_session() {
        let dir = tempfile::tempdir().expect("tempdir");
        let exe = write_fake_codex(dir.path(), Some("codex-cli 1.2.3"), "codex-app-server/1.0");
        // Trust the tempdir via the additive operator override.
        std::env::set_var("MAEKON_CODEX_ALLOWED_DIRS", dir.path());

        let mgr = test_manager().with_codex_app_server_rollout(CodexAppServerRollout::OptIn);
        let result = mgr
            .connect_codex_app_server(&app_server_surface(exe), &subprocess_config())
            .await;
        std::env::remove_var("MAEKON_CODEX_ALLOWED_DIRS");

        let session = result.expect("trusted + probeable + good handshake → app-server session");
        assert_eq!(
            session.session_id(),
            "thr_42",
            "an app-server session uses the server thread id as its session id"
        );
        assert_eq!(
            session.info().model,
            "gpt-5.6-sol",
            "an app-server session with no override must use the GPT-5.6 Sol default"
        );
    }

    #[tokio::test]
    #[serial_test::serial(maekon_codex_allowed_dirs_env)]
    async fn explicit_model_override_reaches_app_server_session_info() {
        let dir = tempfile::tempdir().expect("tempdir");
        let exe = write_fake_codex(dir.path(), Some("codex-cli 1.2.3"), "codex-app-server/1.0");
        std::env::set_var("MAEKON_CODEX_ALLOWED_DIRS", dir.path());

        let mgr = test_manager().with_codex_app_server_rollout(CodexAppServerRollout::OptIn);
        let result = mgr
            .connect_codex_app_server(
                &app_server_surface(exe),
                &subprocess_config_with_model(Some("gpt-5.6-terra")),
            )
            .await;
        std::env::remove_var("MAEKON_CODEX_ALLOWED_DIRS");

        let session = result.expect("explicit model override should create app-server session");
        assert_eq!(session.info().model, "gpt-5.6-terra");
    }

    /// T2 (probe-fail → graceful): a binary whose `--version` exits non-zero is
    /// "unprobeable" → `connect_codex_app_server` returns Err (the ONLY hard
    /// branch). `create_codex_session_with_fallback` then degrades to `codex
    /// exec`. We assert BOTH: the direct Err token, and the fallback producing a
    /// non-app-server (UUID-id) session.
    #[tokio::test]
    #[serial_test::serial(maekon_codex_allowed_dirs_env)]
    async fn t2_unprobeable_binary_degrades_to_exec() {
        let dir = tempfile::tempdir().expect("tempdir");
        // version_line=None → `--version` exits 1 (unprobeable).
        let exe = write_fake_codex(dir.path(), None, "codex-app-server/1.0");
        std::env::set_var("MAEKON_CODEX_ALLOWED_DIRS", dir.path());

        let mgr = test_manager().with_codex_app_server_rollout(CodexAppServerRollout::OptIn);

        // Direct: connect returns the hard probe-fail Err (not an abort).
        let direct = mgr
            .connect_codex_app_server(&app_server_surface(exe.clone()), &subprocess_config())
            .await;
        let err = expect_err_msg(direct);
        std::env::remove_var("MAEKON_CODEX_ALLOWED_DIRS");
        assert!(
            err.contains("codex_version_unverified"),
            "an unprobeable binary must surface codex_version_unverified, got: {err}"
        );

        // Full fallback chain: an exec sibling on the same binary → exec session.
        let probed = vec![
            ProbedSubprocessCli {
                detected: app_server_surface(exe.clone()),
                auth_status: SubprocessCliAuthStatus::Authenticated,
                auth_detail: None,
            },
            ProbedSubprocessCli {
                detected: DetectedSubprocessCli {
                    surface_id: "provider_surface.openai.subprocess_cli".to_string(),
                    executable_path: exe,
                },
                auth_status: SubprocessCliAuthStatus::Authenticated,
                auth_detail: None,
            },
        ];
        std::env::set_var("MAEKON_CODEX_ALLOWED_DIRS", dir.path());
        let app_server_surface = app_server_surface(probed[0].detected.executable_path.clone());
        let session = mgr
            .create_codex_session_with_fallback(
                &app_server_surface,
                &probed,
                &subprocess_config(),
                &None,
            )
            .await
            .expect("probe-fail must degrade to a codex exec session, not abort");
        std::env::remove_var("MAEKON_CODEX_ALLOWED_DIRS");
        assert_ne!(
            session.session_id(),
            "thr_42",
            "the exec fallback must NOT be the app-server session"
        );
    }

    /// T4 (version drift is WARN-NOT-GATE — load-bearing graceful guard): a
    /// trusted, probeable binary whose userAgent reports a known-bad / denylisted
    /// version STILL yields an app-server session. A code mutation turning the
    /// version cross-check into `return Err` on drift would flip this to the
    /// exec/Err path and FAIL this test — pinning the #4871 invariant.
    #[tokio::test]
    #[serial_test::serial(maekon_codex_allowed_dirs_env)]
    async fn t4_known_bad_useragent_version_still_yields_app_server_session() {
        let dir = tempfile::tempdir().expect("tempdir");
        // userAgent embeds the catalog known-bad substring "0.0.0" plus a mismatch
        // vs the `--version` probe (9.9.9) — both inform-only warn paths fire.
        let exe = write_fake_codex(
            dir.path(),
            Some("codex-cli 9.9.9"),
            "codex-app-server/0.0.0",
        );
        std::env::set_var("MAEKON_CODEX_ALLOWED_DIRS", dir.path());

        let mgr = test_manager().with_codex_app_server_rollout(CodexAppServerRollout::OptIn);
        let result = mgr
            .connect_codex_app_server(&app_server_surface(exe), &subprocess_config())
            .await;
        std::env::remove_var("MAEKON_CODEX_ALLOWED_DIRS");

        let session =
            result.expect("version drift must WARN-AND-PROCEED, never gate (#4871/#4863)");
        assert_eq!(
            session.session_id(),
            "thr_42",
            "a denylisted/mismatched version must STILL produce an app-server session"
        );
    }

    /// T5 (untrusted path is WARN-NOT-GATE in default mode — load-bearing guard):
    /// a probeable binary OUTSIDE the allowlist STILL yields an app-server session
    /// (default mode warns, does not block). A mutation hard-blocking on
    /// `UnknownPath` would fail this test.
    #[tokio::test]
    #[serial_test::serial(maekon_codex_allowed_dirs_env)]
    async fn t5_out_of_allowlist_path_still_yields_app_server_session() {
        let dir = tempfile::tempdir().expect("tempdir");
        let exe = write_fake_codex(dir.path(), Some("codex-cli 1.2.3"), "codex-app-server/1.0");
        // Deliberately DO NOT add the tempdir to the allowlist → UnknownPath.
        std::env::remove_var("MAEKON_CODEX_ALLOWED_DIRS");

        let mgr = test_manager().with_codex_app_server_rollout(CodexAppServerRollout::OptIn);
        let session = mgr
            .connect_codex_app_server(&app_server_surface(exe), &subprocess_config())
            .await
            .expect("out-of-allowlist path must WARN-AND-PROCEED in default mode (#4863 R7)");
        assert_eq!(
            session.session_id(),
            "thr_42",
            "an untrusted path must STILL produce an app-server session in default mode"
        );
    }
}
