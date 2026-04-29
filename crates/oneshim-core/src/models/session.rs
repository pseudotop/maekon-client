use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub user_id: String,
    pub started_at: DateTime<Utc>,
    pub last_active_at: DateTime<Utc>,
    pub health: ConnectionHealth,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionHealth {
    pub status: ConnectionStatus,
    pub last_heartbeat: Option<DateTime<Utc>>,
    pub consecutive_failures: u32,
    pub avg_latency_ms: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConnectionStatus {
    Connected,
    Connecting,
    Disconnected,
    Reconnecting,
    Failed,
}
