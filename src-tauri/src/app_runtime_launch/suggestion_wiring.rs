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
    feedback_sink: Arc<dyn maekon_core::ports::feedback_signal_sink::FeedbackSignalSink>,
) -> SuggestionWiring {
    let shared_queue = Arc::new(tokio::sync::Mutex::new(
        maekon_suggestion::queue::SuggestionQueue::new(config.analysis.max_suggestions),
    ));
    restore_pending_suggestions(handle, sqlite_storage.clone(), shared_queue.clone());

    let shared_scorer = Arc::new(tokio::sync::Mutex::new(
        maekon_suggestion::scorer::FeedbackScorer::new(),
    ));
    let manager = build_suggestion_manager(
        app_handle,
        config,
        sqlite_storage.clone(),
        shared_queue.clone(),
        shared_scorer.clone(),
        feedback_sink,
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
    let feedback = Arc::new(maekon_suggestion::feedback::FeedbackSender::new_with_sink(
        api,
        Some(feedback_sink),
    ));
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
) -> Option<Arc<crate::suggestion_manager::SuggestionManager>> {
    use maekon_network::auth::TokenManager;
    use maekon_network::http_client::HttpApiClient;
    use tauri::Manager;

    // deep-review Net-4: prefer the TLS-policy-aware constructor. On a `[tls]`
    // *configuration* error (e.g. `allow_self_signed` rejected by
    // `build_reqwest_client_for_url`) do NOT silently swallow it and downgrade to a
    // client without `[tls]` policy enforcement — surface it LOUDLY so a
    // misconfigured `[tls]` block is observable instead of failing open in silence.
    let token_manager = Arc::new(
        match TokenManager::new_with_tls(
            &config.server.base_url,
            &config.tls,
            Some(config.request_timeout()),
        ) {
            Ok(manager) => manager,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    base_url = %config.server.base_url,
                    "TLS-aware TokenManager construction failed; falling back to a default \
                     client WITHOUT [tls] policy enforcement — verify the [tls] config"
                );
                #[allow(deprecated)]
                TokenManager::new(&config.server.base_url)
            }
        },
    );

    app_handle.manage(crate::commands::auth::TokenManagerState(Some(
        token_manager.clone(),
    )));

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
            let feedback = Arc::new(maekon_suggestion::feedback::FeedbackSender::new_with_sink(
                api,
                Some(feedback_sink),
            ));
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

    let deferred_records = match sqlite_storage.list_suggestions_by_state("deferred", 50) {
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
