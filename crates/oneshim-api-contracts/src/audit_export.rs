//! Audit export API contracts — shared between REST handlers and IPC commands.

use serde::Deserialize;

/// Query parameters for `GET /api/audit/export`.
///
/// Supports optional filtering by `command_id` and a `limit` cap (DoS guard).
/// `status` is reserved for future use (currently no-op).
#[derive(Debug, Deserialize)]
pub struct AuditExportQuery {
    /// Filter entries by exact `command_id` match.
    /// Empty string is treated as absent (falls back to `recent_entries`).
    #[serde(default)]
    pub command_id: Option<String>,
    /// Status-based filter — reserved for future use; currently no-op.
    #[serde(default)]
    pub status: Option<String>,
    /// Maximum number of entries to return (default: 100, capped at 1000).
    #[serde(default)]
    pub limit: Option<usize>,
}
