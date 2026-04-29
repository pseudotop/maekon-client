//! Lightweight online logistic regression for coaching "should show" decisions.
//!
//! Learns from accumulated feedback to predict whether a coaching message
//! will be helpful in a given context. Replaces the fixed rule-based gating
//! when enough training data has been collected.
//!
//! **Resource footprint**: 9 floats (36 bytes) + O(1) per prediction/update.
//! **No external dependencies.**

/// Minimum training samples before the model's prediction is used.
/// Below this, falls back to the rule-based gating (TunableParams).
const MIN_TRAINING_SAMPLES: u32 = 50;

/// Learning rate for SGD updates.
const LEARNING_RATE: f32 = 0.01;

/// L2 regularization strength (prevents overfitting on small datasets).
const L2_LAMBDA: f32 = 0.001;

/// Number of input features.
const NUM_FEATURES: usize = 8;

/// Feature vector extracted from coaching context at evaluation time.
#[derive(Debug, Clone)]
pub struct CoachingFeatures {
    /// Time of day normalized to [0, 1] (0 = midnight, 0.5 = noon).
    pub time_of_day: f32,
    /// Current regime dwell time in seconds, log-scaled.
    pub regime_dwell_log: f32,
    /// Daily context switch count, log-scaled.
    pub context_switches_log: f32,
    /// Goal progress ratio [0, 1+]. 0 = no progress, 1 = target reached.
    pub goal_progress: f32,
    /// Historical effectiveness of this profile [0, 1].
    pub profile_effectiveness: f32,
    /// Whether drift was detected (0 or 1).
    pub drift_detected: f32,
    /// Messages shown today, log-scaled.
    pub messages_today_log: f32,
    /// Ratio of dwell time to average (overstay signal).
    pub overstay_ratio: f32,
}

impl CoachingFeatures {
    /// Convert to a fixed-size array for dot product.
    fn as_array(&self) -> [f32; NUM_FEATURES] {
        [
            self.time_of_day,
            self.regime_dwell_log,
            self.context_switches_log,
            self.goal_progress,
            self.profile_effectiveness,
            self.drift_detected,
            self.messages_today_log,
            self.overstay_ratio,
        ]
    }

    /// Extract features from available coaching context.
    #[allow(clippy::too_many_arguments)]
    pub fn extract(
        hour: u32,
        regime_duration_secs: u64,
        context_switch_count: u32,
        goal_progress_ratio: f32,
        profile_effectiveness: f32,
        drift_detected: bool,
        messages_shown_today: u32,
        avg_regime_duration_secs: u64,
    ) -> Self {
        let overstay = if avg_regime_duration_secs > 0 {
            regime_duration_secs as f32 / avg_regime_duration_secs as f32
        } else {
            1.0
        };

        Self {
            time_of_day: hour as f32 / 24.0,
            regime_dwell_log: (1.0 + regime_duration_secs as f32).ln(),
            context_switches_log: (1.0 + context_switch_count as f32).ln(),
            goal_progress: goal_progress_ratio.clamp(0.0, 2.0),
            profile_effectiveness,
            drift_detected: if drift_detected { 1.0 } else { 0.0 },
            messages_today_log: (1.0 + messages_shown_today as f32).ln(),
            overstay_ratio: overstay.clamp(0.0, 5.0),
        }
    }
}

/// Online logistic regression model for coaching relevance prediction.
///
/// Predicts P(helpful | features) using a simple linear model with sigmoid
/// activation. Weights are updated via SGD on each feedback event.
#[derive(Debug, Clone)]
pub struct AdaptiveScorer {
    /// Model weights (one per feature).
    weights: [f32; NUM_FEATURES],
    /// Bias term.
    bias: f32,
    /// Number of training updates performed.
    train_count: u32,
}

impl Default for AdaptiveScorer {
    fn default() -> Self {
        Self {
            // Initialize weights to zero — model starts as "always 0.5" (neutral)
            weights: [0.0; NUM_FEATURES],
            bias: 0.0,
            train_count: 0,
        }
    }
}

impl AdaptiveScorer {
    /// Predict P(helpful) for the given features.
    /// Returns a value in [0, 1].
    pub fn predict(&self, features: &CoachingFeatures) -> f32 {
        let x = features.as_array();
        let z: f32 = self.bias
            + self
                .weights
                .iter()
                .zip(x.iter())
                .map(|(w, xi)| w * xi)
                .sum::<f32>();
        sigmoid(z)
    }

    /// Returns true if the model has enough training data to be trusted.
    pub fn is_ready(&self) -> bool {
        self.train_count >= MIN_TRAINING_SAMPLES
    }

    /// Update weights using a single SGD step.
    ///
    /// `label`: 1.0 for positive feedback, 0.0 for negative.
    pub fn update(&mut self, features: &CoachingFeatures, label: f32) {
        let x = features.as_array();
        let prediction = self.predict(features);
        let error = prediction - label; // gradient of log-loss

        // SGD with L2 regularization
        for (w, xi) in self.weights.iter_mut().zip(x.iter()) {
            *w -= LEARNING_RATE * (error * xi + L2_LAMBDA * *w);
        }
        self.bias -= LEARNING_RATE * error;

        self.train_count += 1;
    }

    /// Number of training updates performed.
    pub fn train_count(&self) -> u32 {
        self.train_count
    }
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_scorer_predicts_neutral() {
        let scorer = AdaptiveScorer::default();
        let features = CoachingFeatures::extract(12, 1800, 5, 0.5, 0.5, false, 2, 1800);
        let p = scorer.predict(&features);
        // Zero weights + zero bias → sigmoid(0) = 0.5
        assert!((p - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn not_ready_below_min_samples() {
        let mut scorer = AdaptiveScorer::default();
        assert!(!scorer.is_ready());
        let features = CoachingFeatures::extract(12, 1800, 5, 0.5, 0.5, false, 2, 1800);
        for _ in 0..49 {
            scorer.update(&features, 1.0);
        }
        assert!(!scorer.is_ready());
        scorer.update(&features, 1.0);
        assert!(scorer.is_ready());
    }

    #[test]
    fn positive_training_increases_prediction() {
        let mut scorer = AdaptiveScorer::default();
        let features = CoachingFeatures::extract(14, 3600, 3, 0.8, 0.6, false, 1, 1800);

        let before = scorer.predict(&features);
        for _ in 0..20 {
            scorer.update(&features, 1.0);
        }
        let after = scorer.predict(&features);
        assert!(
            after > before,
            "positive training should increase P(helpful)"
        );
    }

    #[test]
    fn negative_training_decreases_prediction() {
        let mut scorer = AdaptiveScorer::default();
        let features = CoachingFeatures::extract(22, 600, 12, 0.1, 0.1, false, 5, 1800);

        let before = scorer.predict(&features);
        for _ in 0..20 {
            scorer.update(&features, 0.0);
        }
        let after = scorer.predict(&features);
        assert!(
            after < before,
            "negative training should decrease P(helpful)"
        );
    }

    #[test]
    fn prediction_stays_bounded() {
        let mut scorer = AdaptiveScorer::default();
        let features = CoachingFeatures::extract(12, 1800, 5, 0.5, 0.5, true, 2, 1800);
        // Extreme training in one direction
        for _ in 0..1000 {
            scorer.update(&features, 1.0);
        }
        let p = scorer.predict(&features);
        assert!(
            p > 0.0 && p < 1.0,
            "prediction must stay in (0, 1), got {p}"
        );
    }

    #[test]
    fn feature_extraction_produces_valid_ranges() {
        let f = CoachingFeatures::extract(23, 7200, 20, 1.5, 0.9, true, 10, 3600);
        assert!(f.time_of_day >= 0.0 && f.time_of_day <= 1.0);
        assert!(f.regime_dwell_log > 0.0);
        assert!(f.context_switches_log > 0.0);
        assert!(f.goal_progress >= 0.0 && f.goal_progress <= 2.0);
        assert!(f.drift_detected == 1.0);
        assert!(f.overstay_ratio == 2.0); // 7200 / 3600
    }

    #[test]
    fn different_contexts_get_different_scores_after_training() {
        let mut scorer = AdaptiveScorer::default();

        let good_context = CoachingFeatures::extract(10, 3600, 2, 0.8, 0.7, false, 1, 1800);
        let bad_context = CoachingFeatures::extract(22, 300, 15, 0.1, 0.1, false, 8, 1800);

        // Train: good context = helpful, bad context = not helpful
        for _ in 0..100 {
            scorer.update(&good_context, 1.0);
            scorer.update(&bad_context, 0.0);
        }

        let good_score = scorer.predict(&good_context);
        let bad_score = scorer.predict(&bad_context);
        assert!(
            good_score > bad_score,
            "good context ({good_score}) should score higher than bad context ({bad_score})"
        );
    }
}
