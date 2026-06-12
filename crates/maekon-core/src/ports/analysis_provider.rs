//! LLM-backed analysis port for context-to-suggestion and summarization pipelines.

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::memory_graph::{ClaimStatusChange, RelationEdgeProposal};
use crate::models::suggestion::Suggestion;

/// LLM-backed analysis port.
///
/// # Errors
/// - `CoreError::Analysis` (wire: `provider.analysis_failed`) for
///   provider-side failures: LLM returned bad JSON, empty body,
///   non-parseable intent, schema-violating suggestion fields. This
///   is also the default return from `summarize_text` when the
///   adapter does not override it.
/// - HTTP-layer failures follow the canonical semantic status mapping:
///   `CoreError::Auth` (401/403), `CoreError::RequestTimeout` (408/504),
///   `CoreError::RateLimit` (429), `CoreError::ServiceUnavailable` (502/503).
///   See `docs/guides/http-status-error-mapping.md` for the full table.
/// - `CoreError::Network` (wire: `network.generic`) for pre-response
///   transport failures (DNS, connection refused) that don't match the
///   timeout / rate-limit / service-unavailable specific variants.
#[async_trait]
pub trait AnalysisProvider: Send + Sync {
    /// Analyze assembled context and return productivity suggestions.
    /// The adapter is responsible for parsing the LLM response into Suggestions.
    async fn analyze(
        &self,
        context_json: &str,
        system_prompt: &str,
    ) -> Result<Vec<Suggestion>, CoreError>;

    /// Generate a plain text summary from context.
    ///
    /// Default impl returns `CoreError::Analysis { AnalysisFailed }` with a
    /// "not implemented" message — adapters that don't support summarize
    /// can leave the default. Adapters that DO support it must override.
    /// Previously the default called `analyze()` which parses JSON
    /// suggestions and is incompatible with the plain-text contract here.
    async fn summarize_text(
        &self,
        _context_json: &str,
        _system_prompt: &str,
    ) -> Result<String, CoreError> {
        Err(CoreError::Analysis {
            code: crate::error_codes::ProviderCode::AnalysisFailed,
            message: "summarize_text not implemented for this provider".into(),
        })
    }

    /// ADR-023 Phase-2 D1: extract epistemic relations (`supports`/`refines`/
    /// `contradicts`) between memory-graph claims.
    ///
    /// DEFAULT returns `Ok(vec![])` — an empty result is meaningful ("no
    /// relations found"), so `NoOp`/unconfigured providers degrade SILENTLY
    /// (never `Err`, never panic), mirroring `analyze`'s empty-vec contract and
    /// deliberately NOT `summarize_text`'s `Err` default. Adapters that support
    /// it MUST override. This text is fed to a LOCAL-only LLM (the caller gates
    /// egress; see `AnalysisClient::new_local_enrichment`).
    async fn extract_relations(
        &self,
        _claims_json: &str,
        _system_prompt: &str,
    ) -> Result<Vec<RelationEdgeProposal>, CoreError> {
        Ok(Vec::new())
    }

    /// ADR-023 Phase-2 D2: detect contradictions among claims and propose
    /// `active → superseded` resolutions.
    ///
    /// DEFAULT returns `Ok(vec![])` (same degrade-by-default contract as
    /// `extract_relations`). Adapters that support it MUST override.
    async fn detect_contradictions(
        &self,
        _claims_json: &str,
        _system_prompt: &str,
    ) -> Result<Vec<ClaimStatusChange>, CoreError> {
        Ok(Vec::new())
    }

    fn provider_name(&self) -> &str;
}
