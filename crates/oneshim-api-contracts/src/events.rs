use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct EventResponse {
    pub event_id: String,
    pub event_type: String,
    pub timestamp: String,
    pub app_name: Option<String>,
    pub window_title: Option<String>,
    pub data: serde_json::Value,
}
