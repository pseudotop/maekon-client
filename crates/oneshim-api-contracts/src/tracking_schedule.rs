//! Tracking-schedule API contracts — shared between IPC commands and REST handlers.

use serde::{Deserialize, Serialize};

/// Snapshot of the current tracking-schedule state.
///
/// Returned by the `get_tracking_schedule_status` IPC command and the
/// `GET /config/tracking_schedule/status` REST endpoint.
///
/// Timestamps are RFC 3339 strings so they survive JSON serialization losslessly
/// and are unambiguous about UTC offset.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrackingScheduleStatus {
    /// Whether tracking is currently muted by the schedule.
    pub active_now: bool,
    /// RFC 3339 timestamp when the current mute window ends, if active.
    pub ends_at: Option<String>,
    /// RFC 3339 timestamp when the next mute window begins, within 7 days.
    pub next_starts_at: Option<String>,
    /// Human-readable label of the currently active window, or empty string.
    pub label: String,
}
