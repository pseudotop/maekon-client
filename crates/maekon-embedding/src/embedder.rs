//! fastembed-rs (ONNX Runtime) backed embedding provider.
//!
//! Only compiled when the `fastembed-local` feature is enabled.

#[cfg(feature = "fastembed-local")]
pub mod fastembed_impl {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

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
    ///
    /// #6441 F18: the ONNX model is **lazily loaded** — `new()` only resolves the
    /// model metadata; the heavyweight `TextEmbedding` is built on the first
    /// `embed`, and `evict_if_idle` / `reload` reset it to `None` so an enabled-
    /// but-idle provider holds no model RSS for the process lifetime.
    pub struct LocalEmbeddingProvider {
        /// `None` until the first `embed`; reset to `None` by `reload()` and
        /// `evict_model_if_idle()` so the next `embed` lazily reloads.
        model: Arc<Mutex<Option<fastembed::TextEmbedding>>>,
        model_id: String,
        model_name_raw: Mutex<Option<String>>,
        dimensions: usize,
        model_version: AtomicU64,
        /// Timestamp of the last successful embed — gates idle eviction.
        last_used: Arc<Mutex<Instant>>,
    }

    impl LocalEmbeddingProvider {
        /// Create a new provider with the given fastembed model.
        ///
        /// `model_name` is an `EmbeddingModel` variant name such as
        /// `"AllMiniLML6V2"`. If omitted or unrecognised the default model
        /// (`AllMiniLML6V2`, 384-dim) is used.
        pub fn new(model_name: Option<&str>) -> Result<Self, EmbeddingError> {
            // #6441 F18: resolve + validate the model metadata now, but do NOT
            // load the ONNX model — defer to the first embed (lazy). resolve_model
            // is infallible (it falls back to the default for unknown names), so
            // new() no longer downloads/loads weights or blocks startup.
            let (_model_enum, id, dims) = resolve_model(model_name);

            Ok(Self {
                model: Arc::new(Mutex::new(None)),
                model_id: id,
                model_name_raw: Mutex::new(model_name.map(String::from)),
                dimensions: dims,
                model_version: AtomicU64::new(1),
                last_used: Arc::new(Mutex::new(Instant::now())),
            })
        }

        /// Current model version — incremented on each successful `reload()`.
        pub fn model_version(&self) -> u64 {
            self.model_version.load(Ordering::Relaxed)
        }

        /// Build a fresh `TextEmbedding` for `raw_name` (heavyweight — downloads
        /// and loads the ONNX weights). Called lazily under the model lock on the
        /// first embed and after a reload/eviction. #6441 F18.
        fn init_model(raw_name: Option<&str>) -> Result<fastembed::TextEmbedding, EmbeddingError> {
            let (model_enum, _id, _dims) = resolve_model(raw_name);
            let options = fastembed::InitOptions::new(model_enum).with_show_download_progress(true);
            fastembed::TextEmbedding::try_new(options)
                .map_err(|e| EmbeddingError::Internal(format!("fastembed init failed: {e}")))
        }

        /// Re-initialise the ONNX model without restarting the app.
        ///
        /// #6441 F18: with lazy loading, reload drops the resident model so the
        /// next `embed` rebuilds it from `model_name_raw`, and bumps
        /// `model_version` for cache invalidation. The observable contract is
        /// unchanged (version increments; the next embed uses a fresh model) but
        /// the rebuild now happens on next use rather than synchronously here —
        /// which also means a config-driven model change is picked up lazily.
        pub fn reload(&self) -> Result<u64, EmbeddingError> {
            let mut guard = self
                .model
                .lock()
                .map_err(|e| EmbeddingError::Internal(format!("model lock poisoned: {e}")))?;
            *guard = None;
            drop(guard);

            let new_version = self.model_version.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::info!(
                version = new_version,
                "Embedding model marked for lazy reload"
            );
            Ok(new_version)
        }

        /// Drop the resident model if it has not been used within `idle_after`,
        /// reclaiming the ONNX RSS; the next `embed` lazily reloads it (#6441 F18).
        /// Returns `true` only when a loaded model was actually evicted.
        ///
        /// Named distinctly from the `EmbeddingProvider::evict_if_idle` trait
        /// method to avoid the inherent-vs-trait shadow (the trait method
        /// delegates here via `self.evict_model_if_idle(..)`).
        pub fn evict_model_if_idle(&self, idle_after: Duration) -> bool {
            let idle = {
                let last = self.last_used.lock().unwrap_or_else(|e| e.into_inner());
                last.elapsed() >= idle_after
            };
            if !idle {
                return false;
            }
            // Drop the model and release the lock before logging/returning so the
            // guard is not held across the tracing call (significant_drop_tightening).
            let evicted = {
                let mut guard = self.model.lock().unwrap_or_else(|e| e.into_inner());
                let was_loaded = guard.is_some();
                *guard = None;
                was_loaded
            };
            if evicted {
                tracing::info!(
                    idle_secs = idle_after.as_secs(),
                    "Idle embedding model evicted — ONNX RSS reclaimed; next embed reloads"
                );
            }
            evicted
        }

        /// Snapshot the configured model name under its lock, so the lazy init
        /// inside `spawn_blocking` can rebuild the model without holding the
        /// name lock across the `.await`.
        fn model_name_raw_snapshot(&self) -> Result<Option<String>, CoreError> {
            self.model_name_raw
                .lock()
                .map(|g| g.clone())
                .map_err(|e| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: format!("[embedding] model_name lock poisoned: {e}"),
                })
        }

        /// Record the current time as the last successful embed (gates eviction).
        fn mark_used(last_used: &Arc<Mutex<Instant>>) {
            if let Ok(mut lu) = last_used.lock() {
                *lu = Instant::now();
            }
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
            let last_used = Arc::clone(&self.last_used);
            let raw_name = self.model_name_raw_snapshot()?;
            let text = text.to_owned();

            tokio::task::spawn_blocking(move || {
                let mut guard = model.lock().map_err(|e| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: format!("[embedding] fastembed lock poisoned: {e}"),
                })?;
                // #6441 F18: lazy-load on first use (or after reload/eviction).
                if guard.is_none() {
                    *guard = Some(Self::init_model(raw_name.as_deref()).map_err(CoreError::from)?);
                }
                let embedder = guard.as_mut().ok_or_else(|| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: "[embedding] model unexpectedly absent after lazy load".into(),
                })?;
                let results =
                    embedder
                        .embed(vec![text], None)
                        .map_err(|e| CoreError::Internal {
                            code: maekon_core::error_codes::InternalCode::Generic,
                            message: format!("[embedding] fastembed embed failed: {e}"),
                        })?;
                Self::mark_used(&last_used);
                results
                    .into_iter()
                    .next()
                    .ok_or_else(|| CoreError::Internal {
                        code: maekon_core::error_codes::InternalCode::Generic,
                        message: "[embedding] fastembed returned empty result".into(),
                    })
            })
            .await
            .map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("[embedding] spawn_blocking join error: {e}"),
            })?
        }

        #[allow(clippy::significant_drop_tightening)]
        async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, CoreError> {
            let model = Arc::clone(&self.model);
            let last_used = Arc::clone(&self.last_used);
            let raw_name = self.model_name_raw_snapshot()?;
            let texts = texts.to_vec();

            tokio::task::spawn_blocking(move || {
                let mut guard = model.lock().map_err(|e| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: format!("[embedding] fastembed lock poisoned: {e}"),
                })?;
                // #6441 F18: lazy-load on first use (or after reload/eviction).
                if guard.is_none() {
                    *guard = Some(Self::init_model(raw_name.as_deref()).map_err(CoreError::from)?);
                }
                let embedder = guard.as_mut().ok_or_else(|| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: "[embedding] model unexpectedly absent after lazy load".into(),
                })?;
                let out = embedder
                    .embed(texts, None)
                    .map_err(|e| CoreError::Internal {
                        code: maekon_core::error_codes::InternalCode::Generic,
                        message: format!("[embedding] fastembed batch embed failed: {e}"),
                    })?;
                Self::mark_used(&last_used);
                Ok(out)
            })
            .await
            .map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("[embedding] spawn_blocking join error: {e}"),
            })?
        }

        fn dimensions(&self) -> usize {
            self.dimensions
        }

        fn model_id(&self) -> &str {
            &self.model_id
        }

        /// #6441 F18: delegate to the inherent `evict_model_if_idle` (named
        /// distinctly to avoid the inherent-vs-trait method shadow).
        fn evict_if_idle(&self, idle_after: Duration) -> bool {
            self.evict_model_if_idle(idle_after)
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

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::time::Duration;

        // These exercise the #6441 F18 lazy state machine WITHOUT downloading
        // model weights: new() must not load, eviction is a no-op while nothing
        // is resident, and reload() bumps the version for cache invalidation.

        #[test]
        fn new_is_lazy_and_evict_is_noop_before_first_embed() {
            let p = LocalEmbeddingProvider::new(Some("AllMiniLML6V2Q")).expect("new must succeed");
            // Metadata is resolved without loading the ONNX model.
            assert_eq!(p.dimensions(), 384);
            assert_eq!(p.model_id(), "all-MiniLM-L6-v2-Q");
            // No model resident yet → nothing to evict even at a zero threshold.
            assert!(
                !p.evict_model_if_idle(Duration::ZERO),
                "eviction must be a no-op before the model is lazily loaded"
            );
        }

        #[test]
        fn reload_bumps_version_without_resident_model() {
            let p = LocalEmbeddingProvider::new(None).expect("new must succeed");
            assert_eq!(p.model_version(), 1);
            assert_eq!(p.reload().expect("reload must succeed"), 2);
            assert_eq!(p.model_version(), 2);
            // Reload drops any resident model; with none loaded, evict stays a no-op.
            assert!(!p.evict_model_if_idle(Duration::ZERO));
        }
    }
}
