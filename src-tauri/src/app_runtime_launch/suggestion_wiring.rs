#[cfg(feature = "local-suggestions")]
use std::sync::Arc;

#[cfg(feature = "local-suggestions")]
pub(super) struct SuggestionWiring {
    pub(super) shared_queue: Arc<tokio::sync::Mutex<maekon_suggestion::queue::SuggestionQueue>>,
    pub(super) shared_scorer: Arc<tokio::sync::Mutex<maekon_suggestion::scorer::FeedbackScorer>>,
    pub(super) manager: Option<Arc<crate::suggestion_manager::SuggestionManager>>,
}

#[cfg(feature = "local-suggestions")]
#[allow(clippy::too_many_arguments)]
pub(super) fn build_suggestion_wiring(
    app_handle: &tauri::AppHandle,
    handle: &tokio::runtime::Handle,
    config: &maekon_core::config::AppConfig,
    sqlite_storage: Arc<maekon_storage::sqlite::SqliteStorage>,
    regime_classifier: Arc<parking_lot::Mutex<maekon_analysis::RegimeClassifier>>,
    // #9459: the ONE session built by the composition root (auth_wiring.rs).
    // Threaded through rather than reconstructed so the network `ApiClient`
    // built below shares the manager the login IPC writes to.
    #[cfg(feature = "server")] shared_token_manager: Option<
        Arc<maekon_network::auth::TokenManager>,
    >,
) -> SuggestionWiring {
    // Composite feedback sink over the shared regime classifier (this wiring is
    // its sole consumer). #7913 T2.1c: persist the per-regime reaction stats the
    // classifier learns from accept/reject/defer so they survive restart.
    let feedback_sink: Arc<dyn maekon_core::ports::feedback_signal_sink::FeedbackSignalSink> =
        Arc::new(
            crate::feedback_sink::CompositeFeedbackSink::new(Some(regime_classifier))
                .with_reaction_store(sqlite_storage.clone()
                    as Arc<dyn maekon_core::ports::regime_reaction_store::RegimeReactionStore>),
        );

    let shared_queue = Arc::new(tokio::sync::Mutex::new(
        maekon_suggestion::queue::SuggestionQueue::new(config.analysis.max_suggestions),
    ));
    restore_pending_suggestions(handle, sqlite_storage.clone(), shared_queue.clone());

    let shared_scorer = Arc::new(tokio::sync::Mutex::new(
        maekon_suggestion::scorer::FeedbackScorer::new(),
    ));
    // #7913 T2.1c: hydrate the FeedbackScorer's learned (type, source) tallies so
    // relevance learning survives restart (the write-through lives in the
    // submit_suggestion_feedback command). Wall-clock decay is preserved on load.
    restore_feedback_scorer_tallies(handle, sqlite_storage.clone(), shared_scorer.clone());
    let manager = build_suggestion_manager(
        app_handle,
        config,
        sqlite_storage.clone(),
        shared_queue.clone(),
        shared_scorer.clone(),
        feedback_sink,
        #[cfg(feature = "server")]
        shared_token_manager,
    );
    restore_deferred_suggestions_and_feedbacks(
        handle,
        sqlite_storage,
        shared_queue.clone(),
        manager.as_ref(),
    );

    SuggestionWiring {
        shared_queue,
        shared_scorer,
        manager,
    }
}

/// #7913 T2.1c: load persisted `FeedbackScorer` tallies into the shared scorer
/// on startup. Fail-safe: a load failure logs and starts empty (today's
/// behavior); a tally aged past its 12h window reads as decayed via the scorer's
/// preserved `last_updated` (see `FeedbackScorer::hydrate`).
#[cfg(feature = "local-suggestions")]
fn restore_feedback_scorer_tallies(
    handle: &tokio::runtime::Handle,
    sqlite_storage: Arc<maekon_storage::sqlite::SqliteStorage>,
    shared_scorer: Arc<tokio::sync::Mutex<maekon_suggestion::scorer::FeedbackScorer>>,
) {
    use maekon_core::ports::feedback_scorer_store::FeedbackScorerStore;
    let records = match sqlite_storage.load_feedback_tallies() {
        Ok(records) => records,
        Err(e) => {
            tracing::warn!("failed to restore feedback scorer tallies: {e}");
            Vec::new()
        }
    };
    if records.is_empty() {
        return;
    }
    let count = records.len();
    handle.block_on(shared_scorer.lock()).hydrate(records);
    tracing::info!(count, "restored feedback scorer tallies from storage");
}

#[cfg(feature = "local-suggestions")]
fn restore_pending_suggestions(
    handle: &tokio::runtime::Handle,
    sqlite_storage: Arc<maekon_storage::sqlite::SqliteStorage>,
    shared_queue: Arc<tokio::sync::Mutex<maekon_suggestion::queue::SuggestionQueue>>,
) {
    let pending = match sqlite_storage.list_suggestions_by_state("pending", 50) {
        Ok(records) => records,
        Err(e) => {
            tracing::warn!(state = "pending", "failed to restore suggestions: {e}");
            Vec::new()
        }
    };
    if pending.is_empty() {
        return;
    }

    let mut queue = handle.block_on(shared_queue.lock());
    let mut restored = 0usize;
    let now = chrono::Utc::now();
    for record in pending {
        if let Some(suggestion) = record.try_into_suggestion() {
            // #5696: list_suggestions_by_state filters on state only — skip
            // rows whose expires_at has passed so a restart does not resurrect
            // stale time-sensitive suggestions (e.g. an hours-old break nudge).
            if suggestion.expires_at.is_some_and(|e| e <= now) {
                continue;
            }
            if queue.push(suggestion) {
                restored += 1;
            }
        }
    }
    if restored > 0 {
        tracing::info!(count = restored, "restored suggestions from storage");
    }
}

/// E20-24 (#4816) LOCAL constructor — used in OSS builds with `local-suggestions`
/// but WITHOUT `server`. Builds an identical `SuggestionManager` but injects the
/// no-op [`crate::local_api_client::LocalApiClient`] instead of a network client,
/// so accept/reject learns on-device (via `feedback_sink`) with zero egress. Takes
/// the same signature as the server constructor so the single call site in
/// `build_suggestion_wiring` is cfg-agnostic. No `maekon-network` reference — this
/// compiles under `--no-default-features --features local-suggestions`.
#[cfg(all(feature = "local-suggestions", not(feature = "server")))]
fn build_suggestion_manager(
    _app_handle: &tauri::AppHandle,
    _config: &maekon_core::config::AppConfig,
    sqlite_storage: Arc<maekon_storage::sqlite::SqliteStorage>,
    shared_queue: Arc<tokio::sync::Mutex<maekon_suggestion::queue::SuggestionQueue>>,
    shared_scorer: Arc<tokio::sync::Mutex<maekon_suggestion::scorer::FeedbackScorer>>,
    feedback_sink: Arc<dyn maekon_core::ports::feedback_signal_sink::FeedbackSignalSink>,
) -> Option<Arc<crate::suggestion_manager::SuggestionManager>> {
    let api: Arc<dyn maekon_core::ports::api_client::ApiClient> =
        Arc::new(crate::local_api_client::LocalApiClient);
    let history = Arc::new(tokio::sync::Mutex::new(
        maekon_suggestion::history::SuggestionHistory::new(100),
    ));
    let feedback = Arc::new(
        maekon_suggestion::feedback::FeedbackSender::new_with_sink(api, Some(feedback_sink))
            // #6442 (F9): record feedback egress in the audit ledger (SqliteStorage impls
            // EgressLedgerSink); captures the initial send + retry re-sends.
            .with_egress_ledger(sqlite_storage.clone()),
    );
    let deferred = Arc::new(tokio::sync::Mutex::new(
        maekon_suggestion::deferred::DeferredManager::new(50),
    ));
    let retry_queue = Arc::new(tokio::sync::Mutex::new(
        maekon_suggestion::feedback_retry::FeedbackRetryQueue::new(100, 5),
    ));
    Some(Arc::new(crate::suggestion_manager::SuggestionManager::new(
        shared_queue,
        history,
        feedback,
        shared_scorer,
        deferred,
        retry_queue,
        sqlite_storage,
    )))
}

/// SERVER constructor — builds the network-backed `SuggestionManager`
/// (TokenManager + HttpApiClient/gRPC + feedback POST). Compiled only with the
/// `server` feature.
#[cfg(feature = "server")]
fn build_suggestion_manager(
    app_handle: &tauri::AppHandle,
    config: &maekon_core::config::AppConfig,
    sqlite_storage: Arc<maekon_storage::sqlite::SqliteStorage>,
    shared_queue: Arc<tokio::sync::Mutex<maekon_suggestion::queue::SuggestionQueue>>,
    shared_scorer: Arc<tokio::sync::Mutex<maekon_suggestion::scorer::FeedbackScorer>>,
    feedback_sink: Arc<dyn maekon_core::ports::feedback_signal_sink::FeedbackSignalSink>,
    shared_token_manager: Option<Arc<maekon_network::auth::TokenManager>>,
) -> Option<Arc<crate::suggestion_manager::SuggestionManager>> {
    use maekon_network::http_client::HttpApiClient;
    use tauri::Manager;

    // #7733: fail loud on an invalid `[tls]` config, unifying with the sibling
    // `build_server_transports` wiring site (agent_runtime_support.rs), which has
    // propagated the identical TokenManager construction error via `?` since this
    // client's initial migration commit — it has never had a fallback. This site
    // used to warn-log then silently fall back to the deprecated no-TLS-policy
    // `TokenManager::new` constructor (deep-review Net-4, #4530/#4531): that made
    // the failure *observable* but the client still ran with a weakened TLS
    // policy. On a privacy product, silently downgrading TLS enforcement is a
    // security regression, not an acceptable degrade — treat a `[tls]`
    // *configuration* error (e.g. `allow_self_signed`, rejected by
    // `build_reqwest_client_for_url`) as a wiring failure and skip the
    // SuggestionManager entirely (same convention as the `api_result` failure
    // handling below — this subsystem is non-critical, so the rest of the app
    // keeps running; `logout_all_sessions` also becomes unavailable, which
    // `TokenManagerState`'s own doc already treats as an expected `None` state).
    // #9459: prefer the composition root's shared manager; only the locally-built
    // fallback still owns the slot write below (the shared one was registered by
    // `app_runtime_launch::mod`, which is where that responsibility now lives).
    let shared_manager_provided = shared_token_manager.is_some();
    let token_manager = resolve_suggestion_token_manager(config, shared_token_manager)?;

    // Populate the build-time slot registered once in `main.rs`. We MUST write
    // through the slot (via the already-imported `tauri::Manager::state`) rather
    // than calling `app_handle.manage(..)` again — Tauri's `manage()` does not
    // overwrite an already-managed type (it returns `false` and discards the
    // value), which previously left `logout_all_sessions` permanently reading
    // `None` (2nd-pass #22).
    if !shared_manager_provided {
        app_handle
            .state::<crate::commands::auth::TokenManagerState>()
            .set(token_manager.clone());
    }

    #[cfg(feature = "grpc")]
    let api_result: anyhow::Result<Arc<dyn maekon_core::ports::api_client::ApiClient>> = {
        use maekon_network::grpc::{GrpcApiAdapter, GrpcConfig, UnifiedClient};
        let grpc_config =
            GrpcConfig::from_core_with_rest_tls(&config.grpc, &config.server.base_url, &config.tls);
        match (
            UnifiedClient::new(grpc_config, token_manager.clone()),
            HttpApiClient::new_with_tls(
                &config.server.base_url,
                token_manager.clone(),
                config.request_timeout(),
                &config.tls,
            ),
        ) {
            (Ok(unified), Ok(http_fallback)) => Ok(Arc::new(GrpcApiAdapter::new(
                Arc::new(unified),
                http_fallback,
            ))),
            (Err(e), _) => Err(anyhow::anyhow!("UnifiedClient init failed: {e}")),
            (_, Err(e)) => Err(anyhow::anyhow!("HttpApiClient init failed: {e}")),
        }
    };

    #[cfg(not(feature = "grpc"))]
    let api_result: anyhow::Result<Arc<dyn maekon_core::ports::api_client::ApiClient>> = {
        HttpApiClient::new_with_tls(
            &config.server.base_url,
            token_manager,
            config.request_timeout(),
            &config.tls,
        )
        .map(|c| Arc::new(c) as Arc<dyn maekon_core::ports::api_client::ApiClient>)
        .map_err(|e| anyhow::anyhow!("{e}"))
    };

    match api_result {
        Ok(api) => {
            let history = Arc::new(tokio::sync::Mutex::new(
                maekon_suggestion::history::SuggestionHistory::new(100),
            ));
            let feedback = Arc::new(
                maekon_suggestion::feedback::FeedbackSender::new_with_sink(
                    api,
                    Some(feedback_sink),
                )
                // #6442 (F9): record feedback egress in the audit ledger (SqliteStorage
                // impls EgressLedgerSink); captures the initial send + retry re-sends.
                .with_egress_ledger(sqlite_storage.clone()),
            );
            let deferred = Arc::new(tokio::sync::Mutex::new(
                maekon_suggestion::deferred::DeferredManager::new(50),
            ));
            let retry_queue = Arc::new(tokio::sync::Mutex::new(
                maekon_suggestion::feedback_retry::FeedbackRetryQueue::new(100, 5),
            ));
            Some(Arc::new(crate::suggestion_manager::SuggestionManager::new(
                shared_queue,
                history,
                feedback,
                shared_scorer,
                deferred,
                retry_queue,
                sqlite_storage,
            )))
        }
        Err(e) => {
            tracing::warn!("SuggestionManager init skipped: {e}");
            None
        }
    }
}

/// #9459: adopt the composition root's single shared `TokenManager` when one was
/// handed down, else fall back to building this pipeline's own.
///
/// Sharing is what makes the login IPC effective: `TokenManagerState` holds the
/// composition root's manager, so a manager built locally here would be a second,
/// never-signed-in session backing every suggestion-pipeline request. The
/// fallback exists only for the case where the composition root's own
/// construction failed (a `[tls]` config error) — preserving pre-#9459 behavior
/// rather than additionally disabling the suggestion pipeline.
#[cfg(feature = "server")]
fn resolve_suggestion_token_manager(
    config: &maekon_core::config::AppConfig,
    shared: Option<Arc<maekon_network::auth::TokenManager>>,
) -> Option<Arc<maekon_network::auth::TokenManager>> {
    match shared {
        Some(manager) => Some(manager),
        None => build_suggestion_token_manager(config),
    }
}

/// Builds the TLS-aware `TokenManager` used by the suggestion pipeline's
/// network client. Returns `None` (logged at `error!`) on a `[tls]`
/// *configuration* error instead of falling back to the deprecated
/// no-TLS-policy `TokenManager::new` constructor — see the fail-loud
/// rationale documented on `build_suggestion_manager` above (#7733).
///
/// Split out as a pure `&AppConfig -> Option<Arc<TokenManager>>` function
/// (no `tauri::AppHandle` dependency) specifically so the fail-loud branch is
/// unit-testable without standing up a Tauri app.
///
/// #9459: now the FALLBACK path only — the shared manager built by
/// `auth_wiring` is preferred (see `resolve_suggestion_token_manager`).
#[cfg(feature = "server")]
fn build_suggestion_token_manager(
    config: &maekon_core::config::AppConfig,
) -> Option<Arc<maekon_network::auth::TokenManager>> {
    match maekon_network::auth::TokenManager::new_with_tls(
        &config.server.base_url,
        &config.tls,
        Some(config.request_timeout()),
    ) {
        Ok(manager) => Some(Arc::new(manager)),
        Err(error) => {
            tracing::error!(
                error = %error,
                base_url = %config.server.base_url,
                "TLS-aware TokenManager construction failed; SuggestionManager init \
                 skipped (fail-loud — verify the [tls] config; refusing to fall back \
                 to a client without TLS policy enforcement)"
            );
            None
        }
    }
}

#[cfg(feature = "local-suggestions")]
fn restore_deferred_suggestions_and_feedbacks(
    handle: &tokio::runtime::Handle,
    sqlite_storage: Arc<maekon_storage::sqlite::SqliteStorage>,
    shared_queue: Arc<tokio::sync::Mutex<maekon_suggestion::queue::SuggestionQueue>>,
    manager: Option<&Arc<crate::suggestion_manager::SuggestionManager>>,
) {
    let Some(manager) = manager else {
        return;
    };

    // #6938: order by SOONEST resurface_at (not created_at DESC) so a deferred
    // backlog over the 50-row limit keeps the snoozes about to resurface, not the
    // newest-created ones (created_at and resurface_at are decoupled).
    let deferred_records = match sqlite_storage.list_deferred_suggestions_by_resurface(50) {
        Ok(records) => records,
        Err(e) => {
            tracing::warn!(state = "deferred", "failed to restore suggestions: {e}");
            Vec::new()
        }
    };
    if !deferred_records.is_empty() {
        let total = deferred_records.len();
        let entries: Vec<_> = deferred_records
            .into_iter()
            .filter_map(|record| {
                let resurface_at = record
                    .resurface_at
                    .as_ref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&chrono::Utc))?;
                let created_at = chrono::DateTime::parse_from_rfc3339(&record.created_at)
                    .ok()
                    .map(|dt| dt.with_timezone(&chrono::Utc))?;
                let suggestion = record.try_into_suggestion()?;
                Some((suggestion, created_at, resurface_at))
            })
            .collect();
        if entries.len() < total {
            tracing::warn!(
                dropped = total - entries.len(),
                "skipped malformed deferred records"
            );
        }

        let mut deferred_mgr = handle.block_on(manager.deferred().lock());
        let already_due = deferred_mgr.restore(entries);
        let deferred_count = deferred_mgr.pending_count();
        drop(deferred_mgr);

        if !already_due.is_empty() {
            let mut queue = handle.block_on(shared_queue.lock());
            for suggestion in already_due {
                queue.push(suggestion);
            }
        }
        if deferred_count > 0 {
            tracing::info!(count = deferred_count, "restored deferred suggestions");
        }
    }

    let pending_feedbacks = match sqlite_storage.list_pending_feedbacks(100) {
        Ok(records) => records,
        Err(e) => {
            tracing::warn!("failed to restore pending suggestion feedback: {e}");
            Vec::new()
        }
    };
    if pending_feedbacks.is_empty() {
        return;
    }

    let cutoff = (chrono::Utc::now() - chrono::Duration::days(7)).to_rfc3339();
    let mut retry_queue = handle.block_on(manager.retry_queue().lock());
    let mut feedback_count = 0usize;
    for record in pending_feedbacks {
        if record.created_at < cutoff {
            let _ = sqlite_storage.delete_pending_feedback(&record.suggestion_id);
            continue;
        }
        if let Some((sid, ft, comment, attempts, next_retry)) = record.into_domain_parts() {
            // Restore path: records come FROM SQLite, so an evicted entry's durable
            // row is the source of truth (retried on a later restart), not an
            // orphan — intentionally do not delete it here (review4).
            let _evicted =
                retry_queue.enqueue(maekon_suggestion::feedback_retry::PendingFeedback {
                    suggestion_id: sid,
                    feedback_type: ft,
                    comment,
                    attempts,
                    next_retry_at: next_retry,
                });
            feedback_count += 1;
        }
    }
    if feedback_count > 0 {
        tracing::info!(
            count = feedback_count,
            "restored pending feedbacks for retry"
        );
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use std::sync::Arc;

    use maekon_core::config::AppConfig;

    use super::{build_suggestion_token_manager, resolve_suggestion_token_manager};

    /// #7733 fails-before: an invalid `[tls]` config (`allow_self_signed = true`,
    /// rejected by `build_reqwest_client_for_url`) must produce a hard failure
    /// (`None`), NOT a silently-degraded `TokenManager` without TLS policy
    /// enforcement. Before this fix, the caller (`build_suggestion_manager`)
    /// caught this same error and fell back to the deprecated
    /// `TokenManager::new` constructor — this test targets the extracted pure
    /// helper directly so the fail-loud branch is provable without a
    /// `tauri::AppHandle`.
    #[test]
    fn invalid_tls_config_fails_loud_instead_of_falling_back() {
        let mut config = AppConfig::default_config();
        config.tls.allow_self_signed = true;

        let result = build_suggestion_token_manager(&config);

        assert!(
            result.is_none(),
            "an invalid [tls] config must fail loud (None), not silently build a \
             TokenManager without TLS policy enforcement"
        );
    }

    /// Sanity counterpart: a valid `[tls]` config must still build normally.
    #[test]
    fn valid_tls_config_builds_token_manager() {
        let config = AppConfig::default_config();

        let result = build_suggestion_token_manager(&config);

        assert!(
            result.is_some(),
            "a valid [tls] config must build a TokenManager"
        );
    }

    /// #9459 fails-before: when the composition root hands down the ONE shared
    /// `TokenManager`, this wiring must adopt that exact `Arc` instead of
    /// constructing a second, independent session. Two managers means the login
    /// IPC writes a bearer token into one of them while the upload/SSE
    /// transports keep reading the other — a logged-in client that uploads
    /// unauthenticated.
    #[test]
    // `TokenManager::new` is deprecated for production wiring (no TLS policy);
    // used here only as a cheap identity fixture — no request is ever issued.
    #[allow(deprecated)]
    fn shared_token_manager_is_reused_verbatim() {
        let shared = Arc::new(maekon_network::auth::TokenManager::new(
            "http://127.0.0.1:19999",
        ));
        let config = AppConfig::default_config();

        let resolved = resolve_suggestion_token_manager(&config, Some(shared.clone()))
            .expect("a shared manager must resolve");

        assert!(
            Arc::ptr_eq(&resolved, &shared),
            "the shared TokenManager must be reused verbatim; a different Arc means \
             the suggestion pipeline built its own second session"
        );
    }

    /// The `None` path keeps the pre-#9459 behavior: no shared manager (e.g. the
    /// composition root's construction failed on a `[tls]` config error) still
    /// builds this wiring's own manager rather than disabling the pipeline.
    #[test]
    fn absent_shared_token_manager_falls_back_to_own() {
        let config = AppConfig::default_config();

        let resolved = resolve_suggestion_token_manager(&config, None);

        assert!(
            resolved.is_some(),
            "without a shared manager the wiring must fall back to building its own"
        );
    }
}
