//! Pomodoro timer API contracts.

use serde::{Deserialize, Serialize};

/// Request body for POST /api/pomodoro/start.
#[derive(Debug, Deserialize)]
pub struct StartPomodoroRequest {
    /// Work duration in minutes. Defaults to 25.
    pub duration_minutes: Option<u32>,
    /// Break duration in minutes. Defaults to 5.
    pub break_minutes: Option<u32>,
}

/// Response for Pomodoro session endpoints.
#[derive(Debug, Serialize)]
pub struct PomodoroSessionResponse {
    pub id: String,
    pub started_at: String,
    pub duration_minutes: u32,
    pub break_minutes: u32,
    pub status: String,
    pub remaining_secs: i64,
    pub completed_at: Option<String>,
}
