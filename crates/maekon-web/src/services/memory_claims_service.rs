//! Memory-graph claims browser query + retraction service (T1.3, #7911).
//!
//! Pure orchestration over the already-wired async
//! [`MemoryGraphPort`](maekon_core::ports::memory_graph_port). It composes the
//! per-status `list_claims_by_status` primitive into a filtered/paged browser
//! query (the port has no all-status / by-kind / by-date / paging query), maps
//! claims into the transport [`ClaimDto`] with a cheap edge-derived
//! evidence/provenance summary, and performs idempotent retraction via
//! `set_claim_status(_, Retracted, _)`.
//!
//! Retraction is a **status change, never a delete** — the claim node and its
//! provenance edges are preserved; flipping `status → Retracted` drops it from
//! every default read surface (including the `Active`-only daily-digest
//! appendix) and from retrieval, mirroring the egress ledger's
//! evidence-preserving posture.

use std::sync::Arc;

use maekon_api_contracts::memory_claims::{
    ClaimDto, ClaimListQuery, ClaimListResponse, RetractClaimResponse,
};
use maekon_core::error::CoreError;
use maekon_core::models::memory_graph::{ClaimStatus, EdgeType, MemoryClaim, MemoryEdge};
use maekon_core::ports::memory_graph_port::MemoryGraphPort;

/// Default number of claims returned when `limit` is absent.
const DEFAULT_LIMIT: usize = 200;

/// DoS guard: maximum claims returned in one page (mirrors the audit-export /
/// egress-ledger sibling caps).
const MAX_LIMIT: usize = 1000;

/// Resolve the requested `status` filter into the concrete set of statuses to
/// query.
///
/// - absent / empty → non-retracted (`Active` + `Superseded`), the default that
///   keeps retracted claims hidden;
/// - a specific status name → that status only;
/// - `all` → every status, retracted included (transparency);
/// - any other value → an empty set (an explicit filter that matches nothing),
///   rather than silently falling back to a status the caller did not ask for.
fn statuses_for_filter(status: Option<&str>) -> Vec<ClaimStatus> {
    match status.map(str::trim) {
        None | Some("") => vec![ClaimStatus::Active, ClaimStatus::Superseded],
        Some("active") => vec![ClaimStatus::Active],
        Some("superseded") => vec![ClaimStatus::Superseded],
        Some("retracted") => vec![ClaimStatus::Retracted],
        Some("all") => vec![
            ClaimStatus::Active,
            ClaimStatus::Superseded,
            ClaimStatus::Retracted,
        ],
        Some(_) => Vec::new(),
    }
}

/// Build the [`ClaimDto`] for one claim from its already-read outbound edges,
/// projecting the cheap edge-derived summary (`Evidence` → linked `segment_id`s,
/// `Supersedes` → superseded `claim_id`s).
///
/// Pure: the caller supplies the edges (one batched `edges_from_many` for the
/// list page, a single `edges_from` for the retract path), so the enrichment
/// logic is shared and the list page pays exactly one edge query instead of one
/// per returned claim (the former N+1).
fn claim_dto_with_edges(claim: MemoryClaim, edges: &[MemoryEdge]) -> ClaimDto {
    let mut dto = ClaimDto::from(claim);
    for edge in edges {
        match edge.edge_type {
            EdgeType::Evidence => dto.evidence_segment_ids.push(edge.dst_id.clone()),
            EdgeType::Supersedes => dto.supersedes_claim_ids.push(edge.dst_id.clone()),
            _ => {}
        }
    }
    dto.evidence_count = dto.evidence_segment_ids.len();
    dto
}

/// Read one claim's outbound edges and build its enriched [`ClaimDto`] (retract
/// path — a single claim, so one `edges_from` query, no N+1). Edge reads are
/// best-effort: a read hiccup yields an empty summary rather than failing.
async fn single_claim_dto(mg: &Arc<dyn MemoryGraphPort>, claim: MemoryClaim) -> ClaimDto {
    let edges = mg
        .edges_from(&claim.claim_id, None)
        .await
        .unwrap_or_default();
    claim_dto_with_edges(claim, &edges)
}

/// Build the claims-list response for a `GET /api/memory/claims` request.
///
/// Queries the status set implied by `query.status` (default: non-retracted),
/// applies the optional `kind` and `[from, to]` `created_at` filters in memory,
/// orders newest-updated-first, then truncates to the clamped `limit`. `total`
/// is the pre-truncation match count. Only the returned page pays the
/// per-claim edge read (bounded by `limit`).
pub async fn build_claim_list_response(
    mg: &Arc<dyn MemoryGraphPort>,
    query: &ClaimListQuery,
) -> Result<ClaimListResponse, CoreError> {
    // 1. Gather claims across the requested status set.
    let mut claims: Vec<MemoryClaim> = Vec::new();
    for status in statuses_for_filter(query.status.as_deref()) {
        claims.extend(mg.list_claims_by_status(status).await?);
    }

    // 2. In-memory filters the port does not push down.
    if let Some(kind) = query
        .kind
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
    {
        claims.retain(|c| c.kind.as_str() == kind);
    }
    if let Some(from) = query.from {
        claims.retain(|c| c.created_at >= from);
    }
    if let Some(to) = query.to {
        claims.retain(|c| c.created_at <= to);
    }

    // 3. Deterministic order: newest-updated first (merged multi-status lists
    //    are otherwise only per-status ordered), tie-broken by claim_id.
    claims.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.claim_id.cmp(&b.claim_id))
    });

    let total = claims.len();

    // 4. Clamp + truncate, then enrich only the returned page with edges.
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
    claims.truncate(limit);

    // Single batched edge read for the whole page — replaces the former N+1 of
    // one `edges_from` per returned claim. Ids are bounded by `limit`
    // (≤ MAX_LIMIT). Best-effort: a read hiccup yields empty summaries (via
    // `unwrap_or_default`) rather than failing the whole browser query, matching
    // the prior per-claim posture.
    let page_ids: Vec<String> = claims.iter().map(|c| c.claim_id.clone()).collect();
    let mut edges_by_claim = mg.edges_from_many(&page_ids).await.unwrap_or_default();

    let dtos = claims
        .into_iter()
        .map(|claim| {
            let edges = edges_by_claim.remove(&claim.claim_id).unwrap_or_default();
            claim_dto_with_edges(claim, &edges)
        })
        .collect();

    Ok(ClaimListResponse {
        claims: dtos,
        total,
    })
}

/// Retract a claim by id (idempotent). Returns `Ok(None)` when the claim does
/// not exist (→ 404). Retracting an already-retracted claim is a 200 no-op with
/// `already_retracted = true`. Never deletes — provenance is preserved.
pub async fn retract_claim(
    mg: &Arc<dyn MemoryGraphPort>,
    claim_id: &str,
    now_secs: i64,
) -> Result<Option<RetractClaimResponse>, CoreError> {
    // The `get_claim` read is kept as a fast-path (NOT merely a guard): the
    // response returns the full claim + its evidence summary, and the idempotent
    // no-op branch needs the current status. `set_claim_status` is now itself
    // NotFound-safe on a missing id, so this is defense-in-depth against a
    // get_claim→set_claim_status TOCTOU race, not the sole missing-id check.
    // HTTP semantics are unchanged: missing id → Ok(None) → 404;
    // already-retracted → idempotent 200 no-op.
    let Some(claim) = mg.get_claim(claim_id).await? else {
        return Ok(None);
    };

    if claim.status == ClaimStatus::Retracted {
        // Idempotent no-op: already retracted, report the current state.
        return Ok(Some(RetractClaimResponse {
            claim: single_claim_dto(mg, claim).await,
            already_retracted: true,
        }));
    }

    mg.set_claim_status(claim_id, ClaimStatus::Retracted, now_secs)
        .await?;

    // Reflect the write locally (status flipped, updated_at bumped) — matches
    // exactly what set_claim_status persisted, without a re-read round trip.
    let mut retracted = claim;
    retracted.status = ClaimStatus::Retracted;
    retracted.updated_at = now_secs;

    Ok(Some(RetractClaimResponse {
        claim: single_claim_dto(mg, retracted).await,
        already_retracted: false,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::models::memory_graph::{ClaimKind, MemoryClaim, MemoryEdge};
    use maekon_storage::sqlite::SqliteStorage;

    fn claim(
        id: &str,
        kind: ClaimKind,
        status: ClaimStatus,
        created_at: i64,
        updated_at: i64,
    ) -> MemoryClaim {
        MemoryClaim {
            claim_id: id.to_string(),
            kind,
            text: format!("belief {id}"),
            source: "digest_highlight".to_string(),
            confidence: 0.8,
            status,
            created_at,
            updated_at,
        }
    }

    fn evidence_edge(edge_id: &str, claim_id: &str, segment_id: &str) -> MemoryEdge {
        MemoryEdge {
            edge_id: edge_id.to_string(),
            src_id: claim_id.to_string(),
            dst_id: segment_id.to_string(),
            edge_type: EdgeType::Evidence,
            confidence: 1.0,
            evidence_ref: Some(segment_id.to_string()),
            source: "rule".to_string(),
            created_at: 1_700_000_000,
        }
    }

    /// Seed a real in-memory `SqliteStorage` with an active + superseded +
    /// retracted claim (plus evidence on the active one), returned as the async
    /// `MemoryGraphPort` (Port Instance Sharing).
    async fn seeded_graph() -> Arc<dyn MemoryGraphPort> {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory sqlite"));
        let mg: Arc<dyn MemoryGraphPort> = storage;
        mg.save_claim(&claim(
            "clm_active",
            ClaimKind::Reflective,
            ClaimStatus::Active,
            1_000,
            3_000,
        ))
        .await
        .unwrap();
        mg.save_claim(&claim(
            "clm_super",
            ClaimKind::Episodic,
            ClaimStatus::Superseded,
            1_000,
            2_000,
        ))
        .await
        .unwrap();
        mg.save_claim(&claim(
            "clm_retracted",
            ClaimKind::Semantic,
            ClaimStatus::Retracted,
            1_000,
            2_500,
        ))
        .await
        .unwrap();
        mg.add_edge(&evidence_edge("edg_1", "clm_active", "seg_a"))
            .await
            .unwrap();
        mg.add_edge(&evidence_edge("edg_2", "clm_active", "seg_b"))
            .await
            .unwrap();
        mg
    }

    #[tokio::test]
    async fn default_list_excludes_retracted_newest_updated_first() {
        let mg = seeded_graph().await;
        let response = build_claim_list_response(&mg, &ClaimListQuery::default())
            .await
            .unwrap();

        // Retracted claim is hidden by default; active + superseded remain.
        assert_eq!(response.total, 2);
        let ids: Vec<&str> = response
            .claims
            .iter()
            .map(|c| c.claim_id.as_str())
            .collect();
        assert_eq!(ids, vec!["clm_active", "clm_super"]);
        assert!(!ids.contains(&"clm_retracted"));
    }

    #[tokio::test]
    async fn batched_page_summary_equals_per_claim_loop() {
        // Item 2 equivalence: the batched-edge list response must be identical to
        // what the former per-claim `edges_from` loop produced. Seed several
        // claims (some with evidence, one without) and compare the service output
        // DTO-for-DTO against a hand-rolled per-claim reconstruction.
        let mg = seeded_graph().await;
        let query = ClaimListQuery {
            status: Some("all".to_string()),
            ..Default::default()
        };
        let response = build_claim_list_response(&mg, &query).await.unwrap();

        // Rebuild the expected DTOs the old way: per-claim edges_from + the same
        // pure projection the service uses.
        let mut expected = Vec::new();
        for dto in &response.claims {
            let claim = mg.get_claim(&dto.claim_id).await.unwrap().unwrap();
            let edges = mg.edges_from(&dto.claim_id, None).await.unwrap();
            expected.push(claim_dto_with_edges(claim, &edges));
        }
        assert_eq!(
            response.claims, expected,
            "batched page must equal per-claim loop"
        );
    }

    #[tokio::test]
    async fn active_claim_carries_evidence_summary() {
        let mg = seeded_graph().await;
        let response = build_claim_list_response(&mg, &ClaimListQuery::default())
            .await
            .unwrap();

        let active = response
            .claims
            .iter()
            .find(|c| c.claim_id == "clm_active")
            .expect("active claim present");
        assert_eq!(active.evidence_count, 2);
        assert_eq!(active.evidence_segment_ids, vec!["seg_a", "seg_b"]);
    }

    #[tokio::test]
    async fn status_retracted_filter_shows_only_retracted() {
        let mg = seeded_graph().await;
        let query = ClaimListQuery {
            status: Some("retracted".to_string()),
            ..Default::default()
        };
        let response = build_claim_list_response(&mg, &query).await.unwrap();

        assert_eq!(response.total, 1);
        assert_eq!(response.claims[0].claim_id, "clm_retracted");
        assert_eq!(response.claims[0].status, "retracted");
    }

    #[tokio::test]
    async fn status_all_filter_shows_every_status() {
        let mg = seeded_graph().await;
        let query = ClaimListQuery {
            status: Some("all".to_string()),
            ..Default::default()
        };
        let response = build_claim_list_response(&mg, &query).await.unwrap();
        assert_eq!(response.total, 3);
    }

    #[tokio::test]
    async fn unknown_status_matches_nothing() {
        let mg = seeded_graph().await;
        let query = ClaimListQuery {
            status: Some("bogus".to_string()),
            ..Default::default()
        };
        let response = build_claim_list_response(&mg, &query).await.unwrap();
        assert_eq!(response.total, 0);
        assert!(response.claims.is_empty());
    }

    #[tokio::test]
    async fn kind_filter_scopes_to_one_kind() {
        let mg = seeded_graph().await;
        let query = ClaimListQuery {
            status: Some("all".to_string()),
            kind: Some("semantic".to_string()),
            ..Default::default()
        };
        let response = build_claim_list_response(&mg, &query).await.unwrap();
        assert_eq!(response.total, 1);
        assert_eq!(response.claims[0].claim_id, "clm_retracted");
        assert_eq!(response.claims[0].kind, "semantic");
    }

    #[tokio::test]
    async fn date_range_filters_on_created_at() {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let mg: Arc<dyn MemoryGraphPort> = storage;
        mg.save_claim(&claim(
            "clm_old",
            ClaimKind::Reflective,
            ClaimStatus::Active,
            1_000,
            1_000,
        ))
        .await
        .unwrap();
        mg.save_claim(&claim(
            "clm_new",
            ClaimKind::Reflective,
            ClaimStatus::Active,
            9_000,
            9_000,
        ))
        .await
        .unwrap();

        let query = ClaimListQuery {
            from: Some(5_000),
            to: Some(10_000),
            ..Default::default()
        };
        let response = build_claim_list_response(&mg, &query).await.unwrap();
        assert_eq!(response.total, 1);
        assert_eq!(response.claims[0].claim_id, "clm_new");
    }

    #[tokio::test]
    async fn limit_clamps_page_but_total_reflects_full_match() {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let mg: Arc<dyn MemoryGraphPort> = storage;
        for i in 0..5 {
            mg.save_claim(&claim(
                &format!("clm_{i}"),
                ClaimKind::Reflective,
                ClaimStatus::Active,
                1_000,
                1_000 + i as i64,
            ))
            .await
            .unwrap();
        }

        let query = ClaimListQuery {
            limit: Some(2),
            ..Default::default()
        };
        let response = build_claim_list_response(&mg, &query).await.unwrap();
        assert_eq!(response.claims.len(), 2, "page truncated to limit");
        assert_eq!(response.total, 5, "total reflects the full match count");
    }

    #[tokio::test]
    async fn retract_flips_active_to_retracted() {
        let mg = seeded_graph().await;
        let outcome = retract_claim(&mg, "clm_active", 9_999)
            .await
            .unwrap()
            .expect("claim exists");
        assert!(!outcome.already_retracted);
        assert_eq!(outcome.claim.status, "retracted");
        assert_eq!(outcome.claim.updated_at, 9_999);
        // Provenance preserved: the retracted claim keeps its evidence edges.
        assert_eq!(outcome.claim.evidence_count, 2);

        // The claim is now hidden from the default (non-retracted) list.
        let response = build_claim_list_response(&mg, &ClaimListQuery::default())
            .await
            .unwrap();
        assert!(response.claims.iter().all(|c| c.claim_id != "clm_active"));
    }

    #[tokio::test]
    async fn retract_is_idempotent_noop_on_already_retracted() {
        let mg = seeded_graph().await;
        let outcome = retract_claim(&mg, "clm_retracted", 9_999)
            .await
            .unwrap()
            .expect("claim exists");
        assert!(outcome.already_retracted, "already retracted → no-op");
        assert_eq!(outcome.claim.status, "retracted");
        // No-op must NOT bump updated_at (the original 2_500 is preserved).
        assert_eq!(outcome.claim.updated_at, 2_500);
    }

    #[tokio::test]
    async fn retract_missing_claim_returns_none() {
        let mg = seeded_graph().await;
        let outcome = retract_claim(&mg, "clm_does_not_exist", 9_999)
            .await
            .unwrap();
        assert!(outcome.is_none(), "missing claim → None → 404");
    }
}
