use maekon_core::models::recalibration::UserOverrideAction;
use serde::{Deserialize, Serialize};

/// Request body for creating a regime override.
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CreateOverrideRequest {
    /// Segment ID to override.
    pub segment_id: String,
    /// Original regime ID (optional).
    pub original_regime_id: Option<String>,
    /// The corrective action.
    // Cross-crate `maekon-core` tagged enum — contained as an opaque object.
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "crate::schema_support::opaque_object")
    )]
    pub action: UserOverrideAction,
}

/// Query parameters for listing overrides.
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ListOverridesQuery {
    /// ISO 8601 datetime — start of range.
    pub from: Option<String>,
    /// ISO 8601 datetime — end of range.
    pub to: Option<String>,
}

/// Generic success response with a message.
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SuccessResponse {
    pub ok: bool,
    pub message: String,
}
