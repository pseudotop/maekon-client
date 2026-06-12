use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DateQuery {
    pub date: Option<String>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AppUsageEntry {
    pub name: String,
    pub duration_secs: u64,
    pub event_count: u64,
    pub frame_count: u64,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DailySummaryResponse {
    pub date: String,
    pub total_active_secs: u64,
    pub total_idle_secs: u64,
    pub top_apps: Vec<AppUsageEntry>,
    pub cpu_avg: f64,
    pub memory_avg_percent: f64,
    pub frames_captured: u64,
    pub events_logged: u64,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AppUsageResponse {
    pub date: String,
    pub apps: Vec<AppUsageEntry>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct HeatmapQuery {
    pub days: Option<u32>,
}

#[derive(Debug, Serialize, Clone)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct HeatmapCell {
    pub day: u8,
    pub hour: u8,
    pub value: u32,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct HeatmapResponse {
    pub from_date: String,
    pub to_date: String,
    pub cells: Vec<HeatmapCell>,
    pub max_value: u32,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GuiHeatmapQuery {
    pub start: Option<String>,
    pub end: Option<String>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GuiHeatmapCell {
    pub hour: String,
    pub count: u32,
}
