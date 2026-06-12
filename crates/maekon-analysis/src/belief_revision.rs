//! ADR-023 Phase-2 belief revision (D1 relation extraction + D2 contradiction →
//! supersede), the LLM-gated consumer of the memory graph.
//!
//! **Privacy-critical.** This component reads durable, activity-derived claim
//! text and hands it to an LLM. It is safe by construction:
//! - the `provider` it receives is already **local-only-gated** by the
//!   composition root (see `AnalysisClient::new_local_enrichment`); this module
//!   never builds a network client itself;
//! - every claim's text is **masked** at the enrichment boundary via `pii_filter`
//!   before it is serialized for the provider (MG-PII-03/01) — the active-window
//!   guard does not protect previously-stored claim text;
//! - it holds **only** a [`MemoryGraphPort`] handle — never a regime-manager /
//!   regime-storage / regime-classifier handle (AC8); belief revision touches
//!   `memory_claims`/`memory_edges` exclusively, never the behavioral-regime
//!   lifecycle;
//! - it degrades to a no-op (`Ok(default)`, never `Err`/panic) when disabled or
//!   when the provider is `NoOp`/returns empty.

use std::collections::HashSet;
use std::sync::Arc;

use maekon_core::error::CoreError;
use maekon_core::generate_id;
use maekon_core::models::memory_graph::{ClaimStatus, EdgeType, MemoryEdge};
use maekon_core::ports::analysis_provider::AnalysisProvider;
use maekon_core::ports::memory_graph_port::MemoryGraphPort;
use tracing::debug;

/// System prompt instructing the LLM to return D1 relation proposals as a JSON
/// array of `{src_claim_id, dst_id, edge_type, confidence}` objects.
const RELATION_PROMPT: &str = "You are given a JSON array of [claim_id, text] pairs from a personal \
activity knowledge graph. Identify epistemic relations BETWEEN these claims. Respond with ONLY a \
JSON array of objects {\"src_claim_id\":<id>,\"dst_id\":<id>,\"edge_type\":<\"supports\"|\"refines\"\
|\"contradicts\">,\"confidence\":<0..1>}. Use only claim_ids present in the input. Empty array if none.";

/// System prompt instructing the LLM to return D2 contradiction resolutions.
const CONTRADICTION_PROMPT: &str = "You are given a JSON array of [claim_id, text] pairs. Identify \
pairs where one claim is clearly SUPERSEDED by a more recent/correct one. Respond with ONLY a JSON \
array of objects {\"loser_claim_id\":<id>,\"winner_claim_id\":<id>,\"new_status\":\"superseded\",\
\"confidence\":<0..1>}. Use only claim_ids present in the input. Be conservative. Empty array if none.";

/// Per-claim text masker applied at the enrichment boundary (MG-PII-03).
pub type PiiFilter = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// Outcome of one belief-revision pass.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BeliefRevisionStats {
    pub relations_added: usize,
    pub claims_superseded: usize,
}

/// LLM-gated belief revision over the local memory graph.
pub struct BeliefRevision {
    /// Already local-only-gated by the composition root. Never built here.
    provider: Arc<dyn AnalysisProvider>,
    /// AC8: the ONLY storage handle — no regime surface.
    memory_graph: Arc<dyn MemoryGraphPort>,
    /// MG-PII-03 boundary masker applied to each claim's text before send.
    pii_filter: PiiFilter,
    /// D2 supersede confidence gate (config-driven, conservatively high).
    supersede_threshold: f64,
    enabled: bool,
}

impl BeliefRevision {
    pub fn new(
        provider: Arc<dyn AnalysisProvider>,
        memory_graph: Arc<dyn MemoryGraphPort>,
        pii_filter: PiiFilter,
        supersede_threshold: f64,
        enabled: bool,
    ) -> Self {
        Self {
            provider,
            memory_graph,
            pii_filter,
            supersede_threshold,
            enabled,
        }
    }

    /// Run one belief-revision pass. `now_secs` is the epoch-second stamp for new
    /// edges / status updates. Never errors out of a single bad proposal; the
    /// whole pass degrades to `Ok(default)` when disabled or with no LLM.
    pub async fn run_pass(&self, now_secs: i64) -> Result<BeliefRevisionStats, CoreError> {
        if !self.enabled {
            return Ok(BeliefRevisionStats::default());
        }
        let active = self
            .memory_graph
            .list_claims_by_status(ClaimStatus::Active)
            .await?;
        // Need at least two claims to form any relation/contradiction.
        if active.len() < 2 {
            return Ok(BeliefRevisionStats::default());
        }
        let active_ids: HashSet<&str> = active.iter().map(|c| c.claim_id.as_str()).collect();

        // MG-PII-03/01: mask EACH claim's text at the boundary before it is sent.
        let masked: Vec<(String, String)> = active
            .iter()
            .map(|c| (c.claim_id.clone(), (self.pii_filter)(&c.text)))
            .collect();
        let claims_json = serde_json::to_string(&masked).unwrap_or_default();

        let mut stats = BeliefRevisionStats::default();

        // ── D1: relation extraction (supports / refines / contradicts) ──
        // NoOp / no-LLM → Ok(vec![]) (default trait impl) → emits nothing.
        let relations = self
            .provider
            .extract_relations(&claims_json, RELATION_PROMPT)
            .await
            .unwrap_or_default();
        for r in relations {
            // Only epistemic relations BETWEEN two KNOWN active claims.
            if !matches!(
                r.edge_type,
                EdgeType::Supports | EdgeType::Refines | EdgeType::Contradicts
            ) {
                continue;
            }
            // Both endpoints must be KNOWN active claims, and a claim cannot bear
            // an epistemic relation to itself — mirrors the D2 `loser != winner`
            // guard; an LLM-proposed self-loop is meaningless noise.
            if !active_ids.contains(r.src_claim_id.as_str())
                || !active_ids.contains(r.dst_id.as_str())
                || r.src_claim_id == r.dst_id
            {
                continue;
            }
            let edge = MemoryEdge {
                edge_id: generate_id("edg"),
                src_id: r.src_claim_id,
                dst_id: r.dst_id,
                edge_type: r.edge_type,
                confidence: r.confidence,
                evidence_ref: r.evidence_ref,
                source: "llm".to_string(),
                created_at: now_secs,
            };
            if self.memory_graph.add_edge(&edge).await.is_ok() {
                stats.relations_added += 1;
            }
        }

        // ── D2: contradiction pass → atomic supersede with provenance ──
        let threshold = self.supersede_threshold.clamp(0.0, 1.0);
        let changes = self
            .provider
            .detect_contradictions(&claims_json, CONTRADICTION_PROMPT)
            .await
            .unwrap_or_default();
        for ch in changes {
            // F2: a wrong supersede destroys a durable belief — gate hard.
            if f64::from(ch.confidence) < threshold {
                continue;
            }
            if ch.new_status != ClaimStatus::Superseded {
                continue;
            }
            // Both winner and loser must be known active claims (don't let the LLM
            // supersede an unknown / already-superseded / fabricated claim).
            if !active_ids.contains(ch.loser_claim_id.as_str())
                || !active_ids.contains(ch.winner_claim_id.as_str())
                || ch.loser_claim_id == ch.winner_claim_id
            {
                continue;
            }
            // F3: provenance edge written atomically with the status flip.
            let edge = MemoryEdge {
                edge_id: generate_id("edg"),
                src_id: ch.winner_claim_id.clone(),
                dst_id: ch.loser_claim_id.clone(),
                edge_type: EdgeType::Supersedes,
                confidence: ch.confidence,
                evidence_ref: None,
                source: "llm".to_string(),
                created_at: now_secs,
            };
            if self
                .memory_graph
                .supersede_claim(&ch.loser_claim_id, &edge, now_secs)
                .await
                .is_ok()
            {
                stats.claims_superseded += 1;
            }
        }

        if stats != BeliefRevisionStats::default() {
            debug!(
                relations = stats.relations_added,
                superseded = stats.claims_superseded,
                "ADR-023 belief revision pass complete"
            );
        }
        Ok(stats)
    }
}
