use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Coaching profile — maps to a behavioral coaching personality.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CoachingProfile {
    /// Guards focus sessions from context-switch drift.
    FocusGuard,
    /// Time-awareness nudges (overstay, goal progress).
    TimeAware,
    /// Deep work session coaching (break reminders, session pacing).
    DeepWorkCoach,
    /// Helps restore context after returning from idle/break.
    ContextRestore,
    /// Tracks per-regime daily time goals and milestone alerts.
    GoalTracker,
}

/// Trigger that caused a coaching message to be generated.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TriggerType {
    /// User switched from one regime to another.
    RegimeTransition {
        from_regime: Option<String>,
        to_regime: Option<String>,
    },
    /// User stayed in a regime longer than the historical average.
    RegimeOverstay {
        regime_label: String,
        duration_secs: u64,
        avg_duration_secs: u64,
    },
    /// Detected attention drift within a regime (frequent app switches).
    RegimeDrift { regime_label: String },
    /// A daily time goal crossed a milestone threshold.
    GoalThreshold {
        regime_label: String,
        target_minutes: u32,
        current_minutes: u32,
        threshold_percent: u8,
    },
}

/// A coaching message produced by the engine and ready for display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoachingMessage {
    /// Unique identifier for feedback tracking.
    pub message_id: String,
    /// Which coaching profile produced this message.
    pub profile: CoachingProfile,
    /// The trigger that caused this message.
    pub trigger: TriggerType,
    /// Raw template text with placeholders already substituted.
    pub template_text: String,
    /// LLM-personalized text (Phase 2). Falls back to `template_text` when None.
    pub personalized_text: Option<String>,
    /// Variable key-value pairs used for template substitution.
    pub variables: HashMap<String, String>,
    /// Timestamp when the message was created.
    pub created_at: DateTime<Utc>,
    /// Human-readable explanation of why this message was triggered.
    #[serde(default)]
    pub explanation: String,
}

impl CoachingMessage {
    /// Returns the best available display text: personalized if present, otherwise template.
    pub fn display_text(&self) -> &str {
        self.personalized_text
            .as_deref()
            .unwrap_or(&self.template_text)
    }
}

/// How the user dismissed a coaching notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DismissAction {
    /// User acknowledged the message.
    Ok,
    /// User chose to be reminded later.
    Later,
    /// Message auto-dismissed after timeout.
    Timeout,
}

/// Feedback signal for coaching effectiveness tracking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeedbackSignal {
    /// User explicitly gave positive feedback (thumbs-up).
    ExplicitPositive,
    /// User explicitly gave negative feedback (thumbs-down).
    ExplicitNegative,
    /// Behavior changed positively within 5-minute window.
    ImplicitPositive,
    /// No clear behavior change detected.
    ImplicitNeutral,
    /// Behavior worsened (e.g., more context switches) within 5-minute window.
    ImplicitNegative,
}

/// Progress toward a per-regime daily time goal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalProgress {
    /// The regime this goal tracks (e.g., "Deep Work", "Communication").
    pub regime_label: String,
    /// Minutes accumulated today in this regime.
    pub current_minutes: u32,
    /// User-configured daily target for this regime.
    pub target_minutes: u32,
    /// Completion percentage. Values >100 are valid (user exceeded the goal).
    pub percentage: u16,
}

/// Extended goal progress with UI display hints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalProgressView {
    /// The regime this goal tracks.
    pub regime_label: String,
    /// Minutes accumulated today in this regime.
    pub current_minutes: u32,
    /// User-configured daily target.
    pub target_minutes: u32,
    /// Completion percentage. Values >100 are valid (user exceeded the goal).
    pub percentage: u16,
    /// CSS-compatible color string for progress bar rendering.
    pub display_color: String,
}

/// Storage query result for coaching history display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoachingEventRow {
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

/// A single day's habit record for one regime — maps to the `habit_streaks` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HabitStreakRow {
    pub regime_label: String,
    pub date: String,
    pub minutes_logged: u32,
    pub target_minutes: u32,
    pub met: bool,
}

/// Extract the variant name of a `TriggerType` for template matching and storage keys.
pub fn trigger_type_name(trigger: &TriggerType) -> String {
    match trigger {
        TriggerType::RegimeTransition { .. } => "RegimeTransition".to_string(),
        TriggerType::RegimeOverstay { .. } => "RegimeOverstay".to_string(),
        TriggerType::RegimeDrift { .. } => "RegimeDrift".to_string(),
        TriggerType::GoalThreshold { .. } => "GoalThreshold".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coaching_message_serde_roundtrip() {
        let mut variables = HashMap::new();
        variables.insert("regime".to_string(), "Deep Work".to_string());
        variables.insert("duration".to_string(), "2h 15m".to_string());

        let msg = CoachingMessage {
            message_id: "msg-001".to_string(),
            profile: CoachingProfile::DeepWorkCoach,
            trigger: TriggerType::RegimeOverstay {
                regime_label: "Deep Work".to_string(),
                duration_secs: 8100,
                avg_duration_secs: 5400,
            },
            template_text: "Deep work for {duration}. Take a break.".to_string(),
            personalized_text: Some(
                "Great focus! 2h 15m in deep work. Time for a break.".to_string(),
            ),
            variables,
            created_at: Utc::now(),
            explanation: String::new(),
        };

        let json = serde_json::to_string(&msg).expect("serialize");
        let restored: CoachingMessage = serde_json::from_str(&json).expect("deserialize");

        // Compare via serde_json::Value (HashMap order is non-deterministic)
        let val1: serde_json::Value = serde_json::from_str(&json).expect("parse original");
        let json2 = serde_json::to_string(&restored).expect("re-serialize");
        let val2: serde_json::Value = serde_json::from_str(&json2).expect("parse restored");
        assert_eq!(val1, val2, "JSON values must match after round-trip");
    }

    #[test]
    fn display_text_prefers_personalized() {
        let msg = CoachingMessage {
            message_id: "msg-002".to_string(),
            profile: CoachingProfile::FocusGuard,
            trigger: TriggerType::RegimeDrift {
                regime_label: "Coding".to_string(),
            },
            template_text: "template fallback".to_string(),
            personalized_text: Some("personalized text".to_string()),
            variables: HashMap::new(),
            created_at: Utc::now(),
            explanation: String::new(),
        };

        assert_eq!(msg.display_text(), "personalized text");
    }

    #[test]
    fn display_text_falls_back_to_template() {
        let msg = CoachingMessage {
            message_id: "msg-003".to_string(),
            profile: CoachingProfile::TimeAware,
            trigger: TriggerType::GoalThreshold {
                regime_label: "Communication".to_string(),
                target_minutes: 120,
                current_minutes: 90,
                threshold_percent: 75,
            },
            template_text: "template fallback text".to_string(),
            personalized_text: None,
            variables: HashMap::new(),
            created_at: Utc::now(),
            explanation: String::new(),
        };

        assert_eq!(msg.display_text(), "template fallback text");
    }

    #[test]
    fn trigger_type_name_variants() {
        assert_eq!(
            trigger_type_name(&TriggerType::RegimeTransition {
                from_regime: None,
                to_regime: Some("Focus".to_string()),
            }),
            "RegimeTransition"
        );
        assert_eq!(
            trigger_type_name(&TriggerType::RegimeOverstay {
                regime_label: "Work".to_string(),
                duration_secs: 3600,
                avg_duration_secs: 1800,
            }),
            "RegimeOverstay"
        );
        assert_eq!(
            trigger_type_name(&TriggerType::RegimeDrift {
                regime_label: "Coding".to_string(),
            }),
            "RegimeDrift"
        );
        assert_eq!(
            trigger_type_name(&TriggerType::GoalThreshold {
                regime_label: "Deep Work".to_string(),
                target_minutes: 240,
                current_minutes: 60,
                threshold_percent: 25,
            }),
            "GoalThreshold"
        );
    }
}
