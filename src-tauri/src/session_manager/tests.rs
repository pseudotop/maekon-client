use super::*;
use oneshim_core::models::ai_session::SessionTransport;

fn test_config() -> Arc<AiSessionConfig> {
    Arc::new(AiSessionConfig {
        max_concurrent_sessions: 2,
        idle_timeout_secs: 1,
        ..Default::default()
    })
}

fn test_manager() -> SessionManagerImpl {
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
    !crate::subprocess_provider::probe_known_cli_surfaces().is_empty()
}

#[tokio::test]
async fn list_sessions_empty() {
    let mgr = test_manager();
    assert!(mgr.list_sessions().await.is_empty());
}

#[tokio::test]
async fn kill_nonexistent_session_returns_error() {
    let mgr = test_manager();
    let result = mgr.kill_session("nonexistent").await;
    assert!(result.is_err());
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
    };
    let result = mgr.create_session(config).await;

    if has_any_subprocess_cli() {
        let session = match result {
            Ok(session) => session,
            Err(e) => panic!("should create session when a supported CLI is present: {e}"),
        };
        assert!(!session.session_id().is_empty());
        assert!(!session.provider_name().is_empty());

        // Verify it was stored and is retrievable
        let retrieved = mgr.get_session(session.session_id()).await;
        assert!(retrieved.is_ok());

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
    };
    let err_msg = expect_err_msg(mgr.create_session(config).await);
    assert!(
        err_msg.contains("surface_id is required"),
        "expected surface_id error, got: {err_msg}",
    );
}

#[tokio::test]
async fn create_local_llm_session_succeeds() {
    let mgr = test_manager();
    let config = SessionConfig {
        transport: SessionTransport::LocalLlm,
        surface_id: None,
        model: Some("llama3".to_string()),
        system_prompt: Some("Be concise.".to_string()),
        tools_enabled: false,
    };
    let session = mgr
        .create_session(config)
        .await
        .expect("should create LocalLlm session");
    assert_eq!(session.provider_name(), "ollama");
    assert!(!session.session_id().is_empty());

    // Verify stored and retrievable.
    let retrieved = mgr.get_session(session.session_id()).await;
    assert!(retrieved.is_ok());

    let list = mgr.list_sessions().await;
    assert_eq!(list.len(), 1);
}

#[tokio::test]
async fn create_local_llm_session_uses_default_model() {
    let mgr = test_manager();
    let config = SessionConfig {
        transport: SessionTransport::LocalLlm,
        surface_id: None,
        model: None,
        system_prompt: None,
        tools_enabled: false,
    };
    let session = mgr
        .create_session(config)
        .await
        .expect("should create LocalLlm session");
    let info = session.info();
    assert_eq!(info.model, "llama3");
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
    };
    let session = mgr.create_session(config).await.expect("create session");
    let id = session.session_id().to_string();

    assert!(mgr.get_session(&id).await.is_ok());
    mgr.kill_session(&id).await.unwrap();
    assert!(mgr.get_session(&id).await.is_err());
    assert!(mgr.list_sessions().await.is_empty());
}

#[tokio::test]
async fn touch_session_resets_state_to_active() {
    let mgr = test_manager();

    // Create a LocalLlm session (no CLI dependency).
    let config = SessionConfig {
        transport: SessionTransport::LocalLlm,
        surface_id: None,
        model: Some("llama3".to_string()),
        system_prompt: None,
        tools_enabled: false,
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
    let mgr = test_manager();

    let config = SessionConfig {
        transport: SessionTransport::LocalLlm,
        surface_id: None,
        model: Some("llama3".to_string()),
        system_prompt: None,
        tools_enabled: false,
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
        mgr.get_session(&id).await.is_err(),
        "session should be removed after second reap"
    );
    assert!(mgr.list_sessions().await.is_empty());
}

#[tokio::test]
async fn create_session_uses_context_assembler() {
    use crate::scheduler::shared_regime_state::SharedRegimeState;
    use oneshim_core::config::AppConfig;
    use oneshim_storage::sqlite::SqliteStorage;

    let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
    let app_config = Arc::new(AppConfig::default_config());
    let regime_state = Arc::new(SharedRegimeState::new());
    let assembler = Arc::new(SessionContextAssembler::new(
        storage,
        app_config,
        regime_state,
    ));

    let mgr = SessionManagerImpl::new(
        test_config(),
        Arc::new(crate::auditing_session::tests::MockAudit::default()),
        Some(assembler),
    );

    // Create a LocalLlm session with system_prompt = None.
    // The context assembler should inject a system prompt automatically.
    let config = SessionConfig {
        transport: SessionTransport::LocalLlm,
        surface_id: None,
        model: Some("llama3".to_string()),
        system_prompt: None,
        tools_enabled: false,
    };

    let session = mgr
        .create_session(config)
        .await
        .expect("should create session with assembled context");

    // The session should have been created successfully.
    assert!(!session.session_id().is_empty());

    // Verify the session is stored and retrievable.
    let retrieved = mgr.get_session(session.session_id()).await;
    assert!(retrieved.is_ok());
}

#[tokio::test]
async fn create_session_preserves_explicit_system_prompt() {
    use crate::scheduler::shared_regime_state::SharedRegimeState;
    use oneshim_core::config::AppConfig;
    use oneshim_storage::sqlite::SqliteStorage;

    let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
    let app_config = Arc::new(AppConfig::default_config());
    let regime_state = Arc::new(SharedRegimeState::new());
    let assembler = Arc::new(SessionContextAssembler::new(
        storage,
        app_config,
        regime_state,
    ));

    let mgr = SessionManagerImpl::new(
        test_config(),
        Arc::new(crate::auditing_session::tests::MockAudit::default()),
        Some(assembler),
    );

    // Create a LocalLlm session with an explicit system prompt.
    // The context assembler should NOT override it.
    let config = SessionConfig {
        transport: SessionTransport::LocalLlm,
        surface_id: None,
        model: Some("llama3".to_string()),
        system_prompt: Some("Custom prompt".to_string()),
        tools_enabled: false,
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
    };
    let session = mgr.create_session(config).await.expect("create session");
    let id = session.session_id().to_string();

    // First recovery should succeed with retry_count = 1.
    let recovered = mgr.recover_session(&id).await;
    assert!(recovered.is_ok());

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
    );

    let session_config = SessionConfig {
        transport: SessionTransport::Subprocess,
        surface_id: None,
        model: None,
        system_prompt: None,
        tools_enabled: false,
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

// ── report_failure tests ───────────────────────────────────

#[tokio::test]
async fn report_failure_transient_auto_recovers() {
    let mgr = test_manager();
    let config = SessionConfig {
        transport: SessionTransport::LocalLlm,
        surface_id: None,
        model: Some("llama3".to_string()),
        system_prompt: None,
        tools_enabled: false,
    };
    let session = mgr.create_session(config).await.expect("create session");
    let id = session.session_id().to_string();

    let err = CoreError::Network {
        code: oneshim_core::error_codes::NetworkCode::Generic,
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
async fn report_failure_permanent_sets_failed() {
    let mgr = test_manager();
    let config = SessionConfig {
        transport: SessionTransport::LocalLlm,
        surface_id: None,
        model: Some("llama3".to_string()),
        system_prompt: None,
        tools_enabled: false,
    };
    let session = mgr.create_session(config).await.expect("create session");
    let id = session.session_id().to_string();

    let err = CoreError::Auth {
        code: oneshim_core::error_codes::AuthCode::Failed,
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
    let mgr = SessionManagerImpl::new(
        config,
        Arc::new(crate::auditing_session::tests::MockAudit::default()),
        None,
    );

    let session_config = SessionConfig {
        transport: SessionTransport::LocalLlm,
        surface_id: None,
        model: Some("llama3".to_string()),
        system_prompt: None,
        tools_enabled: false,
    };
    let session = mgr.create_session(session_config).await.expect("create");
    let id = session.session_id().to_string();

    let err = CoreError::Network {
        code: oneshim_core::error_codes::NetworkCode::Generic,
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
        code: oneshim_core::error_codes::NetworkCode::Generic,
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
    let mgr = SessionManagerImpl::new(
        config,
        Arc::new(crate::auditing_session::tests::MockAudit::default()),
        None,
    );

    let session_config = SessionConfig {
        transport: SessionTransport::LocalLlm,
        surface_id: None,
        model: Some("llama3".to_string()),
        system_prompt: None,
        tools_enabled: false,
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
        mgr.get_session(&id).await.is_err(),
        "session should be removed after absolute timeout"
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
    let mgr = SessionManagerImpl::new(
        config,
        Arc::new(crate::auditing_session::tests::MockAudit::default()),
        None,
    );

    let session_config = SessionConfig {
        transport: SessionTransport::LocalLlm,
        surface_id: None,
        model: Some("llama3".to_string()),
        system_prompt: None,
        tools_enabled: false,
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
        mgr.get_session(&id).await.is_err(),
        "session should be reaped despite recent activity (absolute timeout)"
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
