//! Local symbolic memory-graph models (ADR-023 substrate).
//!
//! Durable claim/thesis nodes (`MemoryClaim`) and typed edges (`MemoryEdge`)
//! that sit on top of the existing vector/regime memory. This is the LLM-free
//! substrate; the D3/D5 producers and the LLM-gated D1/D2 edge inference are
//! separate (consumer) slices per ADR-023.
//!
//! Timestamps are epoch **seconds** (`i64`) to match the ADR-023 SQLite DDL
//! (`created_at`/`updated_at INTEGER NOT NULL`); this intentionally diverges
//! from the `TEXT`/RFC3339 convention used by some sibling tables.

use serde::{Deserialize, Serialize};

/// Cognitive memory-unit taxonomy (ADR-023 D5).
///
/// A rule-assigned classification of what kind of memory a claim represents.
/// Wire spelling is snake_case (stored verbatim in the `memory_claims.kind`
/// TEXT column).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClaimKind {
    Semantic,
    Episodic,
    Procedural,
    Reflective,
}

impl ClaimKind {
    /// Stable string for the SQLite TEXT column (snake_case per ADR-023 DDL).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Semantic => "semantic",
            Self::Episodic => "episodic",
            Self::Procedural => "procedural",
            Self::Reflective => "reflective",
        }
    }

    /// Parse from a SQLite TEXT value; unknown values fall back to `Semantic`.
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "semantic" => Self::Semantic,
            "episodic" => Self::Episodic,
            "procedural" => Self::Procedural,
            "reflective" => Self::Reflective,
            _ => Self::Semantic,
        }
    }
}

/// Typed edge kind between memory units (ADR-023 D1 + evidence).
///
/// `Evidence` is the LLM-free Phase-1 edge (claim → supporting `segment_id`);
/// `Associated` is the LLM-free rule-seeded temporal association between two
/// consecutive timeline claims (app-sequence; `src` = earlier → `dst` = later,
/// `source = "rule"` — #4441 box 5); `Supports`/`Refines`/`Contradicts` are the
/// epistemic edges produced by the LLM-gated Phase-2 D1 relation extraction;
/// `Supersedes` is the Phase-2 D2 provenance edge (winner claim → superseded
/// loser claim), written atomically with the `active → superseded` status flip.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EdgeType {
    Evidence,
    Associated,
    Supports,
    Refines,
    Contradicts,
    Supersedes,
}

impl EdgeType {
    /// Stable string for the SQLite TEXT column (snake_case per ADR-023 DDL).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Evidence => "evidence",
            Self::Associated => "associated",
            Self::Supports => "supports",
            Self::Refines => "refines",
            Self::Contradicts => "contradicts",
            Self::Supersedes => "supersedes",
        }
    }

    /// Parse from a SQLite TEXT value; unknown values fall back to `Evidence`.
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "evidence" => Self::Evidence,
            "associated" => Self::Associated,
            "supports" => Self::Supports,
            "refines" => Self::Refines,
            "contradicts" => Self::Contradicts,
            "supersedes" => Self::Supersedes,
            _ => Self::Evidence,
        }
    }
}

/// Lifecycle status of a claim node (ADR-023).
///
/// Belief revision (Phase 2) transitions an `Active` claim to `Superseded`;
/// the substrate only provides the primitive (no automatic transitions here).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClaimStatus {
    Active,
    Superseded,
    Retracted,
}

impl ClaimStatus {
    /// Stable string for the SQLite TEXT column (snake_case).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Superseded => "superseded",
            Self::Retracted => "retracted",
        }
    }

    /// Parse from a SQLite TEXT value; unknown values fall back to `Active`.
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "active" => Self::Active,
            "superseded" => Self::Superseded,
            "retracted" => Self::Retracted,
            _ => Self::Active,
        }
    }
}

/// A durable claim/thesis node in the local memory graph.
///
/// IDs use `generate_id("clm")` (ADR-022). `created_at`/`updated_at` are epoch
/// seconds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryClaim {
    pub claim_id: String,
    pub kind: ClaimKind,
    pub text: String,
    /// Provenance of the claim, e.g. `"digest_highlight"`, `"pattern_miner"`, `"llm"`.
    pub source: String,
    pub confidence: f32,
    pub status: ClaimStatus,
    /// Epoch seconds.
    pub created_at: i64,
    /// Epoch seconds.
    pub updated_at: i64,
}

/// ADR-032 §2.6 Mode A edge projection tuple.
///
/// The FULL field set a Mode A consumer may see: (`src_id`, `dst_id`,
/// `edge_type`, `confidence`). The endpoints are in-process join keys for
/// ranking (for `Evidence` edges `dst_id` may reference a `segment_id`) and
/// MUST NOT be disclosed beyond the ranking computation. Deliberately absent
/// by contract: claim `text`/`kind`/`source`, `evidence_ref`, timestamps —
/// adding any of them is an ADR-032 §2.5/§2.6 violation, not an enhancement.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectedEdge {
    pub src_id: String,
    pub dst_id: String,
    pub edge_type: EdgeType,
    pub confidence: f32,
}

/// Bounded Mode A projection result (ADR-032 §2).
///
/// Produced only by the shared projection helper behind
/// `MemoryGraphProjectionPort`; consumers never assemble one from raw
/// `MemoryGraphPort` reads. An empty projection is the fail-closed outcome
/// for every unevaluable bound (disabled config, missing consent, invalid
/// window/floor/cap) and MUST rank identically to "no memory graph at all".
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct EdgeProjection {
    /// Bounded, deterministically ordered edge tuples (`created_at DESC`,
    /// `edge_id` tie-break at selection time; the tuple itself carries no
    /// timestamp).
    pub edges: Vec<ProjectedEdge>,
    /// How many `Active` claims survived the window/floor/cap bounds — a
    /// derived count for join-coverage observability (ADR-032 Known
    /// Follow-up 2), never an identifier.
    pub claims_selected: usize,
}

/// A typed edge between two memory units (claims and/or `segment_id`-keyed units).
///
/// IDs use `generate_id("edg")` (ADR-022). `src_id`/`dst_id` may reference a
/// `claim_id` or an existing `segment_id`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryEdge {
    pub edge_id: String,
    pub src_id: String,
    pub dst_id: String,
    pub edge_type: EdgeType,
    pub confidence: f32,
    /// Optional `segment_id`/`frame_id` provenance for the relation.
    pub evidence_ref: Option<String>,
    /// `"rule"` (Phase 1) or `"llm"` (Phase 2).
    pub source: String,
    /// Epoch seconds.
    pub created_at: i64,
}

/// LLM-proposed epistemic edge (ADR-023 Phase-2 D1 relation extraction).
///
/// Returned by `AnalysisProvider::extract_relations`. The caller validates the
/// `edge_type` ∈ {`Supports`, `Refines`, `Contradicts`} and applies a confidence
/// gate before persisting it as a `MemoryEdge` (`source = "llm"`). Lives in
/// `maekon-core` so the `AnalysisProvider` port stays decoupled from the
/// analysis/network adapters.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RelationEdgeProposal {
    pub src_claim_id: String,
    /// Target `claim_id` (or `segment_id`).
    pub dst_id: String,
    pub edge_type: EdgeType,
    pub confidence: f32,
    pub evidence_ref: Option<String>,
}

/// LLM-proposed contradiction resolution (ADR-023 Phase-2 D2).
///
/// Returned by `AnalysisProvider::detect_contradictions`. The caller applies a
/// (config-driven, conservatively-high) confidence gate; only above it does the
/// `loser` claim transition `Active → Superseded`, written atomically with a
/// `Supersedes` provenance edge (`winner → loser`). Belief revision operates on
/// `memory_claims` **only** — never on regimes (AC8).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClaimStatusChange {
    pub loser_claim_id: String,
    pub winner_claim_id: String,
    pub new_status: ClaimStatus,
    pub confidence: f32,
    pub rationale: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_kind_roundtrip() {
        for k in [
            ClaimKind::Semantic,
            ClaimKind::Episodic,
            ClaimKind::Procedural,
            ClaimKind::Reflective,
        ] {
            assert_eq!(ClaimKind::from_str_lossy(k.as_str()), k);
        }
        // Exact wire spellings (must match the ADR-023 DDL comment).
        assert_eq!(ClaimKind::Semantic.as_str(), "semantic");
        assert_eq!(ClaimKind::Episodic.as_str(), "episodic");
        assert_eq!(ClaimKind::Procedural.as_str(), "procedural");
        assert_eq!(ClaimKind::Reflective.as_str(), "reflective");
    }

    #[test]
    fn claim_kind_fallback() {
        assert_eq!(ClaimKind::from_str_lossy("nonsense"), ClaimKind::Semantic);
    }

    #[test]
    fn edge_type_roundtrip() {
        for e in [
            EdgeType::Evidence,
            EdgeType::Associated,
            EdgeType::Supports,
            EdgeType::Refines,
            EdgeType::Contradicts,
            EdgeType::Supersedes,
        ] {
            assert_eq!(EdgeType::from_str_lossy(e.as_str()), e);
        }
        assert_eq!(EdgeType::Contradicts.as_str(), "contradicts");
        assert_eq!(EdgeType::Supersedes.as_str(), "supersedes");
        assert_eq!(EdgeType::Associated.as_str(), "associated");
    }

    #[test]
    fn edge_type_fallback() {
        assert_eq!(EdgeType::from_str_lossy("nope"), EdgeType::Evidence);
    }

    #[test]
    fn claim_status_roundtrip() {
        for s in [
            ClaimStatus::Active,
            ClaimStatus::Superseded,
            ClaimStatus::Retracted,
        ] {
            assert_eq!(ClaimStatus::from_str_lossy(s.as_str()), s);
        }
        assert_eq!(ClaimStatus::Superseded.as_str(), "superseded");
    }

    #[test]
    fn claim_status_fallback() {
        assert_eq!(ClaimStatus::from_str_lossy("x"), ClaimStatus::Active);
    }

    #[test]
    fn memory_claim_serde_roundtrip() {
        let claim = MemoryClaim {
            claim_id: "clm_001".to_string(),
            kind: ClaimKind::Reflective,
            text: "Deep-work blocks cluster in the morning".to_string(),
            source: "digest_highlight".to_string(),
            confidence: 0.82,
            status: ClaimStatus::Active,
            created_at: 1_700_000_000,
            updated_at: 1_700_000_000,
        };
        let json = serde_json::to_string(&claim).unwrap();
        let back: MemoryClaim = serde_json::from_str(&json).unwrap();
        assert_eq!(back, claim);
        // kind serializes snake_case
        assert!(json.contains("\"reflective\""));
    }

    #[test]
    fn memory_edge_serde_roundtrip() {
        let edge = MemoryEdge {
            edge_id: "edg_001".to_string(),
            src_id: "clm_001".to_string(),
            dst_id: "ses_042".to_string(),
            edge_type: EdgeType::Evidence,
            confidence: 1.0,
            evidence_ref: Some("ses_042".to_string()),
            source: "rule".to_string(),
            created_at: 1_700_000_000,
        };
        let json = serde_json::to_string(&edge).unwrap();
        let back: MemoryEdge = serde_json::from_str(&json).unwrap();
        assert_eq!(back, edge);
    }
}
