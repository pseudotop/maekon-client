use chrono::{Local, NaiveDate};
use oneshim_core::models::coaching::GoalProgress;
use std::collections::HashMap;

/// Tracks per-regime daily time goals and fires threshold triggers
/// at 25/50/75/100% milestones.
///
/// Uses `chrono::Local::now().date_naive()` for date comparison,
/// consistent with the aggregation loop in the scheduler.
pub struct RegimeGoalTracker {
    /// Regime label -> daily target minutes (from config).
    goals: HashMap<String, u32>,
    /// Regime label -> minutes accumulated today.
    today_minutes: HashMap<String, u32>,
    /// Date for which `today_minutes` is valid.
    tracking_date: NaiveDate,
    /// Regime label -> list of already-notified thresholds (25, 50, 75, 100).
    notified_thresholds: HashMap<String, Vec<u8>>,
}

impl RegimeGoalTracker {
    pub fn new() -> Self {
        Self {
            goals: HashMap::new(),
            today_minutes: HashMap::new(),
            tracking_date: Local::now().date_naive(),
            notified_thresholds: HashMap::new(),
        }
    }

    /// Load or replace goals from config.
    pub fn update_goals(&mut self, regime_goals: &HashMap<String, u32>) {
        self.goals = regime_goals.clone();
    }

    /// Record additional minutes for a regime. Triggers date rollover if needed.
    pub fn record_minutes(&mut self, regime_label: &str, additional_minutes: u32) {
        self.ensure_date_rollover();
        let current = self
            .today_minutes
            .entry(regime_label.to_string())
            .or_insert(0);
        *current = current.saturating_add(additional_minutes);
    }

    /// Returns a newly crossed threshold (25, 50, 75, 100) if one was just
    /// reached, or `None` if no new threshold was crossed.
    ///
    /// Each threshold fires exactly once per day per regime.
    pub fn check_threshold(&mut self, regime_label: &str) -> Option<u8> {
        let target = match self.goals.get(regime_label) {
            Some(&t) if t > 0 => t,
            _ => return None,
        };

        let current = self.today_minutes.get(regime_label).copied().unwrap_or(0);
        let percentage = ((current as f64 / target as f64) * 100.0).min(u16::MAX as f64) as u16;

        let thresholds = [25u8, 50, 75, 100];
        let notified = self
            .notified_thresholds
            .entry(regime_label.to_string())
            .or_default();

        for &threshold in &thresholds {
            if percentage >= threshold as u16 && !notified.contains(&threshold) {
                notified.push(threshold);
                return Some(threshold);
            }
        }

        None
    }

    /// Current progress snapshot for a single regime.
    pub fn progress(&self, regime_label: &str) -> Option<GoalProgress> {
        let target = *self.goals.get(regime_label)?;
        if target == 0 {
            return None;
        }
        let current = self.today_minutes.get(regime_label).copied().unwrap_or(0);
        let percentage = ((current as f64 / target as f64) * 100.0).min(u16::MAX as f64) as u16;

        Some(GoalProgress {
            regime_label: regime_label.to_string(),
            current_minutes: current,
            target_minutes: target,
            percentage,
        })
    }

    /// Progress snapshots for all regimes that have goals configured.
    pub fn all_progress(&self) -> Vec<GoalProgress> {
        self.goals
            .keys()
            .filter_map(|label| self.progress(label))
            .collect()
    }

    /// Clears counters and notified thresholds if the date has changed.
    fn ensure_date_rollover(&mut self) {
        let today = Local::now().date_naive();
        if today != self.tracking_date {
            self.today_minutes.clear();
            self.notified_thresholds.clear();
            self.tracking_date = today;
        }
    }
}

impl Default for RegimeGoalTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker_with_goal(label: &str, target: u32) -> RegimeGoalTracker {
        let mut tracker = RegimeGoalTracker::new();
        let mut goals = HashMap::new();
        goals.insert(label.to_string(), target);
        tracker.update_goals(&goals);
        tracker
    }

    #[test]
    fn record_and_check_threshold_25() {
        let mut tracker = tracker_with_goal("Deep Work", 100);
        // Record 25 minutes of 100 target = 25%
        tracker.record_minutes("Deep Work", 25);
        assert_eq!(tracker.check_threshold("Deep Work"), Some(25));
    }

    #[test]
    fn threshold_not_repeated() {
        let mut tracker = tracker_with_goal("Deep Work", 100);
        tracker.record_minutes("Deep Work", 25);
        assert_eq!(tracker.check_threshold("Deep Work"), Some(25));
        // Second check should not repeat the 25% threshold
        assert_eq!(tracker.check_threshold("Deep Work"), None);
        // Record 1 more minute — still below 50%
        tracker.record_minutes("Deep Work", 1);
        assert_eq!(tracker.check_threshold("Deep Work"), None);
    }

    #[test]
    fn multiple_thresholds_sequential() {
        let mut tracker = tracker_with_goal("Coding", 100);

        tracker.record_minutes("Coding", 25);
        assert_eq!(tracker.check_threshold("Coding"), Some(25));

        tracker.record_minutes("Coding", 25);
        assert_eq!(tracker.check_threshold("Coding"), Some(50));

        tracker.record_minutes("Coding", 25);
        assert_eq!(tracker.check_threshold("Coding"), Some(75));

        tracker.record_minutes("Coding", 25);
        assert_eq!(tracker.check_threshold("Coding"), Some(100));

        // No more thresholds to fire
        tracker.record_minutes("Coding", 25);
        assert_eq!(tracker.check_threshold("Coding"), None);
    }

    #[test]
    fn date_rollover_resets_counters() {
        let mut tracker = tracker_with_goal("Deep Work", 100);
        tracker.record_minutes("Deep Work", 50);
        assert_eq!(tracker.check_threshold("Deep Work"), Some(25));
        assert_eq!(tracker.check_threshold("Deep Work"), Some(50));

        // Simulate date change by setting tracking_date to yesterday
        tracker.tracking_date = Local::now().date_naive().pred_opt().unwrap();

        // Next record_minutes triggers rollover
        tracker.record_minutes("Deep Work", 10);
        // After rollover, only 10 minutes recorded (not 60)
        let progress = tracker.progress("Deep Work").unwrap();
        assert_eq!(progress.current_minutes, 10);

        // Thresholds are reset — 10% should not fire any threshold
        assert_eq!(tracker.check_threshold("Deep Work"), None);
    }

    #[test]
    fn no_goal_returns_none() {
        let mut tracker = RegimeGoalTracker::new();
        tracker.record_minutes("Unknown", 50);
        assert_eq!(tracker.check_threshold("Unknown"), None);
    }

    #[test]
    fn zero_target_returns_none() {
        let mut tracker = tracker_with_goal("Zero", 0);
        tracker.record_minutes("Zero", 50);
        assert_eq!(tracker.check_threshold("Zero"), None);
    }

    #[test]
    fn progress_returns_correct_values() {
        let mut tracker = tracker_with_goal("Focus", 120);
        tracker.record_minutes("Focus", 90);

        let progress = tracker.progress("Focus").unwrap();
        assert_eq!(progress.regime_label, "Focus");
        assert_eq!(progress.current_minutes, 90);
        assert_eq!(progress.target_minutes, 120);
        assert_eq!(progress.percentage, 75);
    }

    #[test]
    fn all_progress_includes_all_goals() {
        let mut tracker = RegimeGoalTracker::new();
        let mut goals = HashMap::new();
        goals.insert("Deep Work".to_string(), 120);
        goals.insert("Communication".to_string(), 60);
        goals.insert("Admin".to_string(), 30);
        tracker.update_goals(&goals);

        // Record minutes for only 2 of the 3 regimes
        tracker.record_minutes("Deep Work", 60);
        tracker.record_minutes("Communication", 30);

        let progress = tracker.all_progress();
        assert_eq!(progress.len(), 3, "all 3 goals should be included");

        // Verify each regime is present
        let labels: Vec<String> = progress.iter().map(|p| p.regime_label.clone()).collect();
        assert!(labels.contains(&"Deep Work".to_string()));
        assert!(labels.contains(&"Communication".to_string()));
        assert!(labels.contains(&"Admin".to_string()));

        // Admin should show 0 minutes
        let admin = progress.iter().find(|p| p.regime_label == "Admin").unwrap();
        assert_eq!(admin.current_minutes, 0);
        assert_eq!(admin.percentage, 0);
    }
}
