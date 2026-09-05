/// #11737 complete-path mutation: the live analysis switch and OCR consent
/// independently revoke provider authority. Restoring both lets the synthetic
/// safe OCR fixture reach ContextAnalyzer and the shared queue with explicit
/// local provenance.
#[tokio::test]
async fn live_authority_mutations_control_analyzer_and_review_queue() {
    let storage: Arc<dyn StorageService> = Arc::new(LocalAnalysisTestStorage);
    let calls = Arc::new(AtomicUsize::new(0));
    let contexts = Arc::new(StdMutex::new(Vec::new()));
    let analyzer = Some(Arc::new(ContextAnalyzer::new(
        storage.clone(),
        Arc::new(CapturingProvider {
            calls: calls.clone(),
            contexts: contexts.clone(),
        }),
        PatternMiner::new(),
        ContextAssembler::new(Box::new(str::to_owned)),
        AnalysisConfig {
            throttle_secs: 0,
            ..AnalysisConfig::default()
        },
    )));
    let queue = Arc::new(Mutex::new(maekon_suggestion::queue::SuggestionQueue::new(
        10,
    )));
    let consent_dir = tempfile::tempdir().expect("consent tempdir");
    let consent_manager = Arc::new(ConsentManager::new(
        consent_dir.path().join("consent.json"),
    ));
    let config_dir = tempfile::tempdir().expect("config tempdir");
    let config_manager = ConfigManager::with_paths(config_dir.path().join("config.json"), None)
        .expect("create isolated config");
    config_manager
        .update_with(|config| {
            config.analysis.enabled = false;
            Ok(())
        })
        .expect("disable analysis");
    consent_manager
        .grant_consent(
            ConsentPermissions {
                screen_capture: true,
                ocr_processing: false,
                activity_pattern_learning: true,
                ..Default::default()
            },
            30,
        )
        .expect("grant consent without OCR processing");
    let consent_port: Arc<dyn ConsentManagerPort> = consent_manager.clone();

    let disabled = run_event_fixture(
        &analyzer,
        &storage,
        &consent_port,
        &config_manager,
        &queue,
        "SYNTHETIC_SAFE_OCR_FIXTURE_WHILE_DISABLED",
    )
    .await;
    assert_eq!(
        disabled.status,
        crate::local_analysis_status::LocalAnalysisStatusKind::PolicyBlocked
    );
    assert_eq!(disabled.reason, "analysis_disabled");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(queue.lock().await.is_empty());

    config_manager
        .update_with(|config| {
            config.analysis.enabled = true;
            Ok(())
        })
        .expect("enable analysis");
    let blocked = run_event_fixture(
        &analyzer,
        &storage,
        &consent_port,
        &config_manager,
        &queue,
        "SYNTHETIC_SAFE_OCR_FIXTURE",
    )
    .await;
    assert_eq!(
        blocked.status,
        crate::local_analysis_status::LocalAnalysisStatusKind::ConsentRequired
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(queue.lock().await.is_empty());

    consent_manager
        .grant_consent(
            ConsentPermissions {
                screen_capture: true,
                ocr_processing: true,
                activity_pattern_learning: true,
                ..Default::default()
            },
            30,
        )
        .expect("grant complete local-analysis consent");
    let generated = run_event_fixture(
        &analyzer,
        &storage,
        &consent_port,
        &config_manager,
        &queue,
        "SYNTHETIC_SAFE_OCR_FIXTURE",
    )
    .await;
    assert_eq!(
        generated.status,
        crate::local_analysis_status::LocalAnalysisStatusKind::Generated
    );
    assert_eq!(generated.source, "llm_local");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(queue.lock().await.len(), 1);
    assert!(contexts
        .lock()
        .expect("read captured context")
        .iter()
        .any(|context| context.contains("SYNTHETIC_SAFE_OCR_FIXTURE")));

    consent_manager
        .revoke_consent()
        .expect("withdraw local-analysis consent");
    let withdrawn = run_event_fixture(
        &analyzer,
        &storage,
        &consent_port,
        &config_manager,
        &queue,
        "SYNTHETIC_SAFE_OCR_FIXTURE_AFTER_WITHDRAWAL",
    )
    .await;
    assert_eq!(
        withdrawn.status,
        crate::local_analysis_status::LocalAnalysisStatusKind::ConsentRequired
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(queue.lock().await.len(), 1);
    assert!(contexts
        .lock()
        .expect("read captured context after withdrawal")
        .iter()
        .all(|context| !context.contains("SYNTHETIC_SAFE_OCR_FIXTURE_AFTER_WITHDRAWAL")));
}
