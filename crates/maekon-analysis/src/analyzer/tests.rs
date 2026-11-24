use super::*;
use async_trait::async_trait;
use chrono::DateTime;
use maekon_core::error::CoreError;
use maekon_core::models::event::ContextEvent;
use maekon_core::models::suggestion::{Priority, SuggestionSource, SuggestionType};

// ── Mock StorageService ────────────────────────────────────────

struct MockStorage {
    events: Vec<Event>,
}

impl MockStorage {
    fn new(events: Vec<Event>) -> Self {
        Self { events }
    }
}

#[async_trait]
impl StorageService for MockStorage {
    async fn save_event(&self, _event: &Event) -> Result<(), CoreError> {
        Ok(())
    }

    async fn get_events(
        &self,
        _from: DateTime<Utc>,
        _to: DateTime<Utc>,
        _limit: usize,
    ) -> Result<Vec<Event>, CoreError> {
        Ok(self.events.clone())
    }

    async fn get_pending_events(&self, _limit: usize) -> Result<Vec<Event>, CoreError> {
        Ok(vec![])
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

// ── Mock FewShotStorage ────────────────────────────────────────

use maekon_core::models::suggestion::SuggestionHistoryEntry;
use std::sync::atomic::{AtomicBool, Ordering};

/// Few-shot storage whose synchronous query blocks briefly to emulate the real
/// SQLite UNION-ALL read lock. The `analyze()` path must run this on a
/// `spawn_blocking` thread rather than the tokio worker (#6086).
struct BlockingFewShotStorage {
    entries: Vec<SuggestionHistoryEntry>,
    called: Arc<AtomicBool>,
}

impl FewShotStorage for BlockingFewShotStorage {
    fn get_suggestions_with_feedback(
        &self,
        _limit: usize,
    ) -> Result<Vec<SuggestionHistoryEntry>, CoreError> {
        self.called.store(true, Ordering::SeqCst);
        // Synchronous blocking sleep, exactly the kind of call that must NOT
        // occur on the async worker. If this ran on the tokio worker thread it
        // would stall the 1-second scheduler hot path.
        std::thread::sleep(std::time::Duration::from_millis(10));
        Ok(self.entries.clone())
    }

    fn record_suggestion_feedback(
        &self,
        _suggestion_id: &str,
        _feedback_type: &str,
        _context_app: &str,
        _context_window: &str,
        _regime_label: Option<&str>,
    ) -> Result<(), CoreError> {
        Ok(())
    }
}

fn make_history_entry(feedback: &str, content: &str) -> SuggestionHistoryEntry {
    SuggestionHistoryEntry {
        suggestion_id: maekon_core::id_generation::generate_id("sug"),
        suggestion_type: "productivity_tip".to_string(),
        content: content.to_string(),
        confidence: 0.8,
        feedback_type: feedback.to_string(),
        regime_label: None,
        context_app: "VSCode".to_string(),
        context_window: "main.rs".to_string(),
        created_at: Utc::now(),
    }
}

// ── Mock AnalysisProvider ──────────────────────────────────────

struct MockAnalysisProvider {
    suggestions: Vec<Suggestion>,
}

impl MockAnalysisProvider {
    fn new(suggestions: Vec<Suggestion>) -> Self {
        Self { suggestions }
    }
}

#[async_trait]
impl AnalysisProvider for MockAnalysisProvider {
    async fn analyze(
        &self,
        _context_json: &str,
        _system_prompt: &str,
    ) -> Result<Vec<Suggestion>, CoreError> {
        Ok(self.suggestions.clone())
    }

    fn provider_name(&self) -> &str {
        "mock"
    }
}

use std::sync::atomic::AtomicUsize;

/// Provider that records how many times `analyze()` was entered and sleeps
/// for a configurable duration so two concurrent callers can overlap. Used
/// to prove the throttle is claim-then-run AND that the in-flight guard
/// dedups duplicates independent of `throttle_secs`: a second caller must be
/// turned away before invoking the LLM (#6119, #7079).
struct CountingSlowProvider {
    suggestions: Vec<Suggestion>,
    calls: Arc<AtomicUsize>,
    sleep: std::time::Duration,
}

#[async_trait]
impl AnalysisProvider for CountingSlowProvider {
    async fn analyze(
        &self,
        _context_json: &str,
        _system_prompt: &str,
    ) -> Result<Vec<Suggestion>, CoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(self.sleep).await;
        Ok(self.suggestions.clone())
    }

    fn provider_name(&self) -> &str {
        "counting-slow"
    }
}

// ── Helpers ────────────────────────────────────────────────────

fn make_suggestion(content: &str, confidence: f64) -> Suggestion {
    Suggestion {
        suggestion_id: maekon_core::id_generation::generate_id("sug"),
        suggestion_type: SuggestionType::ProductivityTip,
        content: content.to_string(),
        priority: Priority::Medium,
        confidence_score: confidence,
        relevance_score: confidence,
        is_actionable: true,
        created_at: Utc::now(),
        expires_at: None,
        source: SuggestionSource::LlmLocal,
        reasoning: None,
        context_scope: None,
    }
}

fn make_events(count: usize) -> Vec<Event> {
    (0..count)
        .map(|i| {
            Event::Context(ContextEvent {
                app_name: if i.is_multiple_of(2) {
                    "VSCode".to_string()
                } else {
                    "Slack".to_string()
                },
                window_title: format!("Window {}", i),
                timestamp: Utc::now() - Duration::minutes((count - i) as i64),
                ..Default::default()
            })
        })
        .collect()
}

fn make_analyzer(events: Vec<Event>, suggestions: Vec<Suggestion>) -> ContextAnalyzer {
    let storage = Arc::new(MockStorage::new(events));
    let provider = Arc::new(MockAnalysisProvider::new(suggestions));
    let miner = PatternMiner::new();
    let assembler = ContextAssembler::new(Box::new(|t: &str| t.to_string()));
    let config = AnalysisConfig::default();

    ContextAnalyzer::new(storage, provider, miner, assembler, config)
}

/// Build an analyzer with an explicit `throttle_secs`. Used by the #7079
/// claim-state tests that drive the private slot machinery directly.
fn make_analyzer_with_throttle(throttle_secs: u64) -> ContextAnalyzer {
    let storage = Arc::new(MockStorage::new(vec![]));
    let provider = Arc::new(MockAnalysisProvider::new(vec![]));
    let miner = PatternMiner::new();
    let assembler = ContextAssembler::new(Box::new(|t: &str| t.to_string()));
    let config = AnalysisConfig {
        throttle_secs,
        ..AnalysisConfig::default()
    };
    ContextAnalyzer::new(storage, provider, miner, assembler, config)
}

// ── Tests ──────────────────────────────────────────────────────

#[tokio::test]
async fn analyze_returns_filtered_suggestions() {
    let suggestions = vec![
        make_suggestion("High confidence", 0.9),
        make_suggestion("Low confidence", 0.3),
        make_suggestion("Medium confidence", 0.7),
    ];
    let events = make_events(10);
    let analyzer = make_analyzer(events, suggestions);

    let result = analyzer.analyze().await.unwrap();

    // Low confidence (0.3) should be filtered out (min_confidence = 0.6)
    assert_eq!(result.len(), 2);
    assert!(result.iter().all(|s| s.confidence_score >= 0.6));
}

#[tokio::test]
async fn throttle_prevents_rapid_reanalysis() {
    let suggestions = vec![make_suggestion("Tip", 0.8)];
    let events = make_events(5);
    let analyzer = make_analyzer(events, suggestions);

    // First analysis should succeed
    let result1 = analyzer.analyze().await.unwrap();
    assert!(!result1.is_empty());

    // Second analysis should be throttled (throttle_secs = 120)
    let result2 = analyzer.analyze().await.unwrap();
    assert!(result2.is_empty(), "Should be throttled");
}

#[tokio::test]
async fn explicit_outcome_distinguishes_no_candidate_from_throttle() {
    let suggestions = vec![make_suggestion("Below threshold", 0.3)];
    let analyzer = make_analyzer(make_events(5), suggestions);

    let first = analyzer.analyze_with_outcome().await.unwrap();
    assert!(matches!(first, AnalysisRunOutcome::NoCandidate));

    let second = analyzer.analyze_with_outcome().await.unwrap();
    assert!(matches!(second, AnalysisRunOutcome::Throttled));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_analyze_yields_single_proceed() {
    // Two near-simultaneous analyze() calls on the SHARED analyzer must
    // claim-then-run: only one acquires the throttle slot and invokes the
    // LLM; the other observes the in-flight reservation and bails (#6119).
    let suggestions = vec![make_suggestion("Tip", 0.9)];
    let events = make_events(10);

    let storage = Arc::new(MockStorage::new(events));
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(CountingSlowProvider {
        suggestions,
        calls: calls.clone(),
        sleep: std::time::Duration::from_millis(100),
    });
    let miner = PatternMiner::new();
    let assembler = ContextAssembler::new(Box::new(|t: &str| t.to_string()));
    let config = AnalysisConfig::default();

    let analyzer = Arc::new(ContextAnalyzer::new(
        storage, provider, miner, assembler, config,
    ));

    let a1 = analyzer.clone();
    let a2 = analyzer.clone();
    let (r1, r2) = tokio::join!(
        tokio::spawn(async move { a1.analyze().await.unwrap() }),
        tokio::spawn(async move { a2.analyze().await.unwrap() }),
    );
    let r1 = r1.unwrap();
    let r2 = r2.unwrap();

    // Exactly one LLM invocation, regardless of scheduling order.
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "only one concurrent caller should invoke the LLM"
    );

    // Exactly one caller returns suggestions; the throttled one returns empty.
    let proceeded = usize::from(!r1.is_empty()) + usize::from(!r2.is_empty());
    assert_eq!(
        proceeded, 1,
        "exactly one concurrent analyze() should proceed"
    );
}

/// #7079 (revert-provable): the in-flight guard must dedup a concurrent
/// duplicate analysis even when `throttle_secs` is set BELOW the analysis
/// round-trip latency. Pre-fix, `throttle_secs` doubled as the only
/// in-flight window, so a second caller that arrives after the throttle has
/// elapsed (but while the first run is still in flight) would pass the gate
/// and start a duplicate LLM analysis. The explicit in-flight flag now
/// blocks it regardless of `throttle_secs`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn in_flight_dedup_holds_below_throttle_secs() {
    let suggestions = vec![make_suggestion("Tip", 0.9)];
    let events = make_events(10);

    let storage = Arc::new(MockStorage::new(events));
    let calls = Arc::new(AtomicUsize::new(0));
    // LLM round-trip (1500ms) intentionally exceeds throttle_secs (1s).
    let provider = Arc::new(CountingSlowProvider {
        suggestions,
        calls: calls.clone(),
        sleep: std::time::Duration::from_millis(1500),
    });
    let miner = PatternMiner::new();
    let assembler = ContextAssembler::new(Box::new(|t: &str| t.to_string()));
    let config = AnalysisConfig {
        throttle_secs: 1,
        ..AnalysisConfig::default()
    };
    let analyzer = Arc::new(ContextAnalyzer::new(
        storage, provider, miner, assembler, config,
    ));

    // First caller claims the slot and enters the 1.5s LLM round-trip.
    let a1 = analyzer.clone();
    let first = tokio::spawn(async move { a1.analyze().await.unwrap() });

    // Wait until the throttle window (1s) has fully elapsed while the first
    // run is still in flight. Pre-fix, a second caller here would pass the
    // throttle and start a duplicate analysis.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

    let a2 = analyzer.clone();
    let second = tokio::spawn(async move { a2.analyze().await.unwrap() });

    let r1 = first.await.unwrap();
    let r2 = second.await.unwrap();

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "in-flight guard must dedup the duplicate even when throttle_secs < latency"
    );
    let proceeded = usize::from(!r1.is_empty()) + usize::from(!r2.is_empty());
    assert_eq!(proceeded, 1, "exactly one analyze() should proceed");
}

/// #7079: with the throttle window disabled (`throttle_secs = 0`), the
/// in-flight flag alone must still block a concurrent duplicate claim — the
/// dedup no longer depends on the throttle timing at all.
#[tokio::test]
async fn in_flight_flag_blocks_duplicate_with_zero_throttle() {
    let owner = Arc::new(());
    let analyzer = make_analyzer_with_throttle(0);

    // First run claims the slot and is still in flight (not yet completed).
    let claim = analyzer
        .claim_analysis_slot(Arc::downgrade(&owner))
        .await
        .expect("first claim should succeed");

    // throttle_secs = 0 would otherwise admit a second caller immediately;
    // the in-flight gate must reject it.
    assert!(
        analyzer
            .claim_analysis_slot(Arc::downgrade(&owner))
            .await
            .is_none(),
        "in-flight run must block a concurrent duplicate regardless of throttle_secs"
    );

    // After the first run completes, the slot is claimable again.
    analyzer.complete_analysis_claim(claim).await;
    assert!(
        analyzer
            .claim_analysis_slot(Arc::downgrade(&owner))
            .await
            .is_some(),
        "slot should be claimable again after completion (throttle_secs = 0)"
    );
}

/// #7079: a late `restore` from a run whose slot was already reclaimed (or
/// re-taken by a newer run) must be a no-op — the generation token guards
/// against clobbering the live reservation that defeated the #6119 dedup in
/// the original report.
#[tokio::test]
async fn stale_restore_is_noop_via_generation_guard() {
    let owner = Arc::new(());
    let analyzer = make_analyzer_with_throttle(0);

    // Run A claims, then completes — freeing the slot.
    let claim_a = analyzer
        .claim_analysis_slot(Arc::downgrade(&owner))
        .await
        .expect("A claims");
    analyzer.complete_analysis_claim(claim_a).await;

    // Run B claims the now-free slot under a fresh generation.
    let claim_b = analyzer
        .claim_analysis_slot(Arc::downgrade(&owner))
        .await
        .expect("B claims");

    // Run A's delayed restore (e.g. a slow error path) must NOT release B's
    // in-flight reservation: the generation no longer matches.
    analyzer.restore_analysis_claim(claim_a).await;

    // B still owns the slot — a third caller is still blocked.
    assert!(
        analyzer
            .claim_analysis_slot(Arc::downgrade(&owner))
            .await
            .is_none(),
        "A's stale restore must not release B's in-flight reservation"
    );

    // B completes normally and frees the slot.
    analyzer.complete_analysis_claim(claim_b).await;
    assert!(
        analyzer
            .claim_analysis_slot(Arc::downgrade(&owner))
            .await
            .is_some(),
        "slot should be free once the owning run (B) completes"
    );
}

#[tokio::test]
async fn analyze_if_changed_returns_empty_when_patterns_unchanged() {
    let suggestions = vec![make_suggestion("Tip", 0.8)];
    let events = make_events(5);

    let storage = Arc::new(MockStorage::new(events));
    let provider = Arc::new(MockAnalysisProvider::new(suggestions));
    let miner = PatternMiner::new();
    let assembler = ContextAssembler::new(Box::new(|t: &str| t.to_string()));
    let config = AnalysisConfig {
        throttle_secs: 0, // Disable throttle for this test
        ..AnalysisConfig::default()
    };

    let analyzer = ContextAnalyzer::new(storage, provider, miner, assembler, config);

    // First call triggers full analysis
    let result1 = analyzer.analyze().await.unwrap();
    assert!(!result1.is_empty());

    // Second call with same events should detect unchanged patterns
    let result2 = analyzer.analyze_if_changed().await.unwrap();
    assert!(
        result2.is_empty(),
        "Should return empty when patterns unchanged"
    );
}

#[tokio::test]
async fn analyze_if_changed_legacy_wrapper_returns_generated_suggestions() {
    let analyzer = make_analyzer(
        make_events(5),
        vec![make_suggestion("Changed context tip", 0.8)],
    );

    let suggestions = analyzer.analyze_if_changed().await.unwrap();

    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0].content, "Changed context tip");
}

#[tokio::test]
async fn confidence_filter_drops_low_confidence() {
    let suggestions = vec![
        make_suggestion("Very low", 0.1),
        make_suggestion("Below threshold", 0.5),
        make_suggestion("Just above", 0.6),
        make_suggestion("Well above", 0.9),
    ];
    let events = make_events(5);
    let analyzer = make_analyzer(events, suggestions);

    let result = analyzer.analyze().await.unwrap();

    assert_eq!(result.len(), 2);
    assert!(result.iter().all(|s| s.confidence_score >= 0.6));
}

#[tokio::test]
async fn max_suggestions_cap() {
    let suggestions: Vec<_> = (0..10)
        .map(|i| make_suggestion(&format!("Tip {}", i), 0.8))
        .collect();
    let events = make_events(5);
    let analyzer = make_analyzer(events, suggestions);

    let result = analyzer.analyze().await.unwrap();

    // Default max_suggestions is 3
    assert!(result.len() <= 3);
}

#[tokio::test]
async fn analyze_with_empty_events_returns_empty() {
    let analyzer = make_analyzer(vec![], vec![make_suggestion("Tip", 0.9)]);
    let result = analyzer.analyze().await.unwrap();
    assert!(result.is_empty());
}

#[tokio::test]
async fn on_significant_event_calls_analysis() {
    let suggestions = vec![make_suggestion("Context tip", 0.85)];
    let events = make_events(5);
    let analyzer = make_analyzer(events, suggestions);

    let result = analyzer
        .on_significant_event("VSCode", "main.rs", Some("fn main()"))
        .await
        .unwrap();

    assert!(!result.is_empty());
    let scope = result[0]
        .context_scope
        .as_ref()
        .expect("event-driven suggestion should keep context scope");
    assert_eq!(scope.app_name.as_deref(), Some("VSCode"));
    assert_eq!(scope.window_title.as_deref(), Some("main.rs"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn analyze_offloads_few_shot_query_to_blocking_thread() {
    let suggestions = vec![make_suggestion("Tip", 0.9)];
    let events = make_events(10);
    let analyzer = make_analyzer(events, suggestions);

    let called = Arc::new(AtomicBool::new(false));
    let fs_storage = Arc::new(BlockingFewShotStorage {
        entries: vec![
            make_history_entry("accepted", "Take a break"),
            make_history_entry("rejected", "Ignore notifications"),
        ],
        called: called.clone(),
    });
    analyzer.set_few_shot_storage(fs_storage).await;

    // analyze() must complete without stalling the async worker on the
    // synchronous, blocking few-shot query (it is offloaded via spawn_blocking).
    let result = analyzer.analyze().await.unwrap();

    assert!(
        called.load(Ordering::SeqCst),
        "few-shot storage should have been queried during analysis"
    );
    assert!(!result.is_empty());
}

#[test]
fn build_session_metrics_from_events() {
    let events = make_events(10);
    let metrics = ContextAnalyzer::build_session_metrics(&events);
    assert!(metrics.total_work_mins > 0);
    assert!(metrics.context_switches > 0);
}

#[test]
fn build_current_activity_from_events() {
    let events = make_events(5);
    let current = ContextAnalyzer::build_current_activity(&events);
    // Last event in make_events is index 4 (even), so app is "Slack" (index 4 is even -> VSCode)
    assert!(!current.app_name.is_empty());
}

/// The real production contract of get_events is timestamp DESC (newest-first), but `make_events`
/// reverses it into ascending order, which masked the #6893 wrap bug. This helper builds data
/// newest-first as the real contract does (index 0 = newest, "Newest" → "Oldest", 20-minute span).
fn make_events_desc() -> Vec<Event> {
    let base = Utc::now();
    [("Newest", 0i64), ("Middle", 10), ("Oldest", 20)]
        .into_iter()
        .map(|(app, mins_ago)| {
            Event::Context(ContextEvent {
                app_name: app.to_string(),
                window_title: format!("{app} window"),
                timestamp: base - Duration::minutes(mins_ago),
                ..Default::default()
            })
        })
        .collect()
}

/// #6893 regression guard: with real DESC-contract input, total_work_mins must return the actual
/// span (20 minutes) instead of u32-wrapping (~4.29e9). The pre-fix code cast a negative delta
/// `as u32`, yielding ~4_294_967_276.
#[test]
fn build_session_metrics_desc_contract_does_not_wrap() {
    let events = make_events_desc();
    let metrics = ContextAnalyzer::build_session_metrics(&events);
    assert_eq!(
        metrics.total_work_mins, 20,
        "DESC 입력 span 은 20분이어야 한다(u32 wrap 아님)"
    );
}

/// #6893 regression guard: with DESC input, "current" must be the newest context (index 0,
/// "Newest"). The pre-fix `.rev().find_map` picked the oldest "Oldest".
#[test]
fn build_current_activity_desc_contract_picks_newest() {
    let events = make_events_desc();
    let current = ContextAnalyzer::build_current_activity(&events);
    assert_eq!(current.app_name, "Newest");
}

/// Holds only the first provider call so cancellation crosses the real API await.
struct BlockingFirstAnalysisProvider {
    calls: AtomicUsize,
    first_entered: tokio::sync::Notify,
    release_first: tokio::sync::Notify,
    panic_first: bool,
    error_after_first: bool,
}

#[async_trait]
impl AnalysisProvider for BlockingFirstAnalysisProvider {
    async fn analyze(
        &self,
        _context_json: &str,
        _system_prompt: &str,
    ) -> Result<Vec<Suggestion>, CoreError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            self.first_entered.notify_one();
            assert!(!self.panic_first, "intentional reservation provider panic");
            self.release_first.notified().await;
        } else if self.error_after_first {
            return Err(CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: "intentional reservation provider error".to_string(),
            });
        }
        Ok(vec![make_suggestion("reservation-recovered-marker", 0.99)])
    }

    fn provider_name(&self) -> &str {
        "blocking-first-reservation-test"
    }
}

fn reservation_fixture(
    panic_first: bool,
    error_after_first: bool,
) -> (Arc<ContextAnalyzer>, Arc<BlockingFirstAnalysisProvider>) {
    let provider = Arc::new(BlockingFirstAnalysisProvider {
        calls: AtomicUsize::new(0),
        first_entered: tokio::sync::Notify::new(),
        release_first: tokio::sync::Notify::new(),
        panic_first,
        error_after_first,
    });
    let analyzer = Arc::new(ContextAnalyzer::new(
        Arc::new(MockStorage::new(make_events(10))),
        provider.clone(),
        PatternMiner::new(),
        ContextAssembler::new(Box::new(|text: &str| text.to_string())),
        AnalysisConfig {
            throttle_secs: 120,
            ..AnalysisConfig::default()
        },
    ));
    (analyzer, provider)
}

async fn reservation_call(
    analyzer: &ContextAnalyzer,
    mode: u8,
) -> Result<AnalysisRunOutcome, AnalysisError> {
    match mode {
        0 => {
            analyzer
                .on_significant_event_with_outcome("Editor", "Document", None)
                .await
        }
        1 => analyzer.analyze_with_outcome().await,
        _ => analyzer.analyze_if_changed_with_outcome().await,
    }
}

async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(std::time::Duration::from_secs(2), future)
        .await
        .expect("reservation fixture exceeded its bounded wait")
}

#[tokio::test]
async fn cancelled_event_analysis_releases_reservation() {
    let bound = std::time::Duration::from_secs(2);
    let (analyzer, provider) = reservation_fixture(false, false);
    let first_analyzer = analyzer.clone();
    let first = tokio::spawn(async move {
        first_analyzer
            .on_significant_event_with_outcome("Editor", "Document", None)
            .await
    });
    // notify_one retains its permit if the provider reaches the barrier first.
    if tokio::time::timeout(bound, provider.first_entered.notified())
        .await
        .is_err()
    {
        first.abort();
        let _ = tokio::time::timeout(bound, first).await;
        panic!("first public analysis never entered the provider");
    }
    let duplicate = tokio::time::timeout(
        bound,
        analyzer.on_significant_event_with_outcome("Editor", "Document", None),
    )
    .await;
    let calls_while_owned = provider.calls.load(Ordering::SeqCst);
    // Join before assertions so even a failed live-owner control leaves no task.
    first.abort();
    let cancelled = tokio::time::timeout(bound, first)
        .await
        .expect("cancelled public analysis must join")
        .expect_err("the first analysis must be cancelled at its provider await");
    assert!(cancelled.is_cancelled());
    assert!(matches!(
        duplicate.expect("duplicate check must finish").unwrap(),
        AnalysisRunOutcome::Throttled
    ));
    assert_eq!(
        calls_while_owned, 1,
        "live owner must exclude a second provider"
    );

    let recovered = bounded(reservation_call(&analyzer, 0)).await.unwrap();
    assert_eq!(
        provider.calls.load(Ordering::SeqCst),
        2,
        "cancelled event reservation blocked the next provider"
    );
    let AnalysisRunOutcome::Generated(suggestions) = recovered else {
        panic!("fresh public analysis must return the provider result");
    };
    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0].content, "reservation-recovered-marker");
    let completed = bounded(reservation_call(&analyzer, 0)).await.unwrap();
    assert!(matches!(completed, AnalysisRunOutcome::Throttled));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn cancelled_public_apis_and_unwind_restore_exact_previous_timestamp() {
    for (mode, panic_first) in [(0, false), (1, false), (2, false), (0, true)] {
        let (analyzer, provider) = reservation_fixture(panic_first, true);
        let previous = Some(Utc::now() - Duration::seconds(240));
        analyzer.analysis_claim.lock().await.last_analysis_at = previous;
        let first_analyzer = analyzer.clone();
        let first = tokio::spawn(async move { reservation_call(&first_analyzer, mode).await });
        bounded(provider.first_entered.notified()).await;
        if !panic_first {
            first.abort();
        }
        let stopped = bounded(first).await.expect_err("first owner must stop");
        assert_eq!(stopped.is_panic(), panic_first);
        assert_eq!(stopped.is_cancelled(), !panic_first);
        // Reconciliation is lazy: call2 must acquire and then take the normal
        // provider-error restore path before we inspect the exact prior value.
        let retried = bounded(reservation_call(&analyzer, mode)).await;
        assert!(
            matches!(retried, Err(AnalysisError::Core(CoreError::Internal { code, message }))
            if code == maekon_core::error_codes::InternalCode::Generic
                && message == "intentional reservation provider error")
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
        let state = analyzer.analysis_claim.lock().await;
        assert_eq!(
            state.last_analysis_at, previous,
            "cancelled reservation lost exact prior timestamp"
        );
        assert!(!state.in_flight);
    }
}

#[tokio::test]
async fn cancelled_periodic_after_commit_preserves_completed_throttle() {
    let (analyzer, provider) = reservation_fixture(false, false);
    let hash_lock = analyzer.last_patterns_hash.lock().await;
    let first_analyzer = analyzer.clone();
    let first = tokio::spawn(async move { first_analyzer.analyze_with_outcome().await });
    bounded(provider.first_entered.notified()).await;
    provider.release_first.notify_one();
    let committed = bounded(async {
        loop {
            let state = analyzer.analysis_claim.lock().await;
            if !state.in_flight {
                break state.last_analysis_at.expect("provider result must commit");
            }
            drop(state);
            tokio::task::yield_now().await;
        }
    })
    .await;
    first.abort();
    assert!(bounded(first).await.unwrap_err().is_cancelled());
    drop(hash_lock);
    let next = bounded(analyzer.analyze_with_outcome()).await.unwrap();
    assert!(matches!(next, AnalysisRunOutcome::Throttled));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        analyzer.analysis_claim.lock().await.last_analysis_at,
        Some(committed)
    );
}

#[tokio::test]
async fn cancelled_old_owner_cannot_release_watchdog_replacement() {
    let analyzer = make_analyzer_with_throttle(0);
    let old_owner = Arc::new(());
    let old = analyzer
        .claim_analysis_slot(Arc::downgrade(&old_owner))
        .await
        .unwrap();
    analyzer.analysis_claim.lock().await.last_analysis_at =
        Some(Utc::now() - Duration::seconds(601));
    let new_owner = Arc::new(());
    let current = analyzer
        .claim_analysis_slot(Arc::downgrade(&new_owner))
        .await
        .unwrap();
    drop(old_owner);
    analyzer.restore_analysis_claim(old).await;
    analyzer.complete_analysis_claim(old).await;
    assert!(analyzer
        .claim_analysis_slot(Arc::downgrade(&new_owner))
        .await
        .is_none());
    assert_eq!(
        analyzer.analysis_claim.lock().await.generation,
        current.generation
    );
    analyzer.complete_analysis_claim(current).await;
}

mod capture_origin;
