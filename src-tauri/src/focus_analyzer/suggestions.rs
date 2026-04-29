use chrono::{DateTime, Duration, Utc};
use oneshim_analysis::focus_shared::make_rule_suggestion;
use oneshim_core::models::suggestion::{Priority, SuggestionType};
use oneshim_core::models::work_session::FocusMetrics;
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

    pub(super) async fn maybe_suggest_break(&self) {
        let tracker = self.tracker.read().await;
        let continuous_mins = (tracker.continuous_deep_work_secs / 60) as u32;

        if continuous_mins < self.config.break_suggestion_mins {
            return;
        }

        if !self.check_cooldown(CooldownType::Break).await {
            return;
        }

        let suggestion = make_rule_suggestion(
            SuggestionType::ProductivityTip,
            format!(
                "You've been working for {} minutes continuously. Consider taking a short break.",
                continuous_mins
            ),
            0.9,
            Priority::High,
        );

        let suggestion_id = match self.storage.save_rule_suggestion(&suggestion) {
            Ok(id) => id,
            Err(e) => {
                warn!("suggestion save failure: {e}");
                return;
            }
        };

        let title = "☕ 휴식 시간";
        let body = format!(
            "{}분 동안 집중하셨습니다. 잠시 휴식을 취해보세요!",
            continuous_mins
        );

        if let Err(e) = self.notifier.show_notification(title, &body).await {
            warn!("notification failure: {e}");
        } else {
            if let Err(e) = self.storage.mark_suggestion_shown_by_id(&suggestion_id) {
                debug!("mark_suggestion_shown_by_id (break) failed: {e}");
            }
            info!("suggestion sent: {}min consecutive", continuous_mins);
        }

        self.update_cooldown(CooldownType::Break).await;
    }

    pub(super) async fn maybe_suggest_focus_time(&self, metrics: &FocusMetrics) {
        let comm_ratio = metrics.communication_ratio();

        if comm_ratio < self.config.excessive_communication_threshold {
            return;
        }

        if !self.check_cooldown(CooldownType::FocusTime).await {
            return;
        }

        let suggested_focus_mins = (metrics.communication_secs / 60).max(30) as u32;

        let suggestion = make_rule_suggestion(
            SuggestionType::ProductivityTip,
            format!(
                "Communication ratio is {:.0}%. Consider blocking {} minutes of focus time.",
                comm_ratio * 100.0,
                suggested_focus_mins
            ),
            0.85,
            Priority::Medium,
        );

        let suggestion_id = match self.storage.save_rule_suggestion(&suggestion) {
            Ok(id) => id,
            Err(e) => {
                warn!("in progress hour suggestion save failure: {e}");
                return;
            }
        };

        let title = "🎯 집중 시간 필요";
        let body = format!(
            "오늘 소통에 {:.0}%의 시간을 사용했습니다. {}분의 집중 시간을 확보해보세요.",
            comm_ratio * 100.0,
            suggested_focus_mins
        );

        if let Err(e) = self.notifier.show_notification(title, &body).await {
            warn!("in progress hour notification failure: {e}");
        } else {
            if let Err(e) = self.storage.mark_suggestion_shown_by_id(&suggestion_id) {
                debug!("mark_suggestion_shown_by_id (focus_time) failed: {e}");
            }
            info!(
                "in progress hour suggestion sent: {:.1}%",
                comm_ratio * 100.0
            );
        }

        self.update_cooldown(CooldownType::FocusTime).await;
    }

    pub(super) async fn maybe_suggest_restore_context(&self, app: &str, now: DateTime<Utc>) {
        if !self.check_cooldown(CooldownType::RestoreContext).await {
            return;
        }

        let interruption = match self.storage.get_pending_interruption() {
            Ok(Some(int)) => int,
            _ => return,
        };

        if (now - interruption.interrupted_at).num_minutes() > 30 {
            return;
        }

        let duration_mins = (now - interruption.interrupted_at).num_minutes();

        let suggestion = make_rule_suggestion(
            SuggestionType::ContextBased,
            format!(
                "You were interrupted from {} about {} minutes ago. Consider restoring your previous context.",
                app, duration_mins
            ),
            0.9,
            Priority::High,
        );

        let suggestion_id = match self.storage.save_rule_suggestion(&suggestion) {
            Ok(id) => id,
            Err(e) => {
                warn!("context restore suggestion save failure: {e}");
                return;
            }
        };

        let title = "🔄 작업 context";
        let body = format!(
            "{}에서 {}분 전 중단되었습니다. 이전 작업으로 돌아가시겠습니까?",
            app, duration_mins
        );

        if let Err(e) = self.notifier.show_notification(title, &body).await {
            warn!("context restore notification failure: {e}");
        } else {
            if let Err(e) = self.storage.mark_suggestion_shown_by_id(&suggestion_id) {
                debug!("mark_suggestion_shown_by_id (restore_context) failed: {e}");
            }
            info!(
                "context 복원 suggestion 발송: {} ({}분 전 중단)",
                app, duration_mins
            );
        }

        self.update_cooldown(CooldownType::RestoreContext).await;
    }

    pub(super) async fn maybe_suggest_pattern_detected(&self, signal: PlaybookSignal) {
        if !self.check_cooldown(CooldownType::PatternDetected).await {
            return;
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

        let suggestion_id = match self.storage.save_rule_suggestion(&suggestion) {
            Ok(id) => id,
            Err(e) => {
                warn!("suggestion save failure: {e}");
                return;
            }
        };

        let title = "🧭 반복 플레이북";
        let confidence_percent = (signal.confidence * 100.0).round() as i32;
        let body = format!(
            "{} (confidence {}%)",
            signal.description, confidence_percent
        );

        if let Err(e) = self.notifier.show_notification(title, &body).await {
            warn!("suggestion notification failure: {e}");
            return;
        }

        if let Err(e) = self.storage.mark_suggestion_shown_by_id(&suggestion_id) {
            debug!("mark_suggestion_shown_by_id (pattern_detected) failed: {e}");
        }
        info!(
            confidence = signal.confidence,
            description = %signal.description,
            "플레이북 패턴 suggestion 발송"
        );
        self.update_cooldown(CooldownType::PatternDetected).await;
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
