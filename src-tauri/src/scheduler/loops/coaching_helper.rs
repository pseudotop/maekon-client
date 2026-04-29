use std::sync::Arc;
use tracing::{debug, info, warn};

use oneshim_analysis::CoachingEngine;
use oneshim_core::config::PiiFilterLevel;
use oneshim_core::models::coaching;
use oneshim_core::ports::coaching_storage::CoachingStoragePort;
use oneshim_core::ports::pii_sanitizer::PiiSanitizer;
use oneshim_core::sanitized;

use crate::magic_overlay::MagicOverlayHandle;

use super::helpers::{build_personalization_prompt, COACHING_SYSTEM_PROMPT};

/// D5 iter-16: construct the `VisionPiiSanitizer` used for scrubbing LLM
/// error Display output at the coaching tracing boundary. Returned wrapped
/// in `Option` so callers can thread `None` for no-sanitize tests.
pub(super) fn build_pii_sanitizer() -> Option<Arc<dyn PiiSanitizer>> {
    Some(Arc::new(oneshim_vision::privacy::VisionPiiSanitizer))
}

/// Resolve the current `PiiFilterLevel` from an optional `ConfigManager`.
/// Returns the default level (`Standard`) when no manager is configured —
/// matches the established scheduler pattern (`config_manager1.as_ref()...`).
pub(super) fn resolve_pii_level(
    config_manager: &Option<oneshim_core::config_manager::ConfigManager>,
) -> PiiFilterLevel {
    config_manager
        .as_ref()
        .map(|cm| cm.get().privacy.pii_filter_level)
        .unwrap_or_default()
}

/// Mutable state carried across monitor ticks for coaching regime tracking.
pub(super) struct CoachingTickState {
    pub(super) regime_entered_at: Option<std::time::Instant>,
    pub(super) prev_coaching_regime_id: Option<String>,
}

impl CoachingTickState {
    pub(super) fn new() -> Self {
        Self {
            regime_entered_at: None,
            prev_coaching_regime_id: None,
        }
    }
}

/// Parameters needed to evaluate and deliver a coaching message within a
/// single monitor tick.
pub(super) struct CoachingEvalContext<'a> {
    pub(super) coaching_engine: &'a Arc<CoachingEngine>,
    pub(super) overlay: &'a Option<MagicOverlayHandle>,
    pub(super) notifier: &'a Option<Arc<crate::notification_manager::NotificationManager>>,
    pub(super) coaching_storage: &'a Option<Arc<dyn CoachingStoragePort>>,
    pub(super) analysis_provider:
        &'a Option<Arc<dyn oneshim_core::ports::analysis_provider::AnalysisProvider>>,
    pub(super) regime_id: Option<&'a str>,
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
    let regime_label = ctx.regime_id.unwrap_or("Unknown");
    let avg_regime_duration_secs = ctx
        .coaching_engine
        .avg_regime_duration_secs(regime_label)
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

    // Record elapsed minutes for goal tracking
    let elapsed_minutes = (ctx.poll_secs as f32 / 60.0).max(0.0) as u32;
    if elapsed_minutes > 0 {
        ctx.coaching_engine
            .record_minutes(regime_label, elapsed_minutes)
            .await;
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
