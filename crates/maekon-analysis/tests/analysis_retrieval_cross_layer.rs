//! Cross-layer regression tests spanning the real `maekon-storage` read paths
//! and the `maekon-analysis` retrieval components.
//!
//! These guard two silent-failure contract mismatches:
//!   - #7478: storage emits UPPERCASE feedback_type while the few-shot selector
//!     filtered lowercase, so `select()` always returned `[]`.
//!   - #7479: with INT8 quantization disabled (the default) vectors are stored
//!     f32-only and the INT8 `search_quantized` scan returns nothing, so the
//!     adaptive-search coordinator returned an empty RAG result.
//!
//! `maekon-storage` is a dev-dependency here (it depends only on `maekon-core`,
//! so this dev-only edge introduces no dependency cycle).

use std::sync::Arc;

use maekon_analysis::{AdaptiveSearchCoordinator, FewShotOutcome, FewShotSelector, SearchConfig};
use maekon_core::models::embedding::{EmbeddingContentType, EmbeddingMetadata, SearchFilters};
use maekon_core::models::suggestion::{Priority, Suggestion, SuggestionSource, SuggestionType};
use maekon_core::ports::few_shot_storage::FewShotStorage;
use maekon_core::ports::vector_index::VectorIndex;
use maekon_core::ports::vector_store::VectorStore;
use maekon_core::quantization::ScalarQuantizer;
use maekon_storage::sqlite::vector_index_impl::SqliteVectorIndex;
use maekon_storage::sqlite::vector_store_impl::SqliteVectorStore;
use maekon_storage::sqlite::SqliteStorage;

fn rule_suggestion(id: &str, content: &str) -> Suggestion {
    Suggestion {
        suggestion_id: id.to_string(),
        suggestion_type: SuggestionType::WorkGuidance,
        content: content.to_string(),
        priority: Priority::Medium,
        confidence_score: 0.8,
        relevance_score: 0.9,
        is_actionable: true,
        created_at: chrono::Utc::now(),
        expires_at: None,
        source: SuggestionSource::LlmLocal,
        reasoning: None,
        context_scope: None,
    }
}

/// #7478: feedback recorded through the real storage accept/reject path is
/// emitted UPPERCASE ("ACCEPTED"/"REJECTED") by `get_suggestions_with_feedback`.
/// The few-shot selector must pick those up; before the fix its case-sensitive
/// `== "accepted"` filter dropped every row and `select()` returned `[]`.
#[test]
fn few_shot_selector_picks_up_uppercase_storage_feedback() {
    let storage = SqliteStorage::open_in_memory(30).expect("open in-memory storage");

    storage
        .save_rule_suggestion_sync(&rule_suggestion("xl-accepted", "Take a break"))
        .expect("save accepted suggestion");
    storage
        .save_rule_suggestion_sync(&rule_suggestion("xl-rejected", "Ignore notifications"))
        .expect("save rejected suggestion");

    // Real accept/reject path — writes acted_at / dismissed_at, which the read
    // query maps to UPPERCASE feedback_type.
    assert!(
        storage
            .mark_unified_suggestion_acted("xl-accepted")
            .expect("mark acted"),
        "accept must update a row"
    );
    assert!(
        storage
            .dismiss_unified_suggestion("xl-rejected")
            .expect("dismiss"),
        "reject must update a row"
    );

    let history =
        FewShotStorage::get_suggestions_with_feedback(&storage, 10).expect("read feedback history");

    // Guard the premise: the storage read path really emits UPPERCASE.
    assert!(
        history
            .iter()
            .any(|h| h.feedback_type == "ACCEPTED" || h.feedback_type == "REJECTED"),
        "storage read path is expected to emit UPPERCASE feedback_type; got: {:?}",
        history.iter().map(|h| &h.feedback_type).collect::<Vec<_>>()
    );

    let selector = FewShotSelector::new(3);
    let examples = selector.select(&history, None);

    assert!(
        !examples.is_empty(),
        "selector must pick up UPPERCASE storage feedback (was [] before #7478 fix)"
    );
    // An accepted example is always placed first when present.
    assert_eq!(examples[0].outcome, FewShotOutcome::Accepted);
    assert!(
        examples
            .iter()
            .any(|e| e.outcome == FewShotOutcome::Rejected),
        "the rejected example from the storage path must also be selected"
    );
}

fn f32_metadata(segment_id: &str) -> EmbeddingMetadata {
    EmbeddingMetadata {
        segment_id: segment_id.to_string(),
        content_type: EmbeddingContentType::ContentActivity,
        content_label: Some(segment_id.to_string()),
        timestamp: chrono::Utc::now(),
        original_text: format!("text for {segment_id}"),
        model_id: "test-model".to_string(),
    }
}

/// #7479: with INT8 quantization disabled (the default) vectors are stored
/// f32-only, so `SqliteVectorStore::search_quantized` (which filters
/// `vector_int8 IS NOT NULL`) returns nothing. The adaptive-search coordinator
/// must fall back to the f32 `search_filtered` path so retrieval returns
/// results; before the fix it returned an empty RAG set.
#[tokio::test]
async fn adaptive_search_returns_f32_only_vectors_when_quantization_disabled() {
    let storage = SqliteStorage::open_in_memory(30).expect("open in-memory storage");
    let conn = storage.connection_arc();

    let store: Arc<dyn VectorStore> = Arc::new(SqliteVectorStore::new(conn.clone()));
    let index: Arc<dyn VectorIndex> = Arc::new(SqliteVectorIndex::new(conn.clone()));

    // Store f32-only vectors (quantization disabled → vector_int8 stays NULL).
    store
        .store(vec![1.0, 0.0, 0.0], f32_metadata("seg-a"))
        .await
        .expect("store seg-a");
    store
        .store(vec![0.9, 0.1, 0.0], f32_metadata("seg-b"))
        .await
        .expect("store seg-b");
    store
        .store(vec![0.0, 1.0, 0.0], f32_metadata("seg-c"))
        .await
        .expect("store seg-c");

    let query = [1.0f32, 0.0, 0.0];
    let filters = SearchFilters::default();

    // Premise guard: the INT8 scan is genuinely empty for f32-only vectors, so a
    // coordinator that used it (pre-fix) would have returned nothing.
    let quantized = ScalarQuantizer::quantize(&query).expect("quantize query");
    let int8_results = store
        .search_quantized(&quantized, 5, 168.0, &filters)
        .await
        .expect("int8 search");
    assert!(
        int8_results.is_empty(),
        "INT8 scan must be empty for f32-only vectors (proves the pre-fix empty result)"
    );

    // quantization_enabled defaults to false.
    let coordinator = AdaptiveSearchCoordinator::new(store, index, SearchConfig::default());
    coordinator.refresh_count().await.expect("refresh count");

    let results = coordinator
        .search(&query, 5, 168.0, &filters)
        .await
        .expect("adaptive search must succeed");

    assert!(
        !results.is_empty(),
        "coordinator must return f32 vectors via the f32 path when quantization is disabled \
         (was empty before #7479 fix)"
    );
    // The nearest vector to the query is seg-a.
    assert_eq!(results[0].segment_id, "seg-a");
}
