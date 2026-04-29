//! Auto-tuning: per-category EMA statistics tracking and drift detection.
//!
//! `EmaStatsTracker` maintains running EMA of event rate and importance per
//! category/process, and generates per-category `TriggerParams` overrides.
//!
//! `DriftDetector` monitors an EWMA of a signal and flags when the signal
//! deviates beyond a configurable number of sigma (concept drift detection).

use std::collections::HashMap;

use oneshim_core::models::tiered_memory::TriggerParams;

// ---------------------------------------------------------------------------
// EmaStatsTracker
// ---------------------------------------------------------------------------

/// Per-category running statistics.
#[derive(Debug, Clone)]
pub struct CategoryStats {
    /// Exponential moving average of event rate.
    pub ema_event_rate: f32,
    /// Exponential moving average of importance.
    pub ema_importance: f32,
    /// Running variance of importance (derived from Welford's algorithm).
    pub ema_variance: f32,
    /// Total number of observations.
    pub sample_count: u64,
    // Welford's running mean and M2 for variance calculation.
    welford_mean: f64,
    welford_m2: f64,
}

impl CategoryStats {
    fn new() -> Self {
        Self {
            ema_event_rate: 0.0,
            ema_importance: 0.0,
            ema_variance: 0.0,
            sample_count: 0,
            welford_mean: 0.0,
            welford_m2: 0.0,
        }
    }
}

/// Per-process running statistics (lighter weight).
#[derive(Debug, Clone)]
pub struct ProcessStats {
    /// Exponential moving average of event rate.
    pub ema_event_rate: f32,
    /// Total number of observations.
    pub sample_count: u64,
}

impl ProcessStats {
    fn new() -> Self {
        Self {
            ema_event_rate: 0.0,
            sample_count: 0,
        }
    }
}

/// Tracks per-category and per-process EMA statistics for auto-tuning
/// trigger parameters.
#[derive(Debug, Clone)]
pub struct EmaStatsTracker {
    category_stats: HashMap<String, CategoryStats>,
    process_stats: HashMap<String, ProcessStats>,
    alpha: f32,
}

impl EmaStatsTracker {
    /// Create a new tracker with the given EMA smoothing factor.
    ///
    /// `alpha` should be in (0, 1); typical value: 0.05.
    pub fn new(alpha: f32) -> Self {
        Self {
            category_stats: HashMap::new(),
            process_stats: HashMap::new(),
            alpha: alpha.clamp(0.001, 0.999),
        }
    }

    /// Update statistics with a new observation.
    pub fn update(&mut self, category: &str, process: &str, event_rate: f32, importance: f32) {
        // --- Category stats ---
        let cat = self
            .category_stats
            .entry(category.to_string())
            .or_insert_with(CategoryStats::new);

        cat.sample_count += 1;

        if cat.sample_count == 1 {
            // First sample — initialize
            cat.ema_event_rate = event_rate;
            cat.ema_importance = importance;
            cat.welford_mean = importance as f64;
            cat.welford_m2 = 0.0;
            cat.ema_variance = 0.0;
        } else {
            // EMA update
            cat.ema_event_rate += self.alpha * (event_rate - cat.ema_event_rate);
            cat.ema_importance += self.alpha * (importance - cat.ema_importance);

            // Welford's online variance (importance)
            let n = cat.sample_count as f64;
            let delta = importance as f64 - cat.welford_mean;
            cat.welford_mean += delta / n;
            let delta2 = importance as f64 - cat.welford_mean;
            cat.welford_m2 += delta * delta2;
            cat.ema_variance = if n > 1.0 {
                (cat.welford_m2 / (n - 1.0)) as f32
            } else {
                0.0
            };
        }

        // --- Process stats ---
        let proc = self
            .process_stats
            .entry(process.to_string())
            .or_insert_with(ProcessStats::new);

        proc.sample_count += 1;
        if proc.sample_count == 1 {
            proc.ema_event_rate = event_rate;
        } else {
            proc.ema_event_rate += self.alpha * (event_rate - proc.ema_event_rate);
        }
    }

    /// Get adaptive threshold for a category at `mean + sigma_multiplier * sigma`.
    ///
    /// Returns `None` if the category has no data or insufficient variance.
    pub fn threshold(&self, category: &str, sigma_multiplier: f32) -> Option<f32> {
        let cat = self.category_stats.get(category)?;
        if cat.sample_count < 2 {
            return None;
        }
        let sigma = cat.ema_variance.max(0.0).sqrt();
        Some(cat.ema_importance + sigma_multiplier * sigma)
    }

    /// Generate per-category `TriggerParams` overrides from learned statistics.
    ///
    /// Percentiles are approximated via normal distribution:
    /// - `t_high` ~ mean + 0.674 * sigma (approx 75th percentile)
    /// - `t_low`  ~ mean - 0.674 * sigma (approx 25th percentile)
    ///
    /// Only categories with >= `MIN_SAMPLES` observations are included.
    pub fn generate_overrides(&self) -> HashMap<String, TriggerParams> {
        const MIN_SAMPLES: u64 = 20;
        const Z_75: f32 = 0.674;

        let mut overrides = HashMap::new();

        for (category, stats) in &self.category_stats {
            if stats.sample_count < MIN_SAMPLES {
                continue;
            }

            let sigma = stats.ema_variance.max(0.0).sqrt();
            if sigma < 1e-6 {
                // Negligible variance — skip override
                continue;
            }

            let t_high = (stats.ema_importance + Z_75 * sigma).clamp(0.0, 1.0);
            let t_low = (stats.ema_importance - Z_75 * sigma).clamp(0.0, 1.0);

            // alpha_long based on inverse of variance: stable categories get
            // slower adaptation (higher alpha_long → more smoothing).
            let alpha_long = (1.0 / (1.0 + sigma * 10.0)).clamp(0.01, 0.3);

            overrides.insert(
                category.clone(),
                TriggerParams {
                    t_high: Some(t_high),
                    t_low: Some(t_low),
                    alpha_long: Some(alpha_long),
                    ..Default::default()
                },
            );
        }

        overrides
    }

    /// Read-only access to category stats.
    pub fn category_stats(&self) -> &HashMap<String, CategoryStats> {
        &self.category_stats
    }

    /// Read-only access to process stats.
    pub fn process_stats(&self) -> &HashMap<String, ProcessStats> {
        &self.process_stats
    }
}

// ---------------------------------------------------------------------------
// DriftDetector
// ---------------------------------------------------------------------------

/// EWMA-based drift detector.
///
/// Maintains an exponentially weighted moving average and variance of a signal.
/// Reports drift when `|value - ewma| > threshold_sigma * sqrt(variance)`.
#[derive(Debug, Clone)]
pub struct DriftDetector {
    ewma: f32,
    ewma_variance: f32,
    alpha: f32,
    threshold_sigma: f32,
    initialized: bool,
}

impl DriftDetector {
    /// Create a new drift detector.
    ///
    /// - `alpha`: EWMA smoothing factor (typical: 0.05–0.1).
    /// - `threshold_sigma`: number of sigma for drift threshold (typical: 2.0–3.0).
    pub fn new(alpha: f32, threshold_sigma: f32) -> Self {
        Self {
            ewma: 0.0,
            ewma_variance: 0.0,
            alpha: alpha.clamp(0.001, 0.999),
            threshold_sigma,
            initialized: false,
        }
    }

    /// Feed a new observation. Returns `true` if drift is detected.
    pub fn observe(&mut self, value: f32) -> bool {
        if !self.initialized {
            self.ewma = value;
            self.ewma_variance = 0.0;
            self.initialized = true;
            return false;
        }

        let deviation = value - self.ewma;

        // Update EWMA
        self.ewma += self.alpha * deviation;

        // Update EWMA variance (exponentially weighted)
        self.ewma_variance =
            (1.0 - self.alpha) * (self.ewma_variance + self.alpha * deviation * deviation);

        // Check drift
        let sigma = self.ewma_variance.max(0.0).sqrt();
        if sigma < 1e-9 {
            return false;
        }

        deviation.abs() > self.threshold_sigma * sigma
    }

    /// Reset the detector state after an acknowledged drift (e.g., after re-clustering).
    pub fn reset(&mut self) {
        self.ewma = 0.0;
        self.ewma_variance = 0.0;
        self.initialized = false;
    }

    /// Current EWMA value.
    pub fn current_ewma(&self) -> f32 {
        self.ewma
    }

    /// Whether the detector has been initialized with at least one observation.
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- EmaStatsTracker tests ----

    #[test]
    fn ema_convergence() {
        let mut tracker = EmaStatsTracker::new(0.1);

        // Feed constant values — EMA should converge to that value
        for _ in 0..200 {
            tracker.update("dev", "vscode", 0.5, 0.8);
        }

        let stats = &tracker.category_stats["dev"];
        assert!(
            (stats.ema_event_rate - 0.5).abs() < 0.01,
            "event_rate should converge to 0.5, got {}",
            stats.ema_event_rate
        );
        assert!(
            (stats.ema_importance - 0.8).abs() < 0.01,
            "importance should converge to 0.8, got {}",
            stats.ema_importance
        );
    }

    #[test]
    fn variance_tracking() {
        let mut tracker = EmaStatsTracker::new(0.1);

        // Feed alternating values to produce variance
        for i in 0..100 {
            let importance = if i % 2 == 0 { 0.3 } else { 0.7 };
            tracker.update("comm", "slack", 0.5, importance);
        }

        let stats = &tracker.category_stats["comm"];
        assert!(stats.ema_variance > 0.01, "variance should be positive");
        assert_eq!(stats.sample_count, 100);
    }

    #[test]
    fn threshold_computation() {
        let mut tracker = EmaStatsTracker::new(0.1);

        for i in 0..100 {
            let importance = if i % 2 == 0 { 0.3 } else { 0.7 };
            tracker.update("browser", "chrome", 0.5, importance);
        }

        let t = tracker.threshold("browser", 1.0);
        assert!(t.is_some());
        let threshold = t.unwrap();
        // Should be mean + 1*sigma — greater than mean
        let mean = tracker.category_stats["browser"].ema_importance;
        assert!(
            threshold > mean,
            "threshold {} should be greater than mean {}",
            threshold,
            mean
        );

        // Non-existent category returns None
        assert!(tracker.threshold("nonexistent", 1.0).is_none());
    }

    #[test]
    fn override_generation_with_min_samples() {
        let mut tracker = EmaStatsTracker::new(0.1);

        // Only 10 samples — should NOT generate override
        for i in 0..10 {
            let importance = 0.5 + (i as f32) * 0.01;
            tracker.update("few", "app", 0.5, importance);
        }

        // 30 samples with variance — should generate override
        for i in 0..30 {
            let importance = if i % 2 == 0 { 0.3 } else { 0.7 };
            tracker.update("enough", "app", 0.5, importance);
        }

        let overrides = tracker.generate_overrides();
        assert!(
            !overrides.contains_key("few"),
            "should not have override for < 20 samples"
        );
        assert!(
            overrides.contains_key("enough"),
            "should have override for >= 20 samples with variance"
        );

        let params = &overrides["enough"];
        assert!(params.t_high.is_some());
        assert!(params.t_low.is_some());
        assert!(params.alpha_long.is_some());

        // t_high > t_low
        assert!(params.t_high.unwrap() > params.t_low.unwrap());
    }

    #[test]
    fn override_generation_skip_zero_variance() {
        let mut tracker = EmaStatsTracker::new(0.1);

        // Constant values → zero variance
        for _ in 0..30 {
            tracker.update("constant", "app", 0.5, 0.5);
        }

        let overrides = tracker.generate_overrides();
        // Sigma ≈ 0 → should skip
        assert!(
            !overrides.contains_key("constant"),
            "should skip categories with near-zero variance"
        );
    }

    #[test]
    fn process_stats_tracked() {
        let mut tracker = EmaStatsTracker::new(0.1);

        for _ in 0..20 {
            tracker.update("dev", "vscode", 0.6, 0.7);
        }
        for _ in 0..20 {
            tracker.update("dev", "terminal", 0.8, 0.5);
        }

        assert!(tracker.process_stats.contains_key("vscode"));
        assert!(tracker.process_stats.contains_key("terminal"));
        assert!((tracker.process_stats["vscode"].ema_event_rate - 0.6).abs() < 0.05);
    }

    // ---- DriftDetector tests ----

    #[test]
    fn stable_data_no_drift() {
        let mut detector = DriftDetector::new(0.1, 2.0);

        // Feed relatively stable data — no drift expected
        let mut drift_count = 0;
        for _ in 0..100 {
            if detector.observe(0.5) {
                drift_count += 1;
            }
        }

        // Constant data should never drift (or at most 1 time near initialization)
        assert!(
            drift_count <= 1,
            "stable data should produce minimal drift events, got {}",
            drift_count
        );
    }

    #[test]
    fn shifted_data_detects_drift() {
        let mut detector = DriftDetector::new(0.05, 2.0);

        // Build up stable baseline
        for _ in 0..200 {
            detector.observe(0.5);
        }

        // Sudden shift — should detect drift
        let mut drift_detected = false;
        for _ in 0..5 {
            if detector.observe(0.95) {
                drift_detected = true;
                break;
            }
        }

        assert!(drift_detected, "large shift should trigger drift detection");
    }

    #[test]
    fn reset_clears_state() {
        let mut detector = DriftDetector::new(0.1, 2.0);

        for _ in 0..50 {
            detector.observe(0.5);
        }
        assert!(detector.is_initialized());

        detector.reset();
        assert!(!detector.is_initialized());
        assert!((detector.current_ewma() - 0.0).abs() < 1e-6);
    }

    #[test]
    fn first_observation_never_drifts() {
        let mut detector = DriftDetector::new(0.1, 2.0);
        assert!(!detector.observe(100.0));
    }
}
