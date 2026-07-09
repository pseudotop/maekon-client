//! Memory-graph claims browser API contracts (T1.3, #7911).
//!
//! Read + retract DTOs for the local "Trust Console" claims browser:
//! `GET /api/memory/claims` (list with filters) and
//! `POST /api/memory/claims/{id}/retract` (status-change retraction).
//!
//! Backed by the ADR-023 symbolic memory graph via the already-wired async
//! [`MemoryGraphPort`](maekon_core::ports::memory_graph_port). Before this
//! surface the durable claim nodes the agent silently accumulates about the
//! user had NO user-facing read path (only the `Active`-only bullet appendix in
//! the markdown daily-digest export) and NO way to reach
//! [`ClaimStatus::Retracted`](maekon_core::models::memory_graph::ClaimStatus).
//!
//! Retraction is a **status change, never a delete**: it flips a claim to
//! `Retracted` (hiding it from every read surface + retrieval) while preserving
//! the node and its provenance edges, mirroring the egress ledger's
//! evidence-preserving posture. The evidence/provenance summary fields are the
//! claim's own outbound edges (counts + linked segment / superseded-claim ids),
//! never captured content.

use serde::{Deserialize, Serialize};

/// Query parameters for `GET /api/memory/claims`.
///
/// All filters are optional and combine (AND). The handler clamps `limit`
/// (DoS guard). Retracted claims are excluded unless the caller opts in via
/// `status=retracted` or `status=all` (transparency).
#[derive(Debug, Default, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ClaimListQuery {
    /// Claim-kind filter (`semantic` | `episodic` | `procedural` |
    /// `reflective`). An unknown value matches nothing.
    #[serde(default)]
    pub kind: Option<String>,
    /// Lifecycle-status filter. Absent → every **non-retracted** claim
    /// (`active` + `superseded`). `active` / `superseded` / `retracted` → that
    /// status only. `all` → every status, retracted included.
    #[serde(default)]
    pub status: Option<String>,
    /// Inclusive lower bound on the claim's `created_at` (epoch seconds).
    #[serde(default)]
    pub from: Option<i64>,
    /// Inclusive upper bound on the claim's `created_at` (epoch seconds).
    #[serde(default)]
    pub to: Option<i64>,
    /// Maximum claims to return (default 200, capped at 1000).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// One claim node projected for the browser, with a cheap evidence/provenance
/// summary derived from its outbound edges.
///
/// Timestamps are epoch **seconds** (`i64`), matching the ADR-023 SQLite DDL
/// (`created_at` / `updated_at INTEGER`); the frontend humanizes them.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ClaimDto {
    /// Stable claim id (`generate_id("clm")`).
    pub claim_id: String,
    /// Cognitive kind: `semantic` | `episodic` | `procedural` | `reflective`.
    pub kind: String,
    /// The belief statement text.
    pub text: String,
    /// Provenance of the claim (e.g. `digest_highlight`, `digest_timeline`,
    /// `pattern_miner`, `llm`).
    pub source: String,
    /// Belief strength in `[0, 1]`.
    pub confidence: f32,
    /// Lifecycle status: `active` | `superseded` | `retracted`.
    pub status: String,
    /// When the belief was first formed (epoch seconds).
    pub created_at: i64,
    /// When the belief was last updated, e.g. retracted (epoch seconds).
    pub updated_at: i64,
    /// Number of `Evidence` edges supporting this claim.
    pub evidence_count: usize,
    /// The `segment_id`s each `Evidence` edge links to (the captured segments
    /// that support the belief). Ids only — never captured content.
    pub evidence_segment_ids: Vec<String>,
    /// The `claim_id`s this claim supersedes via `Supersedes` provenance edges
    /// (read-only display of existing belief-revision provenance; empty for the
    /// common case).
    pub supersedes_claim_ids: Vec<String>,
}

impl From<maekon_core::models::memory_graph::MemoryClaim> for ClaimDto {
    /// Base mapping of the eight claim columns. The edge-derived summary fields
    /// (`evidence_*`, `supersedes_*`) default empty here and are filled in by
    /// the service after reading the claim's outbound edges.
    fn from(claim: maekon_core::models::memory_graph::MemoryClaim) -> Self {
        Self {
            claim_id: claim.claim_id,
            kind: claim.kind.as_str().to_string(),
            text: claim.text,
            source: claim.source,
            confidence: claim.confidence,
            status: claim.status.as_str().to_string(),
            created_at: claim.created_at,
            updated_at: claim.updated_at,
            evidence_count: 0,
            evidence_segment_ids: Vec::new(),
            supersedes_claim_ids: Vec::new(),
        }
    }
}

/// Response body for `GET /api/memory/claims`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ClaimListResponse {
    /// The matched claims, newest-updated first, truncated to the effective
    /// `limit`.
    pub claims: Vec<ClaimDto>,
    /// Total claims matching the filter **before** the `limit` truncation, so
    /// the UI can show "N of M".
    pub total: usize,
}

/// Response body for `POST /api/memory/claims/{id}/retract`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RetractClaimResponse {
    /// The claim in its post-retraction state (`status = "retracted"`).
    pub claim: ClaimDto,
    /// `true` when the claim was already retracted, so the request was an
    /// idempotent no-op.
    pub already_retracted: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::models::memory_graph::{ClaimKind, ClaimStatus, MemoryClaim};

    fn sample_claim(status: ClaimStatus) -> MemoryClaim {
        MemoryClaim {
            claim_id: "clm_001".to_string(),
            kind: ClaimKind::Reflective,
            text: "Deep-work blocks cluster in the morning".to_string(),
            source: "digest_highlight".to_string(),
            confidence: 0.82,
            status,
            created_at: 1_700_000_000,
            updated_at: 1_700_000_500,
        }
    }

    #[test]
    fn dto_maps_all_claim_columns_with_empty_edge_summary() {
        let dto = ClaimDto::from(sample_claim(ClaimStatus::Active));
        assert_eq!(dto.claim_id, "clm_001");
        assert_eq!(dto.kind, "reflective");
        assert_eq!(dto.text, "Deep-work blocks cluster in the morning");
        assert_eq!(dto.source, "digest_highlight");
        assert_eq!(dto.confidence, 0.82_f32);
        assert_eq!(dto.status, "active");
        assert_eq!(dto.created_at, 1_700_000_000);
        assert_eq!(dto.updated_at, 1_700_000_500);
        // Edge-derived fields default empty; the service fills them.
        assert_eq!(dto.evidence_count, 0);
        assert!(dto.evidence_segment_ids.is_empty());
        assert!(dto.supersedes_claim_ids.is_empty());
    }

    #[test]
    fn retracted_status_serializes_verbatim() {
        let dto = ClaimDto::from(sample_claim(ClaimStatus::Retracted));
        let json = serde_json::to_string(&dto).unwrap();
        assert!(json.contains("\"status\":\"retracted\""));
    }

    #[test]
    fn retract_response_carries_already_flag() {
        let response = RetractClaimResponse {
            claim: ClaimDto::from(sample_claim(ClaimStatus::Retracted)),
            already_retracted: true,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"already_retracted\":true"));
        assert!(json.contains("\"status\":\"retracted\""));
    }
}
