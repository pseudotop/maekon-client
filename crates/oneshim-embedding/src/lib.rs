#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
// P2 PR-A nursery-hardening.
#![deny(clippy::significant_drop_tightening)]
#![cfg_attr(test, allow(clippy::significant_drop_tightening))]

//! Local embedding provider — fastembed-rs (ONNX Runtime) wrapper.
//!
//! Wraps the synchronous fastembed `TextEmbedding` API behind the async
//! `EmbeddingProvider` port defined in `oneshim-core`.  All blocking calls
//! are dispatched via `tokio::task::spawn_blocking` to avoid starving the
//! async runtime.
//!
//! When the `fastembed-local` feature is disabled (or if the ONNX runtime
//! cannot be loaded on the host platform) a compile-time stub is provided
//! that returns `CoreError::Internal` for every operation so that dependent
//! crates can still compile.

pub mod error;
pub use error::EmbeddingError;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use oneshim_core::error::CoreError;
use oneshim_core::ports::embedding_provider::{EmbeddingProvider, ReloadableModel};

// ── fastembed-backed implementation ────────────────────────────────────────

#[cfg(feature = "fastembed-local")]
mod fastembed_impl {
    use super::*;
    use std::sync::{Arc, Mutex};

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
                    code: oneshim_core::error_codes::InternalCode::Generic,
                    message: format!("fastembed lock poisoned: {e}"),
                })?;
                let results = guard
                    .embed(vec![text], None)
                    .map_err(|e| CoreError::Internal {
                        code: oneshim_core::error_codes::InternalCode::Generic,
                        message: format!("fastembed embed failed: {e}"),
                    })?;
                results
                    .into_iter()
                    .next()
                    .ok_or_else(|| CoreError::Internal {
                        code: oneshim_core::error_codes::InternalCode::Generic,
                        message: "fastembed returned empty result".into(),
                    })
            })
            .await
            .map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("spawn_blocking join error: {e}"),
            })?
        }

        async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, CoreError> {
            let model = Arc::clone(&self.model);
            let texts = texts.to_vec();

            tokio::task::spawn_blocking(move || {
                let mut guard = model.lock().map_err(|e| CoreError::Internal {
                    code: oneshim_core::error_codes::InternalCode::Generic,
                    message: format!("fastembed lock poisoned: {e}"),
                })?;
                guard.embed(texts, None).map_err(|e| CoreError::Internal {
                    code: oneshim_core::error_codes::InternalCode::Generic,
                    message: format!("fastembed batch embed failed: {e}"),
                })
            })
            .await
            .map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
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
    pub(super) fn resolve_model(name: Option<&str>) -> (fastembed::EmbeddingModel, String, usize) {
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

#[cfg(feature = "fastembed-local")]
pub use fastembed_impl::LocalEmbeddingProvider;

// ── Stub implementation (no fastembed feature) ─────────────────────────────

#[cfg(not(feature = "fastembed-local"))]
mod stub_impl {
    use super::*;

    /// Stub provider used when the `fastembed-local` feature is disabled.
    ///
    /// Every method returns `CoreError::Internal` with a descriptive message.
    /// `model_version()` and `reload()` are available for API compatibility.
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
                code: oneshim_core::error_codes::ServiceCode::Unavailable,
                message: "fastembed-local feature is not enabled — cannot embed locally".into(),
            })
        }

        async fn embed_batch(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, CoreError> {
            // Iter-109: same as embed() above.
            Err(CoreError::ServiceUnavailable {
                code: oneshim_core::error_codes::ServiceCode::Unavailable,
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

#[cfg(not(feature = "fastembed-local"))]
pub use stub_impl::LocalEmbeddingProvider;

// ── Fallback chaining provider ────────────────────────────────────────────

/// Chains two embedding providers: tries primary first, falls back on error.
///
/// Tracks per-request health of the primary provider via an `AtomicBool`.
/// Callers holding the concrete type can inspect `is_primary_healthy()` to
/// decide whether to surface degraded-mode indicators in the UI or logs.
pub struct FallbackEmbeddingProvider {
    primary: std::sync::Arc<dyn EmbeddingProvider>,
    fallback: std::sync::Arc<dyn EmbeddingProvider>,
    primary_healthy: Arc<AtomicBool>,
}

impl FallbackEmbeddingProvider {
    pub fn new(
        primary: std::sync::Arc<dyn EmbeddingProvider>,
        fallback: std::sync::Arc<dyn EmbeddingProvider>,
    ) -> Self {
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

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── resolve_model parameterized tests (fastembed feature) ─────────────

    #[cfg(feature = "fastembed-local")]
    mod resolve_model_tests {
        use super::*;

        /// Helper: assert resolve_model returns the expected (model_id, dimensions).
        fn assert_resolves(input: Option<&str>, expected_id: &str, expected_dims: usize) {
            let (_model, model_id, dimensions) = fastembed_impl::resolve_model(input);
            assert_eq!(
                model_id, expected_id,
                "model_id mismatch for input {input:?}"
            );
            assert_eq!(
                dimensions, expected_dims,
                "dimensions mismatch for input {input:?}"
            );
        }

        // ── Default (None) ───────────────────────────────────────────────

        #[test]
        fn none_defaults_to_quantized_minilm() {
            assert_resolves(None, "all-MiniLM-L6-v2-Q", 384);
        }

        // ── Quantized variants (INT8) ────────────────────────────────────

        #[test]
        fn all_minilm_l6_v2_q_pascal() {
            assert_resolves(Some("AllMiniLML6V2Q"), "all-MiniLM-L6-v2-Q", 384);
        }

        #[test]
        fn all_minilm_l6_v2_q_kebab() {
            assert_resolves(Some("all-MiniLM-L6-v2-Q"), "all-MiniLM-L6-v2-Q", 384);
        }

        #[test]
        fn all_minilm_l12_v2_q_pascal() {
            assert_resolves(Some("AllMiniLML12V2Q"), "all-MiniLM-L12-v2-Q", 384);
        }

        #[test]
        fn all_minilm_l12_v2_q_kebab() {
            assert_resolves(Some("all-MiniLM-L12-v2-Q"), "all-MiniLM-L12-v2-Q", 384);
        }

        #[test]
        fn bge_small_en_v15_q_pascal() {
            assert_resolves(Some("BGESmallENV15Q"), "bge-small-en-v1.5-Q", 384);
        }

        #[test]
        fn bge_small_en_v15_q_kebab() {
            assert_resolves(Some("bge-small-en-v1.5-Q"), "bge-small-en-v1.5-Q", 384);
        }

        #[test]
        fn bge_base_en_v15_q_pascal() {
            assert_resolves(Some("BGEBaseENV15Q"), "bge-base-en-v1.5-Q", 768);
        }

        #[test]
        fn bge_base_en_v15_q_kebab() {
            assert_resolves(Some("bge-base-en-v1.5-Q"), "bge-base-en-v1.5-Q", 768);
        }

        // ── Full-precision variants (FP32) ───────────────────────────────

        #[test]
        fn all_minilm_l6_v2_pascal() {
            assert_resolves(Some("AllMiniLML6V2"), "all-MiniLM-L6-v2", 384);
        }

        #[test]
        fn all_minilm_l6_v2_kebab() {
            assert_resolves(Some("all-MiniLM-L6-v2"), "all-MiniLM-L6-v2", 384);
        }

        #[test]
        fn all_minilm_l12_v2_pascal() {
            assert_resolves(Some("AllMiniLML12V2"), "all-MiniLM-L12-v2", 384);
        }

        #[test]
        fn all_minilm_l12_v2_kebab() {
            assert_resolves(Some("all-MiniLM-L12-v2"), "all-MiniLM-L12-v2", 384);
        }

        #[test]
        fn bge_small_en_v15_pascal() {
            assert_resolves(Some("BGESmallENV15"), "bge-small-en-v1.5", 384);
        }

        #[test]
        fn bge_small_en_v15_kebab() {
            assert_resolves(Some("bge-small-en-v1.5"), "bge-small-en-v1.5", 384);
        }

        #[test]
        fn bge_base_en_v15_pascal() {
            assert_resolves(Some("BGEBaseENV15"), "bge-base-en-v1.5", 768);
        }

        #[test]
        fn bge_base_en_v15_kebab() {
            assert_resolves(Some("bge-base-en-v1.5"), "bge-base-en-v1.5", 768);
        }

        // ── Unknown / fallback ───────────────────────────────────────────

        #[test]
        fn unknown_model_falls_back_to_quantized_minilm() {
            assert_resolves(Some("bogus-model"), "all-MiniLM-L6-v2-Q", 384);
        }

        #[test]
        fn empty_string_falls_back_to_quantized_minilm() {
            assert_resolves(Some(""), "all-MiniLM-L6-v2-Q", 384);
        }

        #[test]
        fn case_sensitive_mismatch_falls_back() {
            // "allminilml6v2q" is not a recognised name (lowercase)
            assert_resolves(Some("allminilml6v2q"), "all-MiniLM-L6-v2-Q", 384);
        }

        // ── Dimension grouping ───────────────────────────────────────────

        #[test]
        fn all_384_dim_models() {
            let names_384 = [
                "AllMiniLML6V2Q",
                "all-MiniLM-L6-v2-Q",
                "AllMiniLML12V2Q",
                "all-MiniLM-L12-v2-Q",
                "BGESmallENV15Q",
                "bge-small-en-v1.5-Q",
                "AllMiniLML6V2",
                "all-MiniLM-L6-v2",
                "AllMiniLML12V2",
                "all-MiniLM-L12-v2",
                "BGESmallENV15",
                "bge-small-en-v1.5",
            ];
            for name in names_384 {
                let (_, _, dims) = fastembed_impl::resolve_model(Some(name));
                assert_eq!(dims, 384, "expected 384 dims for {name}");
            }
        }

        #[test]
        fn all_768_dim_models() {
            let names_768 = [
                "BGEBaseENV15Q",
                "bge-base-en-v1.5-Q",
                "BGEBaseENV15",
                "bge-base-en-v1.5",
            ];
            for name in names_768 {
                let (_, _, dims) = fastembed_impl::resolve_model(Some(name));
                assert_eq!(dims, 768, "expected 768 dims for {name}");
            }
        }
    }

    // ── fastembed network-dependent tests (ignored) ──────────────────────

    #[cfg(feature = "fastembed-local")]
    mod fastembed_tests {
        use super::*;

        #[test]
        #[ignore = "requires downloading the fastembed model"]
        fn provider_creates_successfully() {
            // Constructor coverage is kept as an ignored network test because
            // fastembed downloads model assets on first initialization.
            let provider = LocalEmbeddingProvider::new(None).expect("should create provider");
            assert_eq!(provider.dimensions(), 384);
            assert!(!provider.model_id().is_empty());
        }

        /// NOTE: This test downloads the model on first run (~25 MB).
        /// It is marked `#[ignore]` for CI — run with `cargo test -- --ignored`.
        #[tokio::test]
        #[ignore]
        async fn embed_returns_correct_dimensions() {
            let provider = LocalEmbeddingProvider::new(None).unwrap();
            let vec = provider.embed("hello world").await.unwrap();
            assert_eq!(vec.len(), provider.dimensions());
        }

        /// Batch embedding test (also requires model download).
        #[tokio::test]
        #[ignore]
        async fn embed_batch_returns_correct_count() {
            let provider = LocalEmbeddingProvider::new(None).unwrap();
            let texts = vec!["hello".to_owned(), "world".to_owned()];
            let vecs = provider.embed_batch(&texts).await.unwrap();
            assert_eq!(vecs.len(), 2);
            for v in &vecs {
                assert_eq!(v.len(), provider.dimensions());
            }
        }
    }

    // ── Stub provider tests (no fastembed feature) ───────────────────────

    #[cfg(not(feature = "fastembed-local"))]
    mod stub_tests {
        use super::*;
        use oneshim_core::ports::embedding_provider::EmbeddingProvider;

        #[test]
        fn stub_new_succeeds() {
            let provider = LocalEmbeddingProvider::new(None);
            assert!(provider.is_ok(), "stub constructor should always succeed");
        }

        #[test]
        fn stub_new_with_any_model_name_succeeds() {
            // Stub ignores the model_name parameter entirely.
            let provider = LocalEmbeddingProvider::new(Some("AllMiniLML6V2"));
            assert!(provider.is_ok());
        }

        #[test]
        fn stub_dimensions_returns_384() {
            let provider = LocalEmbeddingProvider::new(None).unwrap();
            assert_eq!(provider.dimensions(), 384);
        }

        #[test]
        fn stub_model_id_is_stub_identifier() {
            let provider = LocalEmbeddingProvider::new(None).unwrap();
            assert_eq!(provider.model_id(), "stub-no-fastembed");
        }

        #[tokio::test]
        async fn stub_embed_returns_error() {
            let provider = LocalEmbeddingProvider::new(None).unwrap();
            let result = provider.embed("hello").await;
            assert!(result.is_err());
            let err = result.unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("fastembed-local feature is not enabled"),
                "error should explain the feature is disabled, got: {msg}"
            );
        }

        #[tokio::test]
        async fn stub_embed_batch_returns_error() {
            let provider = LocalEmbeddingProvider::new(None).unwrap();
            let texts = vec!["a".to_owned(), "b".to_owned()];
            let result = provider.embed_batch(&texts).await;
            assert!(result.is_err());
            let msg = result.unwrap_err().to_string();
            assert!(
                msg.contains("fastembed-local feature is not enabled"),
                "batch error should explain the feature is disabled, got: {msg}"
            );
        }

        #[tokio::test]
        async fn stub_embed_empty_text_still_returns_error() {
            let provider = LocalEmbeddingProvider::new(None).unwrap();
            let result = provider.embed("").await;
            assert!(result.is_err(), "stub should error even on empty input");
        }

        #[tokio::test]
        async fn stub_embed_batch_empty_slice_returns_error() {
            let provider = LocalEmbeddingProvider::new(None).unwrap();
            let result = provider.embed_batch(&[]).await;
            assert!(result.is_err(), "stub should error even on empty batch");
        }
    }

    // ── Error type tests (feature-independent) ──────────────────────────

    mod error_tests {
        use super::*;

        #[test]
        fn embedding_error_internal_display() {
            let err = EmbeddingError::Internal("test failure".to_owned());
            assert_eq!(err.to_string(), "internal error: test failure");
        }

        #[test]
        fn embedding_error_from_core_error() {
            let core = CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: "core problem".to_owned(),
            };
            let emb: EmbeddingError = core.into();
            // The transparent variant should preserve the CoreError message.
            assert!(
                emb.to_string().contains("core problem"),
                "should contain original message, got: {}",
                emb
            );
        }

        #[test]
        fn embedding_error_into_core_error_internal() {
            let emb = EmbeddingError::Internal("embed fail".to_owned());
            let core: CoreError = emb.into();
            assert!(matches!(core, CoreError::Internal { .. }));
            assert!(core.to_string().contains("embed fail"));
        }

        #[test]
        fn embedding_error_into_core_error_roundtrip() {
            // CoreError -> EmbeddingError -> CoreError preserves the variant.
            let original = CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: "roundtrip".to_owned(),
            };
            let emb: EmbeddingError = original.into();
            let back: CoreError = emb.into();
            assert!(matches!(back, CoreError::Internal { .. }));
            assert!(back.to_string().contains("roundtrip"));
        }

        #[test]
        fn embedding_error_internal_is_debug_printable() {
            let err = EmbeddingError::Internal("debug check".to_owned());
            let debug = format!("{err:?}");
            assert!(
                debug.contains("Internal"),
                "Debug should contain variant name, got: {debug}"
            );
        }

        #[test]
        fn embedding_error_core_variant_is_debug_printable() {
            let core = CoreError::Network {
                code: oneshim_core::error_codes::NetworkCode::Generic,
                message: "net err".to_owned(),
            };
            let emb: EmbeddingError = core.into();
            let debug = format!("{emb:?}");
            assert!(
                debug.contains("Core"),
                "Debug should contain Core variant, got: {debug}"
            );
        }

        #[test]
        fn core_error_network_converts_to_embedding_error() {
            let core = CoreError::Network {
                code: oneshim_core::error_codes::NetworkCode::Generic,
                message: "timeout".to_owned(),
            };
            let emb: EmbeddingError = core.into();
            // Converting back should preserve as CoreError (via transparent).
            let back: CoreError = emb.into();
            assert!(matches!(back, CoreError::Network { .. }));
        }
    }

    // ── FallbackEmbeddingProvider tests ─────────────────────────────────

    mod fallback_tests {
        use super::*;
        use oneshim_core::ports::embedding_provider::EmbeddingProvider;
        use std::sync::Arc;

        /// Mock provider that always succeeds, returning vectors of a given value.
        struct OkProvider {
            value: f32,
            dims: usize,
        }

        #[async_trait]
        impl EmbeddingProvider for OkProvider {
            async fn embed(&self, _text: &str) -> Result<Vec<f32>, CoreError> {
                Ok(vec![self.value; self.dims])
            }

            async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, CoreError> {
                Ok(texts.iter().map(|_| vec![self.value; self.dims]).collect())
            }

            fn dimensions(&self) -> usize {
                self.dims
            }

            fn model_id(&self) -> &str {
                "ok-mock"
            }
        }

        /// Mock provider that always fails.
        struct ErrProvider;

        #[async_trait]
        impl EmbeddingProvider for ErrProvider {
            async fn embed(&self, _text: &str) -> Result<Vec<f32>, CoreError> {
                Err(CoreError::Internal {
                    code: oneshim_core::error_codes::InternalCode::Generic,
                    message: "mock primary failure".into(),
                })
            }

            async fn embed_batch(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, CoreError> {
                Err(CoreError::Internal {
                    code: oneshim_core::error_codes::InternalCode::Generic,
                    message: "mock primary batch failure".into(),
                })
            }

            fn dimensions(&self) -> usize {
                384
            }

            fn model_id(&self) -> &str {
                "err-mock"
            }
        }

        #[tokio::test]
        async fn test_fallback_primary_succeeds() {
            let primary: Arc<dyn EmbeddingProvider> = Arc::new(OkProvider {
                value: 1.0,
                dims: 4,
            });
            let fallback: Arc<dyn EmbeddingProvider> = Arc::new(OkProvider {
                value: 9.9,
                dims: 4,
            });
            let provider = FallbackEmbeddingProvider::new(primary, fallback);

            let result = provider.embed("hello").await.unwrap();
            // Should get primary's value (1.0), not fallback's (9.9)
            assert_eq!(result, vec![1.0; 4]);

            let batch = provider
                .embed_batch(&["a".to_owned(), "b".to_owned()])
                .await
                .unwrap();
            assert_eq!(batch.len(), 2);
            assert_eq!(batch[0], vec![1.0; 4]);
        }

        #[tokio::test]
        async fn test_fallback_primary_fails() {
            let primary: Arc<dyn EmbeddingProvider> = Arc::new(ErrProvider);
            let fallback: Arc<dyn EmbeddingProvider> = Arc::new(OkProvider {
                value: 2.0,
                dims: 4,
            });
            let provider = FallbackEmbeddingProvider::new(primary, fallback);

            let result = provider.embed("hello").await.unwrap();
            // Primary fails, should get fallback's value (2.0)
            assert_eq!(result, vec![2.0; 4]);

            let batch = provider.embed_batch(&["a".to_owned()]).await.unwrap();
            assert_eq!(batch[0], vec![2.0; 4]);
        }

        #[tokio::test]
        async fn test_fallback_both_fail() {
            let primary: Arc<dyn EmbeddingProvider> = Arc::new(ErrProvider);
            let fallback: Arc<dyn EmbeddingProvider> = Arc::new(ErrProvider);
            let provider = FallbackEmbeddingProvider::new(primary, fallback);

            let result = provider.embed("hello").await;
            assert!(result.is_err());

            let batch_result = provider.embed_batch(&["a".to_owned()]).await;
            assert!(batch_result.is_err());
        }

        // ── Health tracking tests ────────────────────────────────────────

        /// Mock provider whose success/failure can be toggled at runtime.
        struct ToggleProvider {
            should_fail: std::sync::Arc<AtomicBool>,
            value: f32,
            dims: usize,
        }

        #[async_trait]
        impl EmbeddingProvider for ToggleProvider {
            async fn embed(&self, _text: &str) -> Result<Vec<f32>, CoreError> {
                if self.should_fail.load(Ordering::Relaxed) {
                    Err(CoreError::Internal {
                        code: oneshim_core::error_codes::InternalCode::Generic,
                        message: "toggle: failing".into(),
                    })
                } else {
                    Ok(vec![self.value; self.dims])
                }
            }

            async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, CoreError> {
                if self.should_fail.load(Ordering::Relaxed) {
                    Err(CoreError::Internal {
                        code: oneshim_core::error_codes::InternalCode::Generic,
                        message: "toggle: batch failing".into(),
                    })
                } else {
                    Ok(texts.iter().map(|_| vec![self.value; self.dims]).collect())
                }
            }

            fn dimensions(&self) -> usize {
                self.dims
            }

            fn model_id(&self) -> &str {
                "toggle-mock"
            }
        }

        #[tokio::test]
        async fn health_starts_true() {
            let primary: Arc<dyn EmbeddingProvider> = Arc::new(OkProvider {
                value: 1.0,
                dims: 4,
            });
            let fallback: Arc<dyn EmbeddingProvider> = Arc::new(OkProvider {
                value: 2.0,
                dims: 4,
            });
            let provider = FallbackEmbeddingProvider::new(primary, fallback);
            assert!(provider.is_primary_healthy());
        }

        #[tokio::test]
        async fn health_false_after_primary_failure() {
            let primary: Arc<dyn EmbeddingProvider> = Arc::new(ErrProvider);
            let fallback: Arc<dyn EmbeddingProvider> = Arc::new(OkProvider {
                value: 2.0,
                dims: 4,
            });
            let provider = FallbackEmbeddingProvider::new(primary, fallback);

            let _ = provider.embed("hello").await;
            assert!(!provider.is_primary_healthy());
        }

        #[tokio::test]
        async fn health_recovers_after_primary_succeeds_again() {
            let should_fail = std::sync::Arc::new(AtomicBool::new(true));
            let primary: Arc<dyn EmbeddingProvider> = Arc::new(ToggleProvider {
                should_fail: Arc::clone(&should_fail),
                value: 1.0,
                dims: 4,
            });
            let fallback: Arc<dyn EmbeddingProvider> = Arc::new(OkProvider {
                value: 9.0,
                dims: 4,
            });
            let provider = FallbackEmbeddingProvider::new(primary, fallback);

            // Primary fails — health should be false.
            let result = provider.embed("first").await.unwrap();
            assert_eq!(result, vec![9.0; 4], "should use fallback value");
            assert!(!provider.is_primary_healthy());

            // Primary recovers — health should flip back to true.
            should_fail.store(false, Ordering::Relaxed);
            let result = provider.embed("second").await.unwrap();
            assert_eq!(result, vec![1.0; 4], "should use primary value");
            assert!(provider.is_primary_healthy());
        }

        #[tokio::test]
        async fn health_tracks_batch_calls() {
            let should_fail = std::sync::Arc::new(AtomicBool::new(false));
            let primary: Arc<dyn EmbeddingProvider> = Arc::new(ToggleProvider {
                should_fail: Arc::clone(&should_fail),
                value: 3.0,
                dims: 2,
            });
            let fallback: Arc<dyn EmbeddingProvider> = Arc::new(OkProvider {
                value: 7.0,
                dims: 2,
            });
            let provider = FallbackEmbeddingProvider::new(primary, fallback);

            // Batch succeeds — healthy.
            let _ = provider.embed_batch(&["a".to_owned()]).await.unwrap();
            assert!(provider.is_primary_healthy());

            // Batch fails — unhealthy.
            should_fail.store(true, Ordering::Relaxed);
            let batch = provider.embed_batch(&["b".to_owned()]).await.unwrap();
            assert_eq!(batch[0], vec![7.0; 2], "should use fallback");
            assert!(!provider.is_primary_healthy());
        }
    }
}
