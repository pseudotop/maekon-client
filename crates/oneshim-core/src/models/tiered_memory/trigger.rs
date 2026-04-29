use serde::{Deserialize, Serialize};

use crate::models::work_session::AppCategory;

use super::content::WorkType;

// ---------------------------------------------------------------------------
// TriggerInput — raw event dispatched to AdaptiveTrigger
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE", tag = "type")]
pub enum TriggerInput {
    AppSwitchNew {
        app_name: String,
        prev_app: String,
        category: AppCategory,
    },
    AppPoll {
        app_name: String,
    },
    WindowTitleChange {
        app_name: String,
        new_title: String,
    },
    IdleTransition {
        to_idle: bool,
    },
    OcrUpdate {
        diff_ratio: f32,
    },
    InputActivity,
    ProcessSnapshot,
    SystemMetric,
    ClipboardChange,
    FileAccess,
    WorkTypeChange {
        from: WorkType,
        to: WorkType,
    },
}

// ---------------------------------------------------------------------------
// TriggerAction / TriggerReason
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TriggerAction {
    Start,
    Close,
    ForceClose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TriggerReason {
    ScoreHigh,
    ScoreLow,
    ForcedMaxDuration,
    RegimeChange,
    IdleStart,
}
