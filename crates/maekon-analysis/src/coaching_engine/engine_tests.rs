use std::collections::HashMap;
use std::time::Duration;

use chrono::{Local, Timelike, Utc};
use maekon_core::config::{CoachingConfig, ProfileConfig, TimeRange};
use maekon_core::models::coaching::{CoachingProfile, TriggerType};

use super::{helpers::humanize_duration, CoachingEngine};

fn disabled_config() -> CoachingConfig {
    CoachingConfig {
        enabled: false,
        ..CoachingConfig::default()
    }
}

fn enabled_config() -> CoachingConfig {
    CoachingConfig {
        enabled: true,
        ..CoachingConfig::default()
    }
}

#[tokio::test]
async fn evaluate_returns_none_when_disabled() {
    let engine = CoachingEngine::new(disabled_config());
    let result = engine
        .evaluate(Some("r1"), "Deep Work", 600, 1800, false, "VS Code")
        .await;
    assert!(result.is_none(), "disabled engine must return None");
}

#[tokio::test]
async fn evaluate_returns_none_during_quiet_hours() {
    let now = Local::now();
    let start = format!("{:02}:{:02}", now.time().hour(), 0);
    let end_hour = (now.time().hour() + 1) % 24;
    let end = format!("{:02}:{:02}", end_hour, 0);

    let config = CoachingConfig {
        enabled: true,
        quiet_hours: vec![TimeRange { start, end }],
        ..CoachingConfig::default()
    };
    let engine = CoachingEngine::new(config);
    let result = engine
        .evaluate(Some("r1"), "Deep Work", 600, 1800, false, "VS Code")
        .await;
    assert!(result.is_none(), "quiet hours must suppress coaching");
}

#[tokio::test]
async fn evaluate_fires_regime_transition() {
    let engine = CoachingEngine::new(enabled_config());

    // Set initial regime
    engine.on_regime_change(Some("regime-a")).await;

    // Evaluate with a different regime -> transition
    let result = engine
        .evaluate(Some("regime-b"), "Communication", 60, 1800, false, "Slack")
        .await;
    assert!(result.is_some(), "regime transition should fire");
    let msg = result.unwrap();
    match msg.trigger {
        TriggerType::RegimeTransition { .. } => {}
        other => panic!("expected RegimeTransition, got {:?}", other),
    }
}

#[tokio::test]
async fn evaluate_fires_overstay() {
    let engine = CoachingEngine::new(enabled_config());
    // Set regime so we don't get a transition trigger
    engine.on_regime_change(Some("regime-a")).await;

    // Duration > 1.2x average -> overstay
    let result = engine
        .evaluate(Some("regime-a"), "Email", 3600, 1800, false, "Outlook")
        .await;
    assert!(result.is_some(), "overstay should fire");
    let msg = result.unwrap();
    match msg.trigger {
        TriggerType::RegimeOverstay { .. } => {}
        other => panic!("expected RegimeOverstay, got {:?}", other),
    }
}

#[tokio::test]
async fn evaluate_respects_cooldown() {
    let config = CoachingConfig {
        enabled: true,
        profiles: {
            let mut p = HashMap::new();
            p.insert(
                "FocusGuard".to_string(),
                ProfileConfig {
                    enabled: true,
                    min_interval_secs: 600, // 10 minutes
                },
            );
            p
        },
        ..CoachingConfig::default()
    };
    let engine = CoachingEngine::new(config);

    // First call: regime transition triggers FocusGuard
    engine.on_regime_change(Some("regime-a")).await;
    let first = engine
        .evaluate(Some("regime-b"), "Work", 60, 1800, false, "VS Code")
        .await;
    assert!(first.is_some(), "first call should fire");

    // Second call immediately: should be on cooldown
    engine.on_regime_change(Some("regime-b")).await;
    let second = engine
        .evaluate(Some("regime-c"), "Work", 60, 1800, false, "Chrome")
        .await;
    assert!(second.is_none(), "second call should be on cooldown");
}

#[tokio::test]
async fn evaluate_fires_goal_threshold() {
    let mut goals = HashMap::new();
    goals.insert("Coding".to_string(), 100);
    let config = CoachingConfig {
        enabled: true,
        regime_goals: goals,
        ..CoachingConfig::default()
    };
    let engine = CoachingEngine::new(config);
    // Set regime so we don't get transition
    engine.on_regime_change(Some("regime-a")).await;

    // Record enough minutes to cross 25% threshold
    engine.record_minutes("Coding", 25).await;

    // Evaluate — should trigger GoalThreshold via check_threshold
    let result = engine
        .evaluate(Some("regime-a"), "Coding", 60, 1800, false, "VS Code")
        .await;
    assert!(result.is_some(), "goal threshold should fire");
    let msg = result.unwrap();
    match msg.trigger {
        TriggerType::GoalThreshold {
            threshold_percent, ..
        } => {
            assert_eq!(threshold_percent, 25);
        }
        other => panic!("expected GoalThreshold, got {:?}", other),
    }
}

#[tokio::test]
async fn profile_matching_context_restore() {
    let engine = CoachingEngine::new(enabled_config());

    // Simulate returning from "idle" regime
    engine.on_regime_change(Some("idle-regime")).await;
    {
        // Manually set the label to contain "idle" — the from_regime
        // in the transition carries the old regime ID, but match_profile
        // checks the from_regime string
        let mut rid = engine.current_regime_id.write().await;
        *rid = Some("idle".to_string());
    }

    let result = engine
        .evaluate(Some("work-regime"), "Work", 60, 1800, false, "VS Code")
        .await;
    assert!(result.is_some());
    let msg = result.unwrap();
    assert_eq!(
        msg.profile,
        CoachingProfile::ContextRestore,
        "transition from idle should map to ContextRestore"
    );
}

#[test]
fn humanize_duration_formats_correctly() {
    assert_eq!(humanize_duration(3750), "1h 2m");
    assert_eq!(humanize_duration(7200), "2h");
    assert_eq!(humanize_duration(300), "5m");
    assert_eq!(humanize_duration(0), "0m");
    assert_eq!(humanize_duration(59), "0m");
    assert_eq!(humanize_duration(60), "1m");
    assert_eq!(humanize_duration(3600), "1h");
}

#[tokio::test]
async fn build_variables_includes_goal_data() {
    let mut goals = HashMap::new();
    goals.insert("Deep Work".to_string(), 120);
    let config = CoachingConfig {
        enabled: true,
        regime_goals: goals,
        ..CoachingConfig::default()
    };
    let engine = CoachingEngine::new(config);

    // Record some minutes
    engine.record_minutes("Deep Work", 60).await;

    let vars = engine.build_variables("Deep Work", 3600, "VS Code").await;
    assert_eq!(vars.get("regime").unwrap(), "Deep Work");
    assert_eq!(vars.get("duration").unwrap(), "1h");
    assert_eq!(vars.get("app_name").unwrap(), "VS Code");
    assert_eq!(vars.get("goal_progress").unwrap(), "50");
    assert_eq!(vars.get("goal_minutes").unwrap(), "120");
    assert_eq!(vars.get("remaining_minutes").unwrap(), "60");
}

/// Smoke integration test: construct CoachingEngine, verify evaluate()
/// returns None when disabled (per review fix instructions).
#[tokio::test]
async fn smoke_test_disabled_engine_returns_none() {
    let engine = CoachingEngine::new(CoachingConfig::default());
    // Default config has enabled=false
    let result = engine
        .evaluate(Some("r1"), "Label", 100, 200, true, "App")
        .await;
    assert!(
        result.is_none(),
        "evaluate() must return None when coaching is disabled"
    );
}

// ── Phase 2 method tests ─────────────────────────────────────

#[tokio::test]
async fn snooze_current_profile_suppresses_evaluation() {
    let engine = CoachingEngine::new(enabled_config());
    engine.on_regime_change(Some("regime-a")).await;

    // Snooze the "FocusGuard" profile for 60 seconds.
    // A regime transition from a non-idle regime maps to FocusGuard.
    engine
        .snooze_current_profile("FocusGuard", Duration::from_secs(60))
        .await;

    // Evaluate with a regime transition -> triggers FocusGuard profile
    let result = engine
        .evaluate(Some("regime-b"), "Communication", 60, 1800, false, "Slack")
        .await;

    // Snooze suppresses the matched FocusGuard profile
    assert!(
        result.is_none(),
        "snoozed profile should suppress evaluation"
    );
}

#[tokio::test]
async fn all_goal_progress_returns_views_with_colors() {
    let mut goals = HashMap::new();
    goals.insert("Deep Work".to_string(), 120);
    goals.insert("Communication".to_string(), 60);
    let config = CoachingConfig {
        enabled: true,
        regime_goals: goals,
        ..CoachingConfig::default()
    };
    let engine = CoachingEngine::new(config);

    engine.record_minutes("Deep Work", 30).await;
    engine.record_minutes("Communication", 45).await;

    let views = engine.all_goal_progress().await;
    assert_eq!(views.len(), 2);
    // All views should have a non-empty display_color
    for view in &views {
        assert!(!view.display_color.is_empty());
        assert!(view.display_color.starts_with('#'));
    }
}

#[tokio::test]
async fn update_regime_goals_changes_tracker() {
    let engine = CoachingEngine::new(enabled_config());

    let mut goals = HashMap::new();
    goals.insert("Coding".to_string(), 180);
    goals.insert("Email".to_string(), 30);
    engine.update_regime_goals(&goals).await;

    let views = engine.all_goal_progress().await;
    assert_eq!(views.len(), 2);
    let coding = views.iter().find(|v| v.regime_label == "Coding").unwrap();
    assert_eq!(coding.target_minutes, 180);
}

#[tokio::test]
async fn avg_regime_duration_updates_on_transition() {
    let engine = CoachingEngine::new(enabled_config());
    engine.on_regime_change(Some("r-a")).await;
    // Simulate a short dwell in regime-a
    tokio::time::sleep(Duration::from_millis(50)).await;
    engine.on_regime_change(Some("r-b")).await;
    let avg = engine.avg_regime_duration_secs("r-a").await;
    // Should be > 0 (actual dwell) and < 1800 (default)
    assert!(
        avg < 1800,
        "avg should reflect actual short dwell, got {}",
        avg
    );
}

#[tokio::test]
async fn context_switch_count_increments() {
    let engine = CoachingEngine::new(enabled_config());
    engine.on_regime_change(Some("r-a")).await;
    engine.on_regime_change(Some("r-b")).await;
    engine.on_regime_change(Some("r-c")).await;
    let vars = engine.build_variables("Test", 600, "VS Code").await;
    assert_eq!(vars.get("context_switches").unwrap(), "3");
}

#[tokio::test]
async fn implicit_feedback_uses_internal_state() {
    let engine = CoachingEngine::new(enabled_config());
    // Simulate an evaluate() call that sets internal state
    engine.on_regime_change(Some("r-a")).await;
    {
        let mut app = engine.last_app_name.write().await;
        *app = "VS Code".to_string();
    }
    // Register a pending message
    engine
        .register_pending_feedback(
            "msg-1",
            "FocusGuard",
            "RegimeTransition",
            Some("r-a"),
            "VS Code",
        )
        .await;
    // Simulate regime change (so implicit feedback would detect it)
    engine.on_regime_change(Some("r-b")).await;
    // Call with placeholder args — should use internal state
    let future = Utc::now() + chrono::Duration::seconds(301);
    engine.evaluate_implicit_feedback(None, "", future).await;
    // Pending should be consumed
    let ft = engine.feedback_tracker.read().await;
    assert_eq!(ft.pending_count(), 0);
}

// ── Gap 6: Full coaching cycle integration test ──────────────

/// Helper: create an enabled config with FocusGuard profile and a regime goal.
fn focus_guard_config_with_goal(regime: &str, target_minutes: u32) -> CoachingConfig {
    let mut goals = HashMap::new();
    goals.insert(regime.to_string(), target_minutes);
    CoachingConfig {
        enabled: true,
        regime_goals: goals,
        ..CoachingConfig::default()
    }
}

/// Helper: clear all cooldowns so the next evaluate() is not suppressed.
async fn clear_cooldowns(engine: &CoachingEngine) {
    let mut la = engine.last_alert.write().await;
    la.clear();
}

/// Integration test exercising the full coaching cycle:
///
/// 1. Construct CoachingEngine with enabled config + FocusGuard + regime goal
/// 2. Regime transition (None -> "deep-work") -> RegimeTransition message
/// 3. Record 60 min on deep-work -> GoalThreshold 50%
/// 4. Record 60 more min -> GoalThreshold 100%
/// 5. Drift detection -> RegimeDrift message
/// 6. Snooze FocusGuard, evaluate again -> no message
/// 7. Cooldown: evaluate immediately -> no message (within 5min cooldown)
/// 8. Explicit positive feedback -> effectiveness score updates
#[tokio::test]
async fn integration_full_coaching_cycle() {
    // ── Step 1: Setup ──────────────────────────────────────
    let config = focus_guard_config_with_goal("DeepWork", 120);
    let engine = CoachingEngine::new(config);

    // ── Step 2: Regime transition (None -> "deep-work") ────
    // Engine starts with no regime. Evaluating with a regime_id triggers
    // a transition from None -> Some("deep-work").
    let msg1 = engine
        .evaluate(Some("deep-work"), "DeepWork", 0, 1800, false, "VS Code")
        .await;
    assert!(
        msg1.is_some(),
        "step 2: initial regime should fire RegimeTransition"
    );
    let m1 = msg1.unwrap();
    assert!(
        matches!(m1.trigger, TriggerType::RegimeTransition { .. }),
        "step 2: expected RegimeTransition, got {:?}",
        m1.trigger
    );

    // ── Step 3: Record 60 minutes -> GoalThreshold 50% ────
    // Clear cooldown so the next evaluate() is not suppressed.
    clear_cooldowns(&engine).await;
    engine.record_minutes("DeepWork", 60).await;

    // Same regime, no transition, no drift -> goal threshold check
    let msg2 = engine
        .evaluate(Some("deep-work"), "DeepWork", 3600, 1800, false, "VS Code")
        .await;
    assert!(msg2.is_some(), "step 3: 50% goal threshold should fire");
    let m2 = msg2.unwrap();
    match &m2.trigger {
        TriggerType::GoalThreshold {
            threshold_percent, ..
        } => {
            // 60 / 120 = 50% -> first uncrossed threshold is 25%, but
            // check_threshold fires the lowest uncrossed, so 25% fires first.
            // After that, 50% fires. Both 25% and 50% are crossed at 60 min.
            // check_threshold returns the *first* uncrossed threshold sequentially.
            assert!(
                *threshold_percent == 25 || *threshold_percent == 50,
                "step 3: expected 25% or 50% threshold, got {}%",
                threshold_percent
            );
        }
        other => panic!("step 3: expected GoalThreshold, got {:?}", other),
    }

    // If 25% fired first, evaluate again to get 50%
    if matches!(
        m2.trigger,
        TriggerType::GoalThreshold {
            threshold_percent: 25,
            ..
        }
    ) {
        clear_cooldowns(&engine).await;
        let msg2b = engine
            .evaluate(Some("deep-work"), "DeepWork", 3600, 1800, false, "VS Code")
            .await;
        assert!(
            msg2b.is_some(),
            "step 3b: 50% threshold should fire after 25%"
        );
        let m2b = msg2b.unwrap();
        match &m2b.trigger {
            TriggerType::GoalThreshold {
                threshold_percent, ..
            } => {
                assert_eq!(*threshold_percent, 50, "step 3b: expected 50%");
            }
            other => panic!("step 3b: expected GoalThreshold, got {:?}", other),
        }
    }

    // ── Step 4: Record 60 more minutes -> GoalThreshold 100% ─
    clear_cooldowns(&engine).await;
    engine.record_minutes("DeepWork", 60).await;

    let msg3 = engine
        .evaluate(Some("deep-work"), "DeepWork", 7200, 1800, false, "VS Code")
        .await;
    assert!(msg3.is_some(), "step 4: 100% goal threshold should fire");
    let m3 = msg3.unwrap();
    match &m3.trigger {
        TriggerType::GoalThreshold {
            threshold_percent, ..
        } => {
            // 75% or 100% should fire (75% not yet notified, fires first)
            assert!(
                *threshold_percent == 75 || *threshold_percent == 100,
                "step 4: expected 75% or 100%, got {}%",
                threshold_percent
            );
        }
        other => panic!("step 4: expected GoalThreshold, got {:?}", other),
    }

    // Drain remaining thresholds to reach 100%
    let mut hit_100 = matches!(
        m3.trigger,
        TriggerType::GoalThreshold {
            threshold_percent: 100,
            ..
        }
    );
    while !hit_100 {
        clear_cooldowns(&engine).await;
        let msg = engine
            .evaluate(Some("deep-work"), "DeepWork", 7200, 1800, false, "VS Code")
            .await;
        match msg {
            Some(m) => match &m.trigger {
                TriggerType::GoalThreshold {
                    threshold_percent: 100,
                    ..
                } => {
                    hit_100 = true;
                }
                TriggerType::GoalThreshold { .. } => {
                    // Intermediate threshold (75%), continue
                }
                _ => break,
            },
            None => break,
        }
    }
    assert!(hit_100, "step 4: should have reached 100% goal threshold");

    // ── Step 5: Drift detection -> RegimeDrift message ─────
    clear_cooldowns(&engine).await;
    let msg4 = engine
        .evaluate(
            Some("deep-work"),
            "DeepWork",
            300,
            1800,
            true, // drift_detected = true
            "VS Code",
        )
        .await;
    assert!(msg4.is_some(), "step 5: drift should fire");
    let m4 = msg4.unwrap();
    assert!(
        matches!(m4.trigger, TriggerType::RegimeDrift { .. }),
        "step 5: expected RegimeDrift, got {:?}",
        m4.trigger
    );

    // ── Step 6: Snooze FocusGuard, evaluate again -> no message
    engine
        .snooze_current_profile("FocusGuard", Duration::from_secs(60))
        .await;
    clear_cooldowns(&engine).await;
    // Trigger another drift (which maps to FocusGuard profile)
    let msg5 = engine
        .evaluate(Some("deep-work"), "DeepWork", 300, 1800, true, "VS Code")
        .await;
    assert!(
        msg5.is_none(),
        "step 6: snoozed FocusGuard should suppress drift message"
    );

    // ── Step 7: Cooldown — evaluate immediately -> no message
    // Un-snooze first by clearing the snooze, then rely on the 5-min
    // (300s) default cooldown from the step 5 alert.
    {
        let mut guard = engine.snoozed_until.write().await;
        *guard = None;
    }
    // Restore the step 5 alert timestamp so cooldown is active.
    // (We cleared cooldowns for step 6, but step 5's alert was real.)
    {
        let mut la = engine.last_alert.write().await;
        la.insert("FocusGuard".to_string(), Utc::now());
    }
    let msg6 = engine
        .evaluate(Some("deep-work"), "DeepWork", 300, 1800, true, "VS Code")
        .await;
    assert!(
        msg6.is_none(),
        "step 7: cooldown should suppress repeated alert"
    );

    // ── Step 8: Explicit positive feedback -> effectiveness update
    // Use the message from step 5 (drift message)
    let drift_msg_id = m4.message_id.clone();
    let profile_name = format!("{:?}", m4.profile);
    let trigger_name = maekon_core::models::coaching::trigger_type_name(&m4.trigger);

    engine
        .register_pending_feedback(
            &drift_msg_id,
            &profile_name,
            &trigger_name,
            Some("deep-work"),
            "VS Code",
        )
        .await;
    engine.record_explicit_feedback(&drift_msg_id, true).await;

    // Verify effectiveness score was updated
    let ft = engine.feedback_tracker.read().await;
    let score = ft
        .get_effectiveness(&profile_name, &trigger_name)
        .expect("step 8: effectiveness score should exist");
    assert!(
        score.positive_signals > 0.0,
        "step 8: positive_signals should be > 0 after explicit positive feedback, got {}",
        score.positive_signals
    );
    assert_eq!(score.total_shown, 1, "step 8: total_shown should be 1");
}

// ── build_explanation tests ──────────────────────────────────

#[test]
fn generates_explanation_for_regime_transition() {
    let trigger = TriggerType::RegimeTransition {
        from_regime: Some("Deep Work".to_string()),
        to_regime: Some("Communication".to_string()),
    };
    let profile = CoachingProfile::FocusGuard;
    let explanation = CoachingEngine::build_explanation(&trigger, &profile);

    assert!(
        explanation.contains("Deep Work"),
        "should contain from regime name"
    );
    assert!(
        explanation.contains("Communication"),
        "should contain to regime name"
    );
    assert!(
        explanation.contains("FocusGuard"),
        "should contain profile name"
    );
    assert!(
        explanation.contains("context switch"),
        "should mention context switch"
    );
}

// ── #5707: apply_config hot-reload tests ─────────────────────────────────────

/// `CoachingPort::apply_config` delegates to `CoachingEngine::update_config`:
/// after the call the engine reflects the new config (enabled flag visible via evaluate).
#[tokio::test]
async fn apply_config_delegates_to_update_config() {
    use maekon_core::ports::coaching::CoachingPort;
    let engine = CoachingEngine::new(CoachingConfig {
        enabled: false,
        ..CoachingConfig::default()
    });

    // Verify disabled initially.
    let result = engine
        .evaluate(Some("r1"), "Deep Work", 600, 1800, false, "VSCode")
        .await;
    assert!(result.is_none(), "must be None when disabled");

    // Hot-reload: enable via apply_config.
    engine
        .apply_config(CoachingConfig {
            enabled: true,
            ..CoachingConfig::default()
        })
        .await;

    // After hot-reload the engine is live; trigger a regime transition to get a message.
    engine.on_regime_change(Some("regime-a")).await;
    let result = engine
        .evaluate(Some("regime-b"), "Communication", 60, 1800, false, "Slack")
        .await;
    assert!(
        result.is_some(),
        "must produce a message after apply_config enabled it"
    );
}

/// Default no-op `apply_config` on a mock implementation must compile and
/// not panic, proving existing mock impls in maekon-web tests are unaffected.
#[tokio::test]
async fn default_apply_config_is_noop() {
    use maekon_core::ports::coaching::CoachingPort;

    struct MockCoaching;
    #[async_trait::async_trait]
    impl CoachingPort for MockCoaching {
        fn all_goal_progress_blocking(
            &self,
        ) -> Vec<maekon_core::models::coaching::GoalProgressView> {
            vec![]
        }
        fn update_regime_goals_blocking(&self, _goals: &std::collections::HashMap<String, u32>) {}
        async fn snooze_profile(&self, _profile: &str, _duration_secs: u64) {}
        async fn record_feedback(&self, _message_id: &str, _positive: bool) {}
        async fn all_goal_progress(&self) -> Vec<maekon_core::models::coaching::GoalProgressView> {
            vec![]
        }
        async fn update_regime_goals(&self, _goals: &std::collections::HashMap<String, u32>) {}
    }

    let mock = MockCoaching;
    // Default no-op: must not panic.
    mock.apply_config(CoachingConfig::default()).await;
}

#[test]
fn generates_explanation_for_overstay() {
    let trigger = TriggerType::RegimeOverstay {
        regime_label: "Coding".to_string(),
        duration_secs: 5400,     // 90 minutes
        avg_duration_secs: 3600, // 60 minutes
    };
    let profile = CoachingProfile::TimeAware;
    let explanation = CoachingEngine::build_explanation(&trigger, &profile);

    assert!(
        explanation.contains("90"),
        "should contain duration in minutes (90)"
    );
    assert!(
        explanation.contains("60"),
        "should contain average in minutes (60)"
    );
    assert!(
        explanation.contains("Coding"),
        "should contain regime label"
    );
    assert!(
        explanation.contains("TimeAware"),
        "should contain profile name"
    );
}
