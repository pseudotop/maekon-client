//! fastembed-rs (ONNX Runtime) backed embedding provider.
//!
//! Only compiled when the `fastembed-local` feature is enabled.

#[cfg(feature = "fastembed-local")]
pub mod fastembed_impl {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use maekon_core::error::CoreError;
    use maekon_core::ports::embedding_provider::{EmbeddingProvider, ReloadableModel};

    use crate::error::EmbeddingError;

    /// Local embedding provider backed by fastembed-rs (ONNX Runtime).
    ///
    /// Thread-safe (`Send + Sync`) — the inner `TextEmbedding` is wrapped
    /// in `Arc<Mutex>` and accessed only through `spawn_blocking`.
    ///
    /// Supports hot-reloading via `reload()` — re-initialises the ONNX model
    /// in-place and bumps `model_version` so callers can detect the change.
    pub struct LocalEmbeddingProvider {
        model: Arc<Mutex<fastembed::TextEmbedding>>,
        model_id: String,
        model_name_raw: Mutex<Option<String>>,
        dimensions: usize,
        model_version: AtomicU64,
    }

    impl LocalEmbeddingProvider {
        /// Create a new provider with the given fastembed model.
        ///
        /// `model_name` is an `EmbeddingModel` variant name such as
        /// `"AllMiniLML6V2"`. If omitted or unrecognised the default model
        /// (`AllMiniLML6V2`, 384-dim) is used.
        pub fn new(model_name: Option<&str>) -> Result<Self, EmbeddingError> {
            let (model_enum, id, dims) = resolve_model(model_name);

            let options = fastembed::InitOptions::new(model_enum).with_show_download_progress(true);

            let model = fastembed::TextEmbedding::try_new(options)
                .map_err(|e| EmbeddingError::Internal(format!("fastembed init failed: {e}")))?;

            Ok(Self {
                model: Arc::new(Mutex::new(model)),
                model_id: id,
                model_name_raw: Mutex::new(model_name.map(String::from)),
                dimensions: dims,
                model_version: AtomicU64::new(1),
            })
        }

        /// Current model version — incremented on each successful `reload()`.
        pub fn model_version(&self) -> u64 {
            self.model_version.load(Ordering::Relaxed)
        }

        /// Re-initialise the ONNX model in-place without restarting the app.
        ///
        /// Uses the same model name that was passed to `new()`. On success the
        /// internal model is swapped and `model_version` is incremented.
        ///
        /// P2 PR-A: model name + model locks are held across fastembed
        /// initialization (downloads ONNX weights, can take seconds). This
        /// is intentional — concurrent reloads must serialize or we risk
        /// one reload overwriting another mid-init.
        #[allow(clippy::significant_drop_tightening)]
        pub fn reload(&self) -> Result<u64, EmbeddingError> {
            let raw_name = self
                .model_name_raw
                .lock()
                .map_err(|e| EmbeddingError::Internal(format!("model_name lock poisoned: {e}")))?;
            let (model_enum, _id, _dims) = resolve_model(raw_name.as_deref());

            let options = fastembed::InitOptions::new(model_enum).with_show_download_progress(true);

            let new_model = fastembed::TextEmbedding::try_new(options)
                .map_err(|e| EmbeddingError::Internal(format!("fastembed reload failed: {e}")))?;

            let mut guard = self
                .model
                .lock()
                .map_err(|e| EmbeddingError::Internal(format!("model lock poisoned: {e}")))?;
            *guard = new_model;

            let new_version = self.model_version.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::info!(version = new_version, "Embedding model reloaded");
            Ok(new_version)
        }
    }

    #[async_trait]
    impl EmbeddingProvider for LocalEmbeddingProvider {
        // P2 PR-A: fastembed model lock is held across the embedding inference
        // call. This is the expected pattern — fastembed is not thread-safe
        // so concurrent embed calls must serialize through the lock.
        #[allow(clippy::significant_drop_tightening)]
        async fn embed(&self, text: &str) -> Result<Vec<f32>, CoreError> {
            let model = Arc::clone(&self.model);
            let text = text.to_owned();

            tokio::task::spawn_blocking(move || {
                let mut guard = model.lock().map_err(|e| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: format!("fastembed lock poisoned: {e}"),
                })?;
                let results = guard
                    .embed(vec![text], None)
                    .map_err(|e| CoreError::Internal {
                        code: maekon_core::error_codes::InternalCode::Generic,
                        message: format!("fastembed embed failed: {e}"),
                    })?;
                results
                    .into_iter()
                    .next()
                    .ok_or_else(|| CoreError::Internal {
                        code: maekon_core::error_codes::InternalCode::Generic,
                        message: "fastembed returned empty result".into(),
                    })
            })
            .await
            .map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("spawn_blocking join error: {e}"),
            })?
        }

        async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, CoreError> {
            let model = Arc::clone(&self.model);
            let texts = texts.to_vec();

            tokio::task::spawn_blocking(move || {
                let mut guard = model.lock().map_err(|e| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: format!("fastembed lock poisoned: {e}"),
                })?;
                guard.embed(texts, None).map_err(|e| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: format!("fastembed batch embed failed: {e}"),
                })
            })
            .await
            .map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("spawn_blocking join error: {e}"),
            })?
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

    /// Resolve a human-readable model name to fastembed enum + metadata.
    ///
    /// The default model is `AllMiniLML6V2Q` — the quantized (INT8) variant of
    /// all-MiniLM-L6-v2.  It provides ~3x faster CPU inference with less than
    /// 1% accuracy degradation compared to the full-precision (FP32) version.
    /// Users who need maximum accuracy can override `local_model` in config to
    /// `"AllMiniLML6V2"` (or any other supported variant) to switch back.
    pub fn resolve_model(name: Option<&str>) -> (fastembed::EmbeddingModel, String, usize) {
        match name {
            // Quantized variants (INT8 ONNX — ~3x faster, ~1% accuracy loss)
            Some("AllMiniLML6V2Q") | Some("all-MiniLM-L6-v2-Q") | None => (
                fastembed::EmbeddingModel::AllMiniLML6V2Q,
                "all-MiniLM-L6-v2-Q".to_owned(),
                384,
            ),
            Some("AllMiniLML12V2Q") | Some("all-MiniLM-L12-v2-Q") => (
                fastembed::EmbeddingModel::AllMiniLML12V2Q,
                "all-MiniLM-L12-v2-Q".to_owned(),
                384,
            ),
            Some("BGESmallENV15Q") | Some("bge-small-en-v1.5-Q") => (
                fastembed::EmbeddingModel::BGESmallENV15Q,
                "bge-small-en-v1.5-Q".to_owned(),
                384,
            ),
            Some("BGEBaseENV15Q") | Some("bge-base-en-v1.5-Q") => (
                fastembed::EmbeddingModel::BGEBaseENV15Q,
                "bge-base-en-v1.5-Q".to_owned(),
                768,
            ),
            // Full-precision variants (FP32 ONNX — higher accuracy, slower)
            Some("AllMiniLML6V2") | Some("all-MiniLM-L6-v2") => (
                fastembed::EmbeddingModel::AllMiniLML6V2,
                "all-MiniLM-L6-v2".to_owned(),
                384,
            ),
            Some("AllMiniLML12V2") | Some("all-MiniLM-L12-v2") => (
                fastembed::EmbeddingModel::AllMiniLML12V2,
                "all-MiniLM-L12-v2".to_owned(),
                384,
            ),
            Some("BGESmallENV15") | Some("bge-small-en-v1.5") => (
                fastembed::EmbeddingModel::BGESmallENV15,
                "bge-small-en-v1.5".to_owned(),
                384,
            ),
            Some("BGEBaseENV15") | Some("bge-base-en-v1.5") => (
                fastembed::EmbeddingModel::BGEBaseENV15,
                "bge-base-en-v1.5".to_owned(),
                768,
            ),
            Some(other) => {
                tracing::warn!(
                    model = other,
                    "Unknown embedding model, falling back to AllMiniLML6V2Q"
                );
                (
                    fastembed::EmbeddingModel::AllMiniLML6V2Q,
                    "all-MiniLM-L6-v2-Q".to_owned(),
                    384,
                )
            }
        }
    }
}
