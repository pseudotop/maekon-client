use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeRange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PatternType {
    AppSequence,
    WorkMode,
    ContextSwitch,
    DeepWorkBlock,
    CommunicationBurst,
    CoOccurrence,
    /// Reserved for Phase 2: statistical change-point detection.
    BehavioralShift,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityPattern {
    pub pattern_type: PatternType,
    pub description: String,
    pub frequency: u32,
    pub confidence: f32,
    pub time_range: TimeRange,
    pub involved_apps: Vec<String>,
}
