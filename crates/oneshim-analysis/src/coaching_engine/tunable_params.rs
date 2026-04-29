//! Auto-tunable coaching parameters.
//!
//! Replaces hardcoded constants with feedback-driven values that adjust
//! themselves based on user behavior. Each parameter has a safe range
//! and nudges toward better values on negative feedback.

/// Step size for each adjustment (5% of current value).
const TUNE_STEP: f32 = 0.05;
/// Decay rate toward defaults every N feedback events.
const DECAY_INTERVAL: u32 = 100;
/// Decay strength: nudge 2% toward default on each decay tick.
const DECAY_RATE: f32 = 0.02;

/// Coaching parameters that adapt based on accumulated feedback.
///
/// All values start at their original hardcoded defaults and are adjusted
/// at runtime. Volatile — resets to defaults on app restart.
#[derive(Debug, Clone)]
pub struct TunableParams {
    /// Weight for explicit feedback (thumbs-up/down). Not auto-tuned.
    pub explicit_weight: f32,
    /// Implicit feedback evaluation window in seconds.
    pub implicit_window_secs: i64,
    /// Effectiveness ratio below which coaching is gated.
    pub low_effectiveness_threshold: f32,
    /// Minimum messages shown before gating activates.
    pub min_shown_for_gating: u32,
    /// Regime overstay trigger ratio (duration > ratio × average).
    pub overstay_ratio: f32,
    /// EMA alpha for dwell time averaging (weight for new data).
    pub ema_alpha: f32,
    /// Fraction of messages allowed when effectiveness is low.
    pub gate_allow_ratio: f32,

    /// Counter for decay scheduling.
    feedback_count: u32,
}

impl Default for TunableParams {
    fn default() -> Self {
        Self {
            explicit_weight: 3.0,
            implicit_window_secs: 300,
            low_effectiveness_threshold: 0.2,
            min_shown_for_gating: 5,
            overstay_ratio: 1.2,
            ema_alpha: 0.2,
            gate_allow_ratio: 0.33,
            feedback_count: 0,
        }
    }
}

impl TunableParams {
    /// Adjust parameters based on a feedback event.
    ///
    /// Positive feedback: no adjustment (current settings are working).
    /// Negative feedback: nudge relevant params to be less intrusive.
    pub fn adjust_on_feedback(&mut self, positive: bool) {
        self.feedback_count += 1;

        if !positive {
            // Be more selective about when to show coaching
            self.low_effectiveness_threshold = step_up(self.low_effectiveness_threshold, 0.05, 0.5);
            // Trigger overstay later (less annoying)
            self.overstay_ratio = step_up(self.overstay_ratio, 1.05, 2.0);
            // Show fewer messages when low effectiveness
            self.gate_allow_ratio = step_down(self.gate_allow_ratio, 0.1, 0.5);
            // Give user more time for implicit feedback
            self.implicit_window_secs = (self.implicit_window_secs as f32 * (1.0 + TUNE_STEP))
                .round()
                .clamp(60.0, 900.0) as i64;
        }

        // Periodic decay toward defaults prevents runaway drift
        if self.feedback_count % DECAY_INTERVAL == 0 {
            self.decay_toward_defaults();
        }
    }

    /// Nudge all params 2% toward their defaults.
    fn decay_toward_defaults(&mut self) {
        let defaults = Self::default();
        self.low_effectiveness_threshold = lerp(
            self.low_effectiveness_threshold,
            defaults.low_effectiveness_threshold,
            DECAY_RATE,
        );
        self.overstay_ratio = lerp(self.overstay_ratio, defaults.overstay_ratio, DECAY_RATE);
        self.gate_allow_ratio = lerp(self.gate_allow_ratio, defaults.gate_allow_ratio, DECAY_RATE);
        self.implicit_window_secs = lerp(
            self.implicit_window_secs as f32,
            defaults.implicit_window_secs as f32,
            DECAY_RATE,
        ) as i64;
        self.ema_alpha = lerp(self.ema_alpha, defaults.ema_alpha, DECAY_RATE);
    }

    /// Overstay threshold as integer ratio (e.g., 1.2 → 120/100).
    /// Used in the integer comparison: `duration > avg * overstay_percent / 100`
    pub fn overstay_percent(&self) -> u64 {
        (self.overstay_ratio * 100.0).round() as u64
    }
}

/// Step a value up by TUNE_STEP %, clamped to [min, max].
fn step_up(value: f32, min: f32, max: f32) -> f32 {
    (value * (1.0 + TUNE_STEP)).clamp(min, max)
}

/// Step a value down by TUNE_STEP %, clamped to [min, max].
fn step_down(value: f32, min: f32, max: f32) -> f32 {
    (value * (1.0 - TUNE_STEP)).clamp(min, max)
}

/// Linear interpolation: moves `current` toward `target` by `t` fraction.
fn lerp(current: f32, target: f32, t: f32) -> f32 {
    current + (target - current) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_original_hardcoded_values() {
        let p = TunableParams::default();
        assert!((p.explicit_weight - 3.0).abs() < f32::EPSILON);
        assert_eq!(p.implicit_window_secs, 300);
        assert!((p.low_effectiveness_threshold - 0.2).abs() < f32::EPSILON);
        assert_eq!(p.min_shown_for_gating, 5);
        assert!((p.overstay_ratio - 1.2).abs() < f32::EPSILON);
        assert!((p.ema_alpha - 0.2).abs() < f32::EPSILON);
        assert_eq!(p.overstay_percent(), 120);
    }

    #[test]
    fn negative_feedback_increases_overstay_ratio() {
        let mut p = TunableParams::default();
        let before = p.overstay_ratio;
        p.adjust_on_feedback(false);
        assert!(p.overstay_ratio > before);
    }

    #[test]
    fn negative_feedback_increases_effectiveness_threshold() {
        let mut p = TunableParams::default();
        let before = p.low_effectiveness_threshold;
        p.adjust_on_feedback(false);
        assert!(p.low_effectiveness_threshold > before);
    }

    #[test]
    fn negative_feedback_decreases_gate_allow_ratio() {
        let mut p = TunableParams::default();
        let before = p.gate_allow_ratio;
        p.adjust_on_feedback(false);
        assert!(p.gate_allow_ratio < before);
    }

    #[test]
    fn positive_feedback_does_not_change_params() {
        let mut p = TunableParams::default();
        let before = p.clone();
        p.adjust_on_feedback(true);
        assert!((p.overstay_ratio - before.overstay_ratio).abs() < f32::EPSILON);
        assert!(
            (p.low_effectiveness_threshold - before.low_effectiveness_threshold).abs()
                < f32::EPSILON
        );
    }

    #[test]
    fn params_stay_within_safe_ranges() {
        let mut p = TunableParams::default();
        // 200 negative feedbacks should not exceed bounds
        for _ in 0..200 {
            p.adjust_on_feedback(false);
        }
        assert!(p.overstay_ratio <= 2.0);
        assert!(p.low_effectiveness_threshold <= 0.5);
        assert!(p.gate_allow_ratio >= 0.1);
        assert!(p.implicit_window_secs <= 900);
    }

    #[test]
    fn decay_nudges_toward_defaults() {
        let mut p = TunableParams::default();
        // Push params away from defaults
        for _ in 0..50 {
            p.adjust_on_feedback(false);
        }
        let drifted_overstay = p.overstay_ratio;
        assert!(drifted_overstay > 1.2);

        // Trigger decay (at count 100)
        for _ in 50..100 {
            p.adjust_on_feedback(true); // positive = no adjustment, just count
        }
        // After decay at count 100, overstay should be slightly closer to 1.2
        assert!(p.overstay_ratio < drifted_overstay);
    }
}
