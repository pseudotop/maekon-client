use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::work_session::AppCategory;

use super::trigger::TriggerAction;

// ---------------------------------------------------------------------------
// CalibrationEntry — one row per trigger event for offline analysis
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalibrationEntry {
    pub timestamp: DateTime<Utc>,
    pub event_type: String,
    pub app_name: String,
    pub app_category: AppCategory,
    pub event_importance: f32,
    pub density_signal: f32,
    pub importance_signal: f32,
    pub context_signal: f32,
    pub buffer_signal: f32,
    pub trigger_score: f32,
    pub trigger_action: Option<TriggerAction>,
    pub active_regime_id: Option<String>,
    pub params_version_id: String,
    /// Serialized JSON of the ResolvedParams used when this entry was produced.
    /// Written to `trigger_params_snapshots` during `log_batch`; not read back
    /// for analysis queries.
    #[serde(default)]
    pub params_json: String,
    pub is_noise: bool,
}
