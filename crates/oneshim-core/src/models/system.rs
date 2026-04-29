use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMetrics {
    pub timestamp: DateTime<Utc>,
    pub cpu_usage: f32,
    pub memory_used: u64,
    pub memory_total: u64,
    pub disk_used: u64,
    pub disk_total: u64,
    pub network: Option<NetworkInfo>,
    #[serde(default)]
    pub typing_wpm: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkInfo {
    pub upload_speed: u64,
    pub download_speed: u64,
    pub is_connected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertInfo {
    pub alert_type: AlertType,
    pub message: String,
    pub severity: f32,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AlertType {
    HighCpu,
    LowMemory,
    LowDisk,
    NetworkDisconnected,
}

/// Static system information for bug reports and diagnostics.
#[derive(Debug, Clone)]
pub struct StaticSystemInfo {
    pub os_version: String,
    pub cpu_count: usize,
    pub memory_total_bytes: u64,
    pub memory_available_bytes: u64,
    pub uptime_seconds: u64,
}
