//! Fallback chaining and no-op implementations of [`AnalysisProvider`].
//!
//! [`FallbackAnalysisProvider`] chains a primary and a fallback provider,
//! automatically switching to the fallback when the primary returns an error.
//! [`NoOpAnalysisProvider`] is a safe placeholder when no LLM is configured.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use async_trait::async_trait;
use maekon_core::{
    error::CoreError,
    models::memory_graph::{ClaimStatusChange, RelationEdgeProposal},
    models::suggestion::Suggestion,
    ports::analysis_provider::AnalysisProvider,
};

// ── FallbackAnalysisProvider ───────────────────────────────────────────────

/// Chains two analysis providers: tries primary first, falls back on error.
///
/// Tracks per-request health of the primary provider via an [`AtomicBool`].
/// Callers holding the concrete type can inspect [`is_primary_healthy`] to
/// decide whether to surface degraded-mode indicators in the UI or logs.
///
/// [`is_primary_healthy`]: FallbackAnalysisProvider::is_primary_healthy
pub struct FallbackAnalysisProvider {
    primary: Arc<dyn AnalysisProvider>,
    fallback: Arc<dyn AnalysisProvider>,
    primary_healthy: Arc<AtomicBool>,
}

impl FallbackAnalysisProvider {
    /// Create a new [`FallbackAnalysisProvider`] with its own health flag.
    pub fn new(primary: Arc<dyn AnalysisProvider>, fallback: Arc<dyn AnalysisProvider>) -> Self {
        Self {
            primary,
            fallback,
            primary_healthy: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Create a new [`FallbackAnalysisProvider`] sharing an external health
    /// flag so that [`AppState`] can observe primary health without holding
    /// a reference to the concrete type.
    ///
    /// [`AppState`]: src_tauri::AppState
    pub fn new_with_flag(
        primary: Arc<dyn AnalysisProvider>,
        fallback: Arc<dyn AnalysisProvider>,
        flag: Arc<AtomicBool>,
    ) -> Self {
        Self {
            primary,
            fallback,
            primary_healthy: flag,
        }
    }

    /// Returns `true` when the most recent primary analysis call succeeded.
    pub fn is_primary_healthy(&self) -> bool {
        self.primary_healthy.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl AnalysisProvider for FallbackAnalysisProvider {
    async fn analyze(
        &self,
        context_json: &str,
        system_prompt: &str,
    ) -> Result<Vec<Suggestion>, CoreError> {
        match self.primary.analyze(context_json, system_prompt).await {
            Ok(suggestions) => {
                self.primary_healthy.store(true, Ordering::Relaxed);
                Ok(suggestions)
            }
            Err(e) => {
                self.primary_healthy.store(false, Ordering::Relaxed);
                tracing::warn!("primary analysis provider failed, trying fallback: {e}");
                self.fallback.analyze(context_json, system_prompt).await
            }
        }
    }

    async fn summarize_text(
        &self,
        context_json: &str,
        system_prompt: &str,
    ) -> Result<String, CoreError> {
        match self
            .primary
            .summarize_text(context_json, system_prompt)
            .await
        {
            Ok(summary) => {
                self.primary_healthy.store(true, Ordering::Relaxed);
                Ok(summary)
            }
            Err(e) => {
                self.primary_healthy.store(false, Ordering::Relaxed);
                tracing::warn!(
                    "primary analysis provider summarize_text failed, trying fallback: {e}"
                );
                self.fallback
                    .summarize_text(context_json, system_prompt)
                    .await
            }
        }
    }

    async fn extract_relations(
        &self,
        claims_json: &str,
        system_prompt: &str,
    ) -> Result<Vec<RelationEdgeProposal>, CoreError> {
        // MUST override: the trait default `Ok(vec![])` would silently bypass a
        // healthy primary provider (F1 fallback-trap).
        match self
            .primary
            .extract_relations(claims_json, system_prompt)
            .await
        {
            Ok(rels) => {
                self.primary_healthy.store(true, Ordering::Relaxed);
                Ok(rels)
            }
            Err(e) => {
                self.primary_healthy.store(false, Ordering::Relaxed);
                tracing::warn!("primary extract_relations failed, trying fallback: {e}");
                self.fallback
                    .extract_relations(claims_json, system_prompt)
                    .await
            }
        }
    }

    async fn detect_contradictions(
        &self,
        claims_json: &str,
        system_prompt: &str,
    ) -> Result<Vec<ClaimStatusChange>, CoreError> {
        match self
            .primary
            .detect_contradictions(claims_json, system_prompt)
            .await
        {
            Ok(changes) => {
                self.primary_healthy.store(true, Ordering::Relaxed);
                Ok(changes)
            }
            Err(e) => {
                self.primary_healthy.store(false, Ordering::Relaxed);
                tracing::warn!("primary detect_contradictions failed, trying fallback: {e}");
                self.fallback
                    .detect_contradictions(claims_json, system_prompt)
                    .await
            }
        }
    }

    fn provider_name(&self) -> &str {
        self.primary.provider_name()
    }
}

// ── NoOpAnalysisProvider ───────────────────────────────────────────────────

/// A no-op analysis provider that returns empty results.
///
/// Used as a safe placeholder when no LLM provider is configured.
/// `analyze()` returns an empty [`Vec`]; `summarize_text()` returns an error
/// indicating that no provider is available.
pub struct NoOpAnalysisProvider;

#[async_trait]
impl AnalysisProvider for NoOpAnalysisProvider {
    async fn analyze(
        &self,
        _context_json: &str,
        _system_prompt: &str,
    ) -> Result<Vec<Suggestion>, CoreError> {
        tracing::debug!("NoOpAnalysisProvider::analyze called — returning empty suggestions");
        Ok(vec![])
    }

    async fn summarize_text(
        &self,
        _context_json: &str,
        _system_prompt: &str,
    ) -> Result<String, CoreError> {
        Err(CoreError::Analysis {
            code: maekon_core::error_codes::ProviderCode::AnalysisFailed,
            message: "No LLM provider configured".into(),
        })
    }

    fn provider_name(&self) -> &str {
        "noop"
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use maekon_core::models::suggestion::{Priority, SuggestionSource, SuggestionType};

    // ── Mock providers ─────────────────────────────────────────────────────

    /// Always succeeds — returns a single test suggestion.
    struct OkProvider;

    #[async_trait]
    impl AnalysisProvider for OkProvider {
        async fn analyze(
            &self,
            _context_json: &str,
            _system_prompt: &str,
        ) -> Result<Vec<Suggestion>, CoreError> {
            Ok(vec![Suggestion {
                suggestion_id: "ok-1".into(),
                suggestion_type: SuggestionType::ProductivityTip,
                content: "ok".into(),
                priority: Priority::Low,
                confidence_score: 1.0,
                relevance_score: 1.0,
                is_actionable: true,
                created_at: Utc::now(),
                expires_at: None,
                source: SuggestionSource::LlmLocal,
                reasoning: None,
                context_scope: None,
            }])
        }

        async fn summarize_text(
            &self,
            _context_json: &str,
            _system_prompt: &str,
        ) -> Result<String, CoreError> {
            Ok("ok-summary".into())
        }

        fn provider_name(&self) -> &str {
            "ok"
        }
    }

    /// Always fails with `CoreError::Analysis`.
    struct FailProvider;

    #[async_trait]
    impl AnalysisProvider for FailProvider {
        async fn analyze(
            &self,
            _context_json: &str,
            _system_prompt: &str,
        ) -> Result<Vec<Suggestion>, CoreError> {
            Err(CoreError::Analysis {
                code: maekon_core::error_codes::ProviderCode::AnalysisFailed,
                message: "provider failure".into(),
            })
        }

        async fn summarize_text(
            &self,
            _context_json: &str,
            _system_prompt: &str,
        ) -> Result<String, CoreError> {
            Err(CoreError::Analysis {
                code: maekon_core::error_codes::ProviderCode::AnalysisFailed,
                message: "provider failure".into(),
            })
        }

        fn provider_name(&self) -> &str {
            "fail"
        }
    }

    // ── FallbackAnalysisProvider tests ─────────────────────────────────────

    #[tokio::test]
    async fn primary_ok_returns_primary_result() {
        let provider =
            FallbackAnalysisProvider::new(Arc::new(OkProvider), Arc::new(NoOpAnalysisProvider));
        let result = provider.analyze("{}", "").await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].suggestion_id, "ok-1");
        assert!(provider.is_primary_healthy());
    }

    #[tokio::test]
    async fn primary_fail_uses_fallback() {
        let provider = FallbackAnalysisProvider::new(Arc::new(FailProvider), Arc::new(OkProvider));
        let result = provider.analyze("{}", "").await.unwrap();
        // Fallback (OkProvider) returns the ok suggestion.
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].suggestion_id, "ok-1");
        // Primary health flag is now false.
        assert!(!provider.is_primary_healthy());
    }

    #[tokio::test]
    async fn both_fail_propagates_error() {
        let provider =
            FallbackAnalysisProvider::new(Arc::new(FailProvider), Arc::new(FailProvider));
        let err = provider.analyze("{}", "").await.unwrap_err();
        assert!(matches!(err, CoreError::Analysis { .. }));
        assert!(!provider.is_primary_healthy());
    }

    #[tokio::test]
    async fn summarize_text_fallback() {
        let provider = FallbackAnalysisProvider::new(Arc::new(FailProvider), Arc::new(OkProvider));
        let summary = provider.summarize_text("{}", "").await.unwrap();
        assert_eq!(summary, "ok-summary");
        assert!(!provider.is_primary_healthy());
    }

    #[tokio::test]
    async fn health_flag_shared_via_new_with_flag() {
        let flag = Arc::new(AtomicBool::new(true));
        let provider = FallbackAnalysisProvider::new_with_flag(
            Arc::new(FailProvider),
            Arc::new(OkProvider),
            Arc::clone(&flag),
        );
        // Trigger a primary failure so the shared flag is updated.
        let _ = provider.analyze("{}", "").await.unwrap();
        // The externally held flag should now reflect the failure.
        assert!(!flag.load(Ordering::Relaxed));
    }

    // ── NoOpAnalysisProvider tests ─────────────────────────────────────────

    #[tokio::test]
    async fn noop_analyze_returns_empty() {
        let provider = NoOpAnalysisProvider;
        let result = provider.analyze("{}", "").await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn noop_summarize_returns_error() {
        let provider = NoOpAnalysisProvider;
        let err = provider.summarize_text("{}", "").await.unwrap_err();
        assert!(matches!(err, CoreError::Analysis { .. }));
        if let CoreError::Analysis {
            code: maekon_core::error_codes::ProviderCode::AnalysisFailed,
            message: msg,
        } = err
        {
            assert!(msg.contains("No LLM provider configured"));
        }
    }

    // ── ADR-023 Phase-2 D1/D2 provider-method tests ────────────────────────

    use maekon_core::models::memory_graph::{ClaimStatus, EdgeType};

    /// Overrides the new D1/D2 methods to return one proposal each.
    struct RelationProvider;

    #[async_trait]
    impl AnalysisProvider for RelationProvider {
        async fn analyze(&self, _c: &str, _s: &str) -> Result<Vec<Suggestion>, CoreError> {
            Ok(vec![])
        }
        async fn extract_relations(
            &self,
            _c: &str,
            _s: &str,
        ) -> Result<Vec<RelationEdgeProposal>, CoreError> {
            Ok(vec![RelationEdgeProposal {
                src_claim_id: "clm_a".into(),
                dst_id: "clm_b".into(),
                edge_type: EdgeType::Supports,
                confidence: 0.9,
                evidence_ref: None,
            }])
        }
        async fn detect_contradictions(
            &self,
            _c: &str,
            _s: &str,
        ) -> Result<Vec<ClaimStatusChange>, CoreError> {
            Ok(vec![ClaimStatusChange {
                loser_claim_id: "clm_a".into(),
                winner_claim_id: "clm_b".into(),
                new_status: ClaimStatus::Superseded,
                confidence: 0.95,
                rationale: None,
            }])
        }
        fn provider_name(&self) -> &str {
            "relation"
        }
    }

    #[tokio::test]
    async fn noop_new_methods_return_empty_not_err() {
        // F1/AC6: NoOp inherits the default Ok(vec![]) for both new methods.
        let provider = NoOpAnalysisProvider;
        assert!(provider
            .extract_relations("{}", "")
            .await
            .unwrap()
            .is_empty());
        assert!(provider
            .detect_contradictions("{}", "")
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn fallback_delegates_new_methods_to_healthy_primary() {
        // F1 fallback-trap: without the Fallback override, the trait default
        // Ok(vec![]) would silently bypass this healthy primary and return empty.
        let provider = FallbackAnalysisProvider::new(
            Arc::new(RelationProvider),
            Arc::new(NoOpAnalysisProvider),
        );
        let rels = provider.extract_relations("{}", "").await.unwrap();
        assert_eq!(
            rels.len(),
            1,
            "healthy primary's relation must not be bypassed"
        );
        assert_eq!(rels[0].edge_type, EdgeType::Supports);
        let changes = provider.detect_contradictions("{}", "").await.unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].new_status, ClaimStatus::Superseded);
    }
}
