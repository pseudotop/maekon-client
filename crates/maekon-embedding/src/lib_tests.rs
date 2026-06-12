// ── resolve_model parameterized tests (fastembed feature) ─────────────────

#[cfg(feature = "fastembed-local")]
mod resolve_model_tests {
    use crate::embedder::fastembed_impl;

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

    // ── Default (None) ─────────────────────────────────────────────────────

    #[test]
    fn none_defaults_to_quantized_minilm() {
        assert_resolves(None, "all-MiniLM-L6-v2-Q", 384);
    }

    // ── Quantized variants (INT8) ──────────────────────────────────────────

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

    // ── Full-precision variants (FP32) ─────────────────────────────────────

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

    // ── Unknown / fallback ─────────────────────────────────────────────────

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

    // ── Dimension grouping ─────────────────────────────────────────────────

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

// ── fastembed network-dependent tests (ignored) ────────────────────────────

#[cfg(feature = "fastembed-local")]
mod fastembed_tests {
    use maekon_core::ports::embedding_provider::EmbeddingProvider;

    use crate::LocalEmbeddingProvider;

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

// ── Stub provider tests (no fastembed feature) ────────────────────────────

#[cfg(not(feature = "fastembed-local"))]
mod stub_tests {
    use maekon_core::ports::embedding_provider::EmbeddingProvider;

    use crate::LocalEmbeddingProvider;

    #[test]
    fn stub_new_succeeds() {
        // Stub constructor must succeed and expose the canonical 384-dimension
        // identity (matching the fastembed-absent contract pinned in sibling tests).
        let provider =
            LocalEmbeddingProvider::new(None).expect("stub constructor should always succeed");
        assert_eq!(
            provider.dimensions(),
            384,
            "stub must advertise 384 dimensions"
        );
    }

    #[test]
    fn stub_new_with_any_model_name_succeeds() {
        // Stub ignores the model_name parameter entirely — pin that claim by
        // comparing against the `None` construction (#5594).
        let named = LocalEmbeddingProvider::new(Some("AllMiniLML6V2"))
            .expect("stub constructor must accept any model name");
        let default = LocalEmbeddingProvider::new(None).expect("stub default constructor");
        assert_eq!(named.dimensions(), default.dimensions());
        assert_eq!(named.model_id(), default.model_id());
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
        let msg = provider.embed("hello").await.unwrap_err().to_string();
        assert!(
            msg.contains("fastembed-local feature is not enabled"),
            "error should explain the feature is disabled, got: {msg}"
        );
    }

    #[tokio::test]
    async fn stub_embed_batch_returns_error() {
        let provider = LocalEmbeddingProvider::new(None).unwrap();
        let texts = vec!["a".to_owned(), "b".to_owned()];
        let msg = provider.embed_batch(&texts).await.unwrap_err().to_string();
        assert!(
            msg.contains("fastembed-local feature is not enabled"),
            "batch error should explain the feature is disabled, got: {msg}"
        );
    }

    #[tokio::test]
    async fn stub_embed_empty_text_still_returns_error() {
        let provider = LocalEmbeddingProvider::new(None).unwrap();
        let msg = provider.embed("").await.unwrap_err().to_string();
        assert!(
            msg.contains("fastembed-local feature is not enabled"),
            "stub should error even on empty input, got: {msg}"
        );
    }

    #[tokio::test]
    async fn stub_embed_batch_empty_slice_returns_error() {
        let provider = LocalEmbeddingProvider::new(None).unwrap();
        let msg = provider.embed_batch(&[]).await.unwrap_err().to_string();
        assert!(
            msg.contains("fastembed-local feature is not enabled"),
            "stub should error even on empty batch, got: {msg}"
        );
    }
}

// ── Error type tests (feature-independent) ────────────────────────────────

mod error_tests {
    use maekon_core::error::CoreError;

    use crate::error::EmbeddingError;

    #[test]
    fn embedding_error_internal_display() {
        let err = EmbeddingError::Internal("test failure".to_owned());
        assert_eq!(err.to_string(), "internal error: test failure");
    }

    #[test]
    fn embedding_error_from_core_error() {
        let core = CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
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
            code: maekon_core::error_codes::InternalCode::Generic,
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
            code: maekon_core::error_codes::NetworkCode::Generic,
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
            code: maekon_core::error_codes::NetworkCode::Generic,
            message: "timeout".to_owned(),
        };
        let emb: EmbeddingError = core.into();
        // Converting back should preserve as CoreError (via transparent).
        let back: CoreError = emb.into();
        assert!(matches!(back, CoreError::Network { .. }));
    }
}

// ── FallbackEmbeddingProvider tests ───────────────────────────────────────

mod fallback_tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;
    use maekon_core::error::CoreError;
    use maekon_core::ports::embedding_provider::EmbeddingProvider;

    use crate::fallback::FallbackEmbeddingProvider;

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
                code: maekon_core::error_codes::InternalCode::Generic,
                message: "mock primary failure".into(),
            })
        }

        async fn embed_batch(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, CoreError> {
            Err(CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
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

        assert!(
            matches!(
                provider.embed("hello").await.unwrap_err(),
                CoreError::Internal { .. }
            ),
            "both-fail embed must yield CoreError::Internal"
        );
        assert!(
            matches!(
                provider.embed_batch(&["a".to_owned()]).await.unwrap_err(),
                CoreError::Internal { .. }
            ),
            "both-fail embed_batch must yield CoreError::Internal"
        );
    }

    // ── Health tracking tests ──────────────────────────────────────────────

    /// Mock provider whose success/failure can be toggled at runtime.
    struct ToggleProvider {
        should_fail: Arc<AtomicBool>,
        value: f32,
        dims: usize,
    }

    #[async_trait]
    impl EmbeddingProvider for ToggleProvider {
        async fn embed(&self, _text: &str) -> Result<Vec<f32>, CoreError> {
            if self.should_fail.load(Ordering::Relaxed) {
                Err(CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: "toggle: failing".into(),
                })
            } else {
                Ok(vec![self.value; self.dims])
            }
        }

        async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, CoreError> {
            if self.should_fail.load(Ordering::Relaxed) {
                Err(CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
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
        let should_fail = Arc::new(AtomicBool::new(true));
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
        let should_fail = Arc::new(AtomicBool::new(false));
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
