//! Audit export API contracts — shared between REST handlers and IPC commands.

use serde::{Deserialize, Serialize};

/// Privacy-safe structural proof for an allowed external chat envelope.
///
/// This deliberately contains only booleans and counts derived from the
/// sanitizer. The free-form audit details, prompt, context values, attachment
/// metadata/body, and provider output remain absent from the wire contract.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ExternalChatEgressOracleDto {
    pub version: u8,
    pub attachment_count: u64,
    pub context_present: bool,
    pub context_changed: bool,
    pub attachments_changed: bool,
    pub attachments_with_inline_data_before: u64,
    pub attachments_with_inline_data_after: u64,
    pub envelope_changed: bool,
}

/// Privacy-bounded row returned by `GET /api/audit/export`.
///
/// Session identifiers and free-form details are deliberately absent. Those
/// fields can contain consent identifiers or other runtime payloads and are not
/// required for a human-readable audit evidence snapshot. Hash-chain integrity
/// remains available through the separate audit verification endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AuditExportEntryDto {
    pub entry_id: String,
    pub timestamp: String,
    pub command_id: String,
    pub action_type: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_time_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub privacy_oracle: Option<ExternalChatEgressOracleDto>,
}

/// Query parameters for `GET /api/audit/export`.
///
/// Supports optional filtering by `command_id` and a `limit` cap (DoS guard).
/// `status` is reserved for future use (currently no-op).
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AuditExportQuery {
    /// Filter entries by exact `command_id` match.
    /// Empty string is treated as absent (falls back to `recent_entries`).
    #[serde(default)]
    pub command_id: Option<String>,
    /// Status filter (#8114): full-window query by the entry's status token
    /// (`Completed`, `Failed`, `Denied`, `Timeout`, `Started`) — same
    /// semantics as the automation buffer list. Unknown tokens are a 400.
    /// Empty string is treated as absent; `command_id` takes precedence.
    #[serde(default)]
    pub status: Option<String>,
    /// Maximum number of entries to return (default: 100, capped at 1000).
    #[serde(default)]
    pub limit: Option<usize>,
}
