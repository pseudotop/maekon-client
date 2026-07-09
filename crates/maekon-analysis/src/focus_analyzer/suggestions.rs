use crate::focus_shared::make_rule_suggestion;
use chrono::{DateTime, Duration, Utc};
use maekon_core::models::suggestion::{Priority, Suggestion, SuggestionType};
use maekon_core::models::work_session::FocusMetrics;
use tracing::{debug, info, warn};

use crate::workflow_intelligence::PlaybookSignal;

use super::models::CooldownType;
use super::FocusAnalyzer;

impl FocusAnalyzer {
    pub(super) fn calculate_focus_score(&self, metrics: &FocusMetrics) -> f32 {
        if metrics.total_active_secs == 0 {
            return 0.0;
        }

        let deep_work_ratio = metrics.deep_work_secs as f32 / metrics.total_active_secs as f32;
        let interruption_penalty = (metrics.interruption_count as f32
            * self.config.focus_score_interruption_penalty)
            .min(0.5);

        ((deep_work_ratio * self.config.focus_score_deep_work_weight) - interruption_penalty)
            .clamp(0.0, 1.0)
    }

    pub(super) async fn maybe_suggest_break(&self) -> Option<Suggestion> {
        let tracker = self.tracker.read().await;
        let continuous_mins = (tracker.continuous_deep_work_secs / 60) as u32;

        if continuous_mins < self.config.break_suggestion_mins {
            return None;
        }

        if !self.check_cooldown(CooldownType::Break).await {
            return None;
        }

        let suggestion = make_rule_suggestion(
            // #5696: dedicated type (was ProductivityTip) — break/focus_time
            // previously collided in the dashboard read-dedupe and shared one icon.
            SuggestionType::TakeBreak,
            format!(
                "You've been working for {} minutes continuously. Consider taking a short break.",
                continuous_mins
            ),
            0.9,
            Priority::High,
        );

        let suggestion_id = match self.storage.save_rule_suggestion(&suggestion).await {
            Ok(id) => id,
            Err(e) => {
                warn!("suggestion save failure: {e}");
                return None;
            }
        };

        // Native OS notification copy is English-only by convention: it is rendered
        // by the OS outside the WebView, so the frontend i18n layer (which owns the
        // user's locale) cannot translate it (cf. scheduler/loops/sync.rs). The
        // frontend-displayed `suggestion.content` above is the localizable surface.
        let title = "☕ Break time";
        let body = format!(
            "You've focused for {} minutes. Consider taking a short break!",
            continuous_mins
        );

        if let Err(e) = self.notifier.show_notification(title, &body).await {
            warn!("notification failure: {e}");
        } else {
            if let Err(e) = self
                .storage
                .mark_suggestion_shown_by_id(&suggestion_id)
                .await
            {
                debug!("mark_suggestion_shown_by_id (break) failed: {e}");
            }
            info!("suggestion sent: {}min consecutive", continuous_mins);
        }

        self.update_cooldown(CooldownType::Break).await;
        Some(suggestion)
    }

    pub(super) async fn maybe_suggest_focus_time(
        &self,
        metrics: &FocusMetrics,
    ) -> Option<Suggestion> {
        let comm_ratio = metrics.communication_ratio();

        if comm_ratio < self.config.excessive_communication_threshold {
            return None;
        }

        if !self.check_cooldown(CooldownType::FocusTime).await {
            return None;
        }

        let suggested_focus_mins = (metrics.communication_secs / 60).max(30) as u32;

        let suggestion = make_rule_suggestion(
            // #5696: dedicated type (was ProductivityTip) — see maybe_suggest_break.
            SuggestionType::NeedFocusTime,
            format!(
                "Communication ratio is {:.0}%. Consider blocking {} minutes of focus time.",
                comm_ratio * 100.0,
                suggested_focus_mins
            ),
            0.85,
            Priority::Medium,
        );

        let suggestion_id = match self.storage.save_rule_suggestion(&suggestion).await {
            Ok(id) => id,
            Err(e) => {
                warn!("in progress hour suggestion save failure: {e}");
                return None;
            }
        };

        let title = "🎯 Focus time needed";
        let body = format!(
            "You've spent {:.0}% of today on communication. Consider blocking {} minutes of focus time.",
            comm_ratio * 100.0,
            suggested_focus_mins
        );

        if let Err(e) = self.notifier.show_notification(title, &body).await {
            warn!("in progress hour notification failure: {e}");
        } else {
            if let Err(e) = self
                .storage
                .mark_suggestion_shown_by_id(&suggestion_id)
                .await
            {
                debug!("mark_suggestion_shown_by_id (focus_time) failed: {e}");
            }
            info!(
                "in progress hour suggestion sent: {:.1}%",
                comm_ratio * 100.0
            );
        }

        self.update_cooldown(CooldownType::FocusTime).await;
        Some(suggestion)
    }

    pub(super) async fn maybe_suggest_restore_context(
        &self,
        app: &str,
        now: DateTime<Utc>,
    ) -> Option<Suggestion> {
        if !self.check_cooldown(CooldownType::RestoreContext).await {
            return None;
        }

        let interruption = match self.storage.get_pending_interruption().await {
            Ok(Some(int)) => int,
            _ => return None,
        };

        if (now - interruption.interrupted_at).num_minutes() > 30 {
            return None;
        }

        let duration_mins = (now - interruption.interrupted_at).num_minutes();

        let suggestion = make_rule_suggestion(
            // #5696: dedicated type (was ContextBased).
            SuggestionType::RestoreContext,
            format!(
                "You were interrupted from {} about {} minutes ago. Consider restoring your previous context.",
                app, duration_mins
            ),
            0.9,
            Priority::High,
        );

        let suggestion_id = match self.storage.save_rule_suggestion(&suggestion).await {
            Ok(id) => id,
            Err(e) => {
                warn!("context restore suggestion save failure: {e}");
                return None;
            }
        };

        let title = "🔄 Restore context";
        let body = format!(
            "You were interrupted from {} {} minutes ago. Return to your previous task?",
            app, duration_mins
        );

        if let Err(e) = self.notifier.show_notification(title, &body).await {
            warn!("context restore notification failure: {e}");
        } else {
            if let Err(e) = self
                .storage
                .mark_suggestion_shown_by_id(&suggestion_id)
                .await
            {
                debug!("mark_suggestion_shown_by_id (restore_context) failed: {e}");
            }
            info!(
                "context restore suggestion sent: {} (interrupted {} min ago)",
                app, duration_mins
            );
        }

        self.update_cooldown(CooldownType::RestoreContext).await;
        Some(suggestion)
    }

    pub(super) async fn maybe_suggest_pattern_detected(
        &self,
        signal: PlaybookSignal,
    ) -> Option<Suggestion> {
        if !self.check_cooldown(CooldownType::PatternDetected).await {
            return None;
        }

        let suggestion = make_rule_suggestion(
            SuggestionType::WorkflowOptimization,
            format!(
                "Recurring workflow pattern detected: {} (confidence {:.0}%)",
                signal.description,
                signal.confidence * 100.0
            ),
            signal.confidence as f64,
            Priority::Medium,
        );

        let suggestion_id = match self.storage.save_rule_suggestion(&suggestion).await {
            Ok(id) => id,
            Err(e) => {
                warn!("suggestion save failure: {e}");
                return None;
            }
        };

        let title = "🧭 Recurring playbook";
        let confidence_percent = (signal.confidence * 100.0).round() as i32;
        let body = format!(
            "{} (confidence {}%)",
            signal.description, confidence_percent
        );

        if let Err(e) = self.notifier.show_notification(title, &body).await {
            warn!("suggestion notification failure: {e}");
            // Saved already — report it to the live-queue bridge (#5696); the
            // cooldown stays un-bumped so the OS toast retries next tick.
            return Some(suggestion);
        }

        if let Err(e) = self
            .storage
            .mark_suggestion_shown_by_id(&suggestion_id)
            .await
        {
            debug!("mark_suggestion_shown_by_id (pattern_detected) failed: {e}");
        }
        info!(
            confidence = signal.confidence,
            description = %signal.description,
            "playbook pattern suggestion sent"
        );
        self.update_cooldown(CooldownType::PatternDetected).await;
        Some(suggestion)
    }

    pub(super) async fn check_cooldown(&self, cooldown_type: CooldownType) -> bool {
        let cooldowns = self.cooldowns.read().await;
        let now = Utc::now();
        let cooldown_duration = Duration::seconds(self.config.suggestion_cooldown_secs as i64);

        let last_time = match cooldown_type {
            CooldownType::Break => cooldowns.last_break,
            CooldownType::FocusTime => cooldowns.last_focus_time,
            CooldownType::RestoreContext => cooldowns.last_restore_context,
            CooldownType::ExcessiveComm => cooldowns.last_excessive_comm,
            CooldownType::PatternDetected => cooldowns.last_pattern_detected,
        };

        match last_time {
            Some(last) => now - last > cooldown_duration,
            None => true,
        }
    }

    pub(super) async fn update_cooldown(&self, cooldown_type: CooldownType) {
        let mut cooldowns = self.cooldowns.write().await;
        let now = Utc::now();

        match cooldown_type {
            CooldownType::Break => cooldowns.last_break = Some(now),
            CooldownType::FocusTime => cooldowns.last_focus_time = Some(now),
            CooldownType::RestoreContext => cooldowns.last_restore_context = Some(now),
            CooldownType::ExcessiveComm => cooldowns.last_excessive_comm = Some(now),
            CooldownType::PatternDetected => cooldowns.last_pattern_detected = Some(now),
        }
    }
}
