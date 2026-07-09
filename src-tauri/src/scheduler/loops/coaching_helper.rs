use std::sync::Arc;
use tracing::{debug, info, warn};

use maekon_analysis::CoachingEngine;
use maekon_core::config::PiiFilterLevel;
use maekon_core::models::coaching;
use maekon_core::ports::coaching_storage::CoachingStoragePort;
use maekon_core::ports::pii_sanitizer::PiiSanitizer;
use maekon_core::sanitized;

use crate::magic_overlay::MagicOverlayHandle;

use super::helpers::{build_personalization_prompt, COACHING_SYSTEM_PROMPT};

/// D5 iter-16: construct the `VisionPiiSanitizer` used for scrubbing LLM
/// error Display output at the coaching tracing boundary. Returned wrapped
/// in `Option` so callers can thread `None` for no-sanitize tests.
pub(super) fn build_pii_sanitizer() -> Option<Arc<dyn PiiSanitizer>> {
    Some(Arc::new(maekon_vision::privacy::VisionPiiSanitizer))
}

/// Resolve the current `PiiFilterLevel` from the per-tick `AppConfig` snapshot.
/// Returns the default level (`Standard`) when no config is available.
///
/// #6830/#4796: reads from the already-computed cheap `Arc<AppConfig>` snapshot
/// rather than calling `ConfigManager::get()` (a FULL deep clone of AppConfig)
/// on every coaching tick — the sibling of the `tick_ts_notifications` fix.
pub(super) fn resolve_pii_level(config: Option<&maekon_core::config::AppConfig>) -> PiiFilterLevel {
    config
        .map(|cfg| cfg.privacy.pii_filter_level)
        .unwrap_or_default()
}

/// Mutable state carried across monitor ticks for coaching regime tracking.
pub(super) struct CoachingTickState {
    pub(super) regime_entered_at: Option<std::time::Instant>,
    pub(super) prev_coaching_regime_id: Option<String>,
    /// Sub-minute seconds carried between ticks for goal-minute accounting
    /// (#5669). The default monitor poll is 1s, so the old per-tick
    /// `poll_secs / 60` always truncated to 0 and `record_minutes` never
    /// fired in production — goal progress and habit streaks stayed at zero.
    pub(super) minute_carry_secs: u64,
}

impl CoachingTickState {
    pub(super) fn new() -> Self {
        Self {
            regime_entered_at: None,
            prev_coaching_regime_id: None,
            minute_carry_secs: 0,
        }
    }
}

/// Accumulate `poll_secs` into the carry and return the whole minutes that
/// flushed out (remainder stays in the carry). Pure so the truncation bug
/// class (#5669) stays pinned by unit tests.
pub(super) fn flush_whole_minutes(carry_secs: &mut u64, poll_secs: u64) -> u32 {
    *carry_secs += poll_secs;
    let minutes = (*carry_secs / 60) as u32;
    *carry_secs %= 60;
    minutes
}

/// Parameters needed to evaluate and deliver a coaching message within a
/// single monitor tick.
pub(super) struct CoachingEvalContext<'a> {
    pub(super) coaching_engine: &'a Arc<CoachingEngine>,
    pub(super) overlay: &'a Option<MagicOverlayHandle>,
    pub(super) notifier: &'a Option<Arc<crate::notification_manager::NotificationManager>>,
    pub(super) coaching_storage: &'a Option<Arc<dyn CoachingStoragePort>>,
    /// Habit-streak persistence seam (#5669) — the scheduler's SQLite handle.
    pub(super) scheduler_storage: &'a Arc<dyn crate::scheduler::SchedulerStorage>,
    pub(super) analysis_provider:
        &'a Option<Arc<dyn maekon_core::ports::analysis_provider::AnalysisProvider>>,
    pub(super) regime_id: Option<&'a str>,
    /// Human-readable regime label (`name` > `auto_label`, e.g.
    /// "Deep Focus (VSCode)") resolved upstream from the active `Regime`.
    /// Distinct from `regime_id`, which is an opaque positional id ("regime-N").
    /// Downstream coaching that keys on a human label — profile substring
    /// matching, goal + habit tracking, `check_threshold`, and the LLM
    /// personalization prompt — must use THIS, not the id (#7480). `regime_id`
    /// is retained only for transition detection and the engine's EMA
    /// avg-duration map key.
    pub(super) regime_label: Option<&'a str>,
    pub(super) prev_app: Option<&'a str>,
    pub(super) drift_detected: bool,
    pub(super) poll_secs: u64,
    /// D5 iter-16: sanitizer for LLM coaching personalization error tracing.
    /// The LLM prompt embeds `template_text` + regime_label + user context,
    /// and the returned `CoreError` message may carry up to 200 chars of
    /// echoed response body (per `AnalysisClient` error path).
    pub(super) pii_sanitizer: &'a Option<Arc<dyn PiiSanitizer>>,
    pub(super) pii_level: PiiFilterLevel,
}

/// Evaluate coaching triggers and deliver any resulting message.
///
/// This function:
/// 1. Tracks real regime dwell time (reset on regime change)
/// 2. Records elapsed minutes for goal tracking
/// 3. Evaluates coaching triggers via CoachingPort
/// 4. On trigger: shows overlay, sends notification, persists event,
///    registers feedback, and spawns background LLM personalization
pub(super) async fn evaluate_and_deliver(
    ctx: &CoachingEvalContext<'_>,
    tick_state: &mut CoachingTickState,
) {
    // #7480: coaching keys on the HUMAN regime label for profile matching, goal
    // + habit tracking, `check_threshold`, and the LLM personalization prompt.
    // The opaque positional id ("regime-N") is retained only for transition
    // detection and the engine's EMA avg-duration map key — that map is keyed by
    // the id via `CoachingEngine::on_regime_change`, so the read below must use
    // the id too (using the label there would always miss and yield the 30-min
    // default). Feeding the id where the label belongs left the
    // DeepWork/ContextRestore profiles unreachable, goal progress stuck at 0%,
    // habit streaks unpersisted, and messages showing "regime-3" to the user.
    let regime_label = ctx.regime_label.unwrap_or("Unknown");
    let regime_id_key = ctx.regime_id.unwrap_or("Unknown");
    let avg_regime_duration_secs = ctx
        .coaching_engine
        .avg_regime_duration_secs(regime_id_key)
        .await;

    // Track real regime dwell time: reset timer on regime change
    let current_coaching_regime = ctx.regime_id.map(String::from);
    if current_coaching_regime != tick_state.prev_coaching_regime_id {
        tick_state.regime_entered_at = Some(std::time::Instant::now());
        tick_state.prev_coaching_regime_id = current_coaching_regime;
    }
    let regime_duration_secs: u64 = tick_state
        .regime_entered_at
        .map(|t| t.elapsed().as_secs())
        .unwrap_or(0);

    // Record elapsed minutes for goal tracking. Seconds are carried across
    // ticks (#5669): the old per-tick `poll_secs / 60` truncated to 0 at the
    // default 1s poll, so record_minutes never fired in production.
    let elapsed_minutes = flush_whole_minutes(&mut tick_state.minute_carry_secs, ctx.poll_secs);
    if elapsed_minutes > 0 {
        ctx.coaching_engine
            .record_minutes(regime_label, elapsed_minutes)
            .await;

        // Persist today's habit-streak row for the current regime (#5669) —
        // the HabitTracker widget's local producer (previously writer-less, so
        // it rendered "No data" forever on standalone). Only regimes with a
        // user-configured goal appear in all_goal_progress; others skip.
        let goals = ctx.coaching_engine.all_goal_progress().await;
        if let Some(goal) = goals.iter().find(|g| g.regime_label == regime_label) {
            // Local date — must match get_coaching_stats_today and the
            // widget's (local) day columns; a UTC key would land the row in
            // the wrong column for non-UTC users.
            let date = chrono::Local::now().format("%Y-%m-%d").to_string();
            let storage = ctx.scheduler_storage.clone();
            let label = goal.regime_label.clone();
            let (minutes, target) = (goal.current_minutes, goal.target_minutes);
            let met = goal.current_minutes >= goal.target_minutes;
            // Sync SQLite write (write_lock) — offload off the monitor loop,
            // mirroring the GUI-interaction write path.
            let joined = tokio::task::spawn_blocking(move || {
                storage.upsert_habit_streak(&label, &date, minutes, target, met)
            })
            .await;
            match joined {
                Ok(Err(e)) => warn!("habit streak persist failure: {e}"),
                Err(e) => warn!("habit streak persist task failure: {e}"),
                Ok(Ok(())) => {}
            }
        }
    }

    // Evaluate coaching triggers
    let message = ctx
        .coaching_engine
        .evaluate(
            ctx.regime_id,
            regime_label,
            regime_duration_secs,
            avg_regime_duration_secs,
            ctx.drift_detected,
            ctx.prev_app.unwrap_or(""),
        )
        .await;

    let Some(message) = message else { return };

    // 1. Show on MagicOverlay (primary delivery)
    if let Some(ref overlay) = ctx.overlay {
        overlay.show_coaching(&message).await;
    }

    // 2. Also send desktop notification (fallback)
    if let Some(ref notif) = ctx.notifier {
        notif.notify_coaching(&message.template_text).await;
    }

    // 3. Persist coaching event to storage
    if let Some(ref cs) = ctx.coaching_storage {
        let event_row = coaching::CoachingEventRow {
            event_id: message.message_id.clone(),
            trigger_type: coaching::trigger_type_name(&message.trigger),
            profile_name: format!("{:?}", message.profile),
            regime_id: ctx.regime_id.map(String::from),
            message_template: message.template_text.clone(),
            personalized_message: None,
            shown_at: chrono::Utc::now().to_rfc3339(),
            dismissed_at: None,
            dismiss_action: None,
            feedback_type: None,
            feedback_score: None,
        };
        if let Err(e) = cs.insert_coaching_event(&event_row) {
            warn!("coaching event persist failure: {e}");
        }
    }

    // 4. Register for feedback tracking
    ctx.coaching_engine
        .register_pending_feedback(
            &message.message_id,
            &format!("{:?}", message.profile),
            &coaching::trigger_type_name(&message.trigger),
            ctx.regime_id,
            ctx.prev_app.unwrap_or(""),
        )
        .await;

    info!(
        profile = ?message.profile,
        trigger = ?message.trigger,
        "coaching message: {}",
        message.template_text,
    );

    // 5. Spawn background LLM personalization
    if let Some(ref provider) = ctx.analysis_provider {
        let msg_clone = message.clone();
        let provider_clone = provider.clone();
        let overlay_clone = ctx.overlay.clone();
        let storage_clone = ctx.coaching_storage.clone();
        let regime = regime_label.to_string();
        // D5 iter-16: capture sanitizer + level into the spawn closure so
        // the LLM-error Display can be scrubbed at the tracing boundary.
        let pii_sanitizer = ctx.pii_sanitizer.clone();
        let pii_level = ctx.pii_level;
        tokio::spawn(async move {
            let prompt = build_personalization_prompt(&msg_clone.template_text, &regime);
            match provider_clone
                .analyze(&prompt, COACHING_SYSTEM_PROMPT)
                .await
            {
                Ok(suggestions) if !suggestions.is_empty() => {
                    let personalized = &suggestions[0].content;
                    // Upgrade overlay if still visible
                    if let Some(ref overlay) = overlay_clone {
                        overlay
                            .upgrade_message(&msg_clone.message_id, personalized)
                            .await;
                    }
                    // Persist personalized text to storage
                    if let Some(ref cs) = storage_clone {
                        if let Err(e) = cs
                            .update_coaching_event_personalized(&msg_clone.message_id, personalized)
                        {
                            debug!("coaching personalization persist: {e}");
                        }
                    }
                }
                Ok(_) => { /* No suggestions returned — template remains */ }
                Err(e) => {
                    // D5 iter-16: LLM error body can echo user-context PII
                    // from the prompt (template_text + regime + prev_app).
                    // Route Display through `SanitizedDisplay` when attached.
                    match &pii_sanitizer {
                        Some(san) => debug!(
                            err.code = %e.code(),
                            "LLM coaching personalization failed: {}",
                            sanitized(&e, &**san, pii_level),
                        ),
                        None => {
                            debug!(err.code = %e.code(), "LLM coaching personalization failed: {e}")
                        }
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{flush_whole_minutes, resolve_pii_level};
    use maekon_core::config::{AppConfig, PiiFilterLevel};

    #[test]
    fn resolve_pii_level_defaults_to_standard_when_absent() {
        // #6830: no config snapshot → fail-safe to the default (Standard).
        assert_eq!(resolve_pii_level(None), PiiFilterLevel::Standard);
    }

    #[test]
    fn resolve_pii_level_reads_from_snapshot() {
        // #6830: reads the level off the passed snapshot (no ConfigManager::get()).
        let mut cfg = AppConfig::default_config();
        cfg.privacy.pii_filter_level = PiiFilterLevel::Strict;
        assert_eq!(resolve_pii_level(Some(&cfg)), PiiFilterLevel::Strict);
    }

    /// The production default poll is 1s — the exact shape that silently
    /// truncated to 0 minutes forever before #5669.
    #[test]
    fn one_second_polls_accumulate_into_minutes() {
        let mut carry = 0u64;
        for tick in 1..=59 {
            assert_eq!(
                flush_whole_minutes(&mut carry, 1),
                0,
                "no flush before 60s (tick {tick})"
            );
        }
        assert_eq!(
            flush_whole_minutes(&mut carry, 1),
            1,
            "60th second flushes 1 minute"
        );
        assert_eq!(carry, 0);
    }

    #[test]
    fn remainder_stays_in_carry() {
        let mut carry = 0u64;
        assert_eq!(flush_whole_minutes(&mut carry, 90), 1);
        assert_eq!(carry, 30, "30s remainder carried");
        assert_eq!(
            flush_whole_minutes(&mut carry, 30),
            1,
            "carry + 30s flushes again"
        );
        assert_eq!(carry, 0);
    }

    #[test]
    fn long_poll_flushes_multiple_minutes() {
        let mut carry = 5u64;
        assert_eq!(
            flush_whole_minutes(&mut carry, 130),
            2,
            "5+130=135s → 2 min"
        );
        assert_eq!(carry, 15);
    }

    /// #7480 regression: the coaching goal tracker + habit-streak persistence
    /// must key on the HUMAN regime label, not the opaque positional
    /// `regime_id`. A user configures a goal under the label they see
    /// ("Deep Focus (VSCode)"); the active regime carries that label but an
    /// opaque id ("regime-3"). One whole minute must accrue to the human-label
    /// goal and land a habit-streak row under that same label.
    ///
    /// Pre-fix `evaluate_and_deliver` fed `regime_id` as the label, so
    /// `record_minutes("regime-3", 1)` left the "Deep Focus (VSCode)" goal at 0
    /// (widget stuck at 0%) and the goal lookup never matched, so no habit-streak
    /// row was written — both assertions below fail on pre-fix code.
    #[tokio::test]
    async fn coaching_goal_and_habit_keyed_by_human_label_not_opaque_id() {
        use super::{evaluate_and_deliver, CoachingEvalContext, CoachingTickState};
        use maekon_analysis::CoachingEngine;
        use maekon_core::config::{CoachingConfig, PiiFilterLevel};
        use std::collections::HashMap;
        use std::sync::Arc;

        const HUMAN_LABEL: &str = "Deep Focus (VSCode)";
        const OPAQUE_ID: &str = "regime-3";

        // A user goal is configured under the HUMAN label they see in the UI.
        let engine = Arc::new(CoachingEngine::new(CoachingConfig::default()));
        let mut goals = HashMap::new();
        goals.insert(HUMAN_LABEL.to_string(), 60u32);
        engine.update_regime_goals(&goals).await;

        // Real scheduler-storage seam so the habit-streak write path is exercised
        // end-to-end (the widget's local producer).
        let storage_concrete = Arc::new(
            maekon_storage::sqlite::SqliteStorage::open_in_memory(30).expect("in-memory sqlite"),
        );
        let storage: Arc<dyn crate::scheduler::SchedulerStorage> = storage_concrete.clone();

        let mut tick_state = CoachingTickState::new();
        let ctx = CoachingEvalContext {
            coaching_engine: &engine,
            overlay: &None,
            notifier: &None,
            coaching_storage: &None,
            scheduler_storage: &storage,
            analysis_provider: &None,
            // Opaque positional id — the value that WAS wrongly used as the label.
            regime_id: Some(OPAQUE_ID),
            // Human label (name > auto_label) resolved upstream — the correct key.
            regime_label: Some(HUMAN_LABEL),
            prev_app: Some("VSCode"),
            drift_detected: false,
            // 60s flushes exactly one whole minute into `record_minutes`.
            poll_secs: 60,
            pii_sanitizer: &None,
            pii_level: PiiFilterLevel::Standard,
        };

        evaluate_and_deliver(&ctx, &mut tick_state).await;

        // The elapsed minute must accrue to the human-label goal (widget reads
        // this) — not the opaque id.
        let progress = engine.all_goal_progress().await;
        let deep = progress
            .iter()
            .find(|g| g.regime_label == HUMAN_LABEL)
            .expect("the configured human-label goal must be present");
        assert_eq!(
            deep.current_minutes, 1,
            "the elapsed minute must accrue to the human-label goal, not the opaque regime id"
        );

        // The habit-streak row must persist under the human label (pre-fix the
        // goal lookup used the id, never matched, and skipped the write).
        let streaks = storage_concrete
            .query_habit_streaks(7)
            .expect("query_habit_streaks");
        let row = streaks
            .iter()
            .find(|r| r.regime_label == HUMAN_LABEL)
            .expect("a habit-streak row must persist under the human label");
        assert_eq!(row.minutes_logged, 1);
        assert!(
            streaks.iter().all(|r| r.regime_label != OPAQUE_ID),
            "the opaque regime id must never become a habit-streak key"
        );
    }
}
