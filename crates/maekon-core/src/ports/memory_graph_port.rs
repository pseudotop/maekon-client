//! Async port for the local symbolic memory-graph store (ADR-023 substrate).
//!
//! Durable claim nodes + typed edges, persisted locally (SQLite) with zero
//! network dependency. Producers (D3 digest-highlight promotion, D5 taxonomy
//! assignment) and the LLM-gated D1/D2 edge inference are separate consumer
//! slices that build on this port.

use std::collections::HashMap;

use crate::error::CoreError;
use crate::models::memory_graph::{ClaimStatus, EdgeType, MemoryClaim, MemoryEdge};

/// Durable local memory-graph store.
///
/// # Errors
/// `CoreError::Storage` (wire: `storage.failed`) for all SQLite operations.
/// A missing claim is expressed as `Ok(None)`; absent edges as `Ok(Vec::new())`,
/// not an `Err` variant.
#[async_trait::async_trait]
pub trait MemoryGraphPort: Send + Sync {
    /// Insert or replace a claim node (idempotent on `claim_id`).
    async fn save_claim(&self, claim: &MemoryClaim) -> Result<(), CoreError>;

    /// Fetch a claim by id, or `Ok(None)` if it does not exist.
    async fn get_claim(&self, claim_id: &str) -> Result<Option<MemoryClaim>, CoreError>;

    /// List claims with the given status, most-recently-updated first.
    async fn list_claims_by_status(
        &self,
        status: ClaimStatus,
    ) -> Result<Vec<MemoryClaim>, CoreError>;

    /// Update a claim's lifecycle status and bump `updated_at` (epoch seconds).
    ///
    /// # Errors
    /// `CoreError::NotFound` (wire: `not_found.resource_missing`) when no claim
    /// with `claim_id` exists (the UPDATE matched 0 rows) — an affected-rows
    /// check so a caller can never silently no-op against a wrong id.
    /// `CoreError::Storage` for SQLite failures. A consent-erase in progress is
    /// still a silent `Ok(())` (the write is skipped, not a missing claim).
    async fn set_claim_status(
        &self,
        claim_id: &str,
        status: ClaimStatus,
        updated_at: i64,
    ) -> Result<(), CoreError>;

    /// Insert an edge (idempotent on `edge_id`).
    async fn add_edge(&self, edge: &MemoryEdge) -> Result<(), CoreError>;

    /// Edges originating from `src_id`, optionally filtered by `edge_type`.
    async fn edges_from(
        &self,
        src_id: &str,
        edge_type: Option<EdgeType>,
    ) -> Result<Vec<MemoryEdge>, CoreError>;

    /// Batched variant of [`Self::edges_from`]: all edges whose `src_id` is in
    /// `src_ids`, fetched in ONE query and grouped by `src_id`. Avoids the N+1
    /// of calling `edges_from` once per claim when enriching a claims page.
    ///
    /// Within each group edges keep the `created_at ASC` order of `edges_from`,
    /// so `edges_from_many(&[id]).remove(id)` equals `edges_from(id, None)`. An
    /// id absent from the store simply has no map entry; an empty `src_ids`
    /// yields an empty map without touching the DB.
    async fn edges_from_many(
        &self,
        src_ids: &[String],
    ) -> Result<HashMap<String, Vec<MemoryEdge>>, CoreError>;

    /// Delete claims whose `created_at` (epoch seconds) is strictly older than
    /// `cutoff_epoch_secs`, plus the edges originating from them. Bounds the
    /// otherwise-unbounded growth of activity-derived claims (GDPR Art-5(1)(e)
    /// storage limitation). Returns the number of claims deleted.
    async fn prune_claims_older_than(&self, cutoff_epoch_secs: i64) -> Result<u64, CoreError>;

    /// Delete `Evidence` edges whose target (`dst_id`) no longer exists in the
    /// activity store (the source segment was retention-deleted), so provenance
    /// never dangles to erased data. Returns the number of edges deleted.
    async fn prune_orphan_evidence_edges(&self) -> Result<u64, CoreError>;

    /// ADR-023 Phase-2 D2 (AC5): atomically flip `loser_claim_id`
    /// `Active → Superseded` **and** insert its `supersedes_edge`
    /// (`winner → loser`, `source = "llm"`) in a single transaction. The status
    /// flip can never land without the provenance edge, so every superseded
    /// claim is auditable. `updated_at` is epoch seconds. Belief revision touches
    /// `memory_claims`/`memory_edges` only — never regimes (AC8).
    async fn supersede_claim(
        &self,
        loser_claim_id: &str,
        supersedes_edge: &MemoryEdge,
        updated_at: i64,
    ) -> Result<(), CoreError>;
}
