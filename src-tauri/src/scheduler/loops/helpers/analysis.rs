//! Event-driven LLM analysis helper (context analyzer) + the shared
//! suggestion surfacing funnel.

use std::sync::Arc;

use tracing::{debug, info, warn};

use maekon_core::config_manager::ConfigManager;
use maekon_core::models::suggestion::{Priority, Suggestion};
use maekon_core::models::tiered_memory::Regime;
use maekon_core::ports::consent_manager::{ConsentGate, ConsentManagerPort};
use maekon_core::ports::storage::StorageService;

use crate::notification_manager::NotificationManager;

/// Learned + static relevance gates applied UNIFORMLY at the shared surfacing
/// seam, so every LOCAL suggestion producer (rule, periodic-LLM, event-driven,
/// focus/idle-resume) runs the SAME decision function (#7914).
///
/// Before #7914 the three gates were scattered: `FeedbackScorer::adjust` ran
/// only on the server SSE path (`SuggestionReceiver::handle_suggestion`), the
/// learned per-regime acceptance gate only on the event-driven producer, and
/// the periodic-LLM/rule/idle-resume producers applied only the static
/// Deep-Focus `filter_by_regime`. Rejecting a rule-based nudge 10× therefore
/// never suppressed it locally even though the identical signal suppressed
/// server suggestions. Bundling the inputs here and consuming them in
/// [`enqueue_and_surface`] closes that gap.
///
/// Every field is "no signal ⇒ pass-through": an unwired producer, an OSS build
/// without a scorer, or a regime with too few learned samples behaves exactly
/// as before. This mirrors the `SchedulerRequiredDeps`/two-payload taste (named
/// fields, no `Default` escape hatch beyond the explicit [`RelevanceGates::pass_through`]).
pub(crate) struct RelevanceGates<'a> {
    /// FeedbackScorer-driven per (`suggestion_type`, `source`) relevance
    /// adjustment + suppression — the SAME `FeedbackScorer::adjust` the server
    /// SSE path applies, so the scorer decision lives in ONE place across both
    /// streams. `None` ⇒ no scorer wired (kept usable from tests / builds
    /// without the suggestion pipeline). #7913 (T2.1) will back this handle
    /// with persisted state; the plumbing here is deliberately
    /// persistence-agnostic so that swap is transparent.
    pub scorer: Option<&'a Arc<tokio::sync::Mutex<maekon_suggestion::scorer::FeedbackScorer>>>,
    /// Current activity regime for the static Deep-Focus filter
    /// (`filter_by_regime`). `None` ⇒ pass-through. This is a UX posture, NOT a
    /// learned gate — the learned gates below compose WITH it, never replace it.
    pub regime: Option<&'a Regime>,
    /// Learned per-regime acceptance rate (`RegimeClassifier::acceptance_rate`)
    /// for `regime`. `None` ⇒ pass-through (reader unwired, or below the
    /// classifier's minimum sample floor). See
    /// [`maekon_analysis::apply_regime_acceptance_gate`].
    pub regime_acceptance_rate: Option<f64>,
}

impl RelevanceGates<'_> {
    /// Pass-through gates — no scorer, no regime signal; every field is the
    /// "no signal ⇒ no suppression" default. Test-only: production producers
    /// always build real gates via [`relevance_gates`], so gating this
    /// `#[cfg(test)]` keeps it out of the shipped dead-code surface.
    #[cfg(test)]
    pub(crate) fn pass_through() -> Self {
        Self {
            scorer: None,
            regime: None,
            regime_acceptance_rate: None,
        }
    }
}

/// Look up the learned per-regime acceptance rate for `regime` from the
/// adaptive trigger state's `RegimeClassifier`. `None` when there is no active
/// regime, no trigger state (e.g. analysis-disabled builds), or the classifier
/// has fewer than its minimum reaction samples for this regime. Read-only: the
/// `parking_lot` lock is taken and released synchronously with no `.await`
/// held, so it is safe to call anywhere on the scheduler hot path.
pub(crate) fn regime_acceptance_rate(
    regime: Option<&Regime>,
    adaptive_trigger_state: Option<&crate::scheduler::AdaptiveTriggerState>,
) -> Option<f64> {
    let regime = regime?;
    adaptive_trigger_state?
        .regime_classifier
        .lock()
        .acceptance_rate(&regime.regime_id)
}

/// Build [`RelevanceGates`] for a monitor-loop producer path in one call —
/// keeps the acceptance-rate lookup out of `spawn_monitor_loop`'s LOC-capped
/// body (put plumbing in helpers, not inline expansions). The returned gates
/// borrow `scorer`/`regime`; `adaptive_trigger_state` is only read to compute
/// the owned acceptance rate, so its lifetime is independent.
pub(crate) fn relevance_gates<'a>(
    scorer: Option<&'a Arc<tokio::sync::Mutex<maekon_suggestion::scorer::FeedbackScorer>>>,
    regime: Option<&'a Regime>,
    adaptive_trigger_state: Option<&crate::scheduler::AdaptiveTriggerState>,
) -> RelevanceGates<'a> {
    RelevanceGates {
        scorer,
        regime,
        regime_acceptance_rate: regime_acceptance_rate(regime, adaptive_trigger_state),
    }
}

/// Apply the learned + static relevance gates to `suggestions` in the fixed
/// compose order (#7914). Each gate can only SHRINK the set — none reorders or
/// re-admits — so the order is a pure conjunction. Suppression is made
/// OBSERVABLE (groundwork for T1.6 "quieted because you rejected N similar"):
/// every gate that drops anything emits a `debug!` line naming the reason
/// (`focus` / `regime` / `scorer`). No UI is built here.
async fn apply_relevance_gates(
    mut suggestions: Vec<Suggestion>,
    gates: &RelevanceGates<'_>,
) -> Vec<Suggestion> {
    // 1. Static Deep-Focus regime filter (UX posture, NOT a learned gate). The
    //    learned gates below compose WITH it.
    let before = suggestions.len();
    maekon_analysis::filter_by_regime(&mut suggestions, gates.regime);
    if suggestions.len() < before {
        debug!(
            dropped = before - suggestions.len(),
            gate = "focus",
            "local suggestions suppressed by Deep-Focus regime filter"
        );
    }

    // 2. Learned per-regime acceptance gate (#7600) — quiets regimes the user
    //    has historically rejected suggestions in.
    let before = suggestions.len();
    maekon_analysis::apply_regime_acceptance_gate(&mut suggestions, gates.regime_acceptance_rate);
    if suggestions.len() < before {
        debug!(
            dropped = before - suggestions.len(),
            gate = "regime",
            acceptance_rate = ?gates.regime_acceptance_rate,
            "local suggestions suppressed by learned per-regime acceptance gate"
        );
    }

    // 3. Learned per (type, source) FeedbackScorer gate (#7914 headline). Shares
    //    `FeedbackScorer::adjust` with the server SSE path so the scorer logic
    //    lives in ONE place. In OSS/default (no `server`) builds this is the
    //    first time the scorer READS on a local path — before #7914 it was
    //    write-only off the SSE stream.
    if let Some(scorer) = gates.scorer {
        let scorer = scorer.lock().await;
        suggestions.retain_mut(|s| {
            let keep = scorer.adjust(&s.suggestion_type, &s.source, &mut s.relevance_score);
            if !keep {
                debug!(
                    suggestion_id = %s.suggestion_id,
                    relevance = s.relevance_score,
                    gate = "scorer",
                    "local suggestion suppressed — learned relevance below threshold"
                );
            }
            keep
        });
    }

    suggestions
}

/// Shared surfacing funnel (#5694): apply the learned + static relevance gates
/// (#7914), then push survivors into the live review queue (fingerprint dedup)
/// and make them DISCOVERABLE — emit the `overlay:suggestions-changed` refresh
/// via `on_changed` when anything was accepted, and toast the highest-priority
/// accepted suggestion (High and above) unless focus mode is active (A4 gate).
///
/// Gating (`gates`) is applied HERE, at the single seam, rather than at each
/// producer call site, so the periodic analysis loop, the event-driven path,
/// the rule producer, and the focus/idle-resume path all inherit ONE identical
/// decision function ([`apply_relevance_gates`]) — see [`RelevanceGates`].
/// Notification policy (cooldown / Critical bypass / master switch) lives in
/// [`NotificationManager::notify_suggestion`], so every producer inherits one
/// consistent policy there too.
pub(crate) async fn enqueue_and_surface(
    queue: &Arc<tokio::sync::Mutex<maekon_suggestion::queue::SuggestionQueue>>,
    to_enqueue: Vec<Suggestion>,
    gates: RelevanceGates<'_>,
    on_changed: Option<&(dyn Fn(usize) + Send + Sync)>,
    notifier: Option<&Arc<NotificationManager>>,
    focus_active: bool,
) -> usize {
    // #7914: uniform learned + static relevance gating for every LOCAL producer.
    let to_enqueue = apply_relevance_gates(to_enqueue, &gates).await;
    if to_enqueue.is_empty() {
        return 0;
    }

    let mut accepted = 0usize;
    let mut top: Option<Suggestion> = None;
    let count = {
        let mut q = queue.lock().await;
        for s in to_enqueue {
            let id = s.suggestion_id.clone();
            if q.push(s.clone()) {
                accepted += 1;
                debug!(suggestion_id = %id, "local suggestion enqueued for review");
                if top.as_ref().is_none_or(|t| s.priority > t.priority) {
                    top = Some(s);
                }
            }
        }
        q.len()
        // len-then-drop: the lock is released before emit/notify so UI work
        // never serializes against producers (mirrors suggestions.rs:107-110).
    };

    if accepted == 0 {
        return 0;
    }
    if let Some(cb) = on_changed {
        cb(count);
    }
    if let (Some(nm), Some(t)) = (notifier, top) {
        if t.priority >= Priority::High && !focus_active {
            nm.notify_suggestion(&t).await;
        }
    }
    accepted
}

/// Run event-driven LLM analysis when the user switches to a new app.
/// Persists any resulting suggestions to storage.
#[tracing::instrument(skip_all)]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_event_analysis(
    analyzer: &Option<Arc<maekon_analysis::ContextAnalyzer>>,
    storage: &Arc<dyn StorageService>,
    app_name: &str,
    window_title: &str,
    ocr_hint: Option<&str>,
    consent_manager: Option<&Arc<dyn ConsentManagerPort>>,
    config_manager: Option<&ConfigManager>,
    fallback_analysis_enabled: bool,
    // E20-24 (#4816): live suggestion queue (the manager's Arc). `None` when the
    // local-suggestions pipeline is absent; `Some` enqueues for live review.
    suggestion_queue: Option<&Arc<tokio::sync::Mutex<maekon_suggestion::queue::SuggestionQueue>>>,
    // #7914: learned + static relevance gates (regime Deep-Focus filter +
    // learned per-regime acceptance + FeedbackScorer). Applied inside
    // `enqueue_and_surface`, the same seam every other LOCAL producer flows
    // through. `RelevanceGates::pass_through()` disables all gating.
    gates: RelevanceGates<'_>,
    // #5694: overlay auto-refresh + desktop toast for accepted suggestions.
    on_changed: Option<&(dyn Fn(usize) + Send + Sync)>,
    notifier: Option<&Arc<NotificationManager>>,
    focus_active: bool,
) -> crate::local_analysis_status::LocalAnalysisStatus {
    use crate::local_analysis_status::{
        missing_local_analysis_permissions, LocalAnalysisProducer, LocalAnalysisStatus,
        LocalAnalysisStatusKind,
    };
    use maekon_analysis::AnalysisRunOutcome;

    let producer = LocalAnalysisProducer::AppSwitch;
    let initial_queue_count = match suggestion_queue {
        Some(queue) => queue.lock().await.len(),
        None => 0,
    };
    // Re-read the user-controlled feature switch at the same final boundary as
    // consent. The monitor tick performs several awaits before reaching this
    // helper, so its initial config snapshot is not current authorization.
    let live_config = super::super::analysis_invocation_guard::analysis_config_now(
        config_manager,
        fallback_analysis_enabled,
    );
    if !live_config.enabled {
        return LocalAnalysisStatus::new(
            LocalAnalysisStatusKind::PolicyBlocked,
            "analysis_disabled",
            producer,
            0,
            initial_queue_count,
        );
    }
    // Read the authority after the queue-count await and immediately before
    // analyzer invocation. A tick-level snapshot can outlive a consent
    // withdrawal while OCR is still being processed (#11737).
    let permissions = ConsentGate::from_ref(consent_manager).permissions_snapshot();
    let missing = missing_local_analysis_permissions(&permissions);
    if !missing.is_empty() {
        return LocalAnalysisStatus::consent_required(producer, missing, initial_queue_count);
    }
    let Some(analyzer) = analyzer else {
        return LocalAnalysisStatus::new(
            LocalAnalysisStatusKind::ProviderOffline,
            "provider_unavailable",
            producer,
            0,
            initial_queue_count,
        );
    };

    match analyzer
        .on_significant_event_with_outcome(app_name, window_title, ocr_hint)
        .await
    {
        Ok(AnalysisRunOutcome::Generated(suggestions)) => {
            let candidate_count = suggestions.len();
            for s in &suggestions {
                info!(
                    id = %s.suggestion_id,
                    priority = ?s.priority,
                    content = %maekon_monitor::log_privacy::content_digest(&s.content),
                    "event-driven suggestion produced"
                );
                if let Err(e) = storage.save_suggestion(s).await {
                    warn!("suggestion save failure: {e}");
                }
            }
            // E20-24 (#4816): enqueue for live review (same producer wire as the
            // periodic analysis loop). Dedup via queue fingerprint (push -> bool).
            // #7914: regime + learned per-regime acceptance + FeedbackScorer
            // gating now all happen inside `enqueue_and_surface` (one seam,
            // one decision function), so this path applies the SAME gates as
            // every other LOCAL producer instead of a bespoke subset.
            let Some(queue) = suggestion_queue else {
                return LocalAnalysisStatus::new(
                    LocalAnalysisStatusKind::PolicyBlocked,
                    "suggestion_queue_unavailable",
                    producer,
                    candidate_count,
                    0,
                );
            };
            let admitted = enqueue_and_surface(
                queue,
                suggestions,
                gates,
                on_changed,
                notifier,
                focus_active,
            )
            .await;
            let queue_count = queue.lock().await.len();
            if admitted == 0 {
                LocalAnalysisStatus::new(
                    LocalAnalysisStatusKind::NoCandidate,
                    "not_admitted",
                    producer,
                    candidate_count,
                    queue_count,
                )
            } else {
                LocalAnalysisStatus::new(
                    LocalAnalysisStatusKind::Generated,
                    "generated",
                    producer,
                    candidate_count,
                    queue_count,
                )
            }
        }
        Ok(AnalysisRunOutcome::NoCandidate) => LocalAnalysisStatus::new(
            LocalAnalysisStatusKind::NoCandidate,
            "no_valid_candidate",
            producer,
            0,
            initial_queue_count,
        ),
        Ok(AnalysisRunOutcome::Throttled) => LocalAnalysisStatus::new(
            LocalAnalysisStatusKind::Throttled,
            "analysis_throttled",
            producer,
            0,
            initial_queue_count,
        ),
        Ok(AnalysisRunOutcome::NoInput) => LocalAnalysisStatus::new(
            LocalAnalysisStatusKind::NoCandidate,
            "no_input",
            producer,
            0,
            initial_queue_count,
        ),
        Ok(AnalysisRunOutcome::Unchanged) => LocalAnalysisStatus::new(
            LocalAnalysisStatusKind::NoCandidate,
            "context_unchanged",
            producer,
            0,
            initial_queue_count,
        ),
        Err(error) => {
            let core: maekon_core::error::CoreError = error.into();
            debug!(err.code = %core.code(), "event analysis unavailable");
            LocalAnalysisStatus::from_error(&core, producer, initial_queue_count)
        }
    }
}

#[cfg(test)]
mod tests {
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

    include!("analysis_live_authority_tests.rs");

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
        let consent_manager =
            Arc::new(ConsentManager::new(consent_dir.path().join("consent.json")));
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
}
