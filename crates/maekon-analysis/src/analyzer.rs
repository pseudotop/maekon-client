use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Weak};

use chrono::{Duration, Utc};
use tokio::sync::Mutex;
use tracing::debug;

use maekon_core::config::AnalysisConfig;
use maekon_core::models::event::Event;
use maekon_core::models::suggestion::{Suggestion, SuggestionContextScope};
use maekon_core::ports::analysis_provider::AnalysisProvider;
use maekon_core::ports::storage::StorageService;
use maekon_core::ports::vision::{CaptureAccess, CapturePermit};

use maekon_core::ports::few_shot_storage::FewShotStorage;

use crate::assembler::{
    humanize_time_ago, ContextAssembler, CurrentActivity, PiiFilter, RelevantHistoryEntry,
    SegmentStats, SessionMetrics,
};
use crate::error::AnalysisError;
use crate::few_shot_selector::FewShotSelector;
use crate::outcome::AnalysisRunOutcome;
use crate::pattern_miner::{is_communication_app, PatternMiner};
use crate::vector_retriever::VectorRetriever;

/// Maximum events to query for full periodic analysis.
const FULL_ANALYSIS_EVENT_LIMIT: usize = 500;
/// Maximum events to query for lightweight change detection.
const CHANGE_DETECTION_EVENT_LIMIT: usize = 200;
/// Maximum events to query for event-driven significant analysis.
const SIGNIFICANT_EVENT_LIMIT: usize = 100;
/// Lookback duration for significant event analysis (minutes).
const SIGNIFICANT_EVENT_LOOKBACK_MINS: i64 = 5;
/// Default focus score for inferred current activity.
const DEFAULT_FOCUS_SCORE: f32 = 0.5;
/// Elevated focus score for significant event triggers.
const SIGNIFICANT_EVENT_FOCUS_SCORE: f32 = 0.7;
/// Watchdog for the in-flight analysis reservation (#7079).
///
/// The in-flight flag normally clears when a run completes or restores its
/// claim. This watchdog is a safety valve: if a run holds the slot for longer
/// than this (e.g. its task was force-aborted at shutdown before it could
/// release), the next caller reclaims the slot so analysis cannot wedge
/// permanently. It is deliberately far larger than any plausible analysis
/// round-trip and is independent of the user-configurable `throttle_secs`.
const ANALYSIS_INFLIGHT_WATCHDOG_SECS: u64 = 600;

/// In-flight reservation state for the analysis throttle.
///
/// Holds the throttle timestamp together with an explicit in-flight flag and a
/// monotonic generation token. This decouples concurrent-duplicate dedup from
/// the throttle window (#7079): `throttle_secs` governs only the temporal
/// spacing between successive runs, while `in_flight` independently blocks a
/// second concurrent run regardless of how small `throttle_secs` is configured.
#[derive(Default)]
struct AnalysisClaimState {
    /// Timestamp the slot was reserved (at claim) or last committed (at
    /// completion). Drives the throttle window and, while `in_flight`, the
    /// watchdog that reclaims a slot abandoned by a dead/cancelled run.
    last_analysis_at: Option<chrono::DateTime<Utc>>,
    /// True from claim until the owning run completes or restores. Blocks a
    /// concurrent duplicate analysis independent of `throttle_secs`.
    in_flight: bool,
    /// Monotonic id advanced on every successful claim. `complete`/`restore`
    /// only mutate state when this still matches the claiming run, so a run
    /// reclaimed by the watchdog cannot clobber the new owner's reservation.
    generation: u64,
    /// The current future owns the strong token; cancellation drops it without
    /// needing an asynchronous cleanup task or holding this mutex across work.
    reservation: Option<AnalysisReservation>,
}

/// Handle returned by `claim_analysis_slot`, identifying the run that owns the
/// in-flight reservation. `Copy` so the success path and every early-return
/// path can pass it to `complete`/`restore` without move-checker friction.
#[derive(Clone, Copy)]
struct AnalysisClaim {
    /// Throttle timestamp to restore if this run fails (no work committed).
    prev: Option<chrono::DateTime<Utc>>,
    /// Generation this run owns; checked by `complete`/`restore`.
    generation: u64,
}

struct AnalysisReservation {
    claim: AnalysisClaim,
    owner: Weak<()>,
}

/// Central orchestrator for the analysis cycle.
/// Concrete struct, NOT a port trait (ADR-011 section 3).
pub struct ContextAnalyzer {
    storage: Arc<dyn StorageService>,
    analysis_provider: Arc<dyn AnalysisProvider>,
    pattern_miner: PatternMiner,
    context_assembler: ContextAssembler,
    vector_retriever: tokio::sync::RwLock<Option<VectorRetriever>>,
    config: AnalysisConfig,
    /// Decoupled throttle + in-flight reservation state (#7079).
    analysis_claim: Mutex<AnalysisClaimState>,
    last_patterns_hash: Mutex<u64>,
    /// Current segment stats snapshot, updated by the monitor loop via `set_segment_stats()`.
    /// Read by `analyze()` / `analyze_if_changed()` to enrich the LLM context with
    /// `current_segment` data (duration, regime, content summary, GUI patterns).
    segment_stats: tokio::sync::RwLock<Option<SegmentStats>>,
    /// Current accessibility text from the focused element, updated by the monitor loop.
    accessibility_text: tokio::sync::RwLock<Option<String>>,
    few_shot_selector: FewShotSelector,
    few_shot_storage: tokio::sync::RwLock<Option<Arc<dyn FewShotStorage>>>,
}

impl ContextAnalyzer {
    pub fn new(
        storage: Arc<dyn StorageService>,
        analysis_provider: Arc<dyn AnalysisProvider>,
        pattern_miner: PatternMiner,
        context_assembler: ContextAssembler,
        config: AnalysisConfig,
    ) -> Self {
        Self {
            storage,
            analysis_provider,
            pattern_miner,
            context_assembler,
            vector_retriever: tokio::sync::RwLock::new(None),
            config,
            analysis_claim: Mutex::new(AnalysisClaimState::default()),
            last_patterns_hash: Mutex::new(0),
            segment_stats: tokio::sync::RwLock::new(None),
            accessibility_text: tokio::sync::RwLock::new(None),
            few_shot_selector: FewShotSelector::new(2),
            few_shot_storage: tokio::sync::RwLock::new(None),
        }
    }

    /// Create a ContextAnalyzer with a PII filter for few-shot context sanitization.
    pub fn with_pii_filter(
        storage: Arc<dyn StorageService>,
        analysis_provider: Arc<dyn AnalysisProvider>,
        pattern_miner: PatternMiner,
        context_assembler: ContextAssembler,
        config: AnalysisConfig,
        pii_filter: PiiFilter,
    ) -> Self {
        Self {
            storage,
            analysis_provider,
            pattern_miner,
            context_assembler,
            vector_retriever: tokio::sync::RwLock::new(None),
            config,
            analysis_claim: Mutex::new(AnalysisClaimState::default()),
            last_patterns_hash: Mutex::new(0),
            segment_stats: tokio::sync::RwLock::new(None),
            accessibility_text: tokio::sync::RwLock::new(None),
            few_shot_selector: FewShotSelector::with_pii_filter(2, pii_filter),
            few_shot_storage: tokio::sync::RwLock::new(None),
        }
    }

    /// Create a ContextAnalyzer with an optional VectorRetriever for RAG-enriched context.
    pub fn with_vector_retriever(
        storage: Arc<dyn StorageService>,
        analysis_provider: Arc<dyn AnalysisProvider>,
        pattern_miner: PatternMiner,
        context_assembler: ContextAssembler,
        vector_retriever: Option<VectorRetriever>,
        config: AnalysisConfig,
    ) -> Self {
        Self {
            storage,
            analysis_provider,
            pattern_miner,
            context_assembler,
            vector_retriever: tokio::sync::RwLock::new(vector_retriever),
            config,
            analysis_claim: Mutex::new(AnalysisClaimState::default()),
            last_patterns_hash: Mutex::new(0),
            segment_stats: tokio::sync::RwLock::new(None),
            accessibility_text: tokio::sync::RwLock::new(None),
            few_shot_selector: FewShotSelector::new(2),
            few_shot_storage: tokio::sync::RwLock::new(None),
        }
    }

    /// Update the current segment stats snapshot. Called by the monitor loop
    /// after each analysis tick so that `analyze()` can include segment context.
    pub async fn set_segment_stats(&self, stats: Option<SegmentStats>) {
        *self.segment_stats.write().await = stats;
    }

    /// Update the current accessibility text from the focused element.
    /// Called by the monitor loop so that `analyze()` includes extracted text in LLM context.
    pub async fn set_accessibility_text(&self, text: Option<String>) {
        *self.accessibility_text.write().await = text;
    }

    /// Inject a VectorRetriever after construction (called from agent_runtime
    /// once embedding components are available).
    pub async fn set_vector_retriever(&self, retriever: VectorRetriever) {
        *self.vector_retriever.write().await = Some(retriever);
    }

    /// Attach few-shot storage for personalized prompts.
    pub async fn set_few_shot_storage(&self, storage: Arc<dyn FewShotStorage>) {
        *self.few_shot_storage.write().await = Some(storage);
    }

    /// Full periodic analysis: query events, mine patterns, call LLM.
    pub async fn analyze(&self) -> Result<Vec<Suggestion>, AnalysisError> {
        self.analyze_with_outcome()
            .await
            .map(AnalysisRunOutcome::into_suggestions)
    }

    /// Full periodic analysis with an explicit metadata-only outcome.
    pub async fn analyze_with_outcome(&self) -> Result<AnalysisRunOutcome, AnalysisError> {
        // Claim-then-run throttle: atomically check the throttle window AND
        // reserve the in-flight slot before releasing the lock and before the
        // LLM round-trip below. The reservation is an explicit in-flight flag
        // (not just the timestamp), so a second concurrent caller bails even
        // when `throttle_secs` is below the run duration, preventing a duplicate
        // LLM analysis regardless of throttle configuration (#6119, #7079).
        let reservation_owner = Arc::new(());
        let claim = match self
            .claim_analysis_slot(Arc::downgrade(&reservation_owner))
            .await
        {
            Some(claim) => claim,
            None => {
                debug!("Analysis throttled — skipping");
                return Ok(AnalysisRunOutcome::Throttled);
            }
        };

        let now = Utc::now();
        let lookback = Duration::seconds(self.config.full_interval_secs as i64);
        let from = now - lookback;

        let events = match self
            .storage
            .get_events(from, now, FULL_ANALYSIS_EVENT_LIMIT)
            .await
        {
            Ok(events) => events,
            Err(e) => {
                // Restore the prior claim so a failed fetch does not consume the
                // throttle window — the next tick can retry immediately (#6119).
                self.restore_analysis_claim(claim).await;
                return Err(e.into());
            }
        };

        if events.is_empty() {
            debug!("No events found for analysis");
            // No work was done; release the reservation so an empty tick does not
            // count toward the throttle window (#6119).
            self.restore_analysis_claim(claim).await;
            return Ok(AnalysisRunOutcome::NoInput);
        }

        let patterns = self.pattern_miner.detect(&events);
        let mut current = Self::build_current_activity(&events);
        // Inject live accessibility text from monitor loop
        current.accessibility_text = self.accessibility_text.read().await.clone();
        let metrics = Self::build_session_metrics(&events);

        // Retrieve relevant history via RAG if VectorRetriever is available
        let retriever_guard = self.vector_retriever.read().await;
        let relevant_history = if let Some(ref retriever) = *retriever_guard {
            match retriever
                .retrieve_for_context(
                    &current.app_name,
                    &current.window_title,
                    current.ocr_hint.as_deref(),
                )
                .await
            {
                Ok(results) => results
                    .into_iter()
                    .map(|r| RelevantHistoryEntry {
                        when: humanize_time_ago(r.timestamp),
                        summary: r.original_text,
                        similarity: r.similarity,
                    })
                    .collect(),
                Err(e) => {
                    debug!("RAG retrieval skipped: {e}");
                    vec![]
                }
            }
        } else {
            vec![]
        };
        // #5973: the vector_retriever read guard is only needed for the RAG
        // retrieval above. Release it now so it is NOT held across the LLM
        // network round-trip below — otherwise set_vector_retriever() (and all
        // concurrent analysis calls) would block for the full LLM duration on
        // the 1-second scheduler hot path.
        drop(retriever_guard);

        // Snapshot segment stats before any .await so the RwLock read guard is
        // not held across the LLM network round-trip (which would serialize all
        // concurrent analysis calls and risk long lock holds on the hot path).
        let seg_stats_snapshot = self.segment_stats.read().await.clone();
        let regime_hint = seg_stats_snapshot
            .as_ref()
            .and_then(|s| s.regime_label.as_deref());

        // Clone the few-shot storage Arc out of the RwLock and release the guard
        // BEFORE awaiting: the underlying SqliteStorage call is synchronous and
        // takes a blocking read lock + runs a UNION-ALL query, so it must run on
        // a blocking thread (spawn_blocking) rather than the tokio worker. Holding
        // the RwLock guard across the await would also serialize set_few_shot_storage()
        // and all concurrent analysis on the 1-second scheduler hot path.
        let few_shot_storage = self.few_shot_storage.read().await.clone();
        let few_shot_examples = if let Some(fs_storage) = few_shot_storage {
            let history =
                tokio::task::spawn_blocking(move || fs_storage.get_suggestions_with_feedback(10))
                    .await;
            match history {
                Ok(Ok(history)) => self.few_shot_selector.select(&history, regime_hint),
                Ok(Err(e)) => {
                    debug!("Few-shot history retrieval failed: {e}");
                    vec![]
                }
                Err(e) => {
                    debug!("Few-shot history task join failed: {e}");
                    vec![]
                }
            }
        } else {
            vec![]
        };

        let ctx = if few_shot_examples.is_empty() {
            self.context_assembler.build_with_history(
                &current,
                &events,
                &patterns,
                &metrics,
                seg_stats_snapshot.as_ref(),
                &relevant_history,
            )
        } else {
            self.context_assembler.build_with_few_shot(
                &current,
                &events,
                &patterns,
                &metrics,
                seg_stats_snapshot.as_ref(),
                &relevant_history,
                &few_shot_examples,
                regime_hint,
            )
        };

        let suggestions = match self
            .analysis_provider
            .analyze(&ctx.user_context_json, &ctx.system_prompt)
            .await
        {
            Ok(suggestions) => suggestions,
            Err(e) => {
                // Restore the prior claim so a failed LLM run can be retried on
                // the next tick rather than being throttled out (#6119).
                self.restore_analysis_claim(claim).await;
                return Err(e.into());
            }
        };

        let filtered = self.filter_suggestions(Self::attach_context_scope(suggestions, &current));

        // Commit the completed run: advance the throttle window to now and
        // release the in-flight reservation (#6119, #7079).
        self.complete_analysis_claim(claim).await;

        // Update patterns hash
        let hash = Self::compute_patterns_hash(&patterns);
        let mut last_hash = self.last_patterns_hash.lock().await;
        *last_hash = hash;

        debug!(
            suggestion_count = filtered.len(),
            "Analysis cycle completed"
        );

        if filtered.is_empty() {
            Ok(AnalysisRunOutcome::NoCandidate)
        } else {
            Ok(AnalysisRunOutcome::Generated(filtered))
        }
    }

    /// Lightweight check: only call full analysis if patterns changed.
    pub async fn analyze_if_changed(&self) -> Result<Vec<Suggestion>, AnalysisError> {
        self.analyze_if_changed_with_outcome()
            .await
            .map(AnalysisRunOutcome::into_suggestions)
    }

    /// Change-detected analysis with an explicit metadata-only outcome.
    pub async fn analyze_if_changed_with_outcome(
        &self,
    ) -> Result<AnalysisRunOutcome, AnalysisError> {
        let now = Utc::now();
        let lookback = Duration::seconds(self.config.interval_secs as i64);
        let from = now - lookback;

        let events = self
            .storage
            .get_events(from, now, CHANGE_DETECTION_EVENT_LIMIT)
            .await?;

        let patterns = self.pattern_miner.detect(&events);
        let new_hash = Self::compute_patterns_hash(&patterns);

        let old_hash = {
            let guard = self.last_patterns_hash.lock().await;
            *guard
        };

        if new_hash != old_hash && new_hash != 0 {
            debug!(
                old_hash = old_hash,
                new_hash = new_hash,
                "Patterns changed — triggering full analysis"
            );
            return self.analyze_with_outcome().await;
        }

        debug!("Patterns unchanged — skipping analysis");
        Ok(AnalysisRunOutcome::Unchanged)
    }

    /// Triggered by a significant event (e.g., major app switch).
    pub async fn on_significant_event(
        &self,
        app_name: &str,
        window_title: &str,
        ocr_text: Option<&str>,
    ) -> Result<Vec<Suggestion>, AnalysisError> {
        self.on_significant_event_with_outcome(app_name, window_title, ocr_text)
            .await
            .map(AnalysisRunOutcome::into_suggestions)
    }

    /// Event-driven analysis with an explicit metadata-only outcome.
    pub async fn on_significant_event_with_outcome(
        &self,
        app_name: &str,
        window_title: &str,
        ocr_text: Option<&str>,
    ) -> Result<AnalysisRunOutcome, AnalysisError> {
        self.on_significant_event_with_capture_origin(app_name, window_title, ocr_text, None)
            .await
    }

    /// Analyze capture-derived OCR using its original, revocable permit.
    /// Runtime callers must carry the permit acquired with the original capture.
    /// This does not authorize provider policy or recall requests already sent.
    pub async fn on_significant_event_with_original_capture_outcome(
        &self,
        app_name: &str,
        window_title: &str,
        ocr_text: Option<&str>,
        original: &CapturePermit,
    ) -> Result<AnalysisRunOutcome, AnalysisError> {
        self.on_significant_event_with_capture_origin(
            app_name,
            window_title,
            ocr_text,
            Some(original),
        )
        .await
    }

    async fn on_significant_event_with_capture_origin(
        &self,
        app_name: &str,
        window_title: &str,
        ocr_text: Option<&str>,
        origin: Option<&CapturePermit>,
    ) -> Result<AnalysisRunOutcome, AnalysisError> {
        // Claim-then-run throttle (see analyze()): reserve the in-flight slot
        // before the LLM round-trip so a concurrent caller cannot run a
        // duplicate analysis on the shared ContextAnalyzer, independent of
        // `throttle_secs` (#6119, #7079).
        let reservation_owner = Arc::new(());
        let claim = match self
            .claim_analysis_slot(Arc::downgrade(&reservation_owner))
            .await
        {
            Some(claim) => claim,
            None => return Ok(AnalysisRunOutcome::Throttled),
        };

        self.recheck_capture_origin(claim, origin).await?;

        let now = Utc::now();
        let from = now - Duration::minutes(SIGNIFICANT_EVENT_LOOKBACK_MINS);

        let events = match self
            .storage
            .get_events(from, now, SIGNIFICANT_EVENT_LIMIT)
            .await
        {
            Ok(events) => events,
            Err(e) => {
                self.restore_analysis_claim(claim).await;
                return Err(e.into());
            }
        };

        self.recheck_capture_origin(claim, origin).await?;
        let patterns = self.pattern_miner.detect(&events);
        let metrics = Self::build_session_metrics(&events);

        let current = CurrentActivity {
            app_name: app_name.to_string(),
            window_title: window_title.to_string(),
            ocr_hint: ocr_text.map(String::from),
            focus_score: SIGNIFICANT_EVENT_FOCUS_SCORE,
            deep_work_mins: 0,
            accessibility_text: None,
        };

        // Few-shot enrichment is intentionally skipped for event-driven analysis.
        // Event-triggered analysis is latency-sensitive; the periodic analyze()
        // path handles personalized prompts via build_with_few_shot().
        //
        // Snapshot segment stats before the LLM .await so the RwLock read guard
        // is not held across the network round-trip.
        let seg_stats_snapshot = self.segment_stats.read().await.clone();
        self.recheck_capture_origin(claim, origin).await?;
        let ctx = self.context_assembler.build_with_segment(
            &current,
            &events,
            &patterns,
            &metrics,
            seg_stats_snapshot.as_ref(),
        );

        self.recheck_capture_origin(claim, origin).await?;
        let suggestions = match self
            .analysis_provider
            .analyze(&ctx.user_context_json, &ctx.system_prompt)
            .await
        {
            Ok(suggestions) => suggestions,
            Err(e) => {
                self.restore_analysis_claim(claim).await;
                return Err(e.into());
            }
        };

        self.recheck_capture_origin(claim, origin).await?;
        let filtered = self.filter_suggestions(Self::attach_context_scope(suggestions, &current));

        // Commit completion: advance the throttle window and release the
        // in-flight reservation (slot already claimed at entry) (#6119, #7079).
        self.complete_analysis_claim(claim).await;
        // Completion waits for the throttle lock; never return stale capture data.
        self.recheck_capture_origin(claim, origin).await?;

        if filtered.is_empty() {
            Ok(AnalysisRunOutcome::NoCandidate)
        } else {
            Ok(AnalysisRunOutcome::Generated(filtered))
        }
    }

    async fn recheck_capture_origin(
        &self,
        claim: AnalysisClaim,
        origin: Option<&CapturePermit>,
    ) -> Result<(), AnalysisError> {
        if let Some(origin) = origin {
            if let Err(error) = origin
                .check(CaptureAccess::Retain)
                .and_then(|()| origin.check(CaptureAccess::Ocr))
            {
                self.restore_analysis_claim(claim).await;
                return Err(error.into());
            }
        }
        Ok(())
    }

    /// Atomically enforce the in-flight + throttle gates AND reserve the slot.
    ///
    /// Two independent gates are checked under one lock:
    ///
    /// 1. **In-flight dedup (#7079)** — while a run holds the slot, a second
    ///    concurrent caller is rejected outright, *independent of*
    ///    `throttle_secs`. This is the key decoupling: a sub-latency throttle
    ///    can no longer let a duplicate concurrent analysis slip through, and a
    ///    completing run only resets state it actually owns (via the generation
    ///    token below). A watchdog (`ANALYSIS_INFLIGHT_WATCHDOG_SECS`) reclaims a
    ///    slot whose owner appears to have died (e.g. its task was force-aborted
    ///    at shutdown) so analysis cannot wedge permanently.
    /// 2. **Throttle window (#6119)** — enforces the minimum temporal spacing
    ///    between successive committed runs.
    ///
    /// On success this marks the slot in-flight, advances the generation, writes
    /// the reservation timestamp, and returns an `AnalysisClaim` carrying the
    /// prior timestamp (to restore on failure) and the owned generation. A
    /// caller that fails either gate returns `None`.
    async fn claim_analysis_slot(&self, owner: Weak<()>) -> Option<AnalysisClaim> {
        let now = Utc::now();
        let mut state = self.analysis_claim.lock().await;

        // Reconcile an abandoned future before either admission gate. Restoring
        // its exact prior timestamp preserves the normal completed-run throttle.
        if let Some(reservation) = state.reservation.as_ref() {
            if state.in_flight
                && reservation.claim.generation == state.generation
                && reservation.owner.strong_count() == 0
            {
                state.last_analysis_at = reservation.claim.prev;
                state.in_flight = false;
                state.reservation = None;
            }
        }

        // Gate 1: in-flight dedup, decoupled from throttle_secs (#7079).
        if state.in_flight {
            let in_flight_secs = state
                .last_analysis_at
                .map(|t| (now - t).num_seconds().max(0) as u64)
                .unwrap_or(0);
            // A live run still owns the slot — block the duplicate. Only a slot
            // abandoned past the watchdog falls through to be reclaimed.
            if in_flight_secs < ANALYSIS_INFLIGHT_WATCHDOG_SECS {
                return None;
            }
        }

        // Gate 2: throttle window since the last committed (or reserved) run.
        let may_proceed = match state.last_analysis_at {
            None => true,
            Some(last) => {
                // Clamp a backward clock step to 0 before the unsigned cast (review4
                // F12 sibling): a negative i64 delta cast `as u64` wraps to ~1.8e19
                // and would bypass the throttle, letting analysis run when it should
                // be suppressed.
                let elapsed = (now - last).num_seconds().max(0) as u64;
                elapsed >= self.config.throttle_secs
            }
        };
        if !may_proceed {
            return None;
        }

        let prev = state.last_analysis_at;
        state.generation = state.generation.wrapping_add(1);
        state.in_flight = true;
        state.last_analysis_at = Some(now);
        let claim = AnalysisClaim {
            prev,
            generation: state.generation,
        };
        state.reservation = Some(AnalysisReservation { claim, owner });
        Some(claim)
    }

    /// Restore the throttle timestamp to its pre-claim value and release the
    /// in-flight reservation.
    ///
    /// Called on an error or no-op path so a failed/empty run does not consume
    /// the throttle window, letting the next tick retry immediately (#6119). The
    /// generation guard ensures a run only mutates state it actually owns: if a
    /// newer claim has since taken the slot (e.g. after a watchdog reclaim), this
    /// is a no-op and cannot clobber the live reservation (#7079).
    async fn restore_analysis_claim(&self, claim: AnalysisClaim) {
        let mut state = self.analysis_claim.lock().await;
        if state.generation == claim.generation {
            state.last_analysis_at = claim.prev;
            state.in_flight = false;
            state.reservation = None;
        }
    }

    /// Commit a completed run: advance the throttle window to now and release
    /// the in-flight reservation.
    ///
    /// Gated on the owning generation (#7079) so a run reclaimed by the watchdog
    /// cannot overwrite the new owner's reservation on a late completion.
    async fn complete_analysis_claim(&self, claim: AnalysisClaim) {
        let mut state = self.analysis_claim.lock().await;
        if state.generation == claim.generation {
            state.last_analysis_at = Some(Utc::now());
            state.in_flight = false;
            state.reservation = None;
        }
    }

    /// Filter suggestions by min_confidence and cap at max_suggestions.
    fn filter_suggestions(&self, suggestions: Vec<Suggestion>) -> Vec<Suggestion> {
        let min_conf = self.config.min_confidence;
        let max = self.config.max_suggestions;

        suggestions
            .into_iter()
            .filter(|s| {
                if s.confidence_score < min_conf {
                    debug!(
                        confidence = s.confidence_score,
                        min = min_conf,
                        "Dropping low-confidence suggestion"
                    );
                    false
                } else {
                    true
                }
            })
            .take(max)
            .collect()
    }

    /// Build CurrentActivity from the most recent context event.
    fn build_current_activity(events: &[Event]) -> CurrentActivity {
        // #6893: the get_events contract returns timestamp DESC (newest-first), so `.rev().find_map`
        // picked the oldest context as "current". Select the context with the largest (= newest)
        // timestamp regardless of ordering.
        let last_ctx = events
            .iter()
            .filter_map(|e| match e {
                Event::Context(ctx) => Some(ctx),
                _ => None,
            })
            .max_by_key(|ctx| ctx.timestamp);

        match last_ctx {
            Some(ctx) => CurrentActivity {
                app_name: ctx.app_name.clone(),
                window_title: ctx.window_title.clone(),
                ocr_hint: None,
                focus_score: DEFAULT_FOCUS_SCORE,
                deep_work_mins: 0,
                accessibility_text: None,
            },
            None => CurrentActivity {
                app_name: "Unknown".to_string(),
                window_title: "Unknown".to_string(),
                ocr_hint: None,
                focus_score: 0.0,
                deep_work_mins: 0,
                accessibility_text: None,
            },
        }
    }

    /// Build SessionMetrics from event analysis.
    fn build_session_metrics(events: &[Event]) -> SessionMetrics {
        let ctx_events: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                Event::Context(ctx) => Some(ctx),
                _ => None,
            })
            .collect();

        let total_work_mins = if ctx_events.len() >= 2 {
            // #6893: compute session length as an ordering-independent [min, max] timestamp span.
            // The get_events contract is DESC (newest-first), so first=newest, last=oldest; the
            // direct (last - first) difference is then negative and `num_minutes() as u32` two's-
            // complement wraps to ~4.29e9 minutes (.max(1) is useless since it runs after the cast).
            // Derive the span from earliest/latest and clamp with .max(0) before the cast.
            let Some(earliest) = ctx_events.iter().map(|ctx| ctx.timestamp).min() else {
                panic!("len >= 2");
            };
            let Some(latest) = ctx_events.iter().map(|ctx| ctx.timestamp).max() else {
                panic!("len >= 2");
            };
            ((latest - earliest).num_minutes().max(0) as u32).max(1)
        } else {
            0
        };

        // Count context switches (app changes)
        let context_switches = ctx_events
            .windows(2)
            .filter(|pair| pair[0].app_name != pair[1].app_name)
            .count() as u32;

        // Estimate communication ratio using shared app classification
        let comm_count = ctx_events
            .iter()
            .filter(|ctx| is_communication_app(&ctx.app_name.to_lowercase()))
            .count();

        let communication_ratio = if ctx_events.is_empty() {
            0.0
        } else {
            comm_count as f32 / ctx_events.len() as f32
        };

        SessionMetrics {
            total_work_mins,
            context_switches,
            communication_ratio,
        }
    }

    /// Compute a simple hash of detected patterns for change detection.
    fn compute_patterns_hash(patterns: &[maekon_core::models::analysis::ActivityPattern]) -> u64 {
        let mut hasher = DefaultHasher::new();
        for p in patterns {
            p.description.hash(&mut hasher);
            p.frequency.hash(&mut hasher);
        }
        hasher.finish()
    }

    fn attach_context_scope(
        mut suggestions: Vec<Suggestion>,
        current: &CurrentActivity,
    ) -> Vec<Suggestion> {
        let app_name = normalized_optional_scope(&current.app_name);
        let window_title = normalized_optional_scope(&current.window_title);

        for suggestion in &mut suggestions {
            let scope = suggestion
                .context_scope
                .get_or_insert_with(SuggestionContextScope::default);
            if scope.app_name.is_none() {
                scope.app_name = app_name.clone();
            }
            if scope.window_title.is_none() {
                scope.window_title = window_title.clone();
            }
        }

        suggestions
    }
}

fn normalized_optional_scope(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests;
