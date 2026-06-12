//! Fallback chaining embedding provider.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::ports::embedding_provider::EmbeddingProvider;

/// Chains two embedding providers: tries primary first, falls back on error.
///
/// Tracks per-request health of the primary provider via an `AtomicBool`.
/// Callers holding the concrete type can inspect `is_primary_healthy()` to
/// decide whether to surface degraded-mode indicators in the UI or logs.
pub struct FallbackEmbeddingProvider {
    primary: Arc<dyn EmbeddingProvider>,
    fallback: Arc<dyn EmbeddingProvider>,
    primary_healthy: Arc<AtomicBool>,
}

impl FallbackEmbeddingProvider {
    pub fn new(primary: Arc<dyn EmbeddingProvider>, fallback: Arc<dyn EmbeddingProvider>) -> Self {
        Self {
            primary,
            fallback,
            primary_healthy: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Returns `true` when the most recent primary embed call succeeded.
    pub fn is_primary_healthy(&self) -> bool {
        self.primary_healthy.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl EmbeddingProvider for FallbackEmbeddingProvider {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, CoreError> {
        match self.primary.embed(text).await {
            Ok(v) => {
                self.primary_healthy.store(true, Ordering::Relaxed);
                Ok(v)
            }
            Err(e) => {
                self.primary_healthy.store(false, Ordering::Relaxed);
                tracing::warn!("primary embedding failed, trying fallback: {e}");
                self.fallback.embed(text).await
            }
        }
    }

    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, CoreError> {
        match self.primary.embed_batch(texts).await {
            Ok(v) => {
                self.primary_healthy.store(true, Ordering::Relaxed);
                Ok(v)
            }
            Err(e) => {
                self.primary_healthy.store(false, Ordering::Relaxed);
                tracing::warn!("primary batch embedding failed, trying fallback: {e}");
                self.fallback.embed_batch(texts).await
            }
        }
    }

    fn dimensions(&self) -> usize {
        self.primary.dimensions()
    }

    fn model_id(&self) -> &str {
        self.primary.model_id()
    }
}
