use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserContext {
    pub timestamp: DateTime<Utc>,
    pub active_window: Option<WindowInfo>,
    pub processes: Vec<ProcessInfo>,
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

/// On-demand cursor position lookup (Win32 `GetCursorPos`). NOT part of
/// `UserContext` -- that per-tick polling path was removed as dead/wasteful
/// (#7652 HIGH-1: collected every tick on every platform, never read).
/// Still used for two legitimate, bounded, event-triggered call sites:
/// `mouse_hook::windows` resolves a left-click's screen position at the
/// moment of the click, and the `windows_sandbox_overhead` CLI diagnostic
/// resolves the current position once per invocation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MousePosition {
    pub x: i32,
    pub y: i32,
}
