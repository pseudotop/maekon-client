//! Provider CLI diagnostics domain model.
//!
//! Core-owned summary of a single AI-provider-surface CLI diagnostic. The web
//! API transport DTO (`maekon_api_contracts::support::ProviderCliDiagnosticSummaryDto`)
//! is converted *from* this model at the web boundary, keeping the cross-crate
//! port contract (see `ports::provider_cli_diagnostics`) free of any
//! `maekon-api-contracts` dependency (which itself depends on `maekon-core`).

use serde::{Deserialize, Serialize};

/// Diagnostic summary for one provider-surface CLI candidate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderCliDiagnosticSummary {
    pub surface_id: String,
    pub tool_id: Option<String>,
    pub candidate_name: Option<String>,
    pub executable_hint: Option<String>,
    pub readiness: String,
    pub availability: String,
    pub dependency_status: Option<String>,
    pub status_reason: Option<String>,
    pub env_refresh_required: bool,
}
