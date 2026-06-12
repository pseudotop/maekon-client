use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ExportQuery {
    pub from: Option<String>,
    pub to: Option<String>,
    #[serde(default = "default_format")]
    pub format: String,
}

fn default_format() -> String {
    "json".to_string()
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MetricExportRecord {
    pub timestamp: String,
    pub cpu_usage: f32,
    pub memory_used: u64,
    pub memory_total: u64,
    pub memory_percent: f32,
    pub disk_used: u64,
    pub disk_total: u64,
    pub network_upload: u64,
    pub network_download: u64,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct EventExportRecord {
    pub event_id: String,
    pub event_type: String,
    pub timestamp: String,
    pub app_name: Option<String>,
    pub window_title: Option<String>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FrameExportRecord {
    pub id: i64,
    pub timestamp: String,
    pub trigger_type: String,
    pub app_name: String,
    pub window_title: String,
    pub importance: f32,
    pub resolution: String,
    pub ocr_text: Option<String>,
}
