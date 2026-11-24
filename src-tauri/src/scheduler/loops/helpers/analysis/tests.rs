use super::*;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use maekon_analysis::{ContextAnalyzer, ContextAssembler, PatternMiner};
use maekon_core::config::AnalysisConfig;
use maekon_core::consent::{ConsentManager, ConsentPermissions};
use maekon_core::error::CoreError;
use maekon_core::error_codes::ProviderCode;
use maekon_core::models::event::Event;
use maekon_core::models::suggestion::SuggestionType;
use maekon_core::ports::analysis_provider::AnalysisProvider;
use maekon_core::ports::storage::StorageService;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex as StdMutex;
use tokio::sync::Mutex;

struct LocalAnalysisTestStorage;

#[async_trait]
impl StorageService for LocalAnalysisTestStorage {
    async fn save_event(&self, _event: &Event) -> Result<(), CoreError> {
        Ok(())
    }

    async fn get_events(
        &self,
        _from: DateTime<Utc>,
        _to: DateTime<Utc>,
        _limit: usize,
    ) -> Result<Vec<Event>, CoreError> {
        Ok(Vec::new())
    }

    async fn get_pending_events(&self, _limit: usize) -> Result<Vec<Event>, CoreError> {
        Ok(Vec::new())
    }

    async fn mark_as_sent(&self, _event_ids: &[String]) -> Result<(), CoreError> {
        Ok(())
    }

    async fn enforce_retention(&self) -> Result<usize, CoreError> {
        Ok(0)
    }

    async fn save_suggestion(&self, _suggestion: &Suggestion) -> Result<(), CoreError> {
        Ok(())
    }

    async fn save_activity_segment(
        &self,
        _summary: &maekon_core::models::tiered_memory::SegmentSummary,
    ) -> Result<(), CoreError> {
        Ok(())
    }

    async fn update_segment_llm_summary(
        &self,
        _segment_id: &str,
        _llm_summary: &str,
    ) -> Result<(), CoreError> {
        Ok(())
    }
}

struct CapturingProvider {
    calls: Arc<AtomicUsize>,
    contexts: Arc<StdMutex<Vec<String>>>,
}

#[async_trait]
impl AnalysisProvider for CapturingProvider {
    async fn analyze(
        &self,
        context_json: &str,
        _system_prompt: &str,
    ) -> Result<Vec<Suggestion>, CoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.contexts
            .lock()
            .expect("capture context lock")
            .push(context_json.to_owned());
        Ok(vec![make_suggestion("ocr-path", Priority::High)])
    }

    fn provider_name(&self) -> &str {
        "capturing-test-provider"
    }
}

struct FailingProvider;

#[async_trait]
impl AnalysisProvider for FailingProvider {
    async fn analyze(
        &self,
        _context_json: &str,
        _system_prompt: &str,
    ) -> Result<Vec<Suggestion>, CoreError> {
        Err(CoreError::Analysis {
            code: ProviderCode::AnalysisFailed,
            message: "synthetic provider failure".to_string(),
        })
    }

    fn provider_name(&self) -> &str {
        "failing-test-provider"
    }
}

fn make_suggestion(id: &str, priority: Priority) -> Suggestion {
    Suggestion {
        suggestion_id: id.to_string(),
        suggestion_type: SuggestionType::WorkGuidance,
        content: format!("suggestion {id}"),
        priority,
        confidence_score: 0.9,
        relevance_score: 0.8,
        is_actionable: true,
        created_at: Utc::now(),
        expires_at: None,
        source: Default::default(),
        reasoning: None,
        context_scope: None,
    }
}

async fn run_event_fixture(
    analyzer: &Option<Arc<ContextAnalyzer>>,
    storage: &Arc<dyn StorageService>,
    consent_manager: &Arc<dyn ConsentManagerPort>,
    config_manager: &ConfigManager,
    queue: &Arc<Mutex<maekon_suggestion::queue::SuggestionQueue>>,
    ocr_hint: &str,
) -> crate::local_analysis_status::LocalAnalysisStatus {
    handle_event_analysis(
        analyzer,
        storage,
        "FixtureApp",
        "FixtureWindow",
        Some(ocr_hint),
        Some(consent_manager),
        Some(config_manager),
        true,
        Some(queue),
        RelevanceGates::pass_through(),
        None,
        None,
        false,
    )
    .await
}

#[tokio::test]
async fn funnel_emits_on_changed_with_queue_len_once() {
    let queue = Arc::new(Mutex::new(maekon_suggestion::queue::SuggestionQueue::new(
        10,
    )));
    let calls = AtomicUsize::new(0);
    let last_count = AtomicUsize::new(0);
    let cb = |c: usize| {
        calls.fetch_add(1, Ordering::SeqCst);
        last_count.store(c, Ordering::SeqCst);
    };
    let items = vec![
        make_suggestion("s1", Priority::Medium),
        make_suggestion("s2", Priority::High),
    ];
    enqueue_and_surface(
        &queue,
        items,
        RelevanceGates::pass_through(),
        Some(&cb),
        None,
        false,
    )
    .await;
    assert_eq!(calls.load(Ordering::SeqCst), 1, "one emit per batch");
    assert_eq!(
        last_count.load(Ordering::SeqCst),
        2,
        "emit carries queue len"
    );
}

#[tokio::test]
async fn funnel_skips_emit_when_all_duplicates() {
    let queue = Arc::new(Mutex::new(maekon_suggestion::queue::SuggestionQueue::new(
        10,
    )));
    let s = make_suggestion("dup", Priority::High);
    enqueue_and_surface(
        &queue,
        vec![s.clone()],
        RelevanceGates::pass_through(),
        None,
        None,
        false,
    )
    .await;

    let calls = AtomicUsize::new(0);
    let cb = |_c: usize| {
        calls.fetch_add(1, Ordering::SeqCst);
    };
    // Same content → fingerprint dedup rejects → no accepted → no emit.
    enqueue_and_surface(
        &queue,
        vec![s],
        RelevanceGates::pass_through(),
        Some(&cb),
        None,
        false,
    )
    .await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "duplicate-only batch must not emit a refresh"
    );
}

include!("../analysis_live_authority_tests.rs");

#[tokio::test]
async fn provider_failure_is_negative_control_and_never_reaches_review_queue() {
    let storage: Arc<dyn StorageService> = Arc::new(LocalAnalysisTestStorage);
    let analyzer = Some(Arc::new(ContextAnalyzer::new(
        storage.clone(),
        Arc::new(FailingProvider),
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
    let consent_manager = Arc::new(ConsentManager::new(consent_dir.path().join("consent.json")));
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
    let consent_port: Arc<dyn ConsentManagerPort> = consent_manager;

    let status = handle_event_analysis(
        &analyzer,
        &storage,
        "FixtureApp",
        "FixtureWindow",
        Some("SYNTHETIC_SAFE_OCR_FIXTURE"),
        Some(&consent_port),
        None,
        true,
        Some(&queue),
        RelevanceGates::pass_through(),
        None,
        None,
        false,
    )
    .await;

    assert_eq!(
        status.status,
        crate::local_analysis_status::LocalAnalysisStatusKind::ProviderOffline
    );
    assert_eq!(status.reason, "provider_unavailable");
    assert!(queue.lock().await.is_empty());
}

// -----------------------------------------------------------------------
// #7914: uniform learned relevance gates across LOCAL producers.
//
// Headline DoD (design point 6): in a DEFAULT / OSS build (feature
// `local-suggestions` on, `server` OFF — the exact cell `cargo test -p
// maekon-app --lib` compiles), repeated REJECTION feedback recorded on the
// scorer must actually change a RULE-producer suggestion's fate when it
// flows through the `enqueue_and_surface` seam. Before #7914 the scorer was
// consulted only on the server SSE path, so off-SSE (i.e. every OSS build)
// it was write-only and this suppression never happened locally.
// -----------------------------------------------------------------------
mod scorer_gate_through_seam {
    use super::*;
    use maekon_core::models::suggestion::{FeedbackType, SuggestionSource, SuggestionType};
    use maekon_suggestion::scorer::FeedbackScorer;

    /// A rule-produced suggestion with a middling relevance (0.4) — high
    /// enough to survive on its own, low enough that a learned HEAVY_PENALTY
    /// (-0.3) drops it below the scorer's SUPPRESSION_THRESHOLD (0.2).
    fn rule_suggestion(id: &str) -> Suggestion {
        Suggestion {
            suggestion_id: id.to_string(),
            suggestion_type: SuggestionType::WorkGuidance,
            content: format!("rule nudge {id}"),
            priority: Priority::Medium,
            confidence_score: 0.9,
            relevance_score: 0.4,
            is_actionable: true,
            created_at: Utc::now(),
            expires_at: None,
            source: SuggestionSource::RuleBased,
            reasoning: None,
            context_scope: None,
        }
    }

    /// CONTROL: with a fresh scorer (no learned rejections) the rule
    /// suggestion reaches the live queue exactly as before.
    #[tokio::test]
    async fn fresh_scorer_lets_rule_suggestion_through_seam() {
        let queue = Arc::new(tokio::sync::Mutex::new(
            maekon_suggestion::queue::SuggestionQueue::new(10),
        ));
        let scorer = Arc::new(tokio::sync::Mutex::new(FeedbackScorer::new()));
        let gates = RelevanceGates {
            scorer: Some(&scorer),
            regime: None,
            regime_acceptance_rate: None,
        };
        enqueue_and_surface(
            &queue,
            vec![rule_suggestion("keep-1")],
            gates,
            None,
            None,
            false,
        )
        .await;
        assert_eq!(
            queue.lock().await.len(),
            1,
            "a rule suggestion must reach the queue when nothing has been rejected"
        );
    }

    /// HEADLINE: after the user rejects this (type, source) 10× — the exact
    /// signal `submit_suggestion_feedback` records into the SAME shared
    /// scorer — the identical rule suggestion is SUPPRESSED at the seam and
    /// never reaches the live queue. This is the fate-change #7914 makes
    /// happen in an OSS build; it did not before, because off the SSE path
    /// the scorer was write-only.
    #[tokio::test]
    async fn repeated_rejection_suppresses_rule_suggestion_through_seam() {
        let queue = Arc::new(tokio::sync::Mutex::new(
            maekon_suggestion::queue::SuggestionQueue::new(10),
        ));
        let scorer = Arc::new(tokio::sync::Mutex::new(FeedbackScorer::new()));
        {
            let mut s = scorer.lock().await;
            for _ in 0..10 {
                s.record(
                    SuggestionType::WorkGuidance,
                    SuggestionSource::RuleBased,
                    &FeedbackType::Rejected,
                );
            }
        }
        let gates = RelevanceGates {
            scorer: Some(&scorer),
            regime: None,
            regime_acceptance_rate: None,
        };
        enqueue_and_surface(
            &queue,
            vec![rule_suggestion("drop-1")],
            gates,
            None,
            None,
            false,
        )
        .await;
        assert_eq!(
            queue.lock().await.len(),
            0,
            "10 rejections of this (type, source) must suppress the rule suggestion \
             at the seam — the learned signal now changes local fate (#7914)"
        );
    }
}

// Actual async seams and sink markers distinguish revocation from guard mocks.
mod capture_origin {
    use super::*;
    use std::future::{poll_fn, Future};
    use std::pin::Pin;
    use std::sync::atomic::AtomicBool;
    use std::task::Poll;

    use maekon_core::ports::vision::{capture_denied, CaptureGuard, CapturePauseState};
    use maekon_suggestion::scorer::FeedbackScorer;
    use tokio::sync::Notify;

    const MARKER: &str = "ORIGINAL_HELPER_CAPTURE_MARKER";

    struct OriginalGuard {
        pause: Arc<CapturePauseState>,
        epoch: u64,
        ocr_revoked: Arc<AtomicBool>,
    }

    impl CaptureGuard for OriginalGuard {
        fn check(&self, access: CaptureAccess) -> Result<(), CoreError> {
            self.pause.check(self.epoch)?;
            if matches!(access, CaptureAccess::Ocr) && self.ocr_revoked.load(Ordering::SeqCst) {
                return Err(capture_denied("synthetic original OCR consent revoked"));
            }
            Ok(())
        }
    }

    fn permit(pause: &Arc<CapturePauseState>, ocr_revoked: &Arc<AtomicBool>) -> CapturePermit {
        CapturePermit::new(Arc::new(OriginalGuard {
            pause: pause.clone(),
            epoch: pause.snapshot().unwrap(),
            ocr_revoked: ocr_revoked.clone(),
        }))
    }

    fn authority() -> (Arc<CapturePauseState>, Arc<AtomicBool>) {
        (
            Arc::new(CapturePauseState::new(Arc::new(AtomicBool::new(false)))),
            Arc::new(AtomicBool::new(false)),
        )
    }

    fn revoke_aba(pause: &CapturePauseState) {
        assert!(pause.toggle().unwrap());
        assert!(!pause.toggle().unwrap());
    }

    #[derive(Default)]
    struct Gate {
        entered: AtomicBool,
        release: Notify,
    }

    impl Gate {
        async fn once(&self) {
            if !self.entered.swap(true, Ordering::SeqCst) {
                self.release.notified().await;
            }
        }
    }

    async fn poll_pending<F: Future>(mut future: Pin<&mut F>) {
        poll_fn(|cx| {
            assert!(
                future.as_mut().poll(cx).is_pending(),
                "actual await must block"
            );
            Poll::Ready(())
        })
        .await;
    }

    struct ObservedStorage {
        inner: LocalAnalysisTestStorage,
        saved: StdMutex<Vec<String>>,
        gate: Option<Arc<Gate>>,
    }

    #[async_trait]
    impl StorageService for ObservedStorage {
        async fn save_event(&self, event: &Event) -> Result<(), CoreError> {
            self.inner.save_event(event).await
        }
        async fn get_events(
            &self,
            from: DateTime<Utc>,
            to: DateTime<Utc>,
            limit: usize,
        ) -> Result<Vec<Event>, CoreError> {
            self.inner.get_events(from, to, limit).await
        }
        async fn get_pending_events(&self, limit: usize) -> Result<Vec<Event>, CoreError> {
            self.inner.get_pending_events(limit).await
        }
        async fn mark_as_sent(&self, ids: &[String]) -> Result<(), CoreError> {
            self.inner.mark_as_sent(ids).await
        }
        async fn enforce_retention(&self) -> Result<usize, CoreError> {
            self.inner.enforce_retention().await
        }
        async fn save_suggestion(&self, suggestion: &Suggestion) -> Result<(), CoreError> {
            // Model an already committed write before the storage future returns.
            self.saved
                .lock()
                .unwrap()
                .push(suggestion.suggestion_id.clone());
            if let Some(gate) = &self.gate {
                gate.once().await;
            }
            Ok(())
        }
        async fn save_activity_segment(
            &self,
            summary: &maekon_core::models::tiered_memory::SegmentSummary,
        ) -> Result<(), CoreError> {
            self.inner.save_activity_segment(summary).await
        }
        async fn update_segment_llm_summary(
            &self,
            id: &str,
            summary: &str,
        ) -> Result<(), CoreError> {
            self.inner.update_segment_llm_summary(id, summary).await
        }
    }

    struct ObservedProvider {
        inputs: StdMutex<Vec<String>>,
        gate: Option<Arc<Gate>>,
    }

    #[async_trait]
    impl AnalysisProvider for ObservedProvider {
        async fn analyze(&self, context: &str, _: &str) -> Result<Vec<Suggestion>, CoreError> {
            self.inputs.lock().unwrap().push(context.to_owned());
            if let Some(gate) = &self.gate {
                gate.once().await;
            }
            Ok(vec![
                make_suggestion("original-a", Priority::High),
                make_suggestion("original-b", Priority::Medium),
            ])
        }
        fn provider_name(&self) -> &str {
            "observed-capture-helper"
        }
    }

    struct Fixture {
        storage: Arc<ObservedStorage>,
        provider: Arc<ObservedProvider>,
        analyzer: Option<Arc<ContextAnalyzer>>,
        consent: Arc<dyn ConsentManagerPort>,
        queue: Arc<Mutex<maekon_suggestion::queue::SuggestionQueue>>,
        emits: AtomicUsize,
        _consent_dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new(storage_gate: Option<Arc<Gate>>, provider_gate: Option<Arc<Gate>>) -> Self {
            let storage = Arc::new(ObservedStorage {
                inner: LocalAnalysisTestStorage,
                saved: StdMutex::new(Vec::new()),
                gate: storage_gate,
            });
            let provider = Arc::new(ObservedProvider {
                inputs: StdMutex::new(Vec::new()),
                gate: provider_gate,
            });
            let analyzer = Some(Arc::new(ContextAnalyzer::new(
                storage.clone(),
                provider.clone(),
                PatternMiner::new(),
                ContextAssembler::new(Box::new(str::to_owned)),
                AnalysisConfig {
                    throttle_secs: 0,
                    ..AnalysisConfig::default()
                },
            )));
            let dir = tempfile::tempdir().unwrap();
            let consent = Arc::new(ConsentManager::new(dir.path().join("consent.json")));
            consent
                .grant_consent(
                    ConsentPermissions {
                        screen_capture: true,
                        ocr_processing: true,
                        activity_pattern_learning: true,
                        ..Default::default()
                    },
                    30,
                )
                .unwrap();
            Self {
                storage,
                provider,
                analyzer,
                consent,
                queue: Arc::new(Mutex::new(maekon_suggestion::queue::SuggestionQueue::new(
                    10,
                ))),
                emits: AtomicUsize::new(0),
                _consent_dir: dir,
            }
        }

        async fn run(
            &self,
            original: &CapturePermit,
        ) -> crate::local_analysis_status::LocalAnalysisStatus {
            let storage: Arc<dyn StorageService> = self.storage.clone();
            let changed = |_| {
                self.emits.fetch_add(1, Ordering::SeqCst);
            };
            handle_event_analysis_with_capture_origin(
                &self.analyzer,
                &storage,
                "FixtureApp",
                "FixtureWindow",
                Some(MARKER),
                Some(&self.consent),
                None,
                true,
                Some(&self.queue),
                RelevanceGates::pass_through(),
                Some(&changed),
                None,
                false,
                Some(original),
            )
            .await
        }

        async fn assert_positive(&self, original: &CapturePermit) {
            let status = self.run(original).await;
            assert_eq!(
                status.status,
                crate::local_analysis_status::LocalAnalysisStatusKind::Generated
            );
            assert_eq!(
                &*self.storage.saved.lock().unwrap(),
                &["original-a", "original-b"]
            );
            let queue = self.queue.lock().await;
            assert_eq!(queue.len(), 2);
            assert!(queue.iter().any(|s| s.suggestion_id == "original-a"));
            assert!(queue.iter().any(|s| s.suggestion_id == "original-b"));
            drop(queue);
            assert_eq!(self.emits.load(Ordering::SeqCst), 1);
            assert!(self.provider.inputs.lock().unwrap()[0].contains(MARKER));
        }
    }

    fn assert_blocked(status: crate::local_analysis_status::LocalAnalysisStatus) {
        assert_eq!(
            status.status,
            crate::local_analysis_status::LocalAnalysisStatusKind::PolicyBlocked
        );
        assert_eq!(status.reason, "provider_policy_blocked");
    }

    #[tokio::test]
    async fn original_revoked_while_initial_queue_count_waits() {
        for ocr_only in [false, true] {
            let fixture = Fixture::new(None, None);
            let (pause, ocr) = authority();
            let original = permit(&pause, &ocr);
            let held = fixture.queue.lock().await;
            let mut future = Box::pin(fixture.run(&original));
            poll_pending(future.as_mut()).await;
            if ocr_only {
                ocr.store(true, Ordering::SeqCst);
            } else {
                revoke_aba(&pause);
            }
            drop(held);
            assert_blocked(future.await);
            assert!(fixture.provider.inputs.lock().unwrap().is_empty());
            assert!(fixture.storage.saved.lock().unwrap().is_empty());
            assert!(fixture.queue.lock().await.is_empty());
            assert_eq!(fixture.emits.load(Ordering::SeqCst), 0);
            let (fresh_pause, fresh_ocr) = authority();
            fixture
                .assert_positive(&permit(&fresh_pause, &fresh_ocr))
                .await;
        }
    }

    #[tokio::test]
    async fn original_revoked_during_provider_wait() {
        let gate = Arc::new(Gate::default());
        let fixture = Fixture::new(None, Some(gate.clone()));
        let (pause, ocr) = authority();
        let original = permit(&pause, &ocr);
        let mut future = Box::pin(fixture.run(&original));
        poll_pending(future.as_mut()).await;
        assert!(gate.entered.load(Ordering::SeqCst));
        assert!(fixture.provider.inputs.lock().unwrap()[0].contains(MARKER));
        revoke_aba(&pause);
        gate.release.notify_one();
        assert_blocked(future.await);
        assert_eq!(
            fixture.provider.inputs.lock().unwrap().len(),
            1,
            "sent request is not recalled"
        );
        assert!(fixture.storage.saved.lock().unwrap().is_empty());
        assert!(fixture.queue.lock().await.is_empty());
        assert_eq!(fixture.emits.load(Ordering::SeqCst), 0);
        fixture.assert_positive(&permit(&pause, &ocr)).await;
    }

    #[tokio::test]
    async fn revocation_during_first_save_stops_later_effects() {
        let gate = Arc::new(Gate::default());
        let fixture = Fixture::new(Some(gate.clone()), None);
        let (pause, ocr) = authority();
        let original = permit(&pause, &ocr);
        let mut future = Box::pin(fixture.run(&original));
        poll_pending(future.as_mut()).await;
        assert!(gate.entered.load(Ordering::SeqCst));
        assert_eq!(&*fixture.storage.saved.lock().unwrap(), &["original-a"]);
        revoke_aba(&pause);
        gate.release.notify_one();
        assert_blocked(future.await);
        assert_eq!(
            &*fixture.storage.saved.lock().unwrap(),
            &["original-a"],
            "committed save remains; second save is denied"
        );
        assert!(fixture.queue.lock().await.is_empty());
        assert_eq!(fixture.emits.load(Ordering::SeqCst), 0);
        Fixture::new(None, None)
            .assert_positive(&permit(&pause, &ocr))
            .await;
    }

    #[tokio::test]
    async fn revocation_during_scorer_wait_blocks_surface() {
        let queue = Arc::new(Mutex::new(maekon_suggestion::queue::SuggestionQueue::new(
            10,
        )));
        let scorer = Arc::new(Mutex::new(FeedbackScorer::new()));
        let (pause, ocr) = authority();
        let original = permit(&pause, &ocr);
        let emits = AtomicUsize::new(0);
        let changed = |_| {
            emits.fetch_add(1, Ordering::SeqCst);
        };
        let held = scorer.lock().await;
        let gates = RelevanceGates {
            scorer: Some(&scorer),
            regime: None,
            regime_acceptance_rate: None,
        };
        let mut future = Box::pin(enqueue_and_surface_with_capture_origin(
            &queue,
            vec![make_suggestion(MARKER, Priority::High)],
            gates,
            Some(&changed),
            None,
            false,
            Some(&original),
        ));
        poll_pending(future.as_mut()).await;
        revoke_aba(&pause);
        drop(held);
        assert!(matches!(
            future.await.unwrap_err(),
            CoreError::PermissionDenied {
                code: maekon_core::error_codes::PermissionCode::PrivacyDenied,
                ..
            }
        ));
        assert!(queue.lock().await.is_empty());
        assert_eq!(emits.load(Ordering::SeqCst), 0);
        let fresh = permit(&pause, &ocr);
        let gates = RelevanceGates {
            scorer: Some(&scorer),
            regime: None,
            regime_acceptance_rate: None,
        };
        assert_eq!(
            enqueue_and_surface_with_capture_origin(
                &queue,
                vec![make_suggestion(MARKER, Priority::High)],
                gates,
                Some(&changed),
                None,
                false,
                Some(&fresh),
            )
            .await
            .unwrap(),
            1
        );
        assert_eq!(queue.lock().await.len(), 1);
        assert!(queue.lock().await.iter().any(|s| s.suggestion_id == MARKER));
        assert_eq!(emits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn revocation_during_final_queue_wait_blocks_push() {
        let queue = Arc::new(Mutex::new(maekon_suggestion::queue::SuggestionQueue::new(
            10,
        )));
        assert!(queue
            .lock()
            .await
            .push(make_suggestion("unrelated-existing", Priority::Low)));
        let (pause, ocr) = authority();
        let original = permit(&pause, &ocr);
        let emits = AtomicUsize::new(0);
        let changed = |_| {
            emits.fetch_add(1, Ordering::SeqCst);
        };
        let held = queue.lock().await;
        let mut future = Box::pin(enqueue_and_surface_with_capture_origin(
            &queue,
            vec![make_suggestion(MARKER, Priority::High)],
            RelevanceGates::pass_through(),
            Some(&changed),
            None,
            false,
            Some(&original),
        ));
        poll_pending(future.as_mut()).await;
        revoke_aba(&pause);
        drop(held);
        let denied = future.await;
        // The actual sink assertion must turn red if the post-lock guard is removed.
        assert_eq!(
            queue.lock().await.len(),
            1,
            "revoked marker must never enter the real queue"
        );
        assert_eq!(
            queue.lock().await.peek().unwrap().suggestion_id,
            "unrelated-existing"
        );
        assert!(matches!(
            denied.unwrap_err(),
            CoreError::PermissionDenied {
                code: maekon_core::error_codes::PermissionCode::PrivacyDenied,
                ..
            }
        ));
        assert_eq!(emits.load(Ordering::SeqCst), 0);
        let fresh = permit(&pause, &ocr);
        assert_eq!(
            enqueue_and_surface_with_capture_origin(
                &queue,
                vec![make_suggestion(MARKER, Priority::High)],
                RelevanceGates::pass_through(),
                Some(&changed),
                None,
                false,
                Some(&fresh),
            )
            .await
            .unwrap(),
            1
        );
        assert_eq!(queue.lock().await.len(), 2);
        assert!(queue.lock().await.iter().any(|s| s.suggestion_id == MARKER));
        assert_eq!(emits.load(Ordering::SeqCst), 1);
    }
}
