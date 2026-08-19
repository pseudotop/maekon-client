//! CompositeFeedbackSink — routes user reactions to `RegimeClassifier`
//! (per-regime accept/reject/defer learning). Binary-crate composition glue
//! per ADR-017.
//!
//! # Why not `CoachingEngine` (#7600)
//!
//! An earlier revision also fanned out to `CoachingEngine::record_user_reaction`,
//! but that method was a permanent no-op: `CoachingEngine`'s real
//! feedback-learning primitive (`record_explicit_feedback` /
//! `evaluate_implicit_feedback`) is keyed by `(profile_name, trigger_name)` —
//! a reaction to a *coaching message* — and there is no correlation anywhere
//! in the codebase from a `SuggestionFeedback.suggestion_id` (a reaction to a
//! *suggestion* card) to a coaching profile/trigger. Keeping a wired-but-inert
//! `coaching` arm here implied a learning path that did not exist, so it was
//! removed rather than left as a stub. See
//! `maekon_analysis::CoachingEngine`'s module doc for the real coaching
//! feedback path.

use async_trait::async_trait;
use maekon_analysis::RegimeClassifier;
use maekon_core::error::CoreError;
use maekon_core::models::suggestion::SuggestionFeedback;
use maekon_core::ports::feedback_signal_sink::FeedbackSignalSink;
use maekon_core::ports::regime_reaction_store::RegimeReactionStore;
use std::sync::Arc;

pub struct CompositeFeedbackSink {
    regime_classifier: Option<Arc<parking_lot::Mutex<RegimeClassifier>>>,
    /// #7913 T2.1c: optional persistence for the per-regime reaction stats the
    /// classifier learns here. `None` keeps the reactions RAM-only (the pre-#7913
    /// behavior, and the unit tests). Write-through after each reaction; the
    /// matching load-on-start lives in `build_regime_wiring`.
    reaction_store: Option<Arc<dyn RegimeReactionStore>>,
}

impl CompositeFeedbackSink {
    pub fn new(regime_classifier: Option<Arc<parking_lot::Mutex<RegimeClassifier>>>) -> Self {
        Self {
            regime_classifier,
            reaction_store: None,
        }
    }

    /// Attach the reaction persistence port so learned per-regime accept/reject
    /// stats survive restart (#7913 T2.1c).
    pub fn with_reaction_store(mut self, store: Arc<dyn RegimeReactionStore>) -> Self {
        self.reaction_store = Some(store);
        self
    }
}

#[async_trait]
impl FeedbackSignalSink for CompositeFeedbackSink {
    async fn record_user_reaction(&self, feedback: &SuggestionFeedback) -> Result<(), CoreError> {
        if let Some(ref cls) = self.regime_classifier {
            // SAFETY: `RegimeClassifier::record_user_reaction` MUST stay
            // synchronous and fast (~10 ms budget, ADR-017). The
            // `parking_lot::Mutex` guard is held only for the (now
            // per-regime-weighted, #7600) counter update + snapshot and dropped
            // before any `.await` below (ADR-007). A future impl with heavier work
            // MUST offload to `tokio::spawn` rather than growing this section.
            let snapshot = {
                let mut guard = cls.lock();
                guard.record_user_reaction(feedback);
                // #7913 T2.1c: snapshot the touched buckets under the same guard so
                // the write-through persists a consistent view. Always the global
                // aggregate; plus the per-regime bucket when this reaction carried
                // a regime_id.
                let per_regime = feedback
                    .regime_id
                    .as_ref()
                    .and_then(|rid| guard.reaction_record(rid));
                let aggregate = guard.aggregate_record();
                (per_regime, aggregate)
            }; // parking_lot guard dropped here — safe to touch storage.

            // #7913 T2.1c: write-through the reaction stats (best-effort). Learned
            // reactions are advisory (they only quiet historically-rejected
            // regimes), so a persist failure is logged, never propagated — the
            // reaction has already been applied to the in-RAM classifier above.
            if let Some(ref store) = self.reaction_store {
                let (per_regime, aggregate) = snapshot;
                if let Some(rec) = per_regime {
                    if let Err(e) = store.upsert_regime_reaction(&rec) {
                        tracing::warn!(error = %e, regime_id = %rec.regime_id, "regime reaction persist failed (advisory)");
                    }
                }
                if let Err(e) = store.upsert_regime_reaction(&aggregate) {
                    tracing::warn!(error = %e, "regime reaction aggregate persist failed (advisory)");
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use maekon_core::models::suggestion::FeedbackType;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // --- Mock sink that counts calls ---
    struct CountingSink {
        calls: AtomicUsize,
    }

    impl CountingSink {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl FeedbackSignalSink for CountingSink {
        async fn record_user_reaction(
            &self,
            _feedback: &SuggestionFeedback,
        ) -> Result<(), CoreError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn sample_feedback(t: FeedbackType) -> SuggestionFeedback {
        sample_feedback_with_regime(t, None)
    }

    fn sample_feedback_with_regime(t: FeedbackType, regime_id: Option<&str>) -> SuggestionFeedback {
        SuggestionFeedback {
            suggestion_id: "sug_001".into(),
            feedback_type: t,
            timestamp: Utc::now(),
            comment: None,
            regime_id: regime_id.map(str::to_string),
        }
    }

    /// T-X3-5 — `CompositeFeedbackSink` routes to `RegimeClassifier` (#7600:
    /// no longer also routes to `CoachingEngine` — see module doc comment).
    ///
    /// Exercises the real production `RegimeClassifier` end-to-end: fires
    /// the sink 3 times with a regime_id (2 accepted, 1 rejected), then reads
    /// the classifier's own per-regime state directly — proving the
    /// production consumer path (`CompositeFeedbackSink` ->
    /// `RegimeClassifier::record_user_reaction` -> `per_regime_stats` /
    /// `acceptance_rate`) actually updates, not just that a debug log fires.
    #[tokio::test]
    async fn composite_sink_routes_to_regime_classifier() {
        let regime = Arc::new(parking_lot::Mutex::new(
            maekon_analysis::RegimeClassifier::new(1.5),
        ));
        let sink = CompositeFeedbackSink::new(Some(regime.clone()));

        sink.record_user_reaction(&sample_feedback_with_regime(
            FeedbackType::Accepted,
            Some("regime-focus"),
        ))
        .await
        .unwrap();
        sink.record_user_reaction(&sample_feedback_with_regime(
            FeedbackType::Accepted,
            Some("regime-focus"),
        ))
        .await
        .unwrap();
        sink.record_user_reaction(&sample_feedback_with_regime(
            FeedbackType::Rejected,
            Some("regime-focus"),
        ))
        .await
        .unwrap();

        let classifier = regime.lock();
        let stats = classifier
            .per_regime_stats()
            .get("regime-focus")
            .expect("regime-focus bucket populated by the sink");
        assert_eq!(stats.total, 3);
        assert_eq!(stats.accepted, 2);
        assert_eq!(stats.rejected, 1);

        let rate = classifier
            .acceptance_rate("regime-focus")
            .expect("3 samples meets the minimum for a rate");
        assert!((rate - (2.0 / 3.0)).abs() < f64::EPSILON, "got {rate}");
    }

    /// #7913 T2.1c — the sink WRITES THROUGH each reaction to the persistence
    /// port: a reaction with a regime_id upserts both the per-regime bucket AND
    /// the aggregate; a reaction with no regime_id upserts only the aggregate.
    #[tokio::test]
    async fn sink_writes_reactions_through_to_store() {
        use maekon_core::models::tiered_memory::RegimeReactionRecord;
        use maekon_core::ports::regime_reaction_store::RegimeReactionStore;

        #[derive(Default)]
        struct RecordingStore {
            upserts: std::sync::Mutex<Vec<RegimeReactionRecord>>,
        }
        impl RegimeReactionStore for RecordingStore {
            fn upsert_regime_reaction(
                &self,
                record: &RegimeReactionRecord,
            ) -> Result<(), CoreError> {
                self.upserts.lock().unwrap().push(record.clone());
                Ok(())
            }
            fn load_regime_reactions(&self) -> Result<Vec<RegimeReactionRecord>, CoreError> {
                Ok(vec![])
            }
        }

        let regime = Arc::new(parking_lot::Mutex::new(
            maekon_analysis::RegimeClassifier::new(1.5),
        ));
        let store = Arc::new(RecordingStore::default());
        let sink = CompositeFeedbackSink::new(Some(regime)).with_reaction_store(store.clone());

        // Reaction WITH a regime_id → per-regime + aggregate upserts.
        sink.record_user_reaction(&sample_feedback_with_regime(
            FeedbackType::Accepted,
            Some("regime-focus"),
        ))
        .await
        .unwrap();

        {
            let ups = store.upserts.lock().unwrap();
            assert_eq!(
                ups.len(),
                2,
                "regime reaction must upsert per-regime + aggregate"
            );
            assert!(ups
                .iter()
                .any(|r| r.regime_id == "regime-focus" && r.accepted == 1));
            assert!(ups
                .iter()
                .any(|r| r.regime_id == RegimeReactionRecord::AGGREGATE_KEY && r.accepted == 1));
        }

        // Reaction WITHOUT a regime_id → only the aggregate is upserted.
        sink.record_user_reaction(&sample_feedback(FeedbackType::Rejected))
            .await
            .unwrap();
        let ups = store.upserts.lock().unwrap();
        // 2 from before + 1 aggregate now.
        assert_eq!(ups.len(), 3);
        let last = ups.last().unwrap();
        assert_eq!(last.regime_id, RegimeReactionRecord::AGGREGATE_KEY);
        assert_eq!(last.total, 2, "aggregate now counts both reactions");
    }

    /// T-X3-1 — accept / reject / defer each land exactly once.
    #[tokio::test]
    async fn sink_receives_accept_reject_defer() {
        let sink = CountingSink::new();
        sink.record_user_reaction(&sample_feedback(FeedbackType::Accepted))
            .await
            .unwrap();
        sink.record_user_reaction(&sample_feedback(FeedbackType::Rejected))
            .await
            .unwrap();
        sink.record_user_reaction(&sample_feedback(FeedbackType::Deferred))
            .await
            .unwrap();
        assert_eq!(sink.calls.load(Ordering::SeqCst), 3);
    }

    /// T-X3-2 — `CompositeFeedbackSink` with no consumer is a happy-path no-op.
    ///
    /// The spec property "sink error does NOT fail send_feedback" lives on
    /// `FeedbackSender::send_feedback` (asserted implicitly by the
    /// `sink_fires_before_api_client` test in maekon-suggestion — when the
    /// sink returns Err, send_feedback still proceeds to the API call and
    /// returns Ok). Here we just guard the `None` (no `RegimeClassifier`
    /// wired) path from a silent regression.
    #[tokio::test]
    async fn composite_sink_ok_with_no_consumers() {
        let sink = CompositeFeedbackSink::new(None);
        // The Option field is None, so record_user_reaction skips the
        // if-let branch and reaches the unconditional `Ok(())`.
        // The spec requires that a missing sink never fails send_feedback
        // (ADR-017).  There is no value to inspect beyond the Ok — the
        // result is always the unit type.
        // (#5594: ok-only IS the full contract — no consumer means pure no-op)
        sink.record_user_reaction(&sample_feedback(FeedbackType::Accepted))
            .await
            .expect("CompositeFeedbackSink with no consumers must be a no-op Ok");
    }

    // T-X3-3 — no sink configured on FeedbackSender still works.
    // Exercised by existing crates/maekon-suggestion/src/feedback.rs tests
    // (accept_feedback / reject_feedback_with_comment / defer_feedback) which
    // construct `FeedbackSender::new(api)` — that shim calls
    // `new_with_sink(api, None)`. Their passing IS the regression guard.

    // T-X3-4 — sink is invoked BEFORE the server ApiClient call.
    // Property lives in FeedbackSender::send_feedback (maekon-suggestion),
    // not in CompositeFeedbackSink. Asserted by the
    // `sink_fires_before_api_client` test added in Task 3.
}
