pub mod adaptive_scorer;
mod guards;
mod triggers;
pub mod tunable_params;

use chrono::{DateTime, Timelike, Utc};
use oneshim_core::config::{CoachingConfig, PiiFilterLevel};
use oneshim_core::models::coaching::{
    trigger_type_name, CoachingMessage, CoachingProfile, GoalProgressView, TriggerType,
};
use oneshim_core::ports::pii_sanitizer::PiiSanitizer;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::debug;

use crate::coaching_template::CoachingTemplateRegistry;
use crate::feedback_tracker::FeedbackTracker;
use crate::regime_goal_tracker::RegimeGoalTracker;

pub use adaptive_scorer::{AdaptiveScorer, CoachingFeatures};
pub use tunable_params::TunableParams;

/// Central coaching orchestrator.
///
/// Evaluates triggers, matches profiles, applies guards (quiet hours,
/// cooldown, effectiveness gating), and produces `CoachingMessage` instances.
///
/// This is a **pure analysis component** — it does NOT take `Arc<dyn StorageService>`.
/// Persistence is handled by the caller (scheduler loop).
pub struct CoachingEngine {
    config: RwLock<CoachingConfig>,
    templates: CoachingTemplateRegistry,
    pub(super) goal_tracker: RwLock<RegimeGoalTracker>,
    pub(super) feedback_tracker: RwLock<FeedbackTracker>,

    /// Profile display name -> last alert timestamp (cooldown enforcement).
    pub(super) last_alert: RwLock<HashMap<String, DateTime<Utc>>>,
    /// Current regime ID for transition detection.
    pub(super) current_regime_id: RwLock<Option<String>>,
    /// Timestamp when the current regime was entered.
    pub(super) current_regime_entered: RwLock<Option<DateTime<Utc>>>,

    /// Tracks a snoozed profile: (profile_name, snooze_expiry_instant).
    /// When set, `evaluate()` skips triggers for this profile until the Instant passes.
    pub(super) snoozed_until: RwLock<Option<(String, Instant)>>,

    /// Per-regime-label EMA of dwell duration in seconds.
    /// Key: regime_label (not regime_id, since IDs are opaque).
    pub(super) regime_avg_duration: RwLock<HashMap<String, f64>>,

    /// Count of regime transitions today. Reset at midnight.
    pub(super) context_switch_count: RwLock<u32>,
    /// Date of last reset (for daily reset logic).
    pub(super) context_switch_date: RwLock<chrono::NaiveDate>,

    /// Last app name passed to evaluate() — used for implicit feedback.
    pub(super) last_app_name: RwLock<String>,

    /// Auto-tunable parameters — adjusted by feedback, reset on restart.
    pub(super) tunable_params: RwLock<TunableParams>,

    /// Adaptive scorer — online logistic regression for should-show decisions.
    /// Used when enough training data has accumulated (50+ feedback events).
    pub(super) adaptive_scorer: RwLock<AdaptiveScorer>,
    /// Last extracted features — cached for feedback update after display.
    pub(super) last_features: RwLock<Option<CoachingFeatures>>,
    /// Count of coaching messages shown today (for feature extraction).
    pub(super) messages_shown_today: RwLock<u32>,

    /// Human-readable label of the current regime (set during evaluate).
    pub(super) current_regime_label: RwLock<Option<String>>,

    /// D5 iter-8: optional PII sanitizer. When set, `template_text` is
    /// sanitized after variable substitution so any regime_label / app_name
    /// that slipped through capture-time sanitization is caught at the
    /// coaching boundary before display/persistence.
    pub(super) pii_sanitizer: Option<Arc<dyn PiiSanitizer>>,
    pub(super) pii_level: PiiFilterLevel,
}

impl CoachingEngine {
    /// Construct a new `CoachingEngine` with the given config.
    ///
    /// Initializes the goal tracker from `config.regime_goals`.
    pub fn new(config: CoachingConfig) -> Self {
        let mut goal_tracker = RegimeGoalTracker::new();
        goal_tracker.update_goals(&config.regime_goals);
        Self {
            config: RwLock::new(config),
            templates: CoachingTemplateRegistry::new(),
            goal_tracker: RwLock::new(goal_tracker),
            feedback_tracker: RwLock::new(FeedbackTracker::new()),
            last_alert: RwLock::new(HashMap::new()),
            current_regime_id: RwLock::new(None),
            current_regime_entered: RwLock::new(None),
            snoozed_until: RwLock::new(None),
            regime_avg_duration: RwLock::new(HashMap::new()),
            context_switch_count: RwLock::new(0),
            context_switch_date: RwLock::new(chrono::Utc::now().date_naive()),
            last_app_name: RwLock::new(String::new()),
            tunable_params: RwLock::new(TunableParams::default()),
            adaptive_scorer: RwLock::new(AdaptiveScorer::default()),
            last_features: RwLock::new(None),
            messages_shown_today: RwLock::new(0),
            current_regime_label: RwLock::new(None),
            pii_sanitizer: None,
            pii_level: PiiFilterLevel::Standard,
        }
    }

    /// D5 iter-8: attach a PII sanitizer for template_text sanitization.
    pub fn with_pii_sanitizer(
        mut self,
        sanitizer: Arc<dyn PiiSanitizer>,
        level: PiiFilterLevel,
    ) -> Self {
        self.pii_sanitizer = Some(sanitizer);
        self.pii_level = level;
        self
    }

    /// Main entry point: evaluate whether a coaching message should be shown.
    ///
    /// Returns `Some(CoachingMessage)` when a trigger fires and all guards pass,
    /// or `None` when coaching is disabled, in quiet hours, on cooldown, or
    /// gated by low effectiveness.
    ///
    /// # Parameters
    /// - `regime_id`: opaque ID of the current regime (for transition detection)
    /// - `regime_label`: human-readable label (e.g., "Deep Work")
    /// - `regime_duration_secs`: seconds in the current regime
    /// - `avg_regime_duration_secs`: historical EMA average (from `avg_regime_duration_secs()`)
    /// - `drift_detected`: true if the drift detector flagged attention drift
    /// - `app_name`: currently focused application name
    pub async fn evaluate(
        &self,
        regime_id: Option<&str>,
        regime_label: &str,
        regime_duration_secs: u64,
        avg_regime_duration_secs: u64,
        drift_detected: bool,
        app_name: &str,
    ) -> Option<CoachingMessage> {
        // 1. Clone config and drop read guard immediately to prevent deadlock.
        //    evaluate() later acquires goal_tracker.write(); update_config() acquires
        //    config.write() then goal_tracker.write(). Holding config.read() here while
        //    waiting on goal_tracker.write() would deadlock if update_config() holds
        //    goal_tracker.write() and waits for config.write().
        let config = self.config.read().await.clone();
        if !config.enabled {
            return None;
        }

        // 2. Quiet hours check
        if Self::is_quiet_hour(&config) {
            debug!("coaching suppressed: quiet hours");
            return None;
        }

        // 2b. Clear expired snooze eagerly (before trigger detection)
        self.clear_expired_snooze().await;

        // 2c. Record last app name for implicit feedback + current regime label
        {
            let mut app = self.last_app_name.write().await;
            *app = app_name.to_string();
        }
        *self.current_regime_label.write().await = Some(regime_label.to_string());

        // 3. Detect trigger
        let trigger = self
            .detect_trigger(
                regime_id,
                regime_label,
                regime_duration_secs,
                avg_regime_duration_secs,
                drift_detected,
            )
            .await?;

        // 4. Match profile
        let profile = Self::match_profile(&config, &trigger, regime_label, app_name)?;

        // 4b. Snooze check
        if self.is_profile_snoozed(&profile).await {
            return None;
        }

        // 5. Cooldown check
        if !self.check_cooldown(&config, &profile).await {
            debug!(profile = ?profile, "coaching suppressed: cooldown");
            return None;
        }

        // 6. Effectiveness gate (rule-based OR adaptive scorer)
        let profile_name = format!("{:?}", profile);
        let trigger_name = trigger_type_name(&trigger);

        // Extract features for adaptive scoring
        let goal_progress = {
            let gt = self.goal_tracker.read().await;
            gt.progress(regime_label)
                .map(|p| p.percentage as f32 / 100.0)
                .unwrap_or(0.0)
        };
        let context_switches = *self.context_switch_count.read().await;
        let messages_today = *self.messages_shown_today.read().await;
        let profile_eff = {
            let ft = self.feedback_tracker.read().await;
            ft.get_effectiveness(&profile_name, &trigger_name)
                .map(|s| s.ratio())
                .unwrap_or(0.5)
        };
        let hour = chrono::Utc::now().hour();
        let features = CoachingFeatures::extract(
            hour,
            regime_duration_secs,
            context_switches,
            goal_progress,
            profile_eff,
            drift_detected,
            messages_today,
            avg_regime_duration_secs,
        );

        // Try adaptive scorer first (when ready), fall back to rule-based gating
        let scorer = self.adaptive_scorer.read().await;
        if scorer.is_ready() {
            let p = scorer.predict(&features);
            drop(scorer);
            if p < 0.5 {
                debug!(
                    profile = %profile_name,
                    p_helpful = p,
                    "coaching suppressed: adaptive scorer"
                );
                return None;
            }
        } else {
            drop(scorer);
            let mut ft = self.feedback_tracker.write().await;
            if !ft.should_show(&profile_name, &trigger_name) {
                debug!(profile = %profile_name, "coaching suppressed: low effectiveness");
                return None;
            }
        }

        // Cache features for feedback update
        *self.last_features.write().await = Some(features);

        // 7. Build variables
        let variables = self
            .build_variables(regime_label, regime_duration_secs, app_name)
            .await;

        // 8. Select template (locale-aware, falls back to "en")
        let raw_template_text =
            self.templates
                .select(&profile, &trigger, &config.tone, &config.locale, &variables);
        // D5 iter-8: sanitize template_text after variable substitution.
        // Coaching templates interpolate regime_label, app_name, and other
        // runtime values that may contain PII if capture-time sanitization
        // missed them. Apply at the coaching boundary as defense-in-depth.
        let template_text = self
            .pii_sanitizer
            .as_ref()
            .map(|s| s.sanitize_text(&raw_template_text, self.pii_level))
            .unwrap_or(raw_template_text);

        // 9. Record alert timestamp + increment daily counter
        self.record_alert(&profile).await;
        *self.messages_shown_today.write().await += 1;

        // 10. Produce message
        let message_id = uuid::Uuid::new_v4().to_string();
        let explanation = Self::build_explanation(&trigger, &profile);
        Some(CoachingMessage {
            message_id,
            profile,
            trigger,
            template_text,
            personalized_text: None,
            variables,
            created_at: Utc::now(),
            explanation,
        })
    }

    /// Build a human-readable explanation of why this coaching message was triggered.
    ///
    /// Each trigger variant produces a distinct explanation referencing the
    /// coaching profile name so users understand both the _what_ and the _who_.
    pub fn build_explanation(trigger: &TriggerType, profile: &CoachingProfile) -> String {
        let profile_name = format!("{:?}", profile);
        match trigger {
            TriggerType::RegimeTransition {
                from_regime,
                to_regime,
            } => {
                let from = from_regime.as_deref().unwrap_or("unknown");
                let to = to_regime.as_deref().unwrap_or("unknown");
                format!(
                    "You switched from '{}' to '{}' (context switch detected). {} profile triggered this coaching nudge.",
                    from, to, profile_name
                )
            }
            TriggerType::RegimeOverstay {
                regime_label,
                duration_secs,
                avg_duration_secs,
            } => {
                let dur_min = duration_secs / 60;
                let avg_min = avg_duration_secs / 60;
                format!(
                    "You've been in '{}' for {} minutes (average: {} min). {} profile suggests a break or status check.",
                    regime_label, dur_min, avg_min, profile_name
                )
            }
            TriggerType::RegimeDrift { regime_label } => {
                format!(
                    "Frequent app switching detected in '{}'. {} profile flagged possible attention drift.",
                    regime_label, profile_name
                )
            }
            TriggerType::GoalThreshold {
                regime_label,
                target_minutes,
                current_minutes,
                threshold_percent,
            } => {
                format!(
                    "You've reached {}% of your '{}' goal ({}/{} min). {} profile is tracking your progress.",
                    threshold_percent, regime_label, current_minutes, target_minutes, profile_name
                )
            }
        }
    }

    // ── Public delegation methods ──────────────────────────────────────

    /// Hot-reload coaching config at runtime.
    ///
    /// Lock ordering: config -> goal_tracker (same as evaluate()) to prevent deadlock.
    pub async fn update_config(&self, config: CoachingConfig) {
        let mut current = self.config.write().await;
        let mut gt = self.goal_tracker.write().await;
        gt.update_goals(&config.regime_goals);
        *current = config;
    }

    /// Train the adaptive scorer with feedback from the last coaching message.
    /// Called after explicit or implicit feedback is recorded.
    pub async fn train_on_feedback(&self, positive: bool) {
        let features = self.last_features.read().await.clone();
        if let Some(features) = features {
            let label = if positive { 1.0 } else { 0.0 };
            self.adaptive_scorer.write().await.update(&features, label);
            debug!(
                positive,
                train_count = self.adaptive_scorer.read().await.train_count(),
                "adaptive scorer trained"
            );
        }
    }

    /// Record additional minutes for goal tracking (delegates to RegimeGoalTracker).
    pub async fn record_minutes(&self, regime_label: &str, minutes: u32) {
        let mut gt = self.goal_tracker.write().await;
        gt.record_minutes(regime_label, minutes);
    }

    /// Get the EMA of dwell duration for a regime label, in seconds.
    /// Returns 1800 (30 min) as default when no history exists.
    pub async fn avg_regime_duration_secs(&self, regime_label: &str) -> u64 {
        let avgs = self.regime_avg_duration.read().await;
        avgs.get(regime_label).copied().unwrap_or(1800.0) as u64
    }

    /// Register a coaching message for pending feedback evaluation.
    pub async fn register_pending_feedback(
        &self,
        message_id: &str,
        profile: &str,
        trigger: &str,
        regime_id: Option<&str>,
        app_name: &str,
    ) {
        let mut ft = self.feedback_tracker.write().await;
        ft.register_pending(message_id, profile, trigger, regime_id, app_name);
    }

    /// Record explicit feedback (thumbs-up/down) for a coaching message.
    pub async fn record_explicit_feedback(&self, message_id: &str, positive: bool) {
        let mut ft = self.feedback_tracker.write().await;
        ft.record_explicit(message_id, positive);
    }

    /// Evaluate implicit feedback for messages past the 5-minute window.
    ///
    /// When called with `(None, "")` (from the coaching loop which lacks live data),
    /// falls back to the engine's internal `current_regime_id` and `last_app_name`.
    /// When called with real data (from the monitor loop), uses the provided values.
    pub async fn evaluate_implicit_feedback(
        &self,
        current_regime_id: Option<&str>,
        current_app: &str,
        now: DateTime<Utc>,
    ) {
        // Use internal state when caller provides placeholders
        let regime_id_to_use: Option<String>;
        let app_to_use: String;
        if current_regime_id.is_none() && current_app.is_empty() {
            regime_id_to_use = self.current_regime_id.read().await.clone();
            app_to_use = self.last_app_name.read().await.clone();
        } else {
            regime_id_to_use = current_regime_id.map(String::from);
            app_to_use = current_app.to_string();
        }

        let mut ft = self.feedback_tracker.write().await;
        ft.evaluate_implicit(regime_id_to_use.as_deref(), &app_to_use, now);
    }

    // ── Phase 2 methods ──────────────────────────────────────────────

    /// Set a temporary cooldown override on the specified coaching profile.
    /// While snoozed, `evaluate()` skips that profile's triggers.
    /// The snooze expires after `duration` elapses.
    ///
    /// Called from the `dismiss_coaching_message` IPC command when the user
    /// clicks "Later". The profile name is read from the most recently
    /// shown coaching message.
    pub async fn snooze_current_profile(&self, profile_name: &str, duration: Duration) {
        let mut guard = self.snoozed_until.write().await;
        *guard = Some((profile_name.to_string(), Instant::now() + duration));
    }

    /// Return goal progress for all configured regimes.
    /// Delegates to `RegimeGoalTracker::all_progress()`, maps each `GoalProgress`
    /// to `GoalProgressView` with a deterministic display color assignment.
    ///
    /// Async because it reads from `RwLock<RegimeGoalTracker>`.
    pub async fn all_goal_progress(&self) -> Vec<GoalProgressView> {
        const COLORS: &[&str] = &[
            "#3b82f6", "#10b981", "#f59e0b", "#ef4444", "#8b5cf6", "#ec4899",
        ];
        let tracker = self.goal_tracker.read().await;
        tracker
            .all_progress()
            .into_iter()
            .enumerate()
            .map(|(i, gp)| GoalProgressView {
                regime_label: gp.regime_label,
                current_minutes: gp.current_minutes,
                target_minutes: gp.target_minutes,
                percentage: gp.percentage,
                display_color: COLORS[i % COLORS.len()].to_string(),
            })
            .collect()
    }

    /// Update the goal tracker's regime targets at runtime.
    /// Called from the IPC `update_regime_goals` command.
    pub async fn update_regime_goals(&self, goals: &HashMap<String, u32>) {
        let mut tracker = self.goal_tracker.write().await;
        tracker.update_goals(goals);
    }

    /// Record a user reaction to a coaching message.
    ///
    /// Phase 3 stub — records no state beyond a trace log. The concrete
    /// learning algorithm (bayesian update of trigger priors, per-profile
    /// acceptance rate) lands in a follow-up phase. Called via
    /// `FeedbackSignalSink` from the composition root.
    ///
    /// Must return within ~10 ms; see ADR-017 for the latency budget.
    pub async fn record_user_reaction(
        &self,
        feedback: &oneshim_core::models::suggestion::SuggestionFeedback,
    ) {
        tracing::debug!(
            suggestion_id = %feedback.suggestion_id,
            feedback_type = ?feedback.feedback_type,
            "coaching_engine: user reaction recorded (no-op learning)"
        );
    }
}

#[async_trait::async_trait]
impl oneshim_core::ports::coaching::CoachingPort for CoachingEngine {
    fn all_goal_progress_blocking(&self) -> Vec<GoalProgressView> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.all_goal_progress())
        })
    }

    fn update_regime_goals_blocking(&self, goals: &HashMap<String, u32>) {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.update_regime_goals(goals))
        })
    }

    async fn snooze_profile(&self, profile: &str, duration_secs: u64) {
        self.snooze_current_profile(profile, Duration::from_secs(duration_secs))
            .await;
    }

    async fn record_feedback(&self, message_id: &str, positive: bool) {
        self.record_explicit_feedback(message_id, positive).await;
    }

    async fn all_goal_progress(&self) -> Vec<GoalProgressView> {
        self.all_goal_progress().await
    }

    async fn update_regime_goals(&self, goals: &HashMap<String, u32>) {
        self.update_regime_goals(goals).await;
    }

    fn current_regime_label_blocking(&self) -> Option<String> {
        let handle = tokio::runtime::Handle::current();
        tokio::task::block_in_place(|| {
            handle.block_on(async { self.current_regime_label.read().await.clone() })
        })
    }

    fn regime_minutes_today_blocking(&self) -> u32 {
        let handle = tokio::runtime::Handle::current();
        tokio::task::block_in_place(|| {
            handle.block_on(async {
                let gt = self.goal_tracker.read().await;
                gt.all_progress().iter().map(|p| p.current_minutes).sum()
            })
        })
    }
}

/// Humanize a duration in seconds to a compact "Xh Ym" format.
fn humanize_duration(secs: u64) -> String {
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    if hours > 0 && minutes > 0 {
        format!("{}h {}m", hours, minutes)
    } else if hours > 0 {
        format!("{}h", hours)
    } else {
        format!("{}m", minutes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, Timelike};
    use oneshim_core::config::{ProfileConfig, TimeRange};
    use oneshim_core::models::coaching::{CoachingProfile, TriggerType};

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
        let trigger_name = oneshim_core::models::coaching::trigger_type_name(&m4.trigger);

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
}
