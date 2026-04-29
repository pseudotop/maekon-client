use std::sync::Arc;

use crate::error::AnalysisError;
use chrono::{DateTime, Utc};
use oneshim_core::models::embedding::{EmbeddingContentType, EmbeddingMetadata};
use oneshim_core::models::tiered_memory::SegmentSummary;
#[cfg(feature = "hnsw")]
use oneshim_core::ports::ann_index::AnnIndex;
use oneshim_core::ports::embedding_provider::EmbeddingProvider;
use oneshim_core::ports::vector_store::VectorStore;
use oneshim_core::quantization::ScalarQuantizer;
#[cfg(feature = "hnsw")]
use tracing::warn;

use crate::PiiFilter;

/// Two-phase embedding pipeline for segment content.
///
/// - **Phase 1** (`process_content_activities`): immediately embed content activity
///   labels on segment close.
/// - **Phase 2** (`process_llm_summary`): embed the LLM-generated segment summary
///   after the async LLM call completes.
pub struct EmbeddingPipeline {
    embedding_provider: Arc<dyn EmbeddingProvider>,
    pii_filter: PiiFilter,
    vector_store: Arc<dyn VectorStore>,
    quantization_enabled: bool,
    /// When true, skip writing the float32 BLOB on quantized inserts.
    /// Derived from `!config.quantization_float32_retention`.
    skip_float32: bool,
    /// Optional HNSW ANN index. When present, newly stored vectors are
    /// also inserted into the in-memory index for fast approximate search.
    #[cfg(feature = "hnsw")]
    ann_index: Option<Arc<dyn AnnIndex>>,
}

impl EmbeddingPipeline {
    pub fn new(
        provider: Arc<dyn EmbeddingProvider>,
        pii_filter: PiiFilter,
        store: Arc<dyn VectorStore>,
        quantization_enabled: bool,
    ) -> Self {
        Self {
            embedding_provider: provider,
            pii_filter,
            vector_store: store,
            quantization_enabled,
            skip_float32: false,
            #[cfg(feature = "hnsw")]
            ann_index: None,
        }
    }

    /// Create a pipeline with explicit float32 retention control.
    ///
    /// `skip_float32`: when `true` AND `quantization_enabled` is `true`,
    /// the f32 BLOB column is set to NULL on quantized inserts to save storage.
    pub fn with_float32_retention(
        provider: Arc<dyn EmbeddingProvider>,
        pii_filter: PiiFilter,
        store: Arc<dyn VectorStore>,
        quantization_enabled: bool,
        skip_float32: bool,
    ) -> Self {
        Self {
            embedding_provider: provider,
            pii_filter,
            vector_store: store,
            quantization_enabled,
            skip_float32,
            #[cfg(feature = "hnsw")]
            ann_index: None,
        }
    }

    /// Attach an HNSW ANN index so newly stored vectors are also added to the
    /// in-memory index. Best-effort: failures are logged but do not fail the store.
    #[cfg(feature = "hnsw")]
    pub fn with_ann_index(mut self, ann: Arc<dyn AnnIndex>) -> Self {
        self.ann_index = Some(ann);
        self
    }

    /// Phase 1: embed content activities immediately on segment close.
    /// Returns the number of vectors stored.
    pub async fn process_content_activities(
        &self,
        segment: &SegmentSummary,
    ) -> Result<usize, AnalysisError> {
        let mut texts = Vec::new();
        let mut metadata = Vec::new();

        let model_id = self.embedding_provider.model_id().to_string();

        for activity in &segment.content_activities {
            let text = format!(
                "{} ({:?}) - {:?}",
                (self.pii_filter)(&activity.content_label),
                activity.content_type,
                activity.work_type
            );
            metadata.push(EmbeddingMetadata {
                segment_id: segment.segment_id.clone(),
                content_type: EmbeddingContentType::ContentActivity,
                content_label: Some((self.pii_filter)(&activity.content_label)),
                timestamp: activity.start_time,
                original_text: text.clone(),
                model_id: model_id.clone(),
            });
            texts.push(text);
        }

        if texts.is_empty() {
            return Ok(0);
        }

        let vectors = self.embedding_provider.embed_batch(&texts).await?;
        let count = vectors.len();

        for (vector, meta) in vectors.into_iter().zip(metadata) {
            #[cfg(feature = "hnsw")]
            let vec_for_hnsw = vector.clone();
            if self.quantization_enabled {
                let quantized = ScalarQuantizer::quantize(&vector)?;
                self.vector_store
                    .store_quantized(vector, &quantized, meta, self.skip_float32)
                    .await?;
            } else {
                self.vector_store.store(vector, meta).await?;
            }
            // Best-effort: add the new vector to HNSW index if present.
            #[cfg(feature = "hnsw")]
            self.try_add_to_hnsw(&vec_for_hnsw).await;
        }

        Ok(count)
    }

    /// Phase 2: embed LLM summary after async completion.
    pub async fn process_llm_summary(
        &self,
        segment_id: &str,
        summary: &str,
        timestamp: DateTime<Utc>,
    ) -> Result<(), AnalysisError> {
        let filtered = (self.pii_filter)(summary);
        let vector = self.embedding_provider.embed(&filtered).await?;

        let metadata = EmbeddingMetadata {
            segment_id: segment_id.to_string(),
            content_type: EmbeddingContentType::SegmentSummary,
            content_label: None,
            timestamp,
            original_text: filtered,
            model_id: self.embedding_provider.model_id().to_string(),
        };

        #[cfg(feature = "hnsw")]
        let vec_for_hnsw = vector.clone();
        if self.quantization_enabled {
            let quantized = ScalarQuantizer::quantize(&vector)?;
            self.vector_store
                .store_quantized(vector, &quantized, metadata, self.skip_float32)
                .await?;
        } else {
            self.vector_store.store(vector, metadata).await?;
        }

        // Best-effort: add the new vector to HNSW index if present.
        #[cfg(feature = "hnsw")]
        self.try_add_to_hnsw(&vec_for_hnsw).await;

        Ok(())
    }

    /// Best-effort helper: insert a vector into the HNSW index using the
    /// last-inserted row ID from the vector store. Failures are logged
    /// but never propagated.
    #[cfg(feature = "hnsw")]
    async fn try_add_to_hnsw(&self, vector: &[f32]) {
        let ann = match self.ann_index {
            Some(ref a) => a,
            None => return,
        };
        match self.vector_store.last_insert_id().await {
            Ok(key) if key > 0 => {
                if let Err(e) = ann.add(key, vector).await {
                    warn!("HNSW add failed (best-effort): {e}");
                }
            }
            Ok(_) => {} // key 0 = not implemented, skip silently
            Err(e) => {
                warn!("last_insert_id failed for HNSW sync: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use oneshim_core::error::CoreError;
    use oneshim_core::models::embedding::{SearchFilters, SearchResult};
    use oneshim_core::models::tiered_memory::{
        ContentActivity, ContentType, EngagementMetrics, TriggerReason, WorkType,
    };
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Mock EmbeddingProvider that returns a fixed-dimension vector.
    struct MockEmbeddingProvider {
        dims: usize,
    }

    #[async_trait]
    impl EmbeddingProvider for MockEmbeddingProvider {
        async fn embed(&self, _text: &str) -> Result<Vec<f32>, CoreError> {
            Ok(vec![0.5; self.dims])
        }

        async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, CoreError> {
            Ok(texts.iter().map(|_| vec![0.5; self.dims]).collect())
        }

        fn dimensions(&self) -> usize {
            self.dims
        }

        fn model_id(&self) -> &str {
            "mock-model"
        }
    }

    /// Mock VectorStore that records stored vectors and skip_float32 flags.
    struct MockVectorStore {
        stored: Mutex<Vec<(Vec<f32>, EmbeddingMetadata)>>,
        stored_quantized: Mutex<Vec<(Vec<f32>, EmbeddingMetadata)>>,
        skip_float32_flags: Mutex<Vec<bool>>,
    }

    impl MockVectorStore {
        fn new() -> Self {
            Self {
                stored: Mutex::new(Vec::new()),
                stored_quantized: Mutex::new(Vec::new()),
                skip_float32_flags: Mutex::new(Vec::new()),
            }
        }

        fn stored_count(&self) -> usize {
            self.stored.lock().unwrap().len()
        }

        fn stored_quantized_count(&self) -> usize {
            self.stored_quantized.lock().unwrap().len()
        }

        fn skip_float32_flags(&self) -> Vec<bool> {
            self.skip_float32_flags.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl VectorStore for MockVectorStore {
        async fn store(
            &self,
            vector: Vec<f32>,
            metadata: EmbeddingMetadata,
        ) -> Result<(), CoreError> {
            self.stored.lock().unwrap().push((vector, metadata));
            Ok(())
        }

        async fn search(
            &self,
            _query_vector: &[f32],
            _limit: usize,
            _time_decay_hours: f32,
        ) -> Result<Vec<SearchResult>, CoreError> {
            Ok(vec![])
        }

        async fn search_filtered(
            &self,
            _query_vector: &[f32],
            _limit: usize,
            _time_decay_hours: f32,
            _filters: &SearchFilters,
        ) -> Result<Vec<SearchResult>, CoreError> {
            Ok(vec![])
        }

        async fn enforce_retention(&self, _max_days: u32) -> Result<u64, CoreError> {
            Ok(0)
        }

        async fn mark_stale(&self, _old_model_id: &str) -> Result<u64, CoreError> {
            Ok(0)
        }

        async fn get_current_model_id(&self) -> Result<Option<String>, CoreError> {
            Ok(None)
        }

        async fn get_stale_vectors(&self, _limit: usize) -> Result<Vec<(i64, String)>, CoreError> {
            Ok(vec![])
        }

        async fn update_vector(
            &self,
            _id: i64,
            _vector: Vec<f32>,
            _model_id: &str,
        ) -> Result<(), CoreError> {
            Ok(())
        }

        async fn store_quantized(
            &self,
            vector_f32: Vec<f32>,
            _vector_int8: &oneshim_core::quantization::QuantizedVector,
            metadata: EmbeddingMetadata,
            skip_float32: bool,
        ) -> Result<(), CoreError> {
            self.stored_quantized
                .lock()
                .unwrap()
                .push((vector_f32, metadata));
            self.skip_float32_flags.lock().unwrap().push(skip_float32);
            Ok(())
        }
    }

    fn identity_filter() -> PiiFilter {
        Box::new(|s: &str| s.to_string())
    }

    fn make_segment_with_activities(activities: Vec<ContentActivity>) -> SegmentSummary {
        SegmentSummary {
            segment_id: "seg-embed-001".to_string(),
            start_time: Utc::now(),
            end_time: Utc::now(),
            duration_secs: 1800,
            regime_id: None,
            trigger_reason: TriggerReason::ForcedMaxDuration,
            event_count: 50,
            app_breakdown: HashMap::new(),
            category_breakdown: HashMap::new(),
            context_switch_count: 2,
            dominant_category: "Development".to_string(),
            avg_importance: 0.7,
            patterns_detected: vec![],
            content_activities: activities,
            container: None,
            llm_summary: None,
        }
    }

    fn make_activity(label: &str) -> ContentActivity {
        ContentActivity {
            content_label: label.to_string(),
            content_type: ContentType::File,
            start_time: Utc::now(),
            duration_secs: 600,
            confidence: 0.9,
            work_type: WorkType::ActiveCoding,
            engagement: EngagementMetrics::default(),
            gui_summary: None,
        }
    }

    #[tokio::test]
    async fn content_activities_embedded() {
        let provider = Arc::new(MockEmbeddingProvider { dims: 3 });
        let store = Arc::new(MockVectorStore::new());
        let pipeline = EmbeddingPipeline::new(provider, identity_filter(), store.clone(), false);

        let segment =
            make_segment_with_activities(vec![make_activity("main.rs"), make_activity("lib.rs")]);

        let count = pipeline.process_content_activities(&segment).await.unwrap();
        assert_eq!(count, 2);
        assert_eq!(store.stored_count(), 2);

        // Verify metadata
        let stored = store.stored.lock().unwrap();
        assert_eq!(
            stored[0].1.content_type,
            EmbeddingContentType::ContentActivity
        );
        assert_eq!(stored[0].1.segment_id, "seg-embed-001");
    }

    #[tokio::test]
    async fn empty_segment_returns_zero() {
        let provider = Arc::new(MockEmbeddingProvider { dims: 3 });
        let store = Arc::new(MockVectorStore::new());
        let pipeline = EmbeddingPipeline::new(provider, identity_filter(), store.clone(), false);

        let segment = make_segment_with_activities(vec![]);
        let count = pipeline.process_content_activities(&segment).await.unwrap();
        assert_eq!(count, 0);
        assert_eq!(store.stored_count(), 0);
    }

    #[tokio::test]
    async fn llm_summary_embedded() {
        let provider = Arc::new(MockEmbeddingProvider { dims: 3 });
        let store = Arc::new(MockVectorStore::new());
        let pipeline = EmbeddingPipeline::new(provider, identity_filter(), store.clone(), false);

        pipeline
            .process_llm_summary(
                "seg-001",
                "Focused coding session on auth module",
                Utc::now(),
            )
            .await
            .unwrap();

        assert_eq!(store.stored_count(), 1);
        let stored = store.stored.lock().unwrap();
        assert_eq!(
            stored[0].1.content_type,
            EmbeddingContentType::SegmentSummary
        );
        assert_eq!(stored[0].1.segment_id, "seg-001");
        assert!(stored[0].1.content_label.is_none());
    }

    #[tokio::test]
    async fn pii_filter_applied_to_content_label() {
        let provider = Arc::new(MockEmbeddingProvider { dims: 3 });
        let store = Arc::new(MockVectorStore::new());
        let filter: PiiFilter = Box::new(|_s: &str| "[REDACTED]".to_string());
        let pipeline = EmbeddingPipeline::new(provider, filter, store.clone(), false);

        let segment = make_segment_with_activities(vec![make_activity("sensitive-file.rs")]);
        pipeline.process_content_activities(&segment).await.unwrap();

        let stored = store.stored.lock().unwrap();
        assert_eq!(stored.len(), 1);
        // content_label must be PII-filtered, not raw
        assert_eq!(stored[0].1.content_label.as_deref(), Some("[REDACTED]"));
    }

    #[tokio::test]
    async fn pii_filter_applied_to_summary() {
        let provider = Arc::new(MockEmbeddingProvider { dims: 3 });
        let store = Arc::new(MockVectorStore::new());
        // PII filter that replaces all text with "[REDACTED]"
        let filter: PiiFilter = Box::new(|_s: &str| "[REDACTED]".to_string());
        let pipeline = EmbeddingPipeline::new(provider, filter, store.clone(), false);

        // Should not error — the filter is applied before embedding
        pipeline
            .process_llm_summary("seg-002", "sensitive info here", Utc::now())
            .await
            .unwrap();

        assert_eq!(store.stored_count(), 1);
    }

    #[tokio::test]
    async fn quantization_enabled_uses_store_quantized() {
        let provider = Arc::new(MockEmbeddingProvider { dims: 5 });
        let store = Arc::new(MockVectorStore::new());
        let pipeline = EmbeddingPipeline::new(provider, identity_filter(), store.clone(), true);

        let segment =
            make_segment_with_activities(vec![make_activity("main.rs"), make_activity("lib.rs")]);

        let count = pipeline.process_content_activities(&segment).await.unwrap();
        assert_eq!(count, 2);
        // store() should NOT be called
        assert_eq!(store.stored_count(), 0);
        // store_quantized() should be called
        assert_eq!(store.stored_quantized_count(), 2);
    }

    #[tokio::test]
    async fn quantization_enabled_llm_summary_uses_store_quantized() {
        let provider = Arc::new(MockEmbeddingProvider { dims: 5 });
        let store = Arc::new(MockVectorStore::new());
        let pipeline = EmbeddingPipeline::new(provider, identity_filter(), store.clone(), true);

        pipeline
            .process_llm_summary("seg-001", "Focused work on auth module", Utc::now())
            .await
            .unwrap();

        assert_eq!(store.stored_count(), 0);
        assert_eq!(store.stored_quantized_count(), 1);
    }

    #[tokio::test]
    async fn with_float32_retention_passes_skip_flag() {
        let provider = Arc::new(MockEmbeddingProvider { dims: 5 });
        let store = Arc::new(MockVectorStore::new());
        let pipeline = EmbeddingPipeline::with_float32_retention(
            provider,
            identity_filter(),
            store.clone(),
            true, // quantization_enabled
            true, // skip_float32
        );

        let segment =
            make_segment_with_activities(vec![make_activity("main.rs"), make_activity("lib.rs")]);

        let count = pipeline.process_content_activities(&segment).await.unwrap();
        assert_eq!(count, 2);
        assert_eq!(store.stored_quantized_count(), 2);

        // Verify skip_float32 was true for all calls
        let flags = store.skip_float32_flags();
        assert_eq!(flags, vec![true, true]);
    }

    #[tokio::test]
    async fn with_float32_retention_false_does_not_skip() {
        let provider = Arc::new(MockEmbeddingProvider { dims: 5 });
        let store = Arc::new(MockVectorStore::new());
        let pipeline = EmbeddingPipeline::with_float32_retention(
            provider,
            identity_filter(),
            store.clone(),
            true,  // quantization_enabled
            false, // skip_float32 = false (retain f32)
        );

        let segment = make_segment_with_activities(vec![make_activity("main.rs")]);
        pipeline.process_content_activities(&segment).await.unwrap();

        let flags = store.skip_float32_flags();
        assert_eq!(flags, vec![false]);
    }

    #[tokio::test]
    async fn new_constructor_defaults_skip_float32_false() {
        let provider = Arc::new(MockEmbeddingProvider { dims: 5 });
        let store = Arc::new(MockVectorStore::new());
        // Using the original `new()` constructor — skip_float32 should default to false
        let pipeline = EmbeddingPipeline::new(provider, identity_filter(), store.clone(), true);

        let segment = make_segment_with_activities(vec![make_activity("main.rs")]);
        pipeline.process_content_activities(&segment).await.unwrap();

        let flags = store.skip_float32_flags();
        assert_eq!(flags, vec![false]);
    }
}
