//! Nearest-centroid regime classifier for real-time activity classification.
//!
//! Given a set of known regimes and a current feature vector, finds the
//! closest active regime within a configurable distance threshold.

use std::collections::HashMap;

use maekon_core::models::suggestion::FeedbackType;
use maekon_core::models::tiered_memory::{
    euclidean_distance, Regime, RegimeFeatures, RegimeReactionRecord, RegimeStatus,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserReactionStats {
    pub total: u64,
    pub accepted: u64,
    pub rejected: u64,
    pub deferred: u64,
}

/// Minimum recorded reactions a regime must accumulate before
/// `RegimeClassifier::acceptance_rate` returns a value instead of `None`
/// (#7600). Below this floor a single early accept/reject would swing the
/// ratio straight to 0.0 or 1.0 and drive downstream gating off a
/// statistically meaningless sample.
const MIN_SAMPLES_FOR_RATE: u64 = 3;

/// Classifies activity feature vectors into the nearest known regime.
///
/// Uses Euclidean distance with a maximum threshold — features that are
/// too far from all known centroids return `None` (fallback to defaults).
pub struct RegimeClassifier {
    regimes: Vec<Regime>,
    max_distance_threshold: f32,
    reaction_stats: UserReactionStats,
    /// Per-regime reaction stats, keyed by `regime_id` (#7600). Populated
    /// only for feedback that carries a `regime_id` (see
    /// `SuggestionFeedback::regime_id`); feedback with no regime context
    /// still updates the aggregate `reaction_stats` above but has no
    /// per-regime bucket to attribute to. `acceptance_rate` reads this map.
    per_regime_stats: HashMap<String, UserReactionStats>,
}

impl RegimeClassifier {
    /// Create a classifier with the given distance threshold.
    /// A typical default is 1.5.
    pub fn new(max_distance_threshold: f32) -> Self {
        Self {
            regimes: vec![],
            max_distance_threshold,
            reaction_stats: UserReactionStats::default(),
            per_regime_stats: HashMap::new(),
        }
    }

    /// Replace the set of known regimes.
    pub fn update_regimes(&mut self, regimes: Vec<Regime>) {
        self.regimes = regimes;
    }

    /// Return a reference to the current regimes.
    pub fn regimes(&self) -> &[Regime] {
        &self.regimes
    }

    pub fn reaction_stats(&self) -> &UserReactionStats {
        &self.reaction_stats
    }

    /// Per-regime reaction stats, keyed by `regime_id` (#7600). Exposed for
    /// tests and diagnostics; production consumers should prefer
    /// `acceptance_rate`, which applies the minimum-sample floor.
    pub fn per_regime_stats(&self) -> &HashMap<String, UserReactionStats> {
        &self.per_regime_stats
    }

    /// Learned accept-rate for `regime_id`: the fraction of past reactions
    /// recorded while the user was in this regime that were `Accepted`
    /// (#7600).
    ///
    /// Returns `None` when the regime has fewer than `MIN_SAMPLES_FOR_RATE`
    /// recorded reactions — including regimes that have never been observed.
    /// This is the production reader for the per-regime suggestion-learning
    /// loop: callers use it to make regimes the user has historically
    /// rejected suggestions in progressively quieter. See
    /// `maekon_analysis::apply_regime_acceptance_gate` for the wired
    /// consumer (invoked from the event-driven suggestion pipeline in
    /// `src-tauri`).
    pub fn acceptance_rate(&self, regime_id: &str) -> Option<f64> {
        let stats = self.per_regime_stats.get(regime_id)?;
        if stats.total < MIN_SAMPLES_FOR_RATE {
            return None;
        }
        Some(stats.accepted as f64 / stats.total as f64)
    }

    /// Classify current activity features into the nearest active regime.
    ///
    /// Returns `None` if no active regime is within `max_distance_threshold`.
    pub fn classify(&self, features: &RegimeFeatures) -> Option<&Regime> {
        self.regimes
            .iter()
            .filter(|r| r.status == RegimeStatus::Active)
            .min_by(|a, b| {
                euclidean_distance(&a.centroid, features)
                    .partial_cmp(&euclidean_distance(&b.centroid, features))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .filter(|r| euclidean_distance(&r.centroid, features) < self.max_distance_threshold)
    }

    /// Record a user reaction to a suggestion (#7600).
    ///
    /// Always updates the global aggregate `reaction_stats`. When
    /// `feedback.regime_id` is `Some`, ALSO updates the per-regime bucket in
    /// `per_regime_stats` keyed by that regime id — this is what makes
    /// `acceptance_rate` a real, non-write-only signal instead of dead state.
    /// Feedback with no regime context (older callers, or a tick where no
    /// regime was classified yet) still counts toward the aggregate but has
    /// no per-regime bucket to attribute to.
    pub fn record_user_reaction(
        &mut self,
        feedback: &maekon_core::models::suggestion::SuggestionFeedback,
    ) {
        Self::bump(&mut self.reaction_stats, &feedback.feedback_type);

        if let Some(ref regime_id) = feedback.regime_id {
            let entry = self.per_regime_stats.entry(regime_id.clone()).or_default();
            Self::bump(entry, &feedback.feedback_type);
        }

        tracing::debug!(
            suggestion_id = %feedback.suggestion_id,
            feedback_type = ?feedback.feedback_type,
            regime_id = feedback.regime_id.as_deref().unwrap_or("none"),
            "regime_classifier: user reaction recorded"
        );
    }

    // ── #7913 T2.1c: restart-surviving persistence of reaction stats ──

    /// Snapshot the per-regime bucket for `regime_id` as a persistable record for
    /// write-through after `record_user_reaction` (#7913 T2.1c). `None` when the
    /// regime has no recorded reactions yet.
    pub fn reaction_record(&self, regime_id: &str) -> Option<RegimeReactionRecord> {
        let stats = self.per_regime_stats.get(regime_id)?;
        Some(Self::to_record(regime_id, stats))
    }

    /// Snapshot the GLOBAL aggregate (all reactions, including those with no
    /// regime context) under the `AGGREGATE_KEY` sentinel (#7913 T2.1c).
    pub fn aggregate_record(&self) -> RegimeReactionRecord {
        Self::to_record(RegimeReactionRecord::AGGREGATE_KEY, &self.reaction_stats)
    }

    /// Snapshot every bucket — the aggregate plus one record per regime (#7913
    /// T2.1c).
    pub fn reaction_snapshot(&self) -> Vec<RegimeReactionRecord> {
        let mut out = Vec::with_capacity(self.per_regime_stats.len() + 1);
        out.push(self.aggregate_record());
        for (regime_id, stats) in &self.per_regime_stats {
            out.push(Self::to_record(regime_id, stats));
        }
        out
    }

    /// Load persisted reaction records on startup (#7913 T2.1c): the
    /// `AGGREGATE_KEY` sentinel restores the global aggregate; every other record
    /// restores a per-regime bucket. Fail-safe by construction — an empty input
    /// leaves the classifier exactly as `new` produced it.
    pub fn hydrate_reactions(&mut self, records: Vec<RegimeReactionRecord>) {
        for r in records {
            let stats = UserReactionStats {
                total: r.total,
                accepted: r.accepted,
                rejected: r.rejected,
                deferred: r.deferred,
            };
            if r.regime_id == RegimeReactionRecord::AGGREGATE_KEY {
                self.reaction_stats = stats;
            } else {
                self.per_regime_stats.insert(r.regime_id, stats);
            }
        }
    }

    fn to_record(regime_id: &str, stats: &UserReactionStats) -> RegimeReactionRecord {
        RegimeReactionRecord {
            regime_id: regime_id.to_string(),
            total: stats.total,
            accepted: stats.accepted,
            rejected: stats.rejected,
            deferred: stats.deferred,
        }
    }

    /// Shared increment logic for the aggregate and per-regime counters.
    fn bump(stats: &mut UserReactionStats, feedback_type: &FeedbackType) {
        stats.total += 1;
        match feedback_type {
            FeedbackType::Accepted => stats.accepted += 1,
            FeedbackType::Rejected => stats.rejected += 1,
            FeedbackType::Deferred => stats.deferred += 1,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use maekon_core::models::suggestion::{FeedbackType, SuggestionFeedback};
    use maekon_core::models::tiered_memory::TriggerParams;

    fn make_regime(id: &str, centroid: RegimeFeatures, status: RegimeStatus) -> Regime {
        Regime {
            regime_id: id.to_string(),
            name: None,
            auto_label: format!("test-{id}"),
            centroid,
            optimal_params: TriggerParams::default(),
            sample_count: 100,
            first_seen: Utc::now(),
            last_seen: Utc::now(),
            status,
        }
    }

    fn coding_centroid() -> RegimeFeatures {
        RegimeFeatures {
            category_coding: 1.0,
            category_communication: 0.0,
            category_browser: 0.0,
            avg_event_rate: 0.3,
            avg_importance: 0.8,
            context_activity_signal: 0.1,
            communication_ratio: 0.05,
        }
    }

    fn comm_centroid() -> RegimeFeatures {
        RegimeFeatures {
            category_coding: 0.0,
            category_communication: 1.0,
            category_browser: 0.0,
            avg_event_rate: 0.7,
            avg_importance: 0.4,
            context_activity_signal: 0.5,
            communication_ratio: 0.8,
        }
    }

    #[test]
    fn classify_exact_centroid_matches() {
        let mut classifier = RegimeClassifier::new(1.5);
        classifier.update_regimes(vec![
            make_regime("coding", coding_centroid(), RegimeStatus::Active),
            make_regime("comm", comm_centroid(), RegimeStatus::Active),
        ]);

        let result = classifier.classify(&coding_centroid());
        assert!(result.is_some());
        assert_eq!(result.unwrap().regime_id, "coding");
    }

    #[test]
    fn classify_near_centroid_matches() {
        let mut classifier = RegimeClassifier::new(1.5);
        classifier.update_regimes(vec![
            make_regime("coding", coding_centroid(), RegimeStatus::Active),
            make_regime("comm", comm_centroid(), RegimeStatus::Active),
        ]);

        // Slightly shifted from coding centroid
        let near_coding = RegimeFeatures {
            category_coding: 0.9,
            avg_event_rate: 0.35,
            avg_importance: 0.75,
            ..coding_centroid()
        };

        let result = classifier.classify(&near_coding);
        assert!(result.is_some());
        assert_eq!(result.unwrap().regime_id, "coding");
    }

    #[test]
    fn classify_far_from_all_returns_none() {
        let mut classifier = RegimeClassifier::new(0.5); // tight threshold
        classifier.update_regimes(vec![make_regime(
            "coding",
            coding_centroid(),
            RegimeStatus::Active,
        )]);

        // Very different from coding centroid
        let far_away = RegimeFeatures {
            category_coding: 0.0,
            category_communication: 0.0,
            category_browser: 1.0,
            avg_event_rate: 0.9,
            avg_importance: 0.1,
            context_activity_signal: 0.9,
            communication_ratio: 0.9,
        };

        let result = classifier.classify(&far_away);
        assert!(result.is_none());
    }

    #[test]
    fn only_active_regimes_considered() {
        let mut classifier = RegimeClassifier::new(1.5);
        classifier.update_regimes(vec![
            make_regime("coding", coding_centroid(), RegimeStatus::Inactive),
            make_regime("comm", comm_centroid(), RegimeStatus::Active),
        ]);

        // This is closest to coding centroid, but it's Inactive
        let result = classifier.classify(&coding_centroid());
        // Should match comm (only active), or None if too far
        if let Some(r) = result {
            assert_eq!(r.regime_id, "comm");
        }
    }

    #[test]
    fn empty_regimes_returns_none() {
        let classifier = RegimeClassifier::new(1.5);
        let result = classifier.classify(&coding_centroid());
        assert!(result.is_none());
    }

    #[test]
    fn archived_regimes_excluded() {
        let mut classifier = RegimeClassifier::new(1.5);
        classifier.update_regimes(vec![make_regime(
            "coding",
            coding_centroid(),
            RegimeStatus::Archived,
        )]);

        let result = classifier.classify(&coding_centroid());
        assert!(result.is_none());
    }

    #[test]
    fn update_regimes_replaces_all() {
        let mut classifier = RegimeClassifier::new(1.5);
        classifier.update_regimes(vec![make_regime(
            "coding",
            coding_centroid(),
            RegimeStatus::Active,
        )]);
        assert_eq!(classifier.regimes().len(), 1);

        classifier.update_regimes(vec![
            make_regime("a", coding_centroid(), RegimeStatus::Active),
            make_regime("b", comm_centroid(), RegimeStatus::Active),
        ]);
        assert_eq!(classifier.regimes().len(), 2);
    }

    #[test]
    fn record_user_reaction_updates_feedback_stats() {
        let mut classifier = RegimeClassifier::new(1.5);
        let feedback = SuggestionFeedback {
            suggestion_id: "suggestion-1".to_string(),
            feedback_type: FeedbackType::Accepted,
            timestamp: Utc::now(),
            comment: None,
            regime_id: None,
        };

        classifier.record_user_reaction(&feedback);

        let stats = classifier.reaction_stats();
        assert_eq!(stats.total, 1);
        assert_eq!(stats.accepted, 1);
        assert_eq!(stats.rejected, 0);
        assert_eq!(stats.deferred, 0);
    }

    fn make_feedback(
        suggestion_id: &str,
        feedback_type: FeedbackType,
        regime_id: Option<&str>,
    ) -> SuggestionFeedback {
        SuggestionFeedback {
            suggestion_id: suggestion_id.to_string(),
            feedback_type,
            timestamp: Utc::now(),
            comment: None,
            regime_id: regime_id.map(str::to_string),
        }
    }

    /// #7600 fails-before: prior to this change `per_regime_stats` did not
    /// exist and `record_user_reaction` had no way to attribute a reaction
    /// to a specific regime — every reaction landed in one global counter.
    /// This proves two DIFFERENT regimes accumulate independent counts.
    #[test]
    fn record_user_reaction_is_per_regime_weighted() {
        let mut classifier = RegimeClassifier::new(1.5);

        classifier.record_user_reaction(&make_feedback(
            "s1",
            FeedbackType::Accepted,
            Some("regime-coding"),
        ));
        classifier.record_user_reaction(&make_feedback(
            "s2",
            FeedbackType::Accepted,
            Some("regime-coding"),
        ));
        classifier.record_user_reaction(&make_feedback(
            "s3",
            FeedbackType::Rejected,
            Some("regime-comm"),
        ));

        let per_regime = classifier.per_regime_stats();
        let coding = per_regime.get("regime-coding").expect("coding bucket");
        assert_eq!(coding.total, 2);
        assert_eq!(coding.accepted, 2);
        assert_eq!(coding.rejected, 0);

        let comm = per_regime.get("regime-comm").expect("comm bucket");
        assert_eq!(comm.total, 1);
        assert_eq!(comm.rejected, 1);

        // The global aggregate still sees all 3 reactions — unchanged
        // behavior for existing consumers of `reaction_stats()`.
        assert_eq!(classifier.reaction_stats().total, 3);
    }

    /// #7913 T2.1c — reaction stats survive a restart: snapshot a classifier's
    /// per-regime + aggregate buckets, hydrate a FRESH classifier from them, and
    /// prove `acceptance_rate` (the production reader) reproduces the same value.
    #[test]
    fn reaction_snapshot_and_hydrate_roundtrip_preserves_acceptance_rate() {
        let mut classifier = RegimeClassifier::new(1.5);
        classifier.record_user_reaction(&make_feedback(
            "s1",
            FeedbackType::Rejected,
            Some("regime-x"),
        ));
        classifier.record_user_reaction(&make_feedback(
            "s2",
            FeedbackType::Accepted,
            Some("regime-x"),
        ));
        classifier.record_user_reaction(&make_feedback(
            "s3",
            FeedbackType::Accepted,
            Some("regime-x"),
        ));
        // A reaction with NO regime → aggregate only.
        classifier.record_user_reaction(&make_feedback("s4", FeedbackType::Accepted, None));

        let before_rate = classifier.acceptance_rate("regime-x").unwrap();
        let snapshot = classifier.reaction_snapshot();
        // aggregate + one per-regime bucket
        assert_eq!(snapshot.len(), 2);

        let mut restored = RegimeClassifier::new(1.5);
        restored.hydrate_reactions(snapshot);

        // Per-regime acceptance rate reproduced exactly.
        let after_rate = restored.acceptance_rate("regime-x").unwrap();
        assert!(
            (after_rate - before_rate).abs() < f64::EPSILON,
            "got {after_rate}"
        );
        // Aggregate (4 total: 3 for regime-x + 1 regime-less) restored too.
        assert_eq!(restored.reaction_stats().total, 4);
        assert_eq!(restored.reaction_stats().accepted, 3);
    }

    /// Feedback with no `regime_id` must not create a bogus bucket, and must
    /// still count toward the global aggregate.
    #[test]
    fn record_user_reaction_without_regime_id_skips_per_regime_bucket() {
        let mut classifier = RegimeClassifier::new(1.5);

        classifier.record_user_reaction(&make_feedback("s1", FeedbackType::Accepted, None));

        assert!(classifier.per_regime_stats().is_empty());
        assert_eq!(classifier.reaction_stats().total, 1);
    }

    /// #7600 fails-before: `acceptance_rate` did not exist prior to this
    /// change — `reaction_stats`/`per_regime_stats` were write-only from a
    /// production standpoint. This proves the reader returns the recorded
    /// per-regime rate once the minimum sample floor is met, and `None`
    /// below it (avoiding a single early rejection driving gating to 0%).
    #[test]
    fn acceptance_rate_requires_minimum_samples_then_reflects_ratio() {
        let mut classifier = RegimeClassifier::new(1.5);

        // Never observed => None.
        assert_eq!(classifier.acceptance_rate("regime-x"), None);

        classifier.record_user_reaction(&make_feedback(
            "s1",
            FeedbackType::Rejected,
            Some("regime-x"),
        ));
        // Below MIN_SAMPLES_FOR_RATE (1 < 3) => still None, even though the
        // rejection alone would otherwise read as a confident 0.0.
        assert_eq!(classifier.acceptance_rate("regime-x"), None);

        classifier.record_user_reaction(&make_feedback(
            "s2",
            FeedbackType::Accepted,
            Some("regime-x"),
        ));
        classifier.record_user_reaction(&make_feedback(
            "s3",
            FeedbackType::Accepted,
            Some("regime-x"),
        ));
        // Now at the floor (3 total: 1 rejected, 2 accepted) => Some(2/3).
        let rate = classifier
            .acceptance_rate("regime-x")
            .expect("rate available at min sample size");
        assert!(
            (rate - (2.0 / 3.0)).abs() < f64::EPSILON,
            "expected 2/3, got {rate}"
        );
    }
}
