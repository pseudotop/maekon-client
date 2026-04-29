use chrono::Utc;
use oneshim_core::config::CoachingConfig;
use oneshim_core::models::coaching::{CoachingProfile, TriggerType};
use std::collections::HashMap;
use tracing::debug;

use super::CoachingEngine;

impl CoachingEngine {
    /// Detect which trigger type fired (if any).
    ///
    /// Priority order:
    /// 1. RegimeTransition (regime ID changed)
    /// 2. RegimeDrift (attention drift flagged)
    /// 3. GoalThreshold (milestone crossed)
    /// 4. RegimeOverstay (duration > 1.2x average)
    pub(super) async fn detect_trigger(
        &self,
        regime_id: Option<&str>,
        regime_label: &str,
        regime_duration_secs: u64,
        avg_regime_duration_secs: u64,
        drift_detected: bool,
    ) -> Option<TriggerType> {
        // 1. Regime transition — read current, compare, then drop guard before write
        let transition = {
            let current = self.current_regime_id.read().await;
            let current_str = current.as_deref();
            let changed = match (current_str, regime_id) {
                (Some(old), Some(new)) => old != new,
                (None, Some(_)) => true,
                (Some(_), None) => true,
                (None, None) => false,
            };
            if changed {
                Some(current.clone()) // clone the Option<String>, then guard drops
            } else {
                None
            }
        }; // read guard dropped here
        if let Some(from_regime) = transition {
            let to_regime = regime_id.map(String::from);
            // Update internal tracking (acquires write lock, safe now)
            self.on_regime_change(regime_id).await;
            return Some(TriggerType::RegimeTransition {
                from_regime,
                to_regime,
            });
        }

        // 2. Regime drift
        if drift_detected {
            return Some(TriggerType::RegimeDrift {
                regime_label: regime_label.to_string(),
            });
        }

        // 3. Goal threshold
        {
            let mut gt = self.goal_tracker.write().await;
            if let Some(threshold) = gt.check_threshold(regime_label) {
                let progress = gt.progress(regime_label);
                let (target, current) = progress
                    .map(|p| (p.target_minutes, p.current_minutes))
                    .unwrap_or((0, 0));
                return Some(TriggerType::GoalThreshold {
                    regime_label: regime_label.to_string(),
                    target_minutes: target,
                    current_minutes: current,
                    threshold_percent: threshold,
                });
            }
        }

        // 4. Regime overstay (duration > tunable ratio × average)
        let overstay_pct = self.tunable_params.read().await.overstay_percent();
        if avg_regime_duration_secs > 0
            && regime_duration_secs > avg_regime_duration_secs * overstay_pct / 100
        {
            return Some(TriggerType::RegimeOverstay {
                regime_label: regime_label.to_string(),
                duration_secs: regime_duration_secs,
                avg_duration_secs: avg_regime_duration_secs,
            });
        }

        None
    }

    /// Map a trigger to a coaching profile, checking if that profile is enabled.
    pub(super) fn match_profile(
        config: &CoachingConfig,
        trigger: &TriggerType,
        _regime_label: &str,
        _app_name: &str,
    ) -> Option<CoachingProfile> {
        let profile = match trigger {
            TriggerType::RegimeTransition { from_regime, .. } => {
                // If returning from idle/break -> ContextRestore
                let from_lower = from_regime.as_deref().unwrap_or("").to_lowercase();
                if from_lower.contains("idle")
                    || from_lower.contains("break")
                    || from_lower.contains("away")
                {
                    CoachingProfile::ContextRestore
                } else {
                    CoachingProfile::FocusGuard
                }
            }
            TriggerType::RegimeDrift { .. } => CoachingProfile::FocusGuard,
            TriggerType::RegimeOverstay { regime_label, .. } => {
                let label_lower = regime_label.to_lowercase();
                if label_lower.contains("deep")
                    || label_lower.contains("focus")
                    || label_lower.contains("coding")
                {
                    CoachingProfile::DeepWorkCoach
                } else {
                    CoachingProfile::TimeAware
                }
            }
            TriggerType::GoalThreshold { .. } => CoachingProfile::GoalTracker,
        };

        // Check if the matched profile is enabled
        let profile_name = format!("{:?}", profile);
        if let Some(profile_config) = config.profiles.get(&profile_name) {
            if !profile_config.enabled {
                debug!(profile = %profile_name, "coaching profile disabled");
                return None;
            }
        }

        Some(profile)
    }

    /// Update internal regime tracking when a regime change is detected.
    ///
    /// Before overwriting the current regime, records the completed dwell time
    /// and updates the per-regime EMA duration tracker.
    ///
    /// Lock ordering: `current_regime_entered(R)` -> `current_regime_id(R)` ->
    /// `regime_avg_duration(W)` -> `current_regime_id(W)` -> `current_regime_entered(W)`.
    pub async fn on_regime_change(&self, new_regime_id: Option<&str>) {
        // Record completed dwell time into the EMA tracker
        let entered = self.current_regime_entered.read().await;
        if let Some(enter_time) = *entered {
            let dwell_secs = (Utc::now() - enter_time).num_seconds().max(0) as f64;
            let prev_id = self.current_regime_id.read().await;
            if let Some(ref label) = *prev_id {
                let alpha = self.tunable_params.read().await.ema_alpha as f64;
                let mut avgs = self.regime_avg_duration.write().await;
                let ema = avgs.entry(label.clone()).or_insert(dwell_secs);
                *ema = *ema * (1.0 - alpha) + dwell_secs * alpha;
            }
        }
        drop(entered);

        let mut rid = self.current_regime_id.write().await;
        *rid = new_regime_id.map(String::from);
        let mut entered = self.current_regime_entered.write().await;
        *entered = Some(Utc::now());

        // Daily reset + increment context switch counter
        let today = Utc::now().date_naive();
        {
            let mut date = self.context_switch_date.write().await;
            let mut count = self.context_switch_count.write().await;
            if *date != today {
                *count = 0;
                *date = today;
            }
            *count += 1;
        }
    }

    /// Build the variable substitution map for template rendering.
    pub(super) async fn build_variables(
        &self,
        regime_label: &str,
        regime_duration_secs: u64,
        app_name: &str,
    ) -> HashMap<String, String> {
        let mut vars = HashMap::new();
        vars.insert("regime".to_string(), regime_label.to_string());
        vars.insert(
            "duration".to_string(),
            super::humanize_duration(regime_duration_secs),
        );
        vars.insert("app_name".to_string(), app_name.to_string());

        // Goal-related variables
        let gt = self.goal_tracker.read().await;
        if let Some(progress) = gt.progress(regime_label) {
            vars.insert("goal_progress".to_string(), progress.percentage.to_string());
            vars.insert(
                "goal_minutes".to_string(),
                progress.target_minutes.to_string(),
            );
            let remaining = progress
                .target_minutes
                .saturating_sub(progress.current_minutes);
            vars.insert("remaining_minutes".to_string(), remaining.to_string());
        } else {
            vars.insert("goal_progress".to_string(), "0".to_string());
            vars.insert("goal_minutes".to_string(), "0".to_string());
            vars.insert("remaining_minutes".to_string(), "0".to_string());
        }

        // Context switch count (daily)
        let switch_count = *self.context_switch_count.read().await;
        vars.insert("context_switches".to_string(), switch_count.to_string());

        // Historical comparison — EMA of regime dwell duration
        let avgs = self.regime_avg_duration.read().await;
        let avg_secs = avgs.get(regime_label).copied().unwrap_or(1800.0) as u64;
        vars.insert("comparison".to_string(), super::humanize_duration(avg_secs));

        // Previous context — last known regime before the current one
        let prev_regime = self.current_regime_id.read().await;
        vars.insert(
            "previous_context".to_string(),
            prev_regime.as_deref().unwrap_or("unknown").to_string(),
        );

        vars
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::config::{CoachingConfig, ProfileConfig};
    use oneshim_core::models::coaching::{CoachingProfile, TriggerType};
    use std::collections::HashMap;

    fn enabled_config() -> CoachingConfig {
        CoachingConfig {
            enabled: true,
            ..CoachingConfig::default()
        }
    }

    // ── detect_trigger: priority ordering ────────────────────────

    #[tokio::test]
    async fn detect_trigger_regime_transition_highest_priority() {
        let engine = CoachingEngine::new(enabled_config());
        // Set an initial regime so transition can be detected
        engine.on_regime_change(Some("old-regime")).await;

        // All conditions true: transition + drift + overstay
        let trigger = engine
            .detect_trigger(
                Some("new-regime"), // different from current -> transition
                "DeepWork",
                5000, // > 1.2 * 1800 -> overstay
                1800, // avg
                true, // drift_detected
            )
            .await;

        assert!(trigger.is_some());
        assert!(
            matches!(trigger.unwrap(), TriggerType::RegimeTransition { .. }),
            "RegimeTransition should have highest priority"
        );
    }

    #[tokio::test]
    async fn detect_trigger_regime_drift_over_overstay() {
        let engine = CoachingEngine::new(enabled_config());
        // Set regime so no transition fires
        engine.on_regime_change(Some("stable")).await;

        // Drift + overstay both true, but drift has higher priority
        let trigger = engine
            .detect_trigger(
                Some("stable"), // same regime -> no transition
                "Work",
                5000, // > 1.2 * 1800 -> overstay
                1800,
                true, // drift_detected
            )
            .await;

        assert!(trigger.is_some());
        assert!(
            matches!(trigger.unwrap(), TriggerType::RegimeDrift { .. }),
            "RegimeDrift should have priority over RegimeOverstay"
        );
    }

    #[tokio::test]
    async fn detect_trigger_goal_threshold_over_overstay() {
        // Setup with goal so threshold can fire
        let mut goals = HashMap::new();
        goals.insert("Coding".to_string(), 100);
        let config = CoachingConfig {
            enabled: true,
            regime_goals: goals,
            ..CoachingConfig::default()
        };
        let engine = CoachingEngine::new(config);
        engine.on_regime_change(Some("r1")).await;

        // Record minutes to cross 25% threshold
        engine.record_minutes("Coding", 25).await;

        // No drift, but overstay and goal threshold both qualify
        let trigger = engine
            .detect_trigger(
                Some("r1"), // same regime
                "Coding",
                5000, // > 1.2 * 1800 -> overstay
                1800,
                false, // no drift
            )
            .await;

        assert!(trigger.is_some());
        assert!(
            matches!(trigger.unwrap(), TriggerType::GoalThreshold { .. }),
            "GoalThreshold should have priority over RegimeOverstay"
        );
    }

    #[tokio::test]
    async fn detect_trigger_overstay_fires_when_no_other_trigger() {
        let engine = CoachingEngine::new(enabled_config());
        engine.on_regime_change(Some("r1")).await;

        // Only overstay condition met: duration > 1.2x average
        let trigger = engine
            .detect_trigger(
                Some("r1"), // same regime
                "Email",
                3000, // > 1.2 * 1800 = 2160
                1800,
                false, // no drift
            )
            .await;

        assert!(trigger.is_some());
        assert!(
            matches!(trigger.unwrap(), TriggerType::RegimeOverstay { .. }),
            "RegimeOverstay should fire as lowest priority"
        );
    }

    #[tokio::test]
    async fn detect_trigger_returns_none_when_no_condition_met() {
        let engine = CoachingEngine::new(enabled_config());
        engine.on_regime_change(Some("r1")).await;

        // Same regime, no drift, no goal, no overstay (duration <= 1.2x avg)
        let trigger = engine
            .detect_trigger(
                Some("r1"),
                "Work",
                1800, // exactly avg, not > 1.2x
                1800,
                false,
            )
            .await;

        assert!(
            trigger.is_none(),
            "no trigger conditions met should return None"
        );
    }

    // ── detect_trigger: edge cases ───────────────────────────────

    #[tokio::test]
    async fn detect_trigger_no_regime_to_some_is_transition() {
        let engine = CoachingEngine::new(enabled_config());
        // Engine starts with current_regime_id = None (default)

        let trigger = engine
            .detect_trigger(Some("first-regime"), "Work", 0, 1800, false)
            .await;

        assert!(trigger.is_some());
        match trigger.unwrap() {
            TriggerType::RegimeTransition {
                from_regime,
                to_regime,
            } => {
                assert!(from_regime.is_none(), "from should be None");
                assert_eq!(to_regime, Some("first-regime".to_string()));
            }
            other => panic!("expected RegimeTransition, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn detect_trigger_some_regime_to_none_is_transition() {
        let engine = CoachingEngine::new(enabled_config());
        engine.on_regime_change(Some("active-regime")).await;

        let trigger = engine.detect_trigger(None, "Unknown", 0, 1800, false).await;

        assert!(trigger.is_some());
        match trigger.unwrap() {
            TriggerType::RegimeTransition {
                from_regime,
                to_regime,
            } => {
                assert_eq!(from_regime, Some("active-regime".to_string()));
                assert!(to_regime.is_none(), "to should be None");
            }
            other => panic!("expected RegimeTransition, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn detect_trigger_none_to_none_no_transition() {
        let engine = CoachingEngine::new(enabled_config());
        // Engine starts with None, evaluate with None -> no transition

        let trigger = engine.detect_trigger(None, "Idle", 0, 1800, false).await;

        assert!(trigger.is_none(), "None to None should not be a transition");
    }

    #[tokio::test]
    async fn detect_trigger_zero_avg_duration_no_overstay() {
        let engine = CoachingEngine::new(enabled_config());
        engine.on_regime_change(Some("r1")).await;

        // avg_regime_duration_secs = 0 should not trigger overstay
        // (division by zero guard)
        let trigger = engine
            .detect_trigger(Some("r1"), "Work", 10000, 0, false)
            .await;

        assert!(
            trigger.is_none(),
            "zero avg duration should not trigger overstay"
        );
    }

    #[tokio::test]
    async fn detect_trigger_duration_exactly_1_2x_no_overstay() {
        let engine = CoachingEngine::new(enabled_config());
        engine.on_regime_change(Some("r1")).await;

        // duration = 1.2 * avg exactly -> NOT overstay (need strictly >)
        // 1.2 * 1000 = 1200
        let trigger = engine
            .detect_trigger(Some("r1"), "Work", 1200, 1000, false)
            .await;

        assert!(
            trigger.is_none(),
            "duration exactly 1.2x avg should NOT trigger overstay"
        );
    }

    #[tokio::test]
    async fn detect_trigger_duration_just_above_1_2x_fires_overstay() {
        let engine = CoachingEngine::new(enabled_config());
        engine.on_regime_change(Some("r1")).await;

        // 1201 > 1200 (1.2 * 1000) -> overstay
        let trigger = engine
            .detect_trigger(Some("r1"), "Work", 1201, 1000, false)
            .await;

        assert!(trigger.is_some());
        match trigger.unwrap() {
            TriggerType::RegimeOverstay {
                duration_secs,
                avg_duration_secs,
                ..
            } => {
                assert_eq!(duration_secs, 1201);
                assert_eq!(avg_duration_secs, 1000);
            }
            other => panic!("expected RegimeOverstay, got {:?}", other),
        }
    }

    // ── match_profile tests ──────────────────────────────────────

    #[test]
    fn match_profile_transition_from_idle_returns_context_restore() {
        let config = enabled_config();
        let trigger = TriggerType::RegimeTransition {
            from_regime: Some("idle-mode".to_string()),
            to_regime: Some("work".to_string()),
        };
        let result = CoachingEngine::match_profile(&config, &trigger, "Work", "VS Code");
        assert_eq!(result, Some(CoachingProfile::ContextRestore));
    }

    #[test]
    fn match_profile_transition_from_break_returns_context_restore() {
        let config = enabled_config();
        let trigger = TriggerType::RegimeTransition {
            from_regime: Some("coffee-break".to_string()),
            to_regime: Some("coding".to_string()),
        };
        let result = CoachingEngine::match_profile(&config, &trigger, "Coding", "VS Code");
        assert_eq!(result, Some(CoachingProfile::ContextRestore));
    }

    #[test]
    fn match_profile_transition_from_away_returns_context_restore() {
        let config = enabled_config();
        let trigger = TriggerType::RegimeTransition {
            from_regime: Some("away-from-desk".to_string()),
            to_regime: Some("work".to_string()),
        };
        let result = CoachingEngine::match_profile(&config, &trigger, "Work", "VS Code");
        assert_eq!(result, Some(CoachingProfile::ContextRestore));
    }

    #[test]
    fn match_profile_transition_non_idle_returns_focus_guard() {
        let config = enabled_config();
        let trigger = TriggerType::RegimeTransition {
            from_regime: Some("email".to_string()),
            to_regime: Some("coding".to_string()),
        };
        let result = CoachingEngine::match_profile(&config, &trigger, "Coding", "VS Code");
        assert_eq!(result, Some(CoachingProfile::FocusGuard));
    }

    #[test]
    fn match_profile_transition_from_none_returns_focus_guard() {
        let config = enabled_config();
        let trigger = TriggerType::RegimeTransition {
            from_regime: None,
            to_regime: Some("work".to_string()),
        };
        let result = CoachingEngine::match_profile(&config, &trigger, "Work", "VS Code");
        assert_eq!(result, Some(CoachingProfile::FocusGuard));
    }

    #[test]
    fn match_profile_drift_returns_focus_guard() {
        let config = enabled_config();
        let trigger = TriggerType::RegimeDrift {
            regime_label: "Coding".to_string(),
        };
        let result = CoachingEngine::match_profile(&config, &trigger, "Coding", "VS Code");
        assert_eq!(result, Some(CoachingProfile::FocusGuard));
    }

    #[test]
    fn match_profile_overstay_deep_work_returns_deep_work_coach() {
        let config = enabled_config();
        let trigger = TriggerType::RegimeOverstay {
            regime_label: "Deep Work".to_string(),
            duration_secs: 7200,
            avg_duration_secs: 3600,
        };
        let result = CoachingEngine::match_profile(&config, &trigger, "Deep Work", "VS Code");
        assert_eq!(result, Some(CoachingProfile::DeepWorkCoach));
    }

    #[test]
    fn match_profile_overstay_focus_returns_deep_work_coach() {
        let config = enabled_config();
        let trigger = TriggerType::RegimeOverstay {
            regime_label: "Focus Session".to_string(),
            duration_secs: 7200,
            avg_duration_secs: 3600,
        };
        let result = CoachingEngine::match_profile(&config, &trigger, "Focus Session", "VS Code");
        assert_eq!(result, Some(CoachingProfile::DeepWorkCoach));
    }

    #[test]
    fn match_profile_overstay_coding_returns_deep_work_coach() {
        let config = enabled_config();
        let trigger = TriggerType::RegimeOverstay {
            regime_label: "Coding Sprint".to_string(),
            duration_secs: 5000,
            avg_duration_secs: 3000,
        };
        let result = CoachingEngine::match_profile(&config, &trigger, "Coding Sprint", "VS Code");
        assert_eq!(result, Some(CoachingProfile::DeepWorkCoach));
    }

    #[test]
    fn match_profile_overstay_generic_returns_time_aware() {
        let config = enabled_config();
        let trigger = TriggerType::RegimeOverstay {
            regime_label: "Email".to_string(),
            duration_secs: 7200,
            avg_duration_secs: 3600,
        };
        let result = CoachingEngine::match_profile(&config, &trigger, "Email", "Outlook");
        assert_eq!(result, Some(CoachingProfile::TimeAware));
    }

    #[test]
    fn match_profile_goal_threshold_returns_goal_tracker() {
        let config = enabled_config();
        let trigger = TriggerType::GoalThreshold {
            regime_label: "Deep Work".to_string(),
            target_minutes: 120,
            current_minutes: 30,
            threshold_percent: 25,
        };
        let result = CoachingEngine::match_profile(&config, &trigger, "Deep Work", "VS Code");
        assert_eq!(result, Some(CoachingProfile::GoalTracker));
    }

    #[test]
    fn match_profile_disabled_profile_returns_none() {
        let mut profiles = HashMap::new();
        profiles.insert(
            "FocusGuard".to_string(),
            ProfileConfig {
                enabled: false,
                min_interval_secs: 300,
            },
        );
        let config = CoachingConfig {
            enabled: true,
            profiles,
            ..CoachingConfig::default()
        };

        // RegimeDrift maps to FocusGuard, but FocusGuard is disabled
        let trigger = TriggerType::RegimeDrift {
            regime_label: "Work".to_string(),
        };
        let result = CoachingEngine::match_profile(&config, &trigger, "Work", "VS Code");
        assert!(result.is_none(), "disabled profile should return None");
    }

    #[test]
    fn match_profile_unconfigured_profile_still_returns_some() {
        // Config with empty profiles map -> profile not found -> defaults to enabled
        let config = CoachingConfig {
            enabled: true,
            profiles: HashMap::new(),
            ..CoachingConfig::default()
        };
        let trigger = TriggerType::RegimeDrift {
            regime_label: "Work".to_string(),
        };
        let result = CoachingEngine::match_profile(&config, &trigger, "Work", "VS Code");
        assert_eq!(
            result,
            Some(CoachingProfile::FocusGuard),
            "unconfigured profile should default to enabled"
        );
    }

    // ── on_regime_change tests ───────────────────────────────────

    #[tokio::test]
    async fn on_regime_change_updates_current_id() {
        let engine = CoachingEngine::new(enabled_config());
        engine.on_regime_change(Some("regime-x")).await;

        let rid = engine.current_regime_id.read().await;
        assert_eq!(*rid, Some("regime-x".to_string()));
    }

    #[tokio::test]
    async fn on_regime_change_sets_entered_timestamp() {
        let engine = CoachingEngine::new(enabled_config());
        let before = Utc::now();
        engine.on_regime_change(Some("r1")).await;
        let after = Utc::now();

        let entered = engine.current_regime_entered.read().await;
        let ts = entered.expect("entered timestamp should be set");
        assert!(ts >= before && ts <= after);
    }

    #[tokio::test]
    async fn on_regime_change_increments_context_switch_count() {
        let engine = CoachingEngine::new(enabled_config());
        engine.on_regime_change(Some("a")).await;
        engine.on_regime_change(Some("b")).await;
        engine.on_regime_change(Some("c")).await;

        let count = *engine.context_switch_count.read().await;
        assert_eq!(count, 3, "3 regime changes should give count 3");
    }

    #[tokio::test]
    async fn on_regime_change_records_ema_duration() {
        let engine = CoachingEngine::new(enabled_config());
        engine.on_regime_change(Some("coding")).await;
        // on_regime_change uses num_seconds() which truncates sub-second dwell
        // to 0. Sleep 1.1s to ensure at least 1 second dwell.
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        engine.on_regime_change(Some("email")).await;

        let avgs = engine.regime_avg_duration.read().await;
        let coding_avg = avgs.get("coding");
        assert!(
            coding_avg.is_some(),
            "EMA should be recorded for completed regime"
        );
        assert!(
            *coding_avg.unwrap() > 0.0,
            "EMA duration should be positive after 1+ second dwell"
        );
    }

    // ── build_variables tests ────────────────────────────────────

    #[tokio::test]
    async fn build_variables_contains_required_keys() {
        let engine = CoachingEngine::new(enabled_config());
        let vars = engine.build_variables("TestRegime", 7200, "TestApp").await;

        assert_eq!(vars.get("regime").unwrap(), "TestRegime");
        assert_eq!(vars.get("duration").unwrap(), "2h");
        assert_eq!(vars.get("app_name").unwrap(), "TestApp");
        assert!(vars.contains_key("goal_progress"));
        assert!(vars.contains_key("goal_minutes"));
        assert!(vars.contains_key("remaining_minutes"));
        assert!(vars.contains_key("context_switches"));
        assert!(vars.contains_key("comparison"));
        assert!(vars.contains_key("previous_context"));
    }

    #[tokio::test]
    async fn build_variables_previous_context_defaults_to_unknown() {
        let engine = CoachingEngine::new(enabled_config());
        let vars = engine.build_variables("Work", 600, "App").await;

        // No regime set -> current_regime_id is None -> "unknown"
        assert_eq!(vars.get("previous_context").unwrap(), "unknown");
    }

    #[tokio::test]
    async fn build_variables_reflects_context_switches() {
        let engine = CoachingEngine::new(enabled_config());
        engine.on_regime_change(Some("a")).await;
        engine.on_regime_change(Some("b")).await;

        let vars = engine.build_variables("Work", 600, "App").await;
        assert_eq!(vars.get("context_switches").unwrap(), "2");
    }
}
