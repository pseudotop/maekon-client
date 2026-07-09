use chrono::{DateTime, Utc};
use maekon_core::models::coaching::{CoachingEffectivenessRecord, FeedbackSignal};
use std::collections::HashMap;

use crate::coaching_engine::tunable_params::TunableParams;

/// Aggregated effectiveness score for a (profile, trigger) pair.
#[derive(Debug, Clone)]
pub struct EffectivenessScore {
    pub total_shown: u32,
    pub positive_signals: f32,
    pub negative_signals: f32,
    pub neutral_count: u32,
}

impl EffectivenessScore {
    fn new() -> Self {
        Self {
            total_shown: 0,
            positive_signals: 0.0,
            negative_signals: 0.0,
            neutral_count: 0,
        }
    }

    /// Effectiveness ratio: positive / (positive + negative + neutral).
    /// Returns 0.5 when no data is available (neutral default).
    pub fn ratio(&self) -> f32 {
        let total = self.positive_signals + self.negative_signals + self.neutral_count as f32;
        if total == 0.0 {
            0.5
        } else {
            self.positive_signals / total
        }
    }
}

/// A coaching message pending implicit evaluation.
#[derive(Debug, Clone)]
struct PendingEvaluation {
    shown_at: DateTime<Utc>,
    profile: String,
    trigger: String,
    regime_at_shown: Option<String>,
    app_at_shown: String,
}

/// Tracks implicit (5-minute behavior window) and explicit (thumbs-up/down)
/// feedback to adaptively reduce coaching frequency for low-effectiveness triggers.
///
/// `should_show()` is intentionally synchronous — it is designed to be called
/// under a `RwLock` read guard without requiring async.
pub struct FeedbackTracker {
    /// (profile, trigger) -> aggregated effectiveness score.
    scores: HashMap<(String, String), EffectivenessScore>,
    /// message_id -> pending implicit evaluation.
    pending: HashMap<String, PendingEvaluation>,
    /// Counter for gating pattern (deterministic round-robin).
    gate_counter: u32,
    /// Auto-tunable parameters — adjusted by feedback.
    pub params: TunableParams,
}

impl FeedbackTracker {
    pub fn new() -> Self {
        Self {
            scores: HashMap::new(),
            pending: HashMap::new(),
            gate_counter: 0,
            params: TunableParams::default(),
        }
    }

    /// Register a coaching message for feedback tracking.
    /// Called immediately after a coaching message is shown.
    pub fn register_pending(
        &mut self,
        message_id: &str,
        profile: &str,
        trigger: &str,
        regime_id: Option<&str>,
        app_name: &str,
    ) {
        self.pending.insert(
            message_id.to_string(),
            PendingEvaluation {
                shown_at: Utc::now(),
                profile: profile.to_string(),
                trigger: trigger.to_string(),
                regime_at_shown: regime_id.map(String::from),
                app_at_shown: app_name.to_string(),
            },
        );

        // Increment total_shown for the (profile, trigger) pair
        let score = self
            .scores
            .entry((profile.to_string(), trigger.to_string()))
            .or_insert_with(EffectivenessScore::new);
        score.total_shown += 1;
    }

    /// Record explicit feedback (thumbs-up or thumbs-down).
    /// Removes from pending and updates score with tunable weight.
    ///
    /// Returns `true` when a pending message with this id existed and was
    /// resolved, `false` when the id was unknown (already resolved or never
    /// registered). The caller uses this to decide whether to also train the
    /// adaptive scorer (#7913) — an unknown id has no feature context to train
    /// on and must not be trained.
    pub fn record_explicit(&mut self, message_id: &str, positive: bool) -> bool {
        let weight = self.params.explicit_weight;
        if let Some(eval) = self.pending.remove(message_id) {
            let score = self
                .scores
                .entry((eval.profile, eval.trigger))
                .or_insert_with(EffectivenessScore::new);

            if positive {
                score.positive_signals += weight;
            } else {
                score.negative_signals += weight;
            }

            self.params.adjust_on_feedback(positive);
            true
        } else {
            false
        }
    }

    /// Evaluate all pending messages whose 5-minute window has elapsed.
    /// Classifies behavior change and updates effectiveness scores.
    ///
    /// Returns the `(message_id, positive)` outcomes that carry a directional
    /// training signal (#7913): `ImplicitPositive` → `(id, true)`,
    /// `ImplicitNegative` → `(id, false)`. `ImplicitNeutral` is deliberately
    /// omitted — a "no observable behavior change" outcome is not evidence
    /// either way, so training the adaptive scorer on it (as 0.0 or 1.0) would
    /// inject noise. The caller trains the adaptive scorer on each returned
    /// outcome using the features cached for that specific message id.
    pub fn evaluate_implicit(
        &mut self,
        current_regime_id: Option<&str>,
        current_app: &str,
        now: DateTime<Utc>,
    ) -> Vec<(String, bool)> {
        // Collect message IDs ready for evaluation
        let window = self.params.implicit_window_secs;
        let ready_ids: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, eval)| (now - eval.shown_at).num_seconds() >= window)
            .map(|(id, _)| id.clone())
            .collect();

        let mut training_signals = Vec::new();
        for id in ready_ids {
            if let Some(eval) = self.pending.remove(&id) {
                let signal = Self::classify_behavior_change(&eval, current_regime_id, current_app);

                let score = self
                    .scores
                    .entry((eval.profile, eval.trigger))
                    .or_insert_with(EffectivenessScore::new);

                match signal {
                    FeedbackSignal::ImplicitPositive => {
                        score.positive_signals += 1.0;
                        training_signals.push((id, true));
                    }
                    FeedbackSignal::ImplicitNegative => {
                        score.negative_signals += 1.0;
                        training_signals.push((id, false));
                    }
                    FeedbackSignal::ImplicitNeutral => {
                        score.neutral_count += 1;
                    }
                    // Explicit signals handled by record_explicit()
                    FeedbackSignal::ExplicitPositive | FeedbackSignal::ExplicitNegative => {}
                }
            }
        }
        training_signals
    }

    /// Determine whether a coaching message for this (profile, trigger) pair
    /// should be shown based on effectiveness gating.
    ///
    /// Returns `false` approximately 2-out-of-3 times when effectiveness is
    /// below the threshold AND enough data has been collected. This is
    /// intentionally synchronous (not async) — called under `RwLock` read guard.
    ///
    /// # Gating logic
    /// - Always returns `true` when no score data exists
    /// - Always returns `true` when `total_shown < 5`
    /// - When `ratio() < 0.2` and `total_shown >= 5`: allows 1-in-3 (round-robin)
    pub fn should_show(&mut self, profile: &str, trigger: &str) -> bool {
        let key = (profile.to_string(), trigger.to_string());
        let score = match self.scores.get(&key) {
            Some(s) => s,
            None => return true,
        };

        if score.total_shown < self.params.min_shown_for_gating {
            return true;
        }

        if score.ratio() < self.params.low_effectiveness_threshold {
            self.gate_counter += 1;
            // gate_allow_ratio determines pass frequency (e.g., 0.33 → ~1-in-3)
            let denominator = (1.0 / self.params.gate_allow_ratio).round() as u32;
            return denominator > 0 && self.gate_counter.is_multiple_of(denominator);
        }

        true
    }

    /// Classify behavior change between when the message was shown and now.
    ///
    /// Heuristic (from spec section 4.6):
    /// - If the regime changed after the coaching message -> ImplicitPositive
    ///   (user acted on the advice)
    /// - If the regime is the same and the app is the same -> ImplicitNeutral
    ///   (no observable change)
    /// - If the regime is the same but the app changed -> ImplicitNeutral
    ///   (ambiguous — could be positive or negative)
    fn classify_behavior_change(
        eval: &PendingEvaluation,
        current_regime_id: Option<&str>,
        current_app: &str,
    ) -> FeedbackSignal {
        let regime_changed = match (&eval.regime_at_shown, current_regime_id) {
            (Some(old), Some(new)) => old != new,
            (None, Some(_)) => true,
            (Some(_), None) => true,
            (None, None) => false,
        };

        if regime_changed {
            // User changed regime after coaching — likely acted on it
            FeedbackSignal::ImplicitPositive
        } else if eval.app_at_shown == current_app {
            // Same regime, same app — no observable change
            FeedbackSignal::ImplicitNeutral
        } else {
            // Same regime, different app — ambiguous
            FeedbackSignal::ImplicitNeutral
        }
    }

    /// Read-only accessor for persisting effectiveness scores to storage.
    pub fn get_effectiveness(&self, profile: &str, trigger: &str) -> Option<&EffectivenessScore> {
        self.scores.get(&(profile.to_string(), trigger.to_string()))
    }

    /// Number of messages pending implicit evaluation.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Snapshot every `(profile, trigger)` effectiveness score as a persistable
    /// record (#7913 T2.1b). `behavior_change_count` is always 0 — the tracker
    /// folds behavior change into `positive_signals`/`neutral_count` rather than
    /// counting it separately; the column exists in the V17 table for forward
    /// compatibility and round-trips faithfully.
    pub fn effectiveness_snapshot(&self) -> Vec<CoachingEffectivenessRecord> {
        self.scores
            .iter()
            .map(|((profile, trigger), score)| CoachingEffectivenessRecord {
                profile_name: profile.clone(),
                trigger_type: trigger.clone(),
                total_shown: score.total_shown,
                positive_feedback: score.positive_signals,
                negative_feedback: score.negative_signals,
                neutral_count: score.neutral_count,
                behavior_change_count: 0,
            })
            .collect()
    }

    /// Load persisted effectiveness records into the in-RAM scores on startup
    /// (#7913 T2.1b). Records overwrite any existing key; the `pending` set is
    /// intentionally left empty (in-flight 5-minute windows do not survive a
    /// restart — a message shown before exit can no longer be observed after it).
    pub fn hydrate_effectiveness(&mut self, records: Vec<CoachingEffectivenessRecord>) {
        for r in records {
            self.scores.insert(
                (r.profile_name, r.trigger_type),
                EffectivenessScore {
                    total_shown: r.total_shown,
                    positive_signals: r.positive_feedback,
                    negative_signals: r.negative_feedback,
                    neutral_count: r.neutral_count,
                },
            );
        }
    }
}

impl Default for FeedbackTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn explicit_positive_increases_score() {
        let mut tracker = FeedbackTracker::new();
        tracker.register_pending("msg-1", "FocusGuard", "RegimeTransition", None, "VS Code");
        tracker.record_explicit("msg-1", true);

        let score = tracker
            .get_effectiveness("FocusGuard", "RegimeTransition")
            .unwrap();
        assert_eq!(
            score.positive_signals,
            TunableParams::default().explicit_weight
        );
        assert_eq!(score.negative_signals, 0.0);
    }

    #[test]
    fn explicit_negative_increases_negative() {
        let mut tracker = FeedbackTracker::new();
        tracker.register_pending("msg-1", "TimeAware", "RegimeOverstay", None, "Chrome");
        tracker.record_explicit("msg-1", false);

        let score = tracker
            .get_effectiveness("TimeAware", "RegimeOverstay")
            .unwrap();
        assert_eq!(score.positive_signals, 0.0);
        assert_eq!(
            score.negative_signals,
            TunableParams::default().explicit_weight
        );
    }

    #[test]
    fn implicit_evaluation_after_5min() {
        let mut tracker = FeedbackTracker::new();
        tracker.register_pending(
            "msg-1",
            "DeepWorkCoach",
            "RegimeOverstay",
            Some("regime-a"),
            "VS Code",
        );
        assert_eq!(tracker.pending_count(), 1);

        // Evaluate with now + 301s (past the 5-min window)
        let future = Utc::now() + Duration::seconds(301);
        tracker.evaluate_implicit(Some("regime-b"), "VS Code", future);

        // Pending should be cleared
        assert_eq!(tracker.pending_count(), 0);

        // Score should be updated (regime changed -> ImplicitPositive)
        let score = tracker
            .get_effectiveness("DeepWorkCoach", "RegimeOverstay")
            .unwrap();
        assert_eq!(score.positive_signals, 1.0);
    }

    #[test]
    fn implicit_not_evaluated_before_5min() {
        let mut tracker = FeedbackTracker::new();
        tracker.register_pending(
            "msg-1",
            "FocusGuard",
            "RegimeDrift",
            Some("regime-a"),
            "VS Code",
        );

        // Evaluate with now + 200s (before the 5-min window)
        let early = Utc::now() + Duration::seconds(200);
        tracker.evaluate_implicit(Some("regime-b"), "Chrome", early);

        // Should still be pending
        assert_eq!(tracker.pending_count(), 1);
    }

    #[test]
    fn should_show_always_true_when_no_data() {
        let mut tracker = FeedbackTracker::new();
        assert!(tracker.should_show("Unknown", "Unknown"));
    }

    #[test]
    fn should_show_reduces_for_low_effectiveness() {
        let mut tracker = FeedbackTracker::new();

        // Register 6 events with all-negative explicit feedback
        for i in 0..6 {
            let id = format!("msg-{}", i);
            tracker.register_pending(&id, "BadProfile", "BadTrigger", None, "App");
            tracker.record_explicit(&id, false);
        }

        // Verify effectiveness is low
        let score = tracker
            .get_effectiveness("BadProfile", "BadTrigger")
            .unwrap();
        assert!(
            score.ratio() < TunableParams::default().low_effectiveness_threshold,
            "ratio should be below threshold: {}",
            score.ratio()
        );
        assert!(score.total_shown >= TunableParams::default().min_shown_for_gating);

        // With 1-in-3 gating, at least some calls should return false
        let mut false_count = 0;
        let mut true_count = 0;
        for _ in 0..9 {
            if tracker.should_show("BadProfile", "BadTrigger") {
                true_count += 1;
            } else {
                false_count += 1;
            }
        }
        assert!(
            false_count > 0,
            "should_show should return false sometimes for low effectiveness"
        );
        assert!(
            true_count > 0,
            "should_show should still allow 1-in-3 through"
        );
        // Auto-tuning adjusts gate_allow_ratio on negative feedback, so the
        // exact pass count varies. Verify gating is active (not all pass).
        assert!(
            true_count <= 4,
            "gating should suppress most messages, got {true_count}/9 passing"
        );
    }

    #[test]
    fn classify_regime_change_is_positive() {
        let eval = PendingEvaluation {
            shown_at: Utc::now(),
            profile: "FocusGuard".to_string(),
            trigger: "RegimeTransition".to_string(),
            regime_at_shown: Some("regime-a".to_string()),
            app_at_shown: "VS Code".to_string(),
        };

        let signal = FeedbackTracker::classify_behavior_change(&eval, Some("regime-b"), "VS Code");
        assert_eq!(signal, FeedbackSignal::ImplicitPositive);
    }

    #[test]
    fn classify_no_change_is_neutral() {
        let eval = PendingEvaluation {
            shown_at: Utc::now(),
            profile: "TimeAware".to_string(),
            trigger: "RegimeOverstay".to_string(),
            regime_at_shown: Some("regime-a".to_string()),
            app_at_shown: "VS Code".to_string(),
        };

        let signal = FeedbackTracker::classify_behavior_change(&eval, Some("regime-a"), "VS Code");
        assert_eq!(signal, FeedbackSignal::ImplicitNeutral);
    }

    #[test]
    fn should_show_returns_true_initially() {
        // Smoke test: construct FeedbackTracker and verify should_show()
        // returns true initially for any profile/trigger combination.
        let mut tracker = FeedbackTracker::new();
        assert!(
            tracker.should_show("AnyProfile", "AnyTrigger"),
            "should_show must return true with no prior data"
        );
    }

    /// #7913 T2.1b — effectiveness snapshot round-trips faithfully through
    /// hydrate, so a restart restores the learned score.
    #[test]
    fn effectiveness_snapshot_and_hydrate_roundtrip() {
        let mut tracker = FeedbackTracker::new();
        tracker.register_pending("m1", "FocusGuard", "RegimeTransition", None, "App");
        tracker.record_explicit("m1", true);

        let snap = tracker.effectiveness_snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].profile_name, "FocusGuard");
        assert_eq!(snap[0].total_shown, 1);

        // A fresh tracker hydrates the persisted snapshot and reads the same score.
        let mut fresh = FeedbackTracker::new();
        fresh.hydrate_effectiveness(snap);
        let restored = fresh
            .get_effectiveness("FocusGuard", "RegimeTransition")
            .expect("hydrated score must be present");
        assert_eq!(restored.total_shown, 1);
        assert_eq!(
            restored.positive_signals,
            TunableParams::default().explicit_weight
        );
    }
}
