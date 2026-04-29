//! End-to-end integration tests for the coaching engine.
//!
//! Tests the full lifecycle: config -> regime transitions -> trigger detection
//! -> profile matching -> guard enforcement (quiet hours, cooldown, snooze)
//! -> coaching message generation.
//!
//! Uses only the public API of `CoachingEngine`.

use chrono::{Local, Timelike};
use oneshim_analysis::coaching_engine::CoachingEngine;
use oneshim_core::config::{CoachingConfig, ProfileConfig, TimeRange};
use oneshim_core::models::coaching::TriggerType;
use std::collections::HashMap;
use std::time::Duration;

// ── Helpers ──────────────────────────────────────────────────────

fn enabled_config() -> CoachingConfig {
    CoachingConfig {
        enabled: true,
        ..CoachingConfig::default()
    }
}

fn config_with_goals(goals: HashMap<String, u32>) -> CoachingConfig {
    CoachingConfig {
        enabled: true,
        regime_goals: goals,
        ..CoachingConfig::default()
    }
}

/// Create a config with a very short cooldown so consecutive evaluates are
/// not suppressed (except when we specifically test cooldown behavior).
fn short_cooldown_config() -> CoachingConfig {
    let mut profiles = HashMap::new();
    for name in [
        "FocusGuard",
        "TimeAware",
        "DeepWorkCoach",
        "ContextRestore",
        "GoalTracker",
    ] {
        profiles.insert(
            name.to_string(),
            ProfileConfig {
                enabled: true,
                min_interval_secs: 0, // no cooldown
            },
        );
    }
    CoachingConfig {
        enabled: true,
        profiles,
        ..CoachingConfig::default()
    }
}

fn short_cooldown_config_with_goals(goals: HashMap<String, u32>) -> CoachingConfig {
    let mut config = short_cooldown_config();
    config.regime_goals = goals;
    config
}

// ── Test 1: Basic regime transition produces a coaching message ──

#[tokio::test]
async fn e2e_regime_transition_produces_message() {
    let engine = CoachingEngine::new(enabled_config());

    // First evaluate with a regime -> transition from None to Some
    let msg = engine
        .evaluate(Some("regime-1"), "Deep Work", 0, 1800, false, "VS Code")
        .await;

    assert!(msg.is_some(), "initial regime should produce a message");
    let m = msg.unwrap();
    assert!(
        matches!(m.trigger, TriggerType::RegimeTransition { .. }),
        "should be RegimeTransition"
    );
    assert!(
        !m.template_text.is_empty(),
        "template text should not be empty"
    );
    assert!(!m.message_id.is_empty(), "message_id should be set");
}

// ── Test 2: Quiet hours suppress all coaching ───────────────────

#[tokio::test]
async fn e2e_quiet_hours_suppress_all_triggers() {
    let now = Local::now();
    let current_hour = now.time().hour();
    let next_hour = (current_hour + 2) % 24;

    let config = CoachingConfig {
        enabled: true,
        quiet_hours: vec![TimeRange {
            start: format!("{:02}:00", current_hour),
            end: format!("{:02}:00", next_hour),
        }],
        ..CoachingConfig::default()
    };
    let engine = CoachingEngine::new(config);

    // Try regime transition — should be suppressed
    let msg1 = engine
        .evaluate(Some("r1"), "Work", 0, 1800, false, "VS Code")
        .await;
    assert!(
        msg1.is_none(),
        "quiet hours should suppress regime transition"
    );

    // Try drift — should be suppressed
    engine.on_regime_change(Some("r1")).await;
    let msg2 = engine
        .evaluate(Some("r1"), "Work", 300, 1800, true, "VS Code")
        .await;
    assert!(msg2.is_none(), "quiet hours should suppress drift");

    // Try overstay — should be suppressed
    let msg3 = engine
        .evaluate(Some("r1"), "Email", 5000, 1800, false, "Outlook")
        .await;
    assert!(msg3.is_none(), "quiet hours should suppress overstay");
}

// ── Test 3: Cooldown prevents rapid-fire messages ───────────────

#[tokio::test]
async fn e2e_cooldown_prevents_rapid_fire() {
    let mut profiles = HashMap::new();
    profiles.insert(
        "FocusGuard".to_string(),
        ProfileConfig {
            enabled: true,
            min_interval_secs: 600, // 10 minutes
        },
    );
    let config = CoachingConfig {
        enabled: true,
        profiles,
        ..CoachingConfig::default()
    };
    let engine = CoachingEngine::new(config);

    // First transition fires
    let msg1 = engine
        .evaluate(Some("r-a"), "Work", 0, 1800, false, "VS Code")
        .await;
    assert!(msg1.is_some(), "first call should produce a message");

    // Second transition immediately — should be on cooldown
    engine.on_regime_change(Some("r-a")).await;
    let msg2 = engine
        .evaluate(Some("r-b"), "Work", 0, 1800, false, "VS Code")
        .await;
    assert!(
        msg2.is_none(),
        "second call within 600s cooldown should be suppressed"
    );
}

// ── Test 4: Snooze suppresses matched profile ───────────────────

#[tokio::test]
async fn e2e_snooze_suppresses_profile() {
    let engine = CoachingEngine::new(short_cooldown_config());

    // Setup: fire a transition to establish context
    engine.on_regime_change(Some("r-a")).await;

    // Snooze FocusGuard for 60 seconds
    engine
        .snooze_current_profile("FocusGuard", Duration::from_secs(60))
        .await;

    // Transition from non-idle maps to FocusGuard -> should be snoozed
    let msg = engine
        .evaluate(Some("r-b"), "Work", 60, 1800, false, "VS Code")
        .await;
    assert!(msg.is_none(), "snoozed FocusGuard should suppress message");
}

// ── Test 5: Snooze does not affect other profiles ───────────────

#[tokio::test]
async fn e2e_snooze_does_not_affect_other_profiles() {
    let mut goals = HashMap::new();
    goals.insert("Work".to_string(), 100);
    let engine = CoachingEngine::new(short_cooldown_config_with_goals(goals));

    // Setup: establish regime
    engine.on_regime_change(Some("r-x")).await;

    // Snooze FocusGuard
    engine
        .snooze_current_profile("FocusGuard", Duration::from_secs(60))
        .await;

    // Record minutes to cross 25% threshold
    engine.record_minutes("Work", 25).await;

    // Evaluate on same regime (no transition) to get GoalThreshold
    let msg = engine
        .evaluate(Some("r-x"), "Work", 600, 1800, false, "VS Code")
        .await;
    assert!(
        msg.is_some(),
        "GoalTracker profile should not be affected by FocusGuard snooze"
    );
    assert!(matches!(
        msg.unwrap().trigger,
        TriggerType::GoalThreshold { .. }
    ));
}

// ── Test 6: Full lifecycle — transition, goals, drift, overstay ─

#[tokio::test]
async fn e2e_full_lifecycle() {
    let mut goals = HashMap::new();
    goals.insert("Coding".to_string(), 100);
    let engine = CoachingEngine::new(short_cooldown_config_with_goals(goals));

    // Step 1: Initial regime transition (None -> "coding")
    let msg1 = engine
        .evaluate(Some("coding"), "Coding", 0, 1800, false, "VS Code")
        .await;
    assert!(msg1.is_some(), "step 1: initial transition should fire");
    let m1 = msg1.unwrap();
    assert!(matches!(m1.trigger, TriggerType::RegimeTransition { .. }));

    // Step 2: Record 25 minutes -> GoalThreshold 25%
    engine.record_minutes("Coding", 25).await;
    let msg2 = engine
        .evaluate(Some("coding"), "Coding", 1500, 1800, false, "VS Code")
        .await;
    assert!(msg2.is_some(), "step 2: 25% goal threshold should fire");
    match &msg2.unwrap().trigger {
        TriggerType::GoalThreshold {
            threshold_percent, ..
        } => {
            assert_eq!(*threshold_percent, 25);
        }
        other => panic!("step 2: expected GoalThreshold, got {:?}", other),
    }

    // Step 3: Drift detection -> FocusGuard
    let msg3 = engine
        .evaluate(Some("coding"), "Coding", 300, 1800, true, "VS Code")
        .await;
    assert!(msg3.is_some(), "step 3: drift should fire");
    assert!(matches!(
        msg3.unwrap().trigger,
        TriggerType::RegimeDrift { .. }
    ));

    // Step 4: Record more minutes and drain remaining goal thresholds
    engine.record_minutes("Coding", 75).await;
    // Drain thresholds: 50%, 75%, 100%
    for expected in [50u8, 75, 100] {
        let msg = engine
            .evaluate(Some("coding"), "Coding", 3000, 1800, false, "VS Code")
            .await;
        assert!(msg.is_some(), "goal threshold {}% should fire", expected);
        match &msg.unwrap().trigger {
            TriggerType::GoalThreshold {
                threshold_percent, ..
            } => {
                assert_eq!(*threshold_percent, expected);
            }
            other => panic!("expected GoalThreshold {}%, got {:?}", expected, other),
        }
    }

    // Step 5: Overstay (duration > 1.2x avg, all goals exhausted)
    let msg5 = engine
        .evaluate(Some("coding"), "Coding", 3000, 1800, false, "VS Code")
        .await;
    assert!(
        msg5.is_some(),
        "step 5: overstay should fire after goals exhausted"
    );
    assert!(matches!(
        msg5.unwrap().trigger,
        TriggerType::RegimeOverstay { .. }
    ));

    // Step 6: Regime change -> transition
    let msg6 = engine
        .evaluate(Some("email"), "Email", 0, 1800, false, "Outlook")
        .await;
    assert!(msg6.is_some(), "step 6: transition to email should fire");
    assert!(matches!(
        msg6.unwrap().trigger,
        TriggerType::RegimeTransition { .. }
    ));
}

// ── Test 7: Disabled config produces no messages ────────────────

#[tokio::test]
async fn e2e_disabled_config_no_messages() {
    let config = CoachingConfig {
        enabled: false,
        ..CoachingConfig::default()
    };
    let engine = CoachingEngine::new(config);

    // Try every trigger type — none should work
    let msg1 = engine
        .evaluate(Some("r1"), "Work", 0, 1800, false, "VS Code")
        .await;
    assert!(msg1.is_none());

    let msg2 = engine
        .evaluate(Some("r1"), "Work", 5000, 1800, true, "VS Code")
        .await;
    assert!(msg2.is_none());
}

// ── Test 8: Hot reload config at runtime ────────────────────────

#[tokio::test]
async fn e2e_hot_reload_config() {
    // Start disabled
    let engine = CoachingEngine::new(CoachingConfig::default());
    let msg1 = engine
        .evaluate(Some("r1"), "Work", 0, 1800, false, "VS Code")
        .await;
    assert!(msg1.is_none(), "disabled engine should produce no message");

    // Hot reload to enabled
    engine.update_config(enabled_config()).await;
    let msg2 = engine
        .evaluate(Some("r1"), "Work", 0, 1800, false, "VS Code")
        .await;
    assert!(
        msg2.is_some(),
        "after hot reload to enabled, should produce message"
    );
}

// ── Test 9: Feedback tracking integration ───────────────────────

#[tokio::test]
async fn e2e_feedback_tracking() {
    let engine = CoachingEngine::new(enabled_config());

    // Generate a message
    let msg = engine
        .evaluate(Some("r1"), "Work", 0, 1800, false, "VS Code")
        .await;
    assert!(msg.is_some());
    let m = msg.unwrap();

    // Register pending feedback
    let profile_name = format!("{:?}", m.profile);
    let trigger_name = oneshim_core::models::coaching::trigger_type_name(&m.trigger);
    engine
        .register_pending_feedback(
            &m.message_id,
            &profile_name,
            &trigger_name,
            Some("r1"),
            "VS Code",
        )
        .await;

    // Record explicit positive feedback
    engine.record_explicit_feedback(&m.message_id, true).await;

    // Verify the engine still works after feedback
    engine.on_regime_change(Some("r1")).await;
    let msg2 = engine
        .evaluate(Some("r2"), "Work", 0, 1800, false, "VS Code")
        .await;
    // May or may not produce message (depends on cooldown), but should not panic
    let _ = msg2;
}

// ── Test 10: Goal progress tracking ─────────────────────────────

#[tokio::test]
async fn e2e_goal_progress_tracking() {
    let mut goals = HashMap::new();
    goals.insert("Deep Work".to_string(), 120);
    goals.insert("Communication".to_string(), 60);
    let engine = CoachingEngine::new(config_with_goals(goals));

    // Record minutes
    engine.record_minutes("Deep Work", 45).await;
    engine.record_minutes("Communication", 30).await;

    // Check progress via all_goal_progress
    let progress = engine.all_goal_progress().await;
    assert_eq!(progress.len(), 2);

    let dw = progress
        .iter()
        .find(|p| p.regime_label == "Deep Work")
        .expect("Deep Work should be tracked");
    assert_eq!(dw.current_minutes, 45);
    assert_eq!(dw.target_minutes, 120);
    assert_eq!(dw.percentage, 37); // 45/120 = 37.5% truncated

    let comm = progress
        .iter()
        .find(|p| p.regime_label == "Communication")
        .expect("Communication should be tracked");
    assert_eq!(comm.current_minutes, 30);
    assert_eq!(comm.target_minutes, 60);
    assert_eq!(comm.percentage, 50);

    // All should have display colors
    for p in &progress {
        assert!(p.display_color.starts_with('#'));
    }
}

// ── Test 11: EMA average regime duration updates ────────────────

#[tokio::test]
async fn e2e_avg_regime_duration() {
    let engine = CoachingEngine::new(enabled_config());

    // Default (no history) should return 1800
    let avg = engine.avg_regime_duration_secs("unknown").await;
    assert_eq!(avg, 1800, "default avg should be 1800s (30 min)");

    // After a regime change with dwell time, EMA should update
    engine.on_regime_change(Some("r-a")).await;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    engine.on_regime_change(Some("r-b")).await;

    let avg_a = engine.avg_regime_duration_secs("r-a").await;
    assert!(
        avg_a < 1800,
        "EMA should reflect actual short dwell, got {}",
        avg_a
    );
}

// ── Test 12: Update regime goals at runtime ─────────────────────

#[tokio::test]
async fn e2e_update_regime_goals_runtime() {
    let engine = CoachingEngine::new(enabled_config());

    // Initially no goals
    let progress = engine.all_goal_progress().await;
    assert_eq!(progress.len(), 0, "no goals initially");

    // Add goals at runtime
    let mut goals = HashMap::new();
    goals.insert("Coding".to_string(), 180);
    goals.insert("Email".to_string(), 30);
    engine.update_regime_goals(&goals).await;

    let progress = engine.all_goal_progress().await;
    assert_eq!(progress.len(), 2, "should have 2 goals after update");

    let coding = progress
        .iter()
        .find(|v| v.regime_label == "Coding")
        .unwrap();
    assert_eq!(coding.target_minutes, 180);
    assert_eq!(coding.current_minutes, 0);
}

// ── Test 13: Message variables include expected keys ────────────

#[tokio::test]
async fn e2e_message_contains_variables() {
    let engine = CoachingEngine::new(short_cooldown_config());

    // Generate a message via regime transition
    let msg = engine
        .evaluate(Some("r1"), "Deep Work", 3600, 1800, false, "VS Code")
        .await;
    assert!(msg.is_some());
    let m = msg.unwrap();

    // Check that variables are populated
    assert!(
        m.variables.contains_key("regime"),
        "should have 'regime' variable"
    );
    assert!(
        m.variables.contains_key("duration"),
        "should have 'duration' variable"
    );
    assert!(
        m.variables.contains_key("app_name"),
        "should have 'app_name' variable"
    );
    assert!(
        m.variables.contains_key("context_switches"),
        "should have 'context_switches' variable"
    );
    assert_eq!(m.variables.get("regime").unwrap(), "Deep Work");
    assert_eq!(m.variables.get("app_name").unwrap(), "VS Code");
}
