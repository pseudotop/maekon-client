use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::analysis::ActivityPattern;

use super::content::{ContainerInfo, ContentActivity};
use super::trigger::TriggerReason;

// ---------------------------------------------------------------------------
// SegmentSummary — output of one closed segment
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SegmentSummary {
    pub segment_id: String,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub duration_secs: u64,
    pub regime_id: Option<String>,
    pub trigger_reason: TriggerReason,
    pub event_count: u32,
    pub app_breakdown: HashMap<String, u64>,
    pub category_breakdown: HashMap<String, u64>,
    pub context_switch_count: u32,
    pub dominant_category: String,
    pub avg_importance: f32,
    pub patterns_detected: Vec<ActivityPattern>,
    pub content_activities: Vec<ContentActivity>,
    pub container: Option<ContainerInfo>,
    pub llm_summary: Option<String>,
}
