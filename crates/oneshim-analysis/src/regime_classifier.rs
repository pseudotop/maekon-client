//! Nearest-centroid regime classifier for real-time activity classification.
//!
//! Given a set of known regimes and a current feature vector, finds the
//! closest active regime within a configurable distance threshold.

use oneshim_core::models::tiered_memory::{
    euclidean_distance, Regime, RegimeFeatures, RegimeStatus,
};

/// Classifies activity feature vectors into the nearest known regime.
///
/// Uses Euclidean distance with a maximum threshold — features that are
/// too far from all known centroids return `None` (fallback to defaults).
pub struct RegimeClassifier {
    regimes: Vec<Regime>,
    max_distance_threshold: f32,
}

impl RegimeClassifier {
    /// Create a classifier with the given distance threshold.
    /// A typical default is 1.5.
    pub fn new(max_distance_threshold: f32) -> Self {
        Self {
            regimes: vec![],
            max_distance_threshold,
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

    /// Record a user reaction to a suggestion. Phase 3 stub — records
    /// no per-regime state beyond a trace log. Called via
    /// `FeedbackSignalSink` from the composition root.
    pub fn record_user_reaction(
        &mut self,
        feedback: &oneshim_core::models::suggestion::SuggestionFeedback,
    ) {
        tracing::debug!(
            suggestion_id = %feedback.suggestion_id,
            feedback_type = ?feedback.feedback_type,
            "regime_classifier: user reaction recorded (no-op learning)"
        );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use oneshim_core::models::tiered_memory::TriggerParams;

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
}
