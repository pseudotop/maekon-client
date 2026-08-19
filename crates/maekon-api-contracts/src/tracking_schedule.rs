//! Tracking-schedule API contracts.
//!
//! #7600: the `get_tracking_schedule`/`set_tracking_schedule`/
//! `get_tracking_schedule_status` Tauri IPC commands were removed as dead
//! duplicates — the frontend drives tracking-schedule config exclusively via
//! the embedded HTTP API. These types remain shared with the REST handlers.

use serde::{Deserialize, Serialize};

/// Snapshot of the current tracking-schedule state.
///
/// Returned by the `GET /tracking-schedule/status` REST endpoint.
///
/// Timestamps are RFC 3339 strings so they survive JSON serialization losslessly
/// and are unambiguous about UTC offset.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TrackingScheduleStatus {
    /// Whether the current time is inside a configured allowed window.
    pub active_now: bool,
    /// RFC 3339 timestamp when the current allowed window ends, if active.
    pub ends_at: Option<String>,
    /// RFC 3339 timestamp when the next allowed window begins, within 7 days.
    pub next_starts_at: Option<String>,
    /// Human-readable label of the currently active window, or empty string.
    pub label: String,
}
