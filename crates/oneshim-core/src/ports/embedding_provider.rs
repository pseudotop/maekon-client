//! Port for generating vector embeddings from text via local or remote models.

use async_trait::async_trait;

use crate::error::CoreError;

/// Port for generating vector embeddings from text.
/// Adapters: local (fastembed-rs) or remote (OpenAI text-embedding-3-small etc.)
///
/// # Errors
/// - Remote adapters: HTTP-layer failures follow the canonical semantic status
///   mapping (`auth.failed` / `network.timeout` / `network.rate_limit` /
///   `service.unavailable` / `network.generic`). See
///   `docs/guides/http-status-error-mapping.md`. Parse/provider failures
///   emit `CoreError::Network` with generic domain fallback.
/// - Local adapters without the `fastembed-local` feature: emit
///   `CoreError::ServiceUnavailable` (wire: `service.unavailable`,
///   iter-109 re-route from Internal) — the embedding service is
///   unavailable in this build.
/// - Local adapters WITH `fastembed-local` enabled: runtime ML inference
///   faults (mutex poison, fastembed library error, empty-result
///   fallback, `spawn_blocking` JoinError) emit `CoreError::Internal`
///   (wire: `internal.generic`) with a context-prefixed message. These
///   are genuine internal faults — the embedding library misbehaved in
///   a way that doesn't fit Network/Analysis/Storage semantics.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Embed a single text into a vector of floats.
    async fn embed(&self, text: &str) -> Result<Vec<f32>, CoreError>;

    /// Embed a batch of texts. Default implementation calls embed() in sequence.
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, CoreError> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed(text).await?);
        }
        Ok(results)
    }

    /// Number of dimensions in the embedding vector.
    fn dimensions(&self) -> usize;

    /// Identifier of the embedding model (used for versioning stored vectors).
    fn model_id(&self) -> &str;
}

/// Port for embedding models that support runtime hot-reloading.
///
/// Implementors can re-initialise the underlying model without restarting the
/// application. `model_version` provides a monotonically increasing counter
/// that callers can use to detect changes (e.g. cache invalidation).
pub trait ReloadableModel: Send + Sync {
    /// Current model version — starts at 1, incremented on each `reload()`.
    fn model_version(&self) -> u64;

    /// Re-initialise the model in-place and return the new version.
    fn reload(&self) -> Result<u64, CoreError>;
}

/// No-op embedding provider that returns zero vectors.
/// Used as fallback when both local and remote embedding are unavailable.
#[derive(Debug)]
pub struct NoOpEmbeddingProvider {
    dimensions: usize,
}

impl NoOpEmbeddingProvider {
    pub fn new(dimensions: usize) -> Self {
        Self { dimensions }
    }
}

#[async_trait]
impl EmbeddingProvider for NoOpEmbeddingProvider {
    async fn embed(&self, _text: &str) -> Result<Vec<f32>, CoreError> {
        Ok(vec![0.0; self.dimensions])
    }

    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, CoreError> {
        Ok(texts.iter().map(|_| vec![0.0; self.dimensions]).collect())
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn model_id(&self) -> &str {
        "noop"
    }
}
