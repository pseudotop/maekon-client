use crate::scheduler::shared_regime_state::SharedRegimeState;
use crate::session_context::SessionContextAssembler;
use crate::session_manager::SessionManagerImpl;
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_core::ports::session_context_store::SessionContextStorePort;
use std::sync::Arc;
use tauri::AppHandle;

pub(super) type SessionManagerLaunch = Option<(Arc<SessionManagerImpl>, std::time::Duration)>;

/// The Codex approval registry the real UI hook writes to (E21 #5044). Returned
/// from [`build_session_manager`] so the composition root can install the SAME
/// instance into `CodexApprovalRuntimeState` for the `respond_codex_approval`
/// command (single-instance dead-writer guard).
pub(super) type CodexApprovalRegistry = crate::provider_adapters::CodexApprovalRegistry;

/// Builds the ONE shared `Arc<PolicyClient>` over the durable policy store that
/// the composition root injects into BOTH the Codex approval decider (via
/// [`build_session_manager`]) AND the automation controller (via the web
/// wiring). Port Instance Sharing: this is the single owner of the policy-store
/// handle, so a within-session CRUD add via the controller is immediately
/// visible to the decider's `verdict_for` — previously the two held separate
/// instances that converged only across restart (#7915 made them share the
/// file, #7932 makes them share the live instance).
pub(super) fn build_shared_policy_client(
    data_dir_path: &std::path::Path,
) -> Arc<maekon_automation::policy::PolicyClient> {
    Arc::new(maekon_automation::policy::PolicyClient::with_persistence(
        data_dir_path.join(maekon_automation::policy::POLICY_STORE_FILE_NAME),
    ))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_session_manager(
    app_handle: &AppHandle,
    // #6123: the background runtime handle the off-reactor audit drain runs on.
    // This wiring executes on the synchronous Tauri main thread, where
    // `Handle::try_current()` is `Err`, so the handle must be passed explicitly.
    runtime_handle: &tokio::runtime::Handle,
    sqlite_storage: Arc<maekon_storage::sqlite::SqliteStorage>,
    config: &maekon_core::config::AppConfig,
    data_dir_path: &std::path::Path,
    shared_regime_state: Arc<SharedRegimeState>,
    consent_manager: Arc<dyn ConsentManagerPort>,
    // D7 (#4812 / E20-20): the single shared workspace-wide circuit-breaker
    // registry from the composition root, threaded into the session manager so
    // `HttpApiSession`s share one breaker with the other network adapters.
    breaker_registry: Arc<crate::breaker_registry::CircuitBreakerRegistry>,
    // #7932 Part B: the ONE shared Arc<PolicyClient> from the composition root
    // (Port Instance Sharing). The Codex approval decider below uses THIS instance
    // — the SAME one injected into the automation controller — so a within-session
    // CRUD add via the controller is immediately visible to `verdict_for`.
    policy_client: Arc<maekon_automation::policy::PolicyClient>,
) -> (SessionManagerLaunch, CodexApprovalRegistry) {
    let storage_for_audit = sqlite_storage.clone();
    // #6123: blocking SQLite must not run on the tokio reactor. Wrap the
    // blocking save in ChannelAuditPersistence so it drains on a dedicated
    // spawn_blocking task off-reactor (spawned onto `runtime_handle`).
    let blocking_persist: Arc<dyn maekon_automation::audit::AuditPersistence> =
        Arc::new(move |entry: &maekon_core::models::audit::AuditEntry| {
            storage_for_audit.save_audit_entry(entry);
        });
    let persistence_cb: Arc<dyn maekon_automation::audit::AuditPersistence> =
        Arc::new(maekon_automation::audit::ChannelAuditPersistence::new(
            blocking_persist,
            runtime_handle.clone(),
        ));
    let audit_query: Arc<dyn maekon_automation::audit::AuditQuery> = Arc::new(
        crate::audit_query::SqliteAuditQuery::new(sqlite_storage.clone()),
    );
    let audit_pii_sanitizer: Arc<dyn maekon_core::ports::pii_sanitizer::PiiSanitizer> =
        Arc::new(maekon_vision::privacy::VisionPiiSanitizer);
    let audit_logger = Arc::new(tokio::sync::RwLock::new(
        maekon_automation::audit::AuditLogger::new(500, 50)
            .with_persistence(persistence_cb)
            .with_query(audit_query)
            .with_pii_sanitizer(audit_pii_sanitizer),
    ));
    // #6168: durable persister for AI conversation session audit entries. The
    // production `AuditLogPort::record_session_event` was a no-op default, so the
    // `session_audit_log` table received ZERO production writes. Wire a
    // SqliteStorage-backed persister and run the blocking SQLite INSERT
    // off-reactor (`spawn_blocking` on `runtime_handle`), mirroring the #6123
    // command-audit constraint that blocking SQLite must not touch the reactor.
    let storage_for_session_audit = sqlite_storage.clone();
    let session_audit_handle = runtime_handle.clone();
    let session_persistence: Arc<dyn maekon_automation::audit::SessionAuditPersistence> = Arc::new(
        move |entry: &maekon_core::models::ai_session::SessionAuditEntry| {
            let storage = storage_for_session_audit.clone();
            let entry = entry.clone();
            session_audit_handle.spawn_blocking(move || {
                storage.save_session_audit_entry(&entry);
            });
        },
    );
    let audit_port: Arc<dyn maekon_core::ports::audit_log::AuditLogPort> = Arc::new(
        maekon_automation::audit::AuditLogAdapter::new(audit_logger.clone())
            .with_session_persistence(session_persistence),
    );

    let session_config = Arc::new(config.ai_session.clone());
    let idle_reaper_interval =
        std::time::Duration::from_secs(session_config.health_check_interval_secs);
    let session_context_store: Arc<dyn SessionContextStorePort> = sqlite_storage.clone();

    let context_assembler = Arc::new(SessionContextAssembler::new(
        session_context_store,
        Arc::new(config.clone()),
        shared_regime_state,
    ));

    let secret_store = {
        let config_dir = maekon_core::config_manager::ConfigManager::config_dir()
            .unwrap_or_else(|_| data_dir_path.to_path_buf());
        let os_store = crate::provider_secret_backend::create_os_secret_store(&config_dir);
        match crate::provider_secret_backend::resolve_provider_secret_backend(&config_dir, os_store)
        {
            Ok(r) => r.secret_store,
            Err(e) => {
                tracing::debug!("provider secret backend unavailable: {e}");
                None
            }
        }
    };

    // E21 #4882/#4883: privacy guard for external chat sessions. Reuses the
    // session audit logger for the egress audit trail and a dedicated process
    // monitor for the active-window/sensitive-app gate.
    let privacy_guard: Arc<dyn crate::provider_adapters::ConversationContentGuard> = {
        let process_monitor: Arc<dyn maekon_core::ports::monitor::ProcessMonitor> =
            Arc::new(maekon_monitor::process::ProcessTracker::new());
        Arc::new(crate::provider_adapters::ExternalOcrPrivacyGuard::new(
            consent_manager.clone(),
            config.privacy.pii_filter_level,
            config.ai_provider.external_data_policy,
            config.privacy.clone(),
            process_monitor,
            Some(audit_logger),
        ))
    };

    // E21 #4870 + #5044 (FU-A): FAIL-CLOSED approval decider for Codex
    // app-server `requestApproval` REQUESTs. Reuses the SAME audit sink as the
    // rest of the session manager (no private dead-writer logger) and a fresh
    // PolicyClient for command-execution policy lookups. The UI escalation is
    // now the REAL `CodexUiApprovalHook`: it parks the decider's oneshot in
    // `approval_registry` and emits `codex:approval-request` to the overlay
    // modal, which answers via the `respond_codex_approval` command (reading the
    // SAME registry). The decider remains the single timeout owner.
    let approval_registry: CodexApprovalRegistry =
        Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let codex_approval_decider = {
        // #7915 + #7932 Part B: use the ONE shared Arc<PolicyClient> from the
        // composition root (Port Instance Sharing) — the SAME instance the
        // automation controller holds. A previously-granted process is matched
        // instead of re-escalating as NoMatch (across restart, #7915), AND a
        // within-session CRUD add via the controller is now immediately visible
        // here (#7932) instead of converging only on the next restart.
        let policy_port: Arc<dyn maekon_core::ports::codex_approval::ApprovalPolicyPort> =
            Arc::new(crate::codex_approval_policy::PolicyClientApprovalAdapter::new(policy_client));
        let ui_hook: Arc<dyn maekon_core::ports::codex_approval::UiApprovalHook> =
            Arc::new(crate::provider_adapters::CodexUiApprovalHook::new(
                app_handle.clone(),
                approval_registry.clone(),
            ));
        Arc::new(maekon_core::codex_approval::CodexApprovalDecider::new(
            policy_port,
            audit_port.clone(),
            ui_hook,
        ))
    };

    let mut manager = SessionManagerImpl::new(session_config, audit_port, Some(context_assembler));
    if let Some(store) = secret_store {
        manager = manager.with_secret_store(store);
    }
    manager = manager.with_app_handle(app_handle.clone());
    manager = manager.with_privacy_guard(privacy_guard);
    // E21 #4871: gate the Codex app-server transport behind the rollout flag
    // (default Off → codex exec; app-server failures fall back to exec).
    manager = manager.with_codex_app_server_rollout(config.ai_provider.codex_app_server_rollout);
    manager = manager.with_codex_approval_decider(codex_approval_decider);
    // D7 (#4812 / E20-20): share the single workspace-wide breaker registry.
    manager = manager.with_breaker_registry(breaker_registry);
    // C2 (#5722): pre-resolve the Ollama base URL + default model from config.
    // Wired here (full AppConfig in scope) so create_local_llm_session can honour
    // a wizard-written custom Ollama endpoint without touching AppConfig directly.
    // The Arc<SessionManagerImpl> is shared with the web runtime (web_server_wiring.rs),
    // so one wiring point covers both Tauri IPC chat and the web dashboard chat.
    manager = manager.with_local_llm_target(
        crate::session_manager::factory::resolve_local_llm_target(&config.ai_provider),
    );
    (
        Some((Arc::new(manager), idle_reaper_interval)),
        approval_registry,
    )
}

pub(super) fn spawn_idle_reaper(
    handle: &tokio::runtime::Handle,
    session_manager: &SessionManagerLaunch,
    sqlite_storage: Arc<maekon_storage::sqlite::SqliteStorage>,
    retention_days: u32,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    if let Some((sm, idle_reaper_interval)) = session_manager {
        let sm_clone = sm.clone();
        let ss_clone: Arc<dyn maekon_core::ports::session_storage::SessionStoragePort> =
            sqlite_storage;
        let idle_reaper_interval = *idle_reaper_interval;
        handle.spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(idle_reaper_interval) => {
                        sm_clone.reap_idle_sessions().await;
                        if let Ok(count) = ss_clone.purge_expired(retention_days).await {
                            if count > 0 {
                                tracing::info!("purged {count} expired session records");
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => break,
                }
            }
        });
        tracing::info!("idle reaper background task started");
    }
}
