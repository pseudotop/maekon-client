//! Semantic search service — mode routing, embedding, hybrid scoring.
//!
//! # Hybrid mode (keyword-first, #5766)
//!
//! The hybrid path runs vector search + FTS keyword search in parallel, then
//! fuses the two result lists with Reciprocal Rank Fusion (RRF). Documents that
//! also appear in a phrase search are promoted to tier-0 (sorted to the front,
//! preserving phrase-result order among themselves).
//!
//! RRF parameters: α=0.6 (vector weight), β=0.4 (FTS weight), k=60.
//! Source: #5733 spike, validated against 40 docs × 18 queries.
//!
//! Availability fallback: when the embedding pipeline is unavailable in hybrid
//! mode, the service degrades gracefully to keyword-only (warn-logged).
//!
//! # Capability boundary (mode=semantic, #7574)
//!
//! Unlike hybrid mode, `mode=semantic` never degrades to keyword — a caller
//! who explicitly asked for semantic search wants vector results, not a
//! silent keyword substitution. When this build has no vector/embedding
//! pipeline wired up (the production default: `vector_store` /
//! `embedding_provider` / `adaptive_search` are only assigned under
//! `#[cfg(test)]`), that is a **permanent build-time capability gap**, not a
//! transient outage. `execute` reports this as [`SearchExecutionError::SemanticNotConfigured`],
//! which the HTTP layer (`crate::error::ApiError`) maps to **501 Not
//! Implemented** with the message `"semantic search is not configured in
//! this build; use mode=keyword or mode=hybrid"`. This replaces the prior
//! behaviour of returning `CoreError::ServiceUnavailable` (HTTP 503), which
//! misleadingly implied a temporary outage the client should retry rather
//! than an absent feature the client should route around.
//!
//! Genuine runtime failures inside an otherwise-configured pipeline (e.g. the
//! embedding provider itself erroring mid-request) still surface as ordinary
//! [`CoreError`] via [`SearchExecutionError::Core`], preserving their typed
//! wire code and HTTP mapping (typically 503) — only the "nothing is wired up
//! at all" case is reclassified as a capability boundary.
//!
//! # Retired: +0.1 flat FTS boost (`fts_boost_set`)
//!
//! Prior to #5766, hybrid mode applied a flat +0.1 score boost to documents
//! that appeared in the FTS top-k result set. This gave a coarse signal but
//! treated every FTS hit equally regardless of rank position. The RRF approach
//! is superior because:
//! - Rank position is the primary signal, not binary presence.
//! - FTS-only documents (not in the vector results) now enter the union.
//! - The spike showed nDCG 0.736→0.790 on 35% un-embedded docs.
//!
//! The `SemanticSearchResult.score` field now carries the RRF score instead of
//! `(vector_similarity + boost).min(1.0)`. The REST contract schema is unchanged
//! — only the value semantics differ (noted in PR description).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tracing::{debug, warn};

use maekon_api_contracts::search::SemanticSearchResult;
use maekon_core::config::PiiFilterLevel;
use maekon_core::error::CoreError;
use maekon_core::models::embedding::{SearchFilters, SearchResult};
use maekon_core::models::memory_graph::{EdgeProjection, EdgeType};
use maekon_core::models::storage_records::SegmentDetailRecord;
use maekon_core::ports::adaptive_search::AdaptiveSearchPort;
use maekon_core::ports::embedding_provider::EmbeddingProvider;
use maekon_core::ports::pii_sanitizer::PiiSanitizer;
use maekon_core::ports::text_search::{TextSearchProvider, TextSearchResult};
use maekon_core::ports::vector_store::VectorStore;

use crate::AppState;

// ── RRF constants (#5733 spike, α/β/k) ──────────────────────────────────────

/// Vector search weight for RRF. Source: #5733 spike (α=0.6).
const RRF_ALPHA: f32 = 0.6;
/// FTS keyword search weight for RRF. Source: #5733 spike (β=0.4).
const RRF_BETA: f32 = 0.4;
/// RRF rank-position constant. Source: #5733 spike (k=60).
const RRF_K: f32 = 60.0;

/// Sanitize query text before local search providers or remote embedding egress.
pub(crate) fn sanitize_query(text: &str, pii_sanitizer: Option<&dyn PiiSanitizer>) -> String {
    match pii_sanitizer {
        Some(sanitizer) => sanitizer.sanitize_text(text, PiiFilterLevel::Standard),
        None => text.to_string(),
    }
}

fn sanitize_state_query(state: &AppState, text: &str) -> String {
    sanitize_query(text, state.diagnostics.pii_sanitizer.as_deref())
}

/// Resolve search mode from optional parameter. Defaults to "hybrid".
pub(crate) fn resolve_mode(mode: Option<&str>) -> &str {
    match mode {
        Some("semantic") => "semantic",
        Some("keyword") => "keyword",
        _ => "hybrid",
    }
}

/// Embed a sanitized query. Errors from the provider pass through unchanged,
/// preserving their typed wire code (ADR-019 §1) so callers / telemetry can
/// distinguish e.g. `provider.analysis_failed` from `config.invalid`.
async fn embed_query(
    embedding_provider: &Arc<dyn EmbeddingProvider>,
    sanitized: &str,
) -> Result<Vec<f32>, CoreError> {
    embedding_provider.embed(sanitized).await
}

/// Fetch segment metadata for the given IDs, logging storage errors at `warn`.
async fn fetch_segment_details(
    state: &AppState,
    segment_ids: &[String],
) -> HashMap<String, SegmentDetailRecord> {
    state
        .core
        .storage
        .get_segment_details(segment_ids)
        .await
        .unwrap_or_else(|e| {
            warn!("Failed to fetch segment details: {e}");
            HashMap::new()
        })
}

/// Convert `SearchResult`s into `SemanticSearchResult`s, enriching with segment
/// details. The `score` field carries the RRF-fused score passed in.
///
/// `fts_boost_ids` is no longer consulted — retained as an empty `HashSet` stub
/// so existing callers in `vector_search` / `adaptive_vector_search` can be
/// removed cleanly. (Caller sites pass `&HashSet::new()`; the 0.1 branch is
/// unreachable and will be removed when the callers are updated.)
fn map_vector_results(
    results: Vec<SearchResult>,
    segment_details: &HashMap<String, SegmentDetailRecord>,
) -> Vec<SemanticSearchResult> {
    results
        .into_iter()
        .map(|r| {
            let detail = segment_details.get(&r.segment_id);
            SemanticSearchResult {
                segment_id: r.segment_id,
                content_type: format!("{:?}", r.content_type),
                content_label: r.content_label,
                original_text: r.original_text,
                // score carries the RRF score (or raw vector score for non-hybrid paths)
                score: r.score,
                similarity: r.similarity,
                time_decay: r.time_decay,
                timestamp: r.timestamp.to_rfc3339(),
                segment_start: detail.map(|d| d.start_time.clone()),
                segment_end: detail.map(|d| d.end_time.clone()),
                duration_secs: detail.map(|d| d.duration_secs),
                llm_summary: detail.and_then(|d| d.llm_summary.clone()),
                dominant_category: detail.map(|d| d.dominant_category.clone()),
                regime_label: detail.and_then(|d| d.regime_label.clone()),
            }
        })
        .collect()
}

/// Fuse vector and FTS result lists using Reciprocal Rank Fusion (keyword-first
/// union, #5766).
///
/// # Algorithm
/// 1. Build rank maps (1-based) for vector results and FTS results separately.
/// 2. Collect the union of all segment IDs.
/// 3. For each ID compute `RRF = α/(k + rank_v) + β/(k + rank_f)` where
///    missing-rank sentinel = `over_fetch + 1`.
/// 4. Promote phrase IDs to tier-0: they are sorted to the front (preserving
///    their mutual FTS order), remaining IDs follow sorted by RRF score desc.
/// 5. Return the top `limit` IDs.
///
/// # Return value
/// Ordered list of `(segment_id, rrf_score)` pairs, length ≤ `limit`.
///
/// # Spike provenance
/// RRF parameters α=0.6/β=0.4/k=60 from #5733 spike (40 docs × 18 queries).
/// The retired `maekon-analysis::HybridSearchService` (removed in #5766)
/// contains identical RRF arithmetic, validated independently.
pub(crate) fn fuse_keyword_first(
    vector: &[SearchResult],
    fts: &[TextSearchResult],
    phrase_ids: &[String],
    limit: usize,
) -> Vec<(String, f32)> {
    if vector.is_empty() && fts.is_empty() {
        return vec![];
    }

    let over_fetch = vector.len().max(fts.len()) * 2 + 1;
    let missing_rank = over_fetch + 1;

    // Build 1-based rank maps
    let vector_ranks: HashMap<&str, usize> = vector
        .iter()
        .enumerate()
        .map(|(i, r)| (r.segment_id.as_str(), i + 1))
        .collect();

    let fts_ranks: HashMap<&str, usize> = fts
        .iter()
        .enumerate()
        .map(|(i, r)| (r.segment_id.as_str(), i + 1))
        .collect();

    // Collect the union of all IDs (preserving insertion order via IndexMap-style
    // iteration, then dedup).
    let mut seen: HashSet<String> = HashSet::new();
    let mut all_ids: Vec<String> = Vec::new();
    for r in vector.iter() {
        if seen.insert(r.segment_id.clone()) {
            all_ids.push(r.segment_id.clone());
        }
    }
    for r in fts.iter() {
        if seen.insert(r.segment_id.clone()) {
            all_ids.push(r.segment_id.clone());
        }
    }

    // Compute RRF scores
    let mut scored: Vec<(String, f32)> = all_ids
        .into_iter()
        .map(|id| {
            let rank_v = *vector_ranks.get(id.as_str()).unwrap_or(&missing_rank);
            let rank_f = *fts_ranks.get(id.as_str()).unwrap_or(&missing_rank);
            let rrf = RRF_ALPHA / (RRF_K + rank_v as f32) + RRF_BETA / (RRF_K + rank_f as f32);
            (id, rrf)
        })
        .collect();

    // Tier-0: phrase-matched IDs float to the front (in their FTS rank order),
    // then remaining IDs sorted by RRF score descending.
    let phrase_set: HashSet<&str> = phrase_ids.iter().map(|s| s.as_str()).collect();
    // Phrase IDs in fts-rank order (stable among themselves)
    let mut tier0: Vec<(String, f32)> = scored
        .iter()
        .filter(|(id, _)| phrase_set.contains(id.as_str()))
        .map(|(id, s)| (id.clone(), *s))
        .collect();
    tier0.sort_by(|(a, _), (b, _)| {
        let ra = fts_ranks.get(a.as_str()).copied().unwrap_or(missing_rank);
        let rb = fts_ranks.get(b.as_str()).copied().unwrap_or(missing_rank);
        ra.cmp(&rb)
    });

    // Remaining IDs sorted by RRF score descending
    let phrase_id_set: HashSet<String> = tier0.iter().map(|(id, _)| id.clone()).collect();
    scored.retain(|(id, _)| !phrase_id_set.contains(id));
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // Concatenate tier-0 + rest, truncate to limit
    tier0.extend(scored);
    tier0.truncate(limit);
    tier0
}

/// Error returned by [`execute`].
///
/// Distinguishes the "semantic search is not configured in this build"
/// capability boundary from ordinary [`CoreError`] failures, so
/// `crate::error::ApiError` can map it to HTTP 501 Not Implemented instead of
/// the misleading transient-outage semantics of HTTP 503 -- without touching
/// the shared `CoreError` wire-code contract
/// (`crates/maekon-core/tests/wire_contract_snapshot.rs`).
#[derive(Debug, thiserror::Error)]
pub enum SearchExecutionError {
    /// Ordinary `CoreError`, passed through unchanged so its typed wire code
    /// (e.g. `provider.analysis_failed`, `network.timeout`) and existing HTTP
    /// mapping survive.
    #[error(transparent)]
    Core(#[from] CoreError),

    /// `mode=semantic` was requested but this build has no vector/embedding
    /// pipeline wired up. A permanent build-time capability gap, not a
    /// transient outage -- the caller should switch to `mode=keyword` or
    /// `mode=hybrid` instead of retrying.
    #[error("semantic search is not configured in this build; use mode=keyword or mode=hybrid")]
    SemanticNotConfigured,
}

impl SearchExecutionError {
    /// Wire code for telemetry/logging, mirroring `CoreError::code()`.
    pub fn code(&self) -> &'static str {
        match self {
            SearchExecutionError::Core(e) => e.code(),
            SearchExecutionError::SemanticNotConfigured => "service.not_implemented",
        }
    }
}

/// Execute a search with mode dispatch and provider availability checks.
///
/// Iter-96: return `CoreError` instead of `String` so the typed
/// `err.code()` survives through to handlers. Provider-unavailable
/// conditions become `ServiceUnavailable` (wire code
/// `service.unavailable`); inner search failures preserve their own
/// wire codes (e.g. `provider.analysis_failed`, `internal.io`).
///
/// #7574: `mode=semantic` with no vector/embedding pipeline configured no
/// longer folds into `CoreError::ServiceUnavailable` -- see
/// [`SearchExecutionError::SemanticNotConfigured`].
pub async fn execute(
    state: &AppState,
    query: &str,
    limit: usize,
    mode: &str,
) -> Result<Vec<SemanticSearchResult>, SearchExecutionError> {
    let mut results = execute_inner(state, query, limit, mode).await?;
    apply_memory_graph_rerank(state, &mut results).await;
    Ok(results)
}

/// ADR-032 Mode A: re-rank `results` with the bounded memory-graph edge
/// projection. Strictly generator-adjacent — endpoints are consumed inside
/// this ranking computation and disclosed nowhere; an empty projection (the
/// fail-closed off state) leaves the ranking bit-identical. A projection
/// STORAGE error must not break search (ranking is an enhancement), so it is
/// warn-logged and skipped — degradation in the consumer, not masking inside
/// the helper.
async fn apply_memory_graph_rerank(state: &AppState, results: &mut [SemanticSearchResult]) {
    if results.is_empty() {
        return;
    }
    let Some(projection_port) = state.analysis.memory_graph_projection.as_ref() else {
        return;
    };
    let now_secs = chrono::Utc::now().timestamp();
    match projection_port.project_edges_for_ranking(now_secs).await {
        Ok(projection) if !projection.edges.is_empty() => {
            let evidence_matches = rerank_with_edge_projection(&projection, results);
            // Join-coverage observability (ADR-032 Known Follow-up 2): how
            // many selected claims/edges actually landed on a result.
            debug!(
                claims_selected = projection.claims_selected,
                edges_projected = projection.edges.len(),
                evidence_matches,
                "ADR-032 Mode A re-rank applied"
            );
        }
        Ok(_) => {}
        Err(e) => {
            warn!(
                err.code = %e.code(),
                "ADR-032 Mode A projection failed; ranking unchanged: {e}"
            );
        }
    }
}

/// Mode A boost weight per unit of summed `Evidence`-edge confidence.
const MEMORY_GRAPH_BOOST_WEIGHT: f32 = 0.25;
/// Upper bound on the multiplicative Mode A boost factor.
const MEMORY_GRAPH_MAX_BOOST_FACTOR: f32 = 2.0;

/// Pure Mode A ranking step: boost results whose `segment_id` is the target
/// of a projected `Evidence` edge (`dst_id → segment_id` join per ADR-032
/// Known Follow-up 2), then stable-sort by score.
///
/// The boost is multiplicative (`1 + 0.25·Σconfidence`, capped ×2.0) so it is
/// scale-independent across the RRF and cosine score paths, and the sort is
/// stable so equal-score results keep their prior deterministic order.
/// Returns how many results were boosted (join-coverage measurement).
pub(crate) fn rerank_with_edge_projection(
    projection: &EdgeProjection,
    results: &mut [SemanticSearchResult],
) -> usize {
    let mut boost_by_segment: HashMap<&str, f32> = HashMap::new();
    for edge in &projection.edges {
        if edge.edge_type == EdgeType::Evidence {
            *boost_by_segment.entry(edge.dst_id.as_str()).or_default() +=
                edge.confidence.clamp(0.0, 1.0);
        }
    }
    if boost_by_segment.is_empty() {
        return 0;
    }
    let mut evidence_matches = 0usize;
    for result in results.iter_mut() {
        if let Some(sum) = boost_by_segment.get(result.segment_id.as_str()) {
            evidence_matches += 1;
            let factor = (1.0 + MEMORY_GRAPH_BOOST_WEIGHT * sum).min(MEMORY_GRAPH_MAX_BOOST_FACTOR);
            result.score *= factor;
        }
    }
    if evidence_matches > 0 {
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    evidence_matches
}

async fn execute_inner(
    state: &AppState,
    query: &str,
    limit: usize,
    mode: &str,
) -> Result<Vec<SemanticSearchResult>, SearchExecutionError> {
    match mode {
        "keyword" => {
            let ts = state.analysis.text_search.as_ref().ok_or_else(|| {
                CoreError::ServiceUnavailable {
                    code: maekon_core::error_codes::ServiceCode::Unavailable,
                    message:
                        "Keyword search is not available (text search provider not configured)"
                            .to_string(),
                }
            })?;
            Ok(keyword_search(ts, state, query, limit).await?)
        }
        "hybrid" => {
            // Hybrid: try vector path; if embedding pipeline is unavailable or
            // returns an error, degrade to keyword-only (warn-logged). The
            // "semantic" mode never degrades -- see the `_` arm below.
            if let Some(ref adaptive) = state.analysis.adaptive_search {
                match state.analysis.embedding_provider.as_ref() {
                    Some(ep) => {
                        match adaptive_vector_search(adaptive, ep, state, query, limit, true).await
                        {
                            Ok(results) => return Ok(results),
                            Err(e) => {
                                warn!(
                                    err.code = %e.code(),
                                    "Hybrid adaptive search failed; degrading to keyword: {e}"
                                );
                            }
                        }
                    }
                    None => {
                        warn!(
                            "Hybrid mode: embedding provider not configured; degrading to keyword"
                        );
                    }
                }
            } else if let Some(ref vs) = state.analysis.vector_store {
                match state.analysis.embedding_provider.as_ref() {
                    Some(ep) => match vector_search(vs, ep, state, query, limit, true).await {
                        Ok(results) => return Ok(results),
                        Err(e) => {
                            warn!(
                                err.code = %e.code(),
                                "Hybrid vector search failed; degrading to keyword: {e}"
                            );
                        }
                    },
                    None => {
                        warn!(
                            "Hybrid mode: embedding provider not configured; degrading to keyword"
                        );
                    }
                }
            }
            // Degraded path -- keyword fallback
            match state.analysis.text_search.as_ref() {
                Some(ts) => Ok(keyword_search(ts, state, query, limit).await?),
                None => Err(SearchExecutionError::Core(CoreError::ServiceUnavailable {
                    code: maekon_core::error_codes::ServiceCode::Unavailable,
                    message:
                        "Search is not available (neither embedding pipeline nor text search configured)"
                            .to_string(),
                })),
            }
        }
        _ => {
            // "semantic" and any unknown mode -- prefer adaptive, fall back to
            // vector store. No keyword degradation on semantic: an absent
            // pipeline is a capability boundary (SemanticNotConfigured ->
            // HTTP 501), not a transient failure to retry.
            let embedding_provider = state.analysis.embedding_provider.as_ref();
            if let Some(ref adaptive) = state.analysis.adaptive_search {
                let ep = embedding_provider.ok_or(SearchExecutionError::SemanticNotConfigured)?;
                return Ok(adaptive_vector_search(adaptive, ep, state, query, limit, false).await?);
            }
            let vs = state
                .analysis
                .vector_store
                .as_ref()
                .ok_or(SearchExecutionError::SemanticNotConfigured)?;
            let ep = embedding_provider.ok_or(SearchExecutionError::SemanticNotConfigured)?;
            Ok(vector_search(vs, ep, state, query, limit, false).await?)
        }
    }
}

/// Execute keyword-only search via TextSearchProvider.
pub async fn keyword_search(
    text_search: &Arc<dyn TextSearchProvider>,
    state: &AppState,
    query: &str,
    limit: usize,
) -> Result<Vec<SemanticSearchResult>, CoreError> {
    let sanitized = sanitize_state_query(state, query);
    let fts_results = text_search.search_fts(&sanitized, limit).await?;

    let segment_ids: Vec<String> = fts_results.iter().map(|r| r.segment_id.clone()).collect();
    let segment_details = state
        .core
        .storage
        .get_segment_details(&segment_ids)
        .await
        .unwrap_or_else(|e| {
            warn!("Failed to fetch segment details: {e}");
            HashMap::new()
        });

    Ok(fts_results
        .into_iter()
        .map(|r| {
            let detail = segment_details.get(&r.segment_id);
            SemanticSearchResult {
                segment_id: r.segment_id,
                content_type: r.content_type,
                content_label: None,
                original_text: r.matched_text,
                score: r.rank,
                similarity: 0.0,
                time_decay: 0.0,
                timestamp: detail.map(|d| d.start_time.clone()).unwrap_or_default(),
                segment_start: detail.map(|d| d.start_time.clone()),
                segment_end: detail.map(|d| d.end_time.clone()),
                duration_secs: detail.map(|d| d.duration_secs),
                llm_summary: detail.and_then(|d| d.llm_summary.clone()),
                dominant_category: detail.map(|d| d.dominant_category.clone()),
                regime_label: detail.and_then(|d| d.regime_label.clone()),
            }
        })
        .collect())
}

/// Execute semantic/hybrid search via VectorStore + EmbeddingProvider.
///
/// When `hybrid=true`: over-fetches from both vector and FTS, fuses via RRF
/// (keyword-first union, tier-0 phrase boosting), then enriches merged IDs with
/// segment details. `score` = RRF value; `similarity` = original vector
/// similarity for vector-hit rows, 0.0 for FTS-only rows.
pub async fn vector_search(
    vector_store: &Arc<dyn VectorStore>,
    embedding_provider: &Arc<dyn EmbeddingProvider>,
    state: &AppState,
    query: &str,
    limit: usize,
    hybrid: bool,
) -> Result<Vec<SemanticSearchResult>, CoreError> {
    let sanitized = sanitize_state_query(state, query);
    let query_vector = embed_query(embedding_provider, &sanitized).await?;

    if hybrid {
        let over_fetch = limit * 2;
        let vector_results = vector_store
            .search(&query_vector, over_fetch, 168.0)
            .await?;

        // FTS keyword results (fail-open: missing provider → empty)
        let fts_results = if let Some(ref ts) = state.analysis.text_search {
            ts.search_fts(&sanitized, over_fetch)
                .await
                .unwrap_or_else(|e| {
                    debug!("FTS search skipped in hybrid mode (fail-open): {e}");
                    vec![]
                })
        } else {
            vec![]
        };

        // Phrase tier-0 IDs (fail-open: missing provider → empty)
        let phrase_ids: Vec<String> = if let Some(ref ts) = state.analysis.text_search {
            ts.search_fts_phrase(&sanitized, over_fetch)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|r| r.segment_id)
                .collect()
        } else {
            vec![]
        };

        let ordered = fuse_keyword_first(&vector_results, &fts_results, &phrase_ids, limit);

        // Build lookup maps for detail enrichment
        let vector_map: HashMap<String, &SearchResult> = vector_results
            .iter()
            .map(|r| (r.segment_id.clone(), r))
            .collect();
        let fts_map: HashMap<String, &TextSearchResult> = fts_results
            .iter()
            .map(|r| (r.segment_id.clone(), r))
            .collect();

        let all_ids: Vec<String> = ordered.iter().map(|(id, _)| id.clone()).collect();
        let segment_details = fetch_segment_details(state, &all_ids).await;

        let results = ordered
            .into_iter()
            .map(|(seg_id, rrf_score)| {
                let detail = segment_details.get(&seg_id);
                if let Some(vr) = vector_map.get(&seg_id) {
                    // Vector hit — preserve original_text and similarity
                    SemanticSearchResult {
                        segment_id: seg_id,
                        content_type: format!("{:?}", vr.content_type),
                        content_label: vr.content_label.clone(),
                        original_text: vr.original_text.clone(),
                        score: rrf_score,
                        similarity: vr.similarity,
                        time_decay: vr.time_decay,
                        timestamp: vr.timestamp.to_rfc3339(),
                        segment_start: detail.map(|d| d.start_time.clone()),
                        segment_end: detail.map(|d| d.end_time.clone()),
                        duration_secs: detail.map(|d| d.duration_secs),
                        llm_summary: detail.and_then(|d| d.llm_summary.clone()),
                        dominant_category: detail.map(|d| d.dominant_category.clone()),
                        regime_label: detail.and_then(|d| d.regime_label.clone()),
                    }
                } else if let Some(fr) = fts_map.get(&seg_id) {
                    // FTS-only hit — similarity is 0.0 (no vector score)
                    SemanticSearchResult {
                        segment_id: seg_id,
                        content_type: fr.content_type.clone(),
                        content_label: None,
                        original_text: fr.matched_text.clone(),
                        score: rrf_score,
                        similarity: 0.0,
                        time_decay: 0.0,
                        timestamp: detail.map(|d| d.start_time.clone()).unwrap_or_default(),
                        segment_start: detail.map(|d| d.start_time.clone()),
                        segment_end: detail.map(|d| d.end_time.clone()),
                        duration_secs: detail.map(|d| d.duration_secs),
                        llm_summary: detail.and_then(|d| d.llm_summary.clone()),
                        dominant_category: detail.map(|d| d.dominant_category.clone()),
                        regime_label: detail.and_then(|d| d.regime_label.clone()),
                    }
                } else {
                    // Should not happen — sentinel fallback
                    SemanticSearchResult {
                        segment_id: seg_id,
                        content_type: String::new(),
                        content_label: None,
                        original_text: String::new(),
                        score: rrf_score,
                        similarity: 0.0,
                        time_decay: 0.0,
                        timestamp: String::new(),
                        segment_start: detail.map(|d| d.start_time.clone()),
                        segment_end: detail.map(|d| d.end_time.clone()),
                        duration_secs: detail.map(|d| d.duration_secs),
                        llm_summary: detail.and_then(|d| d.llm_summary.clone()),
                        dominant_category: detail.map(|d| d.dominant_category.clone()),
                        regime_label: detail.and_then(|d| d.regime_label.clone()),
                    }
                }
            })
            .collect();

        Ok(results)
    } else {
        // Semantic-only path (no FTS)
        let vector_results = vector_store.search(&query_vector, limit, 168.0).await?;
        let segment_ids: Vec<String> = vector_results
            .iter()
            .map(|r| r.segment_id.clone())
            .collect();
        let segment_details = fetch_segment_details(state, &segment_ids).await;
        Ok(map_vector_results(vector_results, &segment_details))
    }
}

/// Execute search via AdaptiveSearchPort (IVF/HNSW auto-selection).
///
/// When `hybrid=true`: same RRF fusion path as `vector_search`.
pub async fn adaptive_vector_search(
    adaptive: &Arc<dyn AdaptiveSearchPort>,
    embedding_provider: &Arc<dyn EmbeddingProvider>,
    state: &AppState,
    query: &str,
    limit: usize,
    hybrid: bool,
) -> Result<Vec<SemanticSearchResult>, CoreError> {
    let sanitized = sanitize_state_query(state, query);
    let query_vector = embed_query(embedding_provider, &sanitized).await?;

    if hybrid {
        let over_fetch = limit * 2;
        let vector_results = adaptive
            .search(&query_vector, over_fetch, 168.0, &SearchFilters::default())
            .await?;

        let fts_results = if let Some(ref ts) = state.analysis.text_search {
            ts.search_fts(&sanitized, over_fetch)
                .await
                .unwrap_or_else(|e| {
                    debug!("FTS search skipped in hybrid mode (fail-open): {e}");
                    vec![]
                })
        } else {
            vec![]
        };

        let phrase_ids: Vec<String> = if let Some(ref ts) = state.analysis.text_search {
            ts.search_fts_phrase(&sanitized, over_fetch)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|r| r.segment_id)
                .collect()
        } else {
            vec![]
        };

        let ordered = fuse_keyword_first(&vector_results, &fts_results, &phrase_ids, limit);

        let vector_map: HashMap<String, &SearchResult> = vector_results
            .iter()
            .map(|r| (r.segment_id.clone(), r))
            .collect();
        let fts_map: HashMap<String, &TextSearchResult> = fts_results
            .iter()
            .map(|r| (r.segment_id.clone(), r))
            .collect();

        let all_ids: Vec<String> = ordered.iter().map(|(id, _)| id.clone()).collect();
        let segment_details = fetch_segment_details(state, &all_ids).await;

        let results = ordered
            .into_iter()
            .map(|(seg_id, rrf_score)| {
                let detail = segment_details.get(&seg_id);
                if let Some(vr) = vector_map.get(&seg_id) {
                    SemanticSearchResult {
                        segment_id: seg_id,
                        content_type: format!("{:?}", vr.content_type),
                        content_label: vr.content_label.clone(),
                        original_text: vr.original_text.clone(),
                        score: rrf_score,
                        similarity: vr.similarity,
                        time_decay: vr.time_decay,
                        timestamp: vr.timestamp.to_rfc3339(),
                        segment_start: detail.map(|d| d.start_time.clone()),
                        segment_end: detail.map(|d| d.end_time.clone()),
                        duration_secs: detail.map(|d| d.duration_secs),
                        llm_summary: detail.and_then(|d| d.llm_summary.clone()),
                        dominant_category: detail.map(|d| d.dominant_category.clone()),
                        regime_label: detail.and_then(|d| d.regime_label.clone()),
                    }
                } else if let Some(fr) = fts_map.get(&seg_id) {
                    SemanticSearchResult {
                        segment_id: seg_id,
                        content_type: fr.content_type.clone(),
                        content_label: None,
                        original_text: fr.matched_text.clone(),
                        score: rrf_score,
                        similarity: 0.0,
                        time_decay: 0.0,
                        timestamp: detail.map(|d| d.start_time.clone()).unwrap_or_default(),
                        segment_start: detail.map(|d| d.start_time.clone()),
                        segment_end: detail.map(|d| d.end_time.clone()),
                        duration_secs: detail.map(|d| d.duration_secs),
                        llm_summary: detail.and_then(|d| d.llm_summary.clone()),
                        dominant_category: detail.map(|d| d.dominant_category.clone()),
                        regime_label: detail.and_then(|d| d.regime_label.clone()),
                    }
                } else {
                    SemanticSearchResult {
                        segment_id: seg_id,
                        content_type: String::new(),
                        content_label: None,
                        original_text: String::new(),
                        score: rrf_score,
                        similarity: 0.0,
                        time_decay: 0.0,
                        timestamp: String::new(),
                        segment_start: detail.map(|d| d.start_time.clone()),
                        segment_end: detail.map(|d| d.end_time.clone()),
                        duration_secs: detail.map(|d| d.duration_secs),
                        llm_summary: detail.and_then(|d| d.llm_summary.clone()),
                        dominant_category: detail.map(|d| d.dominant_category.clone()),
                        regime_label: detail.and_then(|d| d.regime_label.clone()),
                    }
                }
            })
            .collect();

        Ok(results)
    } else {
        // Semantic-only path
        let results = adaptive
            .search(&query_vector, limit, 168.0, &SearchFilters::default())
            .await?;
        let segment_ids: Vec<String> = results.iter().map(|r| r.segment_id.clone()).collect();
        let segment_details = fetch_segment_details(state, &segment_ids).await;
        Ok(map_vector_results(results, &segment_details))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_query_uses_configured_privacy_sanitizer() {
        let q = "emails from user@example.com about auth";
        let sanitizer = MarkerSanitizer;
        let sanitized = sanitize_query(q, Some(&sanitizer));
        assert!(!sanitized.contains("user@example.com"));
        assert!(sanitized.contains("[EMAIL]"));
    }

    #[test]
    fn sanitize_query_covers_non_email_pii_patterns() {
        let q = "call 555-123-4567 about user@example.com";
        let sanitizer = MarkerSanitizer;
        let sanitized = sanitize_query(q, Some(&sanitizer));
        assert!(!sanitized.contains("555-123-4567"));
        assert!(!sanitized.contains("user@example.com"));
        assert!(sanitized.contains("[PHONE]"));
        assert!(sanitized.contains("[EMAIL]"));
    }

    #[test]
    fn sanitize_query_preserves_local_behavior_when_sanitizer_missing() {
        let q = "what did I work on yesterday";
        assert_eq!(sanitize_query(q, None), q);
    }

    #[test]
    fn resolve_mode_defaults_to_hybrid() {
        assert_eq!(resolve_mode(None), "hybrid");
        assert_eq!(resolve_mode(Some("unknown")), "hybrid");
    }

    #[test]
    fn resolve_mode_recognizes_valid_modes() {
        assert_eq!(resolve_mode(Some("semantic")), "semantic");
        assert_eq!(resolve_mode(Some("keyword")), "keyword");
    }

    // ── fuse_keyword_first unit tests ────────────────────────────────────────

    use chrono::Utc;
    use maekon_core::models::embedding::{EmbeddingContentType, EmbeddingMetadata};
    use maekon_core::ports::text_search::TextSearchResult;

    fn make_sr(id: &str, _rank: usize, sim: f32) -> SearchResult {
        SearchResult {
            segment_id: id.to_string(),
            content_type: EmbeddingContentType::SegmentSummary,
            content_label: None,
            score: sim,
            similarity: sim,
            time_decay: 1.0,
            timestamp: Utc::now(),
            original_text: format!("text-{id}"),
        }
    }

    fn make_fts(id: &str, rank_val: f32) -> TextSearchResult {
        TextSearchResult {
            segment_id: id.to_string(),
            content_type: "segment".to_string(),
            matched_text: format!("kw-{id}"),
            rank: rank_val,
        }
    }

    /// ①  Vector-only candidates: FTS empty → same union as vector, RRF uses
    ///     FTS missing-rank sentinel. Current-behaviour preservation.
    #[test]
    fn fuse_vector_only_returns_vector_order() {
        let v = vec![make_sr("a", 1, 0.9), make_sr("b", 2, 0.8)];
        let fts: Vec<TextSearchResult> = vec![];
        let ordered = fuse_keyword_first(&v, &fts, &[], 10);
        let ids: Vec<&str> = ordered.iter().map(|(id, _)| id.as_str()).collect();
        // Both IDs present
        assert!(ids.contains(&"a"));
        assert!(ids.contains(&"b"));
        // "a" ranks first (vector rank 1, both FTS sentinel → RRF higher for rank-1)
        assert_eq!(ids[0], "a");
    }

    /// ②  FTS-only document enters the union — not present in vector results.
    #[test]
    fn fuse_fts_only_doc_enters_union() {
        let v = vec![make_sr("vec-only", 1, 0.9)];
        let fts = vec![make_fts("fts-only", -1.0), make_fts("vec-only", -0.5)];
        let ordered = fuse_keyword_first(&v, &fts, &[], 10);
        let ids: Vec<&str> = ordered.iter().map(|(id, _)| id.as_str()).collect();
        assert!(
            ids.contains(&"fts-only"),
            "FTS-only doc must enter the union"
        );
        assert!(ids.contains(&"vec-only"));
    }

    /// ③  Phrase tier-0 IDs are promoted to the front.
    #[test]
    fn fuse_phrase_ids_promoted_to_tier0() {
        // "phrase-doc" ranks last in vector but is in phrase results
        let v = vec![make_sr("high-vec", 1, 0.95), make_sr("phrase-doc", 2, 0.5)];
        let fts = vec![make_fts("phrase-doc", -5.0), make_fts("high-vec", -3.0)];
        let phrase_ids = vec!["phrase-doc".to_string()];
        let ordered = fuse_keyword_first(&v, &fts, &phrase_ids, 10);
        // phrase-doc must be first
        assert_eq!(ordered[0].0, "phrase-doc");
    }

    /// ④  Deduplication: same ID in both lists appears once.
    #[test]
    fn fuse_deduplication() {
        let v = vec![make_sr("shared", 1, 0.9), make_sr("vec-only", 2, 0.8)];
        let fts = vec![make_fts("shared", -5.0), make_fts("fts-only", -4.0)];
        let ordered = fuse_keyword_first(&v, &fts, &[], 10);
        let shared_count = ordered.iter().filter(|(id, _)| id == "shared").count();
        assert_eq!(shared_count, 1, "shared ID must appear exactly once");
    }

    /// ⑤  limit truncation.
    #[test]
    fn fuse_respects_limit() {
        let v: Vec<SearchResult> = (0..5)
            .map(|i| make_sr(&format!("v{i}"), i + 1, 0.9 - i as f32 * 0.1))
            .collect();
        let fts: Vec<TextSearchResult> = (0..5)
            .map(|i| make_fts(&format!("f{i}"), -(i as f32)))
            .collect();
        let ordered = fuse_keyword_first(&v, &fts, &[], 3);
        assert_eq!(ordered.len(), 3);
    }

    /// ⑥  Empty inputs → empty output.
    #[test]
    fn fuse_empty_inputs_empty_output() {
        let ordered = fuse_keyword_first(&[], &[], &[], 10);
        assert!(ordered.is_empty());
    }

    /// RRF arithmetic correctness (α=0.6, β=0.4, k=60, known ranks).
    #[test]
    fn fuse_rrf_score_arithmetic() {
        // seg-A: vector rank 1, fts rank 2
        // seg-B: vector rank 2, fts rank 1
        let v = vec![make_sr("seg-A", 1, 0.95), make_sr("seg-B", 2, 0.85)];
        let fts = vec![make_fts("seg-B", -10.0), make_fts("seg-A", -8.0)];
        let ordered = fuse_keyword_first(&v, &fts, &[], 10);

        let score_a = ordered.iter().find(|(id, _)| id == "seg-A").unwrap().1;
        let score_b = ordered.iter().find(|(id, _)| id == "seg-B").unwrap().1;

        let expected_a = RRF_ALPHA / (RRF_K + 1.0) + RRF_BETA / (RRF_K + 2.0);
        let expected_b = RRF_ALPHA / (RRF_K + 2.0) + RRF_BETA / (RRF_K + 1.0);

        assert!(
            (score_a - expected_a).abs() < 1e-6,
            "seg-A: got {score_a} expected {expected_a}"
        );
        assert!(
            (score_b - expected_b).abs() < 1e-6,
            "seg-B: got {score_b} expected {expected_b}"
        );
        // alpha=0.6 favours vector; seg-A is vector rank 1 → seg-A wins
        assert!(score_a > score_b);
    }

    // ── Fallback routing tests (Item 2f) ─────────────────────────────────────

    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;

    struct MarkerSanitizer;

    impl PiiSanitizer for MarkerSanitizer {
        fn sanitize_text(&self, text: &str, _level: PiiFilterLevel) -> String {
            text.replace("user@example.com", "[EMAIL]")
                .replace("555-123-4567", "[PHONE]")
        }
    }

    struct CountingAdaptive {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl AdaptiveSearchPort for CountingAdaptive {
        async fn search(
            &self,
            _q: &[f32],
            _limit: usize,
            _decay: f32,
            _filters: &SearchFilters,
        ) -> Result<Vec<SearchResult>, CoreError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        }
        async fn refresh_count(&self) -> Result<(), CoreError> {
            Ok(())
        }
    }

    struct CountingVectorStore {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl VectorStore for CountingVectorStore {
        async fn store(&self, _v: Vec<f32>, _m: EmbeddingMetadata) -> Result<(), CoreError> {
            Ok(())
        }
        async fn search(
            &self,
            _q: &[f32],
            _limit: usize,
            _decay: f32,
        ) -> Result<Vec<SearchResult>, CoreError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        }
        async fn search_filtered(
            &self,
            _q: &[f32],
            _limit: usize,
            _decay: f32,
            _f: &SearchFilters,
        ) -> Result<Vec<SearchResult>, CoreError> {
            Ok(Vec::new())
        }
        async fn enforce_retention(&self, _days: u32) -> Result<u64, CoreError> {
            Ok(0)
        }
        async fn mark_stale(&self, _id: &str) -> Result<u64, CoreError> {
            Ok(0)
        }
        async fn update_vector(&self, _id: i64, _v: Vec<f32>, _m: &str) -> Result<u64, CoreError> {
            Ok(1)
        }
        async fn get_current_model_id(&self) -> Result<Option<String>, CoreError> {
            Ok(None)
        }
        async fn get_stale_vectors(&self, _limit: usize) -> Result<Vec<(i64, String)>, CoreError> {
            Ok(vec![])
        }
    }

    struct ZeroEmbedding;

    #[async_trait]
    impl EmbeddingProvider for ZeroEmbedding {
        async fn embed(&self, _text: &str) -> Result<Vec<f32>, CoreError> {
            Ok(vec![0.0_f32; 4])
        }
        fn dimensions(&self) -> usize {
            4
        }
        fn model_id(&self) -> &str {
            "test-zero"
        }
    }

    struct RecordingEmbedding {
        last_text: std::sync::Mutex<Option<String>>,
    }

    #[async_trait]
    impl EmbeddingProvider for RecordingEmbedding {
        async fn embed(&self, text: &str) -> Result<Vec<f32>, CoreError> {
            *self.last_text.lock().expect("lock") = Some(text.to_string());
            Ok(vec![0.0_f32; 4])
        }
        fn dimensions(&self) -> usize {
            4
        }
        fn model_id(&self) -> &str {
            "test-recording"
        }
    }

    /// Embedding provider that always returns an error.
    struct FailingEmbedding;

    #[async_trait]
    impl EmbeddingProvider for FailingEmbedding {
        async fn embed(&self, _text: &str) -> Result<Vec<f32>, CoreError> {
            Err(CoreError::ServiceUnavailable {
                code: maekon_core::error_codes::ServiceCode::Unavailable,
                message: "embedding daemon is down (test)".to_string(),
            })
        }
        fn dimensions(&self) -> usize {
            4
        }
        fn model_id(&self) -> &str {
            "test-failing"
        }
    }

    // #7738 D-4: funnel through the canonical test-state helper.
    fn build_state() -> AppState {
        crate::test_local_auth::test_app_state_with_event_capacity(10)
    }

    #[tokio::test]
    async fn execute_prefers_adaptive_when_available() {
        let adaptive = Arc::new(CountingAdaptive {
            calls: AtomicUsize::new(0),
        });
        let brute = Arc::new(CountingVectorStore {
            calls: AtomicUsize::new(0),
        });
        let mut state = build_state();
        state.analysis.adaptive_search = Some(adaptive.clone());
        state.analysis.vector_store = Some(brute.clone());
        state.analysis.embedding_provider = Some(Arc::new(ZeroEmbedding));

        execute(&state, "hello", 10, "semantic").await.unwrap();
        assert_eq!(adaptive.calls.load(Ordering::SeqCst), 1);
        assert_eq!(brute.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn execute_falls_back_to_vector_store_when_adaptive_missing() {
        let brute = Arc::new(CountingVectorStore {
            calls: AtomicUsize::new(0),
        });
        let mut state = build_state();
        state.analysis.vector_store = Some(brute.clone());
        state.analysis.embedding_provider = Some(Arc::new(ZeroEmbedding));

        execute(&state, "hello", 10, "semantic").await.unwrap();
        assert_eq!(brute.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn vector_search_sends_sanitized_query_to_embedding_provider() {
        let brute = Arc::new(CountingVectorStore {
            calls: AtomicUsize::new(0),
        });
        let embedding = Arc::new(RecordingEmbedding {
            last_text: std::sync::Mutex::new(None),
        });
        let mut state = build_state();
        state.analysis.vector_store = Some(brute);
        state.analysis.embedding_provider = Some(embedding.clone());
        state.diagnostics.pii_sanitizer = Some(Arc::new(MarkerSanitizer));

        execute(
            &state,
            "find user@example.com and call 555-123-4567",
            10,
            "semantic",
        )
        .await
        .unwrap();

        let sent = embedding
            .last_text
            .lock()
            .expect("lock")
            .clone()
            .expect("embedding text recorded");
        assert!(!sent.contains("user@example.com"));
        assert!(!sent.contains("555-123-4567"));
        assert!(sent.contains("[EMAIL]"));
        assert!(sent.contains("[PHONE]"));
    }

    /// #7574 regression: when no vector/embedding pipeline is configured (the
    /// production default -- `vector_store` / `embedding_provider` /
    /// `adaptive_search` are only wired under `#[cfg(test)]`), `execute` must
    /// report the explicit capability boundary (`SemanticNotConfigured`) with
    /// its honest message, not a generic `CoreError::ServiceUnavailable`.
    ///
    /// Fails before the fix: previously this same setup produced
    /// `CoreError::ServiceUnavailable { code: service.unavailable, .. }`,
    /// which the HTTP layer mapped to 503 -- misleadingly implying a
    /// transient outage a client should retry, when in fact this build never
    /// implements semantic search at all.
    #[tokio::test]
    async fn execute_reports_capability_boundary_when_pipeline_absent() {
        let state = build_state();
        let err = execute(&state, "hello", 10, "semantic").await.unwrap_err();
        assert!(
            matches!(err, SearchExecutionError::SemanticNotConfigured),
            "expected SemanticNotConfigured, got: {err:?}"
        );
        assert_eq!(err.code(), "service.not_implemented");
        assert!(
            err.to_string()
                .contains("semantic search is not configured in this build"),
            "unexpected error message: {err}"
        );
        assert!(
            err.to_string().contains("mode=keyword") && err.to_string().contains("mode=hybrid"),
            "message must point callers at the working modes: {err}"
        );
    }

    /// #7574 regression: the capability boundary is scoped to `mode=semantic`
    /// only. With the same "nothing configured" state, `mode=keyword` and
    /// `mode=hybrid` must both keep returning functional keyword results
    /// (hybrid via its existing degrade-to-keyword path).
    #[tokio::test]
    async fn keyword_and_hybrid_modes_still_work_when_semantic_pipeline_absent() {
        // #7738 D-4: funnel through the canonical test-state helper.
        let (mut state, storage) = crate::test_local_auth::test_app_state_with_storage();
        state.analysis.text_search = Some(storage);
        // vector_store / embedding_provider / adaptive_search intentionally
        // left None -- the production default this fix targets.

        state
            .analysis
            .text_search
            .as_ref()
            .unwrap()
            .sync_segment(
                "seg-boundary-001",
                "capability boundary regression document",
            )
            .await
            .unwrap();

        let keyword_results = execute(&state, "boundary", 10, "keyword")
            .await
            .expect("keyword mode must keep working");
        assert_eq!(keyword_results.len(), 1);
        assert_eq!(keyword_results[0].segment_id, "seg-boundary-001");

        let hybrid_results = execute(&state, "boundary", 10, "hybrid")
            .await
            .expect("hybrid mode must degrade to keyword, not error");
        assert_eq!(hybrid_results.len(), 1);
        assert_eq!(hybrid_results[0].segment_id, "seg-boundary-001");

        let semantic_err = execute(&state, "boundary", 10, "semantic")
            .await
            .expect_err("semantic mode must still report the capability boundary");
        assert!(matches!(
            semantic_err,
            SearchExecutionError::SemanticNotConfigured
        ));
    }

    // ── hybrid fallback: embedding failure → keyword (#5766) ─────────────────

    /// When embedding provider errors in hybrid mode, the service must degrade
    /// to keyword-only and return Ok — not propagate the embedding Err.
    #[tokio::test]
    async fn hybrid_degrades_to_keyword_when_embedding_fails_vector_store() {
        let brute = Arc::new(CountingVectorStore {
            calls: AtomicUsize::new(0),
        });
        // #7738 D-4: funnel through the canonical test-state helper.
        let (mut state, storage) = crate::test_local_auth::test_app_state_with_storage();
        state.analysis.vector_store = Some(brute.clone());
        state.analysis.embedding_provider = Some(Arc::new(FailingEmbedding));
        state.analysis.text_search = Some(storage);

        // Seed one FTS document so we get a non-empty keyword result
        state
            .analysis
            .text_search
            .as_ref()
            .unwrap()
            .sync_segment("seg-kw-001", "authentication fallback document")
            .await
            .unwrap();

        let results = execute(&state, "authentication", 10, "hybrid")
            .await
            .expect("hybrid must degrade to keyword without Err");

        // Vector store must NOT have been called (embedding failed before search)
        assert_eq!(
            brute.calls.load(Ordering::SeqCst),
            0,
            "vector store must not be called after embedding failure"
        );
        // Keyword results must be returned
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].segment_id, "seg-kw-001");
    }

    /// When embedding provider errors in hybrid mode and text_search is also
    /// absent, the service must return ServiceUnavailable.
    #[tokio::test]
    async fn hybrid_errors_when_both_embedding_and_text_search_absent() {
        let brute = Arc::new(CountingVectorStore {
            calls: AtomicUsize::new(0),
        });
        let mut state = build_state();
        state.analysis.vector_store = Some(brute);
        state.analysis.embedding_provider = Some(Arc::new(FailingEmbedding));
        // text_search intentionally None

        let err = execute(&state, "hello", 10, "hybrid").await.unwrap_err();
        assert_eq!(err.code(), "service.unavailable");
    }

    /// "semantic" mode must NOT degrade to keyword when embedding fails — it
    /// must return the Err so callers receive a 503.
    #[tokio::test]
    async fn semantic_mode_does_not_degrade_to_keyword() {
        let brute = Arc::new(CountingVectorStore {
            calls: AtomicUsize::new(0),
        });
        // #7738 D-4: funnel through the canonical test-state helper.
        let (mut state, storage) = crate::test_local_auth::test_app_state_with_storage();
        state.analysis.vector_store = Some(brute);
        state.analysis.embedding_provider = Some(Arc::new(FailingEmbedding));
        state.analysis.text_search = Some(storage);

        let err = execute(&state, "hello", 10, "semantic").await.unwrap_err();
        // Must propagate the embedding error, not silently succeed via keyword
        assert_eq!(err.code(), "service.unavailable");
    }

    /// Hybrid mode with NO embedding provider configured at all degrades to keyword.
    #[tokio::test]
    async fn hybrid_degrades_when_no_embedding_provider_configured() {
        // #7738 D-4: funnel through the canonical test-state helper.
        let (mut state, storage) = crate::test_local_auth::test_app_state_with_storage();
        state.analysis.text_search = Some(storage);
        // embedding_provider intentionally None, no vector_store

        state
            .analysis
            .text_search
            .as_ref()
            .unwrap()
            .sync_segment("seg-001", "relevant keyword content")
            .await
            .unwrap();

        let results = execute(&state, "keyword", 10, "hybrid")
            .await
            .expect("must degrade gracefully to keyword");
        assert!(!results.is_empty());
    }

    // ── keyword mode: FTS BM25 relevance ordering (#7912 T3.3-Tier1) ──────────

    /// The dashboard keyword mode's value contract: `mode=keyword` returns FTS5
    /// BM25 relevance-ranked results end-to-end (`execute` -> `keyword_search`
    /// -> `search_fts`), NOT recency-ordered ones. Uses a real `SqliteStorage`
    /// as the `text_search` provider (no theater mock) so the ordering asserted
    /// here is the genuine FTS5 `ORDER BY rank` the UI depends on.
    #[tokio::test]
    async fn keyword_mode_preserves_bm25_relevance_order() {
        // #7738 D-4: funnel through the canonical test-state helper.
        let (mut state, storage) = crate::test_local_auth::test_app_state_with_storage();
        state.analysis.text_search = Some(storage);
        let ts = state.analysis.text_search.as_ref().unwrap();

        // seg-heavy mentions "rust" twice -> stronger BM25 relevance than
        // seg-light (once); seg-none does not match at all.
        ts.sync_segment("seg-light", "rust programming language systems")
            .await
            .unwrap();
        ts.sync_segment("seg-heavy", "rust compiler optimization rust")
            .await
            .unwrap();
        ts.sync_segment("seg-none", "python web development")
            .await
            .unwrap();

        let results = execute(&state, "rust", 10, "keyword").await.unwrap();

        let ids: Vec<&str> = results.iter().map(|r| r.segment_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["seg-heavy", "seg-light"],
            "keyword mode must return FTS BM25 relevance order (double-mention \
             first), excluding the non-matching python segment"
        );
    }

    // ── ADR-032 Mode A re-rank unit tests ────────────────────────────────────

    use maekon_core::models::memory_graph::ProjectedEdge;

    fn make_semantic_result(id: &str, score: f32) -> SemanticSearchResult {
        SemanticSearchResult {
            segment_id: id.to_string(),
            content_type: "segment_summary".to_string(),
            content_label: None,
            original_text: format!("text-{id}"),
            score,
            similarity: score,
            time_decay: 1.0,
            timestamp: "2026-07-29T00:00:00Z".to_string(),
            segment_start: None,
            segment_end: None,
            duration_secs: None,
            llm_summary: None,
            dominant_category: None,
            regime_label: None,
        }
    }

    fn evidence_edge(dst: &str, confidence: f32) -> ProjectedEdge {
        ProjectedEdge {
            src_id: "clm_src".to_string(),
            dst_id: dst.to_string(),
            edge_type: EdgeType::Evidence,
            confidence,
        }
    }

    #[test]
    fn empty_projection_leaves_ranking_bit_identical() {
        let mut results = vec![
            make_semantic_result("seg_a", 0.9),
            make_semantic_result("seg_b", 0.5),
        ];
        let matched = rerank_with_edge_projection(&EdgeProjection::default(), &mut results);
        assert_eq!(matched, 0);
        assert_eq!(results[0].score, 0.9);
        assert_eq!(results[1].score, 0.5);
    }

    #[test]
    fn evidence_boost_reorders_matching_segment() {
        let mut results = vec![
            make_semantic_result("seg_top", 0.6),
            make_semantic_result("seg_boosted", 0.5),
        ];
        let projection = EdgeProjection {
            edges: vec![evidence_edge("seg_boosted", 1.0)],
            claims_selected: 1,
        };
        let matched = rerank_with_edge_projection(&projection, &mut results);
        assert_eq!(matched, 1);
        // 0.5 × (1 + 0.25·1.0) = 0.625 > 0.6 — the evidence-backed segment
        // overtakes the previously-top result.
        assert_eq!(results[0].segment_id, "seg_boosted");
        assert!((results[0].score - 0.625).abs() < 1e-6);
        assert_eq!(results[1].score, 0.6);
    }

    #[test]
    fn non_evidence_edges_never_influence_ranking() {
        let mut results = vec![
            make_semantic_result("seg_top", 0.6),
            make_semantic_result("seg_b", 0.5),
        ];
        let projection = EdgeProjection {
            edges: vec![ProjectedEdge {
                src_id: "clm_src".to_string(),
                dst_id: "seg_b".to_string(),
                edge_type: EdgeType::Contradicts,
                confidence: 1.0,
            }],
            claims_selected: 1,
        };
        let matched = rerank_with_edge_projection(&projection, &mut results);
        assert_eq!(matched, 0);
        assert_eq!(results[0].segment_id, "seg_top");
        assert_eq!(results[1].score, 0.5);
    }

    #[test]
    fn boost_factor_is_capped_and_confidence_clamped() {
        let mut results = vec![make_semantic_result("seg_a", 1.0)];
        let projection = EdgeProjection {
            // 40 edges × confidence 1.0 (and one absurd 99.0 clamped to 1.0):
            // uncapped factor would be 1 + 0.25·41 = 11.25; the cap holds ×2.
            edges: (0..40)
                .map(|_| evidence_edge("seg_a", 1.0))
                .chain(std::iter::once(evidence_edge("seg_a", 99.0)))
                .collect(),
            claims_selected: 1,
        };
        let matched = rerank_with_edge_projection(&projection, &mut results);
        assert_eq!(matched, 1);
        assert!((results[0].score - 2.0).abs() < 1e-6);
    }
}
