//! Shared types for `FocusAnalyzer` across binary crates (`oneshim-app`, `src-tauri`).
//!
//! This module eliminates code duplication by centralising config structs,
//! cooldown enums, session tracking state, and the `make_rule_suggestion`
//! helper that both binary entry-points need.

use chrono::{DateTime, Utc};
use oneshim_core::models::suggestion::{Priority, Suggestion, SuggestionSource, SuggestionType};
use oneshim_core::models::work_session::AppCategory;
use uuid::Uuid;

// ── Configuration ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FocusAnalyzerConfig {
    #[allow(dead_code)]
    pub deep_work_min_secs: u64,
    pub break_suggestion_mins: u32,
    pub excessive_communication_threshold: f32,
    pub suggestion_cooldown_secs: u64,
    pub focus_score_deep_work_weight: f32,
    pub focus_score_interruption_penalty: f32,
    pub workflow_split_idle_secs: u64,
    pub playbook_min_relevance: f32,
    pub playbook_stale_flush_secs: u64,
}

impl Default for FocusAnalyzerConfig {
    fn default() -> Self {
        Self {
            deep_work_min_secs: 300,                // 5 min
            break_suggestion_mins: 90,              // 90 min
            excessive_communication_threshold: 0.4, // 40%
            suggestion_cooldown_secs: 1800,         // 30 min
            focus_score_deep_work_weight: 0.7,
            focus_score_interruption_penalty: 0.1,
            workflow_split_idle_secs: 300, // 5 min
            playbook_min_relevance: 0.35,
            playbook_stale_flush_secs: 900, // 15 min
        }
    }
}

// ── Cooldown ──────────────────────────────────────────────────────

/// Typed cooldown categories — eliminates magic-string matching in
/// `check_cooldown` / `update_cooldown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CooldownType {
    Break,
    FocusTime,
    RestoreContext,
    /// Reserved for future excessive-communication cooldown.
    #[allow(dead_code)]
    ExcessiveComm,
    PatternDetected,
}

#[derive(Debug, Default)]
pub struct SuggestionCooldowns {
    pub last_break: Option<DateTime<Utc>>,
    pub last_focus_time: Option<DateTime<Utc>>,
    pub last_restore_context: Option<DateTime<Utc>>,
    pub last_excessive_comm: Option<DateTime<Utc>>,
    pub last_pattern_detected: Option<DateTime<Utc>>,
}

// ── Session tracking ──────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct SessionTracker {
    pub active_session_id: Option<i64>,
    pub current_app: Option<String>,
    pub current_category: Option<AppCategory>,
    pub current_app_start: Option<DateTime<Utc>>,
    pub continuous_deep_work_secs: u64,
    pub pending_interruption_id: Option<i64>,
}

// ── Helpers ───────────────────────────────────────────────────────

/// Create a rule-based `Suggestion` with the given parameters.
pub fn make_rule_suggestion(
    suggestion_type: SuggestionType,
    content: String,
    confidence: f64,
    priority: Priority,
) -> Suggestion {
    Suggestion {
        suggestion_id: Uuid::new_v4().to_string(),
        suggestion_type,
        content,
        priority,
        confidence_score: confidence,
        relevance_score: confidence,
        is_actionable: true,
        created_at: Utc::now(),
        expires_at: None,
        source: SuggestionSource::RuleBased,
        reasoning: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_values() {
        let cfg = FocusAnalyzerConfig::default();
        assert_eq!(cfg.deep_work_min_secs, 300);
        assert_eq!(cfg.break_suggestion_mins, 90);
        assert!((cfg.excessive_communication_threshold - 0.4).abs() < f32::EPSILON);
        assert_eq!(cfg.suggestion_cooldown_secs, 1800);
    }

    #[test]
    fn cooldown_type_equality() {
        assert_eq!(CooldownType::Break, CooldownType::Break);
        assert_ne!(CooldownType::Break, CooldownType::FocusTime);
    }

    #[test]
    fn suggestion_cooldowns_default() {
        let cd = SuggestionCooldowns::default();
        assert!(cd.last_break.is_none());
        assert!(cd.last_focus_time.is_none());
        assert!(cd.last_restore_context.is_none());
        assert!(cd.last_excessive_comm.is_none());
        assert!(cd.last_pattern_detected.is_none());
    }

    #[test]
    fn session_tracker_default() {
        let st = SessionTracker::default();
        assert!(st.active_session_id.is_none());
        assert!(st.current_app.is_none());
        assert!(st.current_category.is_none());
        assert_eq!(st.continuous_deep_work_secs, 0);
    }

    #[test]
    fn make_rule_suggestion_fields() {
        let s = make_rule_suggestion(
            SuggestionType::ProductivityTip,
            "test content".into(),
            0.85,
            Priority::High,
        );
        assert_eq!(s.suggestion_type, SuggestionType::ProductivityTip);
        assert_eq!(s.content, "test content");
        assert!((s.confidence_score - 0.85).abs() < f64::EPSILON);
        assert_eq!(s.priority, Priority::High);
        assert!(s.is_actionable);
        assert_eq!(s.source, SuggestionSource::RuleBased);
        assert!(s.reasoning.is_none());
        assert!(s.expires_at.is_none());
        assert!(!s.suggestion_id.is_empty());
    }
}
