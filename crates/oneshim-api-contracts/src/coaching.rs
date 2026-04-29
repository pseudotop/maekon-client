//! Coaching API contracts.

use oneshim_core::models::coaching::{CoachingEventRow, GoalProgressView};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Query parameters for GET /api/coaching/history.
#[derive(Debug, Deserialize)]
pub struct CoachingHistoryQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

/// Response DTO for a single coaching event.
#[derive(Debug, Serialize)]
pub struct CoachingEventResponse {
    pub event_id: String,
    pub trigger_type: String,
    pub profile_name: String,
    pub regime_id: Option<String>,
    pub message_template: String,
    pub personalized_message: Option<String>,
    pub shown_at: String,
    pub dismissed_at: Option<String>,
    pub dismiss_action: Option<String>,
    pub feedback_type: Option<String>,
    pub feedback_score: Option<f64>,
}

impl From<CoachingEventRow> for CoachingEventResponse {
    fn from(row: CoachingEventRow) -> Self {
        Self {
            event_id: row.event_id,
            trigger_type: row.trigger_type,
            profile_name: row.profile_name,
            regime_id: row.regime_id,
            message_template: row.message_template,
            personalized_message: row.personalized_message,
            shown_at: row.shown_at,
            dismissed_at: row.dismissed_at,
            dismiss_action: row.dismiss_action,
            feedback_type: row.feedback_type,
            feedback_score: row.feedback_score,
        }
    }
}

/// Response DTO for goal progress.
#[derive(Debug, Serialize)]
pub struct GoalProgressResponse {
    pub regime_label: String,
    pub current_minutes: u32,
    pub target_minutes: u32,
    pub percentage: u16,
    pub display_color: String,
}

impl From<GoalProgressView> for GoalProgressResponse {
    fn from(gp: GoalProgressView) -> Self {
        Self {
            regime_label: gp.regime_label,
            current_minutes: gp.current_minutes,
            target_minutes: gp.target_minutes,
            percentage: gp.percentage,
            display_color: gp.display_color,
        }
    }
}

/// Response DTO for GET /api/coaching/stats/today — aggregated coaching stats for the current day.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CoachingStatsTodayResponse {
    pub nudges_count: u32,
    pub current_regime: Option<String>,
    pub regime_minutes_today: u32,
}

/// Request body for PUT /api/coaching/goals.
#[derive(Debug, Deserialize)]
pub struct UpdateGoalsRequest {
    pub goals: HashMap<String, u32>,
}

/// Query parameters for GET /api/coaching/habits.
#[derive(Debug, Deserialize)]
pub struct HabitStreakQuery {
    /// Number of days to look back. Defaults to 7.
    pub days: Option<u32>,
}

/// Response DTO for a single habit streak row.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HabitStreakResponse {
    pub regime_label: String,
    pub date: String,
    pub minutes_logged: u32,
    pub target_minutes: u32,
    pub met: bool,
}

impl From<oneshim_core::models::coaching::HabitStreakRow> for HabitStreakResponse {
    fn from(row: oneshim_core::models::coaching::HabitStreakRow) -> Self {
        Self {
            regime_label: row.regime_label,
            date: row.date,
            minutes_logged: row.minutes_logged,
            target_minutes: row.target_minutes,
            met: row.met,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_coaching_stats_today() {
        let original = CoachingStatsTodayResponse {
            nudges_count: 3,
            current_regime: Some("deep_work".to_string()),
            regime_minutes_today: 120,
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: CoachingStatsTodayResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn round_trip_coaching_stats_today_no_regime() {
        let original = CoachingStatsTodayResponse {
            nudges_count: 0,
            current_regime: None,
            regime_minutes_today: 0,
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: CoachingStatsTodayResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn round_trip_habit_streak_response() {
        let original = HabitStreakResponse {
            regime_label: "deep_work".to_string(),
            date: "2026-04-11".to_string(),
            minutes_logged: 90,
            target_minutes: 120,
            met: false,
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: HabitStreakResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn habit_streak_met_roundtrip() {
        let original = HabitStreakResponse {
            regime_label: "communication".to_string(),
            date: "2026-04-10".to_string(),
            minutes_logged: 150,
            target_minutes: 60,
            met: true,
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: HabitStreakResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
        assert!(decoded.met);
    }
}
