use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserContext {
    pub timestamp: DateTime<Utc>,
    pub active_window: Option<WindowInfo>,
    pub processes: Vec<ProcessInfo>,
    pub mouse_position: Option<MousePosition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowInfo {
    pub title: String,
    pub app_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_bundle_id: Option<String>,
    pub pid: u32,
    pub bounds: Option<WindowBounds>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WindowBounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub cpu_usage: f32,
    pub memory_bytes: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MousePosition {
    pub x: i32,
    pub y: i32,
}
