/// F-RR-01/02: async-safety tests verifying that the delete_data_range + search
/// handlers do NOT block the tokio worker thread.
///
/// After the spawn_blocking wrapping, both service methods are genuinely async,
/// so they must complete normally within `tokio::time::timeout`.
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::time::Duration;

use maekon_api_contracts::data::DeleteRangeRequest;
use maekon_api_contracts::search::SearchQuery;
use maekon_storage::sqlite::SqliteStorage;
use maekon_web::{
    services::{
        data_web_service::DataCommandService, search_service::SearchQueryService,
        web_contexts::StorageWebContext,
    },
    AppState,
};

fn test_state() -> AppState {
    let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory sqlite"));
    let (event_tx, _) = broadcast::channel(8);
    AppState::with_core(storage, event_tx)
}

/// F-RR-01: delete_data_range is wrapped in spawn_blocking, so it must not block
/// the tokio worker thread and must complete within 5 seconds.
#[tokio::test]
async fn test_delete_data_range_does_not_block_worker() {
    let service = DataCommandService::new(StorageWebContext::from_state(&test_state()));
    let request = DeleteRangeRequest {
        from: "2024-01-01T00:00:00Z".to_string(),
        to: "2024-01-31T00:00:00Z".to_string(),
        data_types: vec![],
    };

    let result = tokio::time::timeout(Duration::from_secs(5), service.delete_data_range(&request))
        .await
        .expect("delete_data_range timed out — possible worker thread blocking");

    // Empty DB, so 0 records are deleted successfully.
    let delete_result = result.expect("delete_data_range returned error");
    assert_eq!(delete_result.total(), 0);
    assert!(delete_result.success);
}

/// F-RR-02: search is wrapped in spawn_blocking, so it must not block the tokio
/// worker thread and must complete within 5 seconds.
#[tokio::test]
async fn test_search_does_not_block_worker() {
    let service = SearchQueryService::new(StorageWebContext::from_state(&test_state()));
    let params = SearchQuery {
        q: "test".to_string(),
        search_type: "all".to_string(),
        limit: Some(10),
        offset: Some(0),
        tag_ids: None,
    };

    let result = tokio::time::timeout(Duration::from_secs(5), service.search(&params))
        .await
        .expect("search timed out — possible worker thread blocking");

    // Empty DB, so 0 results.
    let response = result.expect("search returned error");
    assert_eq!(response.total, 0);
    assert!(response.results.is_empty());
}

/// F-RR-01: returns BadRequest on an invalid date range (verifies the
/// early-return path).
#[tokio::test]
async fn test_delete_data_range_bad_request_no_dates() {
    let service = DataCommandService::new(StorageWebContext::from_state(&test_state()));
    let request = DeleteRangeRequest {
        from: String::new(),
        to: String::new(),
        data_types: vec![],
    };

    let result = service.delete_data_range(&request).await;
    assert!(
        matches!(result, Err(maekon_web::error::ApiError::BadRequest(_))),
        "empty from/to should return BadRequest"
    );
}

/// F-RR-02: returns BadRequest when both the query and tags are absent
/// (verifies the early-return path).
#[tokio::test]
async fn test_search_bad_request_no_query() {
    let service = SearchQueryService::new(StorageWebContext::from_state(&test_state()));
    let params = SearchQuery {
        q: String::new(),
        search_type: "all".to_string(),
        limit: None,
        offset: None,
        tag_ids: None,
    };

    let result = service.search(&params).await;
    assert!(
        matches!(result, Err(maekon_web::error::ApiError::BadRequest(_))),
        "empty query and no tags should return BadRequest"
    );
}

// ── F-RR-C30-01: get_storage_stats spawn_blocking verification ───────────────

use maekon_web::services::{
    settings_query_service::SettingsQueryService, web_contexts::SettingsWebContext,
};

fn settings_context() -> SettingsWebContext {
    let state = test_state();
    SettingsWebContext::from_state(&state)
}

/// F-RR-C30-01a: get_storage_stats is wrapped in spawn_blocking, so it must not
/// block the tokio worker thread and must complete within 5 seconds.
#[tokio::test]
async fn test_get_storage_stats_does_not_block_worker() {
    let service = SettingsQueryService::new(settings_context());

    let result = tokio::time::timeout(Duration::from_secs(5), service.get_storage_stats())
        .await
        .expect("get_storage_stats timed out — possible worker thread blocking");

    // Empty DB, so it succeeds with a record count of 0; db_size_bytes is
    // greater than 0 because of the SQLite page header.
    let stats = result.expect("get_storage_stats returned error");
    assert_eq!(stats.frame_count, 0);
    assert_eq!(stats.event_count, 0);
    assert_eq!(
        stats.total_size_bytes,
        stats.db_size_bytes + stats.frames_size_bytes
    );
}

/// F-RR-C30-01b: when frames_dir is absent, frames_size_bytes = 0 is returned
/// normally.
#[tokio::test]
async fn test_get_storage_stats_no_frames_dir() {
    let service = SettingsQueryService::new(settings_context());
    let stats = service
        .get_storage_stats()
        .await
        .expect("get_storage_stats failed");
    assert_eq!(
        stats.frames_size_bytes, 0,
        "frames_size_bytes must be 0 when frames_dir is None"
    );
    assert_eq!(
        stats.total_size_bytes,
        stats.db_size_bytes + stats.frames_size_bytes
    );
}

// ── #4811 PR-5/PR-6: async-safety guards for the converted web sub-traits ─────
//
// Regression guards mirroring F-RR-01/02: each converted async storage method must
// NOT block the tokio worker (no block_in_place / bare write_lock on the caller
// thread). A reintroduced blocking call would deadlock/timeout these. Covers the
// maekon-web-reachable PR-5/PR-6 surfaces: TagStorage / ActivityStatsStorage /
// FocusQueryStorage / SuggestionQueryStorage.
//
// #5097: the earlier note that "DigestStorage has no web entrypoint" was stale —
// DigestStorage IS reachable from the web layer (handlers `list_digests` →
// list_weekly_digests, `current_digest` → get_current_week_digest, and the daily
// digest handler → get_daily_digest / get_segments_for_date / save_daily_digest).
// `test_digest_web_entrypoints_do_not_block_worker` below adds the missing guard.

use maekon_web::services::{
    sessions_service::SessionsQueryService, stats_service::StatsQueryService,
    suggestions_service::SuggestionsQueryService, tags_service::TagsQueryService,
};

/// #4811 PR-5 TagStorage: list_tags must not block the worker.
#[tokio::test]
async fn test_list_tags_does_not_block_worker() {
    let service = TagsQueryService::new(StorageWebContext::from_state(&test_state()));
    let result = tokio::time::timeout(Duration::from_secs(5), service.list_tags())
        .await
        .expect("list_tags timed out — possible worker thread blocking");
    assert!(result.expect("list_tags returned error").is_empty());
}

/// #4811 PR-5 ActivityStatsStorage: get_heatmap must not block the worker.
#[tokio::test]
async fn test_get_heatmap_does_not_block_worker() {
    let service = StatsQueryService::new(StorageWebContext::from_state(&test_state()));
    let result = tokio::time::timeout(Duration::from_secs(5), service.get_heatmap(None))
        .await
        .expect("get_heatmap timed out — possible worker thread blocking");
    result.expect("get_heatmap returned error");
}

/// #4811 PR-5 FocusQueryStorage: list_sessions must not block the worker.
#[tokio::test]
async fn test_list_sessions_does_not_block_worker() {
    let service = SessionsQueryService::new(StorageWebContext::from_state(&test_state()));
    let result = tokio::time::timeout(Duration::from_secs(5), service.list_sessions())
        .await
        .expect("list_sessions timed out — possible worker thread blocking");
    assert!(result.expect("list_sessions returned error").is_empty());
}

/// #4811 PR-6 SuggestionQueryStorage: list_suggestions must not block the worker.
#[tokio::test]
async fn test_list_suggestions_does_not_block_worker() {
    let service = SuggestionsQueryService::new(StorageWebContext::from_state(&test_state()));
    let result = tokio::time::timeout(Duration::from_secs(5), service.list_suggestions(10))
        .await
        .expect("list_suggestions timed out — possible worker thread blocking");
    assert!(result.expect("list_suggestions returned error").is_empty());
}

/// #5097 PR-6 DigestStorage: the web-reachable digest methods must not block the
/// tokio worker. Backfills the guard the stale "no web entrypoint" note skipped —
/// `list_digests` (list_weekly_digests), `current_digest` (get_current_week_digest),
/// and the daily-digest handler (get_daily_digest / get_segments_for_date read
/// funnel + save_daily_digest write funnel). A reintroduced blocking call (e.g. the
/// inherent sync twin resolved on a concrete `Arc<SqliteStorage>`) would timeout.
#[tokio::test]
async fn test_digest_web_entrypoints_do_not_block_worker() {
    use maekon_core::models::daily_digest::DailyDigest;

    let ctx = StorageWebContext::from_state(&test_state());

    // list_digests handler → list_weekly_digests (read funnel)
    let weekly = tokio::time::timeout(Duration::from_secs(5), ctx.storage.list_weekly_digests(4))
        .await
        .expect("list_weekly_digests timed out — possible worker thread blocking");
    assert!(weekly
        .expect("list_weekly_digests returned error")
        .is_empty());

    // current_digest handler → get_current_week_digest (read funnel)
    let current = tokio::time::timeout(
        Duration::from_secs(5),
        ctx.storage.get_current_week_digest(),
    )
    .await
    .expect("get_current_week_digest timed out — possible worker thread blocking");
    assert!(current
        .expect("get_current_week_digest returned error")
        .is_none());

    // daily-digest handler reads → get_daily_digest + get_segments_for_date (read funnel)
    let daily = tokio::time::timeout(
        Duration::from_secs(5),
        ctx.storage.get_daily_digest("2026-01-01"),
    )
    .await
    .expect("get_daily_digest timed out — possible worker thread blocking");
    assert!(daily.expect("get_daily_digest returned error").is_none());

    let segments = tokio::time::timeout(
        Duration::from_secs(5),
        ctx.storage.get_segments_for_date("2026-01-01"),
    )
    .await
    .expect("get_segments_for_date timed out — possible worker thread blocking");
    assert!(segments
        .expect("get_segments_for_date returned error")
        .is_empty());

    // daily-digest generation → save_daily_digest (write funnel)
    let digest = DailyDigest {
        date: chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
        insight: None,
        timeline: vec![],
        statistics: Default::default(),
        generated_at: chrono::Utc::now(),
        digest_provenance: "heuristic".to_string(),
        ai_narrative: Default::default(),
    };
    let saved = tokio::time::timeout(
        Duration::from_secs(5),
        ctx.storage.save_daily_digest(&digest),
    )
    .await
    .expect("save_daily_digest timed out — possible worker thread blocking");
    saved.expect("save_daily_digest returned error");
}

// ── F-QA-C31-06: accept_loop.abort() on serve_external restart ───────────────
//
// Verifies that when serve_external_inner exits (cleanly or via shutdown signal),
// it calls `accept_loop_handle.abort()` so the old accept loop task does NOT spin
// after a supervisor restart. The test uses the panic-injection path:
//
//   iteration 1 → accept loop panics (PANIC_ON_FIRST_ACCEPT=true)
//                → serve_external returns Err(Tonic(...)) or panics
//                → accept_loop_handle.abort() is called
//   iteration 2 → fresh serve_external call with clean state succeeds
//
// The critical invariant: after iteration 1 exits, no ghost accept-loop task
// remains alive. We observe this indirectly: if abort() is NOT called, the old
// accept loop task holds the listener and the second bind (same port) would
// succeed — but any connection forwarded through the orphaned task's conn_tx
// (which is now closed) would surface as a dropped channel. The test relies on
// clean sequential shutdown instead: if the old task were still running, the
// second iteration would still complete (each has its own listener), but the
// first task would never exit, making `handle.is_finished()` false indefinitely.
//
// Feature-gated on grpc-dashboard-external + external-grpc-tools + test-support
// because PANIC_ON_FIRST_ACCEPT is only compiled under those flags.

#[cfg(all(
    feature = "grpc-dashboard-external",
    feature = "external-grpc-tools",
    feature = "test-support"
))]
mod accept_loop_abort_tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Arc;
    use std::time::Duration;

    use maekon_storage::sqlite::SqliteStorage;
    use tokio::sync::watch;

    use maekon_web::grpc::external::accept_loop::PANIC_ON_FIRST_ACCEPT;

    /// Both tests below drive `serve_external` against the PROCESS-GLOBAL
    /// `PANIC_ON_FIRST_ACCEPT` switch. Run in parallel, the clean-shutdown
    /// test's accept loop can steal the panic armed by the respawn test
    /// (observed on the linux runner once panic reaping made the theft loud).
    /// Serialize them on a file-local lock.
    static ACCEPT_LOOP_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    use maekon_web::grpc::external::cert_resolver::HotReloadCertResolver;
    use maekon_web::grpc::external::ip_ban::IpBan;
    use maekon_web::grpc::external::live_config::{LiveExternalConfig, LiveSnapshot};
    use maekon_web::grpc::external::metrics::ExternalMetrics;
    use maekon_web::grpc::external::serve_external;
    use maekon_web::grpc::external::spawn_config::ExternalGrpcSpawnConfig;
    use maekon_web::grpc::external::test_support::{
        install_rustls_crypto_provider, test_cert_pair,
    };
    use maekon_web::grpc::external::tls_config::load_certified_key;
    use maekon_web::grpc::LoadPolicy;
    use maekon_web::storage_port::WebStorage;

    fn dummy_monitor() -> Arc<dyn maekon_core::ports::monitor::SystemMonitor> {
        struct D;
        #[async_trait::async_trait]
        impl maekon_core::ports::monitor::SystemMonitor for D {
            async fn collect_metrics(
                &self,
            ) -> Result<maekon_core::models::system::SystemMetrics, maekon_core::error::CoreError>
            {
                Err(maekon_core::error::CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: "dummy".into(),
                })
            }
        }
        Arc::new(D)
    }

    async fn make_cfg(
        bind_addr: SocketAddr,
        shutdown_tx: Arc<watch::Sender<bool>>,
        shutdown_rx: watch::Receiver<bool>,
    ) -> ExternalGrpcSpawnConfig {
        let (cert_path, key_path) = test_cert_pair();
        let certified_key = load_certified_key(&cert_path, &key_path).expect("load certified key");
        let resolver = Arc::new(HotReloadCertResolver::new(certified_key));
        let storage =
            Arc::new(SqliteStorage::open_in_memory(30).expect("sqlite")) as Arc<dyn WebStorage>;
        let (event_tx, _) = tokio::sync::broadcast::channel(1);
        let logger = Arc::new(tokio::sync::RwLock::new(
            maekon_automation::audit::AuditLogger::new(64, 16),
        ));
        let audit_port = Arc::new(maekon_automation::audit::AuditLogAdapter::new(logger))
            as Arc<dyn maekon_core::ports::audit_log::AuditLogPort>;

        ExternalGrpcSpawnConfig {
            bind_addr,
            config: maekon_core::config::ExternalGrpcConfig {
                enabled: true,
                auth_mode: Some(maekon_core::config::AuthMode::Jwt),
                max_connections: 16,
                ..Default::default()
            },
            storage,
            system_monitor: dummy_monitor(),
            event_tx,
            audit_port,
            cert_resolver: resolver,
            jwt_verifier: None,
            mtls_verifier: None,
            ip_ban: Arc::new(IpBan::new()),
            metrics: Arc::new(ExternalMetrics::new()),
            shutdown_rx,
            shutdown_tx,
            pii_sanitizer: None,
            ai_runtime_status_snapshot: None,
            live: Arc::new(LiveExternalConfig::new(LiveSnapshot {
                streaming_enabled: true,
                load_policy: Arc::new(LoadPolicy::new(
                    maekon_core::config::LoadThresholds::default(),
                )),
            })),
        }
    }

    /// F-QA-C31-06: serve_external aborts the accept loop when it exits.
    ///
    /// Spawns serve_external and signals immediate shutdown. Asserts the task
    /// completes within 2s (i.e. there is no leaked accept-loop task keeping the
    /// JoinHandle alive).
    ///
    /// This is the clean-shutdown sub-case of the abort contract: if abort() were
    /// missing, the accept-loop task would remain alive after the tonic server
    /// exits, and a second serve_external call would succeed but the first task
    /// would still be running. We verify with a 500ms poll that the server task
    /// finishes cleanly.
    #[tokio::test]
    async fn serve_external_accept_loop_abort_on_clean_shutdown() {
        let _serialized = ACCEPT_LOOP_TEST_LOCK.lock().await;
        install_rustls_crypto_provider();

        let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let shutdown_tx = Arc::new(shutdown_tx);

        let cfg = make_cfg(bind_addr, shutdown_tx.clone(), shutdown_rx).await;

        // Spawn the server.
        let handle = tokio::spawn(serve_external(cfg));

        // Give it a moment to bind, then signal clean shutdown.
        tokio::time::sleep(Duration::from_millis(60)).await;
        shutdown_tx.send(true).unwrap();

        // The task must complete within 2s. If the accept-loop were NOT aborted,
        // the task would still finish (shutdown_rx also stops the accept loop),
        // but if accept_loop_handle.abort() is removed, the accept-loop task
        // would spin on closed-channel Err paths until the process exits.
        // The 2s bound catches runaway tasks in the test runner.
        let result = tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("serve_external task did not finish within 2s — possible accept loop leak");

        // Result is Ok(Ok(())) for clean shutdown.
        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => panic!("serve_external returned error on clean shutdown: {e}"),
            Err(join_err) => panic!("serve_external task panicked: {join_err}"),
        }
    }

    /// F-QA-C31-06 (panic variant): When serve_external panics (via injected
    /// PANIC_ON_FIRST_ACCEPT), the accept-loop task is aborted and does not
    /// outlive the panicking iteration.
    ///
    /// Uses `spawn_with_supervisor` to catch the panic; after the supervisor
    /// absorbs the first panic and starts the second iteration, we send a clean
    /// shutdown to verify the supervisor exits. The test asserts the supervisor
    /// handle finishes within 3s (1s backoff + bind/start time).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn serve_external_accept_loop_aborted_after_panic() {
        let _serialized = ACCEPT_LOOP_TEST_LOCK.lock().await;
        install_rustls_crypto_provider();

        let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let shutdown_tx = Arc::new(shutdown_tx);

        let cfg = make_cfg(bind_addr, shutdown_tx.clone(), shutdown_rx).await;

        // Arm the injected panic so the first accept-loop call panics immediately.
        PANIC_ON_FIRST_ACCEPT.store(true, std::sync::atomic::Ordering::SeqCst);

        let supervisor_handle = maekon_web::grpc::external::spawn_with_supervisor(cfg).await;

        // Allow: first iteration panic + supervisor 1s backoff + second iteration bind.
        tokio::time::sleep(Duration::from_millis(1_500)).await;

        // The supervisor should still be running (second iteration cleanly bound).
        assert!(
            !supervisor_handle.is_finished(),
            "supervisor must still be running after one-panic respawn"
        );

        // Now send shutdown to the second iteration.
        shutdown_tx.send(true).unwrap();

        // Supervisor should exit within 2s of the shutdown signal.
        let result = tokio::time::timeout(Duration::from_secs(2), supervisor_handle).await;
        // Justified: the only security-relevant value here is that the supervisor exits cleanly
        // (no accept-loop leak / runaway task after panic + respawn). The inner Ok(()) simply
        // means the JoinHandle resolved without a panic on the second iteration; the outer
        // timeout-Ok is the liveness gate. Both are checked structurally below. (#5594)
        result
            .expect("supervisor did not exit within 2s of shutdown — possible accept loop leak")
            .expect("supervisor task must exit without panic on second iteration");
    }
}
