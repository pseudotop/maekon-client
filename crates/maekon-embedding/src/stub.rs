//! Stub embedding provider — used when the `fastembed-local` feature is disabled.

#[cfg(not(feature = "fastembed-local"))]
pub mod stub_impl {
    use std::sync::atomic::{AtomicU64, Ordering};

    use async_trait::async_trait;
    use maekon_core::error::CoreError;
    use maekon_core::ports::embedding_provider::{EmbeddingProvider, ReloadableModel};

    use crate::error::EmbeddingError;

    /// Stub provider used when the `fastembed-local` feature is disabled.
    ///
    /// Every method returns `CoreError::ServiceUnavailable` with a descriptive
    /// message. `model_version()` and `reload()` are available for API
    /// compatibility.
    pub struct LocalEmbeddingProvider {
        model_id: String,
        dimensions: usize,
        model_version: AtomicU64,
    }

    impl LocalEmbeddingProvider {
        pub fn new(_model_name: Option<&str>) -> Result<Self, EmbeddingError> {
            Ok(Self {
                model_id: "stub-no-fastembed".to_owned(),
                dimensions: 384,
                model_version: AtomicU64::new(1),
            })
        }

        /// Current model version — always 1 for stub, incremented by `reload()`.
        pub fn model_version(&self) -> u64 {
            self.model_version.load(Ordering::Relaxed)
        }

        /// Stub reload — no actual model to reinitialise, but bumps version
        /// so the IPC contract is satisfied.
        pub fn reload(&self) -> Result<u64, EmbeddingError> {
            let new_version = self.model_version.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::info!(version = new_version, "Stub embedding model reload (no-op)");
            Ok(new_version)
        }
    }

    #[async_trait]
    impl EmbeddingProvider for LocalEmbeddingProvider {
        async fn embed(&self, _text: &str) -> Result<Vec<f32>, CoreError> {
            // Iter-109: feature-gate disabled at compile = the service is
            // unavailable in this build (iter-108 pattern). Wire code
            // `service.unavailable` lets frontend show a clean "not
            // available in your build" message.
            Err(CoreError::ServiceUnavailable {
                code: maekon_core::error_codes::ServiceCode::Unavailable,
                message: "fastembed-local feature is not enabled — cannot embed locally".into(),
            })
        }

        async fn embed_batch(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, CoreError> {
            // Iter-109: same as embed() above.
            Err(CoreError::ServiceUnavailable {
                code: maekon_core::error_codes::ServiceCode::Unavailable,
                message: "fastembed-local feature is not enabled — cannot embed locally".into(),
            })
        }

        fn dimensions(&self) -> usize {
            self.dimensions
        }

        fn model_id(&self) -> &str {
            &self.model_id
        }
    }

    impl ReloadableModel for LocalEmbeddingProvider {
        fn model_version(&self) -> u64 {
            self.model_version()
        }

        fn reload(&self) -> Result<u64, CoreError> {
            self.reload().map_err(CoreError::from)
        }
    }
}
