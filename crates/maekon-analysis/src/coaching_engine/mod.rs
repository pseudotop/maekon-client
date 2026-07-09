pub mod adaptive_scorer;
mod guards;
mod helpers;
mod port_impl;
mod triggers;
pub mod tunable_params;

// Re-export helpers used by sibling modules via `super::` paths.
pub(super) use helpers::humanize_duration;

use chrono::{DateTime, Timelike, Utc};
use lru::LruCache;
use maekon_core::config::{CoachingConfig, PiiFilterLevel};
use maekon_core::models::coaching::{
    trigger_type_name, CoachingMessage, CoachingProfile, GoalProgressView, TriggerType,
};
use maekon_core::ports::coaching_effectiveness_store::CoachingEffectivenessStore;
use maekon_core::ports::pii_sanitizer::PiiSanitizer;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// Bound on the number of in-flight coaching messages whose extracted features
/// are cached for a later adaptive-scorer training update (#7913 T2.1a). Keyed
/// by `message_id`, this REPLACES the old single shared `last_features` slot,
/// which trained on whichever message was evaluated LAST rather than the one the
/// feedback actually references — a feedback arriving after a newer message
/// would have trained on the wrong features. Coaching messages are infrequent
/// (per-profile cooldowns + the 5-minute implicit window), so a small bound
/// comfortably covers every message still awaiting explicit/implicit feedback;
/// the LRU simply drops the oldest un-resolved entry if that bound is somehow
/// exceeded (bounded memory — house concurrency rule).
const MESSAGE_FEATURE_CACHE_CAP: usize = 256;

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
/// This is a **pure analysis component** — it does NOT take the broad
/// `Arc<dyn StorageService>` god object; ephemeral/event persistence (coaching
/// events) is handled by the caller (scheduler loop). It MAY, however, hold ONE
/// optional NARROW secondary port — [`CoachingEffectivenessStore`] (#7913 T2.1b)
/// — so its `(profile, trigger)` effectiveness LEARNING state survives restart
/// (before #7913 it evaporated on exit while sibling queue/regime state already
/// persisted). `None` keeps the engine purely in-memory; every unit test runs
/// that way.
pub struct CoachingEngine {
    config: RwLock<CoachingConfig>,
    templates: CoachingTemplateRegistry,
    pub(super) goal_tracker: RwLock<RegimeGoalTracker>,
    pub(super) feedback_tracker: RwLock<FeedbackTracker>,

    /// Optional narrow persistence port for `feedback_tracker`'s effectiveness
    /// scores (#7913 T2.1b). `None` ⇒ in-memory only (the pre-#7913 behavior,
    /// and every unit test). Write-through happens after each feedback mutation;
    /// load-on-start via [`hydrate_effectiveness_from_store`]. Advisory state — a
    /// persist/load failure is logged, never fatal.
    pub(super) effectiveness_store: Option<Arc<dyn CoachingEffectivenessStore>>,

    /// Profile display name -> last alert timestamp (cooldown enforcement).
    pub(super) last_alert: RwLock<HashMap<String, DateTime<Utc>>>,
    /// Current regime ID for transition detection.
    pub(super) current_regime_id: RwLock<Option<String>>,
    /// Timestamp when the current regime was entered.
    pub(super) current_regime_entered: RwLock<Option<DateTime<Utc>>>,

    /// Tracks a snoozed profile: (profile_name, snooze_expiry_instant).
    /// When set, `evaluate()` skips triggers for this profile until the Instant passes.
    pub(super) snoozed_until: RwLock<Option<(String, Instant)>>,

    /// Per-regime EMA of dwell duration in seconds.
    /// Key: `regime_id` (the opaque positional id, e.g. "regime-3"), NOT
    /// `regime_label` — see `on_regime_change` for the write path and
    /// `avg_regime_duration_secs`/`build_variables` for readers. This
    /// corrects a stale comment that previously claimed the key was
    /// `regime_label`; that mismatch caused `build_variables`'s lookup to
    /// always miss (#7480 follow-up).
    pub(super) regime_avg_duration: RwLock<HashMap<String, f64>>,

    /// Count of regime transitions today. Reset at midnight.
    pub(super) context_switch_count: RwLock<u32>,
    /// Date of last reset (for daily reset logic).
    pub(super) context_switch_date: RwLock<chrono::NaiveDate>,

    /// Last app name passed to evaluate() — used for implicit feedback.
    pub(super) last_app_name: RwLock<String>,

    /// Engine-level tuning parameters (overstay_percent, ema_alpha). NOTE
    /// (review4 F15 sibling): this instance is currently fixed at
    /// `TunableParams::default()` and only read — it is NOT mutated by feedback.
    /// The live feedback-adjusted parameters live inside `FeedbackTracker` (its own
    /// gating) and are not propagated back here. Reset-on-restart (in-memory).
    pub(super) tunable_params: RwLock<TunableParams>,

    /// Adaptive scorer — online logistic regression for should-show decisions.
    ///
    /// DEFERRED / NOT WIRED (review4 F15): `train_on_feedback` — the only path that
    /// trains this scorer — has no production caller, so `is_ready()` never becomes
    /// true and the rule-based effectiveness gate is always used. The scaffolding is
    /// retained for a future feedback-wiring effort and does not currently influence
    /// coaching decisions.
    pub(super) adaptive_scorer: RwLock<AdaptiveScorer>,
    /// Per-`message_id` extracted features, cached at `evaluate()` time so a later
    /// feedback event trains the adaptive scorer on the features of the SPECIFIC
    /// message it references (#7913 T2.1a). Bounded LRU (`MESSAGE_FEATURE_CACHE_CAP`)
    /// — see that constant for why the old single `last_features` slot was wrong.
    /// Entries are `pop`ed when the message is resolved (explicit thumbs or the
    /// implicit-window sweep), so the cache normally holds only messages still
    /// awaiting feedback.
    pub(super) message_features: RwLock<LruCache<String, CoachingFeatures>>,
    /// Count of coaching messages shown today (feeds the adaptive-scorer feature and
    /// anti-nag logic). Reset daily via `messages_shown_date` (review4 F16).
    pub(super) messages_shown_today: RwLock<u32>,
    /// Local date of the last `messages_shown_today` reset (daily rollover).
    pub(super) messages_shown_date: RwLock<chrono::NaiveDate>,

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
            effectiveness_store: None,
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
            message_features: RwLock::new(LruCache::new(
                // `unwrap_or_else(|| panic!(..))` rather than `.expect(..)` to match
                // the crate's NonZeroUsize construction pattern (clippy expect_used).
                NonZeroUsize::new(MESSAGE_FEATURE_CACHE_CAP)
                    .unwrap_or_else(|| panic!("MESSAGE_FEATURE_CACHE_CAP must be non-zero")),
            )),
            messages_shown_today: RwLock::new(0),
            messages_shown_date: RwLock::new(chrono::Local::now().date_naive()),
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

    /// Attach the narrow effectiveness persistence port (#7913 T2.1b) so learned
    /// `(profile, trigger)` effectiveness survives restart. Call
    /// [`hydrate_effectiveness_from_store`] once after construction to load prior
    /// state.
    pub fn with_effectiveness_store(mut self, store: Arc<dyn CoachingEffectivenessStore>) -> Self {
        self.effectiveness_store = Some(store);
        self
    }

    /// Write the current effectiveness scores through to the persistence port
    /// (#7913 T2.1b). Full-snapshot, idempotent upsert — called after every
    /// feedback mutation so a daily-restart user keeps learned effectiveness. A
    /// `None` store or a write failure is a silent/logged no-op: this state is
    /// advisory and must never block the coaching UX path (ADR-017).
    async fn persist_effectiveness(&self) {
        let Some(store) = self.effectiveness_store.as_ref() else {
            return;
        };
        let records = self.feedback_tracker.read().await.effectiveness_snapshot();
        if let Err(e) = store.upsert_coaching_effectiveness(&records) {
            debug!(error = %e, "coaching effectiveness persist failed (advisory, ignored)");
        }
    }

    /// Load persisted effectiveness scores into the feedback tracker on startup
    /// (#7913 T2.1b). Fail-safe: a missing/corrupt store starts empty (today's
    /// behavior) and only warns — it must never panic the scheduler.
    pub async fn hydrate_effectiveness_from_store(&self) {
        let Some(store) = self.effectiveness_store.as_ref() else {
            return;
        };
        match store.load_coaching_effectiveness() {
            Ok(records) if !records.is_empty() => {
                let count = records.len();
                self.feedback_tracker
                    .write()
                    .await
                    .hydrate_effectiveness(records);
                debug!(count, "coaching effectiveness hydrated from storage");
            }
            Ok(_) => debug!("coaching effectiveness: no persisted state, starting fresh"),
            Err(e) => warn!(error = %e, "coaching effectiveness load failed; starting empty"),
        }
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
        // review4 F16: roll the daily message counter over at local-day boundaries.
        // Without this it accumulates for the lifetime of the 24/7 process, turning
        // the "messages today" signal into a lifetime count.
        {
            let today = chrono::Local::now().date_naive();
            let mut date = self.messages_shown_date.write().await;
            if *date != today {
                *date = today;
                *self.messages_shown_today.write().await = 0;
            }
        }
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

        if !self.commit_trigger(&trigger).await {
            debug!(trigger = ?trigger, "coaching suppressed: trigger already committed");
            return None;
        }

        // 9. Record alert timestamp + increment daily counter
        self.record_alert(&profile).await;
        *self.messages_shown_today.write().await += 1;

        // 10. Produce message
        let message_id = maekon_core::id_generation::generate_id("cch");
        // #7913 (T2.1a): cache THIS message's features under its own id so a later
        // feedback event (explicit thumbs or the implicit-window sweep) trains the
        // adaptive scorer on the features of the message it references — not on
        // whichever message happened to be evaluated most recently.
        self.message_features
            .write()
            .await
            .put(message_id.clone(), features);
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

    /// Train the adaptive scorer on the feedback for a SPECIFIC coaching message
    /// (#7913 T2.1a). Looks up (and removes) the features cached for `message_id`
    /// at `evaluate()` time and applies one SGD step toward the observed label.
    ///
    /// WIRED (#7913): called from both feedback paths —
    /// [`record_explicit_feedback`] (thumbs up/down) and
    /// [`evaluate_implicit_feedback`] (the 5-minute behaviour-window sweep). Each
    /// call moves the adaptive scorer one training sample closer to `is_ready()`,
    /// after which `evaluate()` prefers its learned prediction over the rule-based
    /// effectiveness gate. A `message_id` with no cached features (already
    /// resolved, evicted, or never produced by THIS engine instance) is a
    /// no-op — the model simply does not learn from it.
    pub async fn train_on_feedback(&self, message_id: &str, positive: bool) {
        let features = self.message_features.write().await.pop(message_id);
        if let Some(features) = features {
            let label = if positive { 1.0 } else { 0.0 };
            // Update and read the count under a single guard, then drop it before
            // logging — the tracing temporaries must not be held across an
            // `.await`, or the future stops being `Send` (CoachingPort::record_feedback
            // requires a `Send` future).
            let train_count = {
                let mut scorer = self.adaptive_scorer.write().await;
                scorer.update(&features, label);
                scorer.train_count()
            };
            debug!(message_id, positive, train_count, "adaptive scorer trained");
        }
    }

    /// Record additional minutes for goal tracking (delegates to RegimeGoalTracker).
    pub async fn record_minutes(&self, regime_label: &str, minutes: u32) {
        let mut gt = self.goal_tracker.write().await;
        gt.record_minutes(regime_label, minutes);
    }

    /// Get the EMA of dwell duration for a regime, in seconds.
    /// `regime_id` must be the opaque `regime_id` (NOT `regime_label`) —
    /// `regime_avg_duration` is keyed by id (see `on_regime_change`). The
    /// parameter was previously misnamed `regime_label`, which mirrors the
    /// #7480 lookup-key mismatch fixed in `build_variables`.
    /// Returns 1800 (30 min) as default when no history exists.
    pub async fn avg_regime_duration_secs(&self, regime_id: &str) -> u64 {
        let avgs = self.regime_avg_duration.read().await;
        avgs.get(regime_id).copied().unwrap_or(1800.0) as u64
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
        {
            let mut ft = self.feedback_tracker.write().await;
            ft.register_pending(message_id, profile, trigger, regime_id, app_name);
        }
        // total_shown changed → persist so the effectiveness gate's warm-up count
        // survives restart (#7913 T2.1b).
        self.persist_effectiveness().await;
    }

    /// Record explicit feedback (thumbs-up/down) for a coaching message.
    ///
    /// Updates the rule-based `(profile, trigger)` effectiveness AND (#7913 T2.1a)
    /// trains the adaptive scorer on this message's features so the two gating
    /// paths learn from the same explicit signal. Training is gated on the
    /// tracker actually resolving a pending message: an unknown/duplicate id
    /// (`record_explicit` → `false`) has no effectiveness update, so it must not
    /// train the adaptive scorer either.
    pub async fn record_explicit_feedback(&self, message_id: &str, positive: bool) {
        let resolved = {
            let mut ft = self.feedback_tracker.write().await;
            ft.record_explicit(message_id, positive)
        };
        if resolved {
            self.train_on_feedback(message_id, positive).await;
            // Effectiveness changed → persist (#7913 T2.1b).
            self.persist_effectiveness().await;
        }
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

        // Resolve the pending 5-minute windows and learn what changed. The
        // tracker returns each resolved `(message_id, positive)` that carries a
        // DIRECTIONAL implicit outcome (neutral is omitted — an ambiguous "no
        // observable change" is not evidence either way and must not bias the
        // model), so the adaptive scorer trains on the SAME implicit outcome that
        // moved the rule-based effectiveness score (#7913 T2.1a). A neutral
        // message's cached features are simply left to age out of the bounded LRU.
        let resolved = {
            let mut ft = self.feedback_tracker.write().await;
            ft.evaluate_implicit(regime_id_to_use.as_deref(), &app_to_use, now)
        };
        let resolved_any = !resolved.is_empty();
        for (message_id, positive) in resolved {
            self.train_on_feedback(&message_id, positive).await;
        }
        // The implicit sweep runs every coaching tick; only touch storage when it
        // actually resolved a window (otherwise every idle tick would rewrite the
        // same snapshot for no change) (#7913 T2.1b).
        if resolved_any {
            self.persist_effectiveness().await;
        }
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

    // NOTE (#7600): `CoachingEngine` intentionally does NOT implement a
    // `record_user_reaction(feedback: &SuggestionFeedback)` method. Its real
    // feedback-learning primitive is `record_explicit_feedback`/
    // `evaluate_implicit_feedback` (both above), which are keyed by
    // `(profile_name, trigger_name)` — i.e. reactions to a *coaching
    // message*. A `SuggestionFeedback.suggestion_id` (accept/reject/defer of
    // a *suggestion* card) has no correlation to a coaching message id
    // anywhere in the codebase, so a `record_user_reaction` stub here could
    // only ever be a no-op or a fabricated mapping — both worse than not
    // having the method. `CompositeFeedbackSink` (src-tauri) fans
    // `SuggestionFeedback` out to `RegimeClassifier` only; see that type's
    // module doc comment for the rationale. If suggestion-level feedback
    // should someday influence coaching, add a real
    // `suggestion_id -> (profile, trigger)` correlation first.
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod engine_tests;
