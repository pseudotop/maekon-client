use oneshim_core::models::recalibration::UserOverrideAction;
use serde::{Deserialize, Serialize};

/// Request body for creating a regime override.
#[derive(Debug, Deserialize)]
pub struct CreateOverrideRequest {
    /// Segment ID to override.
    pub segment_id: String,
    /// Original regime ID (optional).
    pub original_regime_id: Option<String>,
    /// The corrective action.
    pub action: UserOverrideAction,
}

/// Query parameters for listing overrides.
#[derive(Debug, Deserialize)]
pub struct ListOverridesQuery {
    /// ISO 8601 datetime — start of range.
    pub from: Option<String>,
    /// ISO 8601 datetime — end of range.
    pub to: Option<String>,
}

/// Generic success response with a message.
#[derive(Debug, Serialize)]
pub struct SuccessResponse {
    pub ok: bool,
    pub message: String,
}
