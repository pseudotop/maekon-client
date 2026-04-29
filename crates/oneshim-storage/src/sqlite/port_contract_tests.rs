//! Port contract tests — verify SqliteStorage adapters comply with
//! oneshim-core port trait contracts (error types, boundary conditions, invariants).

use chrono::{Duration, Utc};
use oneshim_core::error::CoreError;
use oneshim_core::models::embedding::{EmbeddingContentType, EmbeddingMetadata};
use oneshim_core::models::event::{ContextEvent, Event};
use oneshim_core::models::system::SystemMetrics;
use oneshim_core::models::work_session::AppCategory;
use oneshim_core::ports::storage::{MetricsStorage, StorageService};
use oneshim_core::ports::text_search::TextSearchProvider;
use oneshim_core::ports::vector_index::VectorIndex;
use oneshim_core::ports::vector_store::VectorStore;
use oneshim_core::quantization::{QuantizedVector, ScalarQuantizer};

use super::vector_index_impl::SqliteVectorIndex;
use super::vector_store_impl::SqliteVectorStore;
use super::SqliteStorage;

fn storage() -> SqliteStorage {
    SqliteStorage::open_in_memory(30).expect("in-memory storage")
}

fn make_metadata(segment_id: &str) -> EmbeddingMetadata {
    EmbeddingMetadata {
        segment_id: segment_id.to_string(),
        content_type: EmbeddingContentType::ContentActivity,
        content_label: Some("test".to_string()),
        timestamp: Utc::now(),
        original_text: "test content".to_string(),
        model_id: "test-model".to_string(),
    }
}

fn make_event() -> Event {
    Event::Context(ContextEvent {
        app_name: "TestApp".to_string(),
        window_title: "Test Window".to_string(),
        prev_app_name: Some("PrevApp".to_string()),
        timestamp: Utc::now(),
        ..Default::default()
    })
}

fn make_metrics() -> SystemMetrics {
    SystemMetrics {
        timestamp: Utc::now(),
        cpu_usage: 42.0,
        memory_used: 8_000_000_000,
        memory_total: 16_000_000_000,
        disk_used: 100_000_000_000,
        disk_total: 500_000_000_000,
        network: None,
        typing_wpm: 0.0,
    }
}

// ── 3a. VectorStore (6 tests) ──────────────────────────────────

#[tokio::test]
async fn vs_store_empty_vector_returns_invalid_args() {
    let s = storage();
    let store = SqliteVectorStore::new(s.connection_arc());

    let result = store.store(vec![], make_metadata("empty")).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, CoreError::InvalidArguments { .. }),
        "expected InvalidArguments, got: {err}"
    );
}

#[tokio::test]
async fn vs_store_and_search_roundtrip() {
    let s = storage();
    let store = SqliteVectorStore::new(s.connection_arc());

    store
        .store(vec![1.0, 0.0, 0.0], make_metadata("rt-seg"))
        .await
        .unwrap();

    let results = store.search(&[1.0, 0.0, 0.0], 10, 24.0).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].segment_id, "rt-seg");
}

#[tokio::test]
async fn vs_search_empty_store_returns_empty() {
    let s = storage();
    let store = SqliteVectorStore::new(s.connection_arc());

    let results = store.search(&[1.0, 0.0], 10, 24.0).await.unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn vs_enforce_retention_returns_count() {
    let s = storage();
    let store = SqliteVectorStore::new(s.connection_arc());

    let deleted = store.enforce_retention(30).await.unwrap();
    assert_eq!(deleted, 0);
}

#[tokio::test]
async fn vs_store_quantized_empty_int8_rejected() {
    let s = storage();
    let store = SqliteVectorStore::new(s.connection_arc());

    let empty_qv = QuantizedVector {
        data: vec![],
        scale: 1.0,
        offset: 0.0,
    };

    let result = store
        .store_quantized(vec![1.0, 2.0], &empty_qv, make_metadata("empty-q"), false)
        .await;

    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        CoreError::InvalidArguments { .. }
    ));
}

#[tokio::test]
async fn vs_store_quantized_dimension_mismatch_rejected() {
    let s = storage();
    let store = SqliteVectorStore::new(s.connection_arc());

    let qv = ScalarQuantizer::quantize(&[0.1, 0.2, 0.3, 0.4, 0.5]).unwrap();
    let result = store
        .store_quantized(vec![0.1, 0.2, 0.3], &qv, make_metadata("dim-mm"), false)
        .await;

    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        CoreError::InvalidArguments { .. }
    ));
}

// ── 3b. StorageService (4 tests) ───────────────────────────────

#[tokio::test]
async fn ss_save_and_get_event_roundtrip() {
    let s = storage();

    let event = make_event();
    s.save_event(&event).await.unwrap();

    let from = Utc::now() - Duration::minutes(1);
    let to = Utc::now() + Duration::minutes(1);
    let events = s.get_events(from, to, 100).await.unwrap();
    assert!(!events.is_empty());
}

#[tokio::test]
async fn ss_get_pending_limit_respected() {
    let s = storage();

    for _ in 0..5 {
        s.save_event(&make_event()).await.unwrap();
    }

    let pending = s.get_pending_events(2).await.unwrap();
    assert!(pending.len() <= 2);
}

#[tokio::test]
async fn ss_mark_as_sent_nonexistent_is_ok() {
    let s = storage();

    let result = s
        .mark_as_sent(&[
            "nonexistent-id-1".to_string(),
            "nonexistent-id-2".to_string(),
        ])
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn ss_enforce_retention_returns_count() {
    let s = storage();

    let deleted = s.enforce_retention().await.unwrap();
    assert_eq!(deleted, 0);
}

// ── 3c. TextSearchProvider (3 tests) ───────────────────────────

#[tokio::test]
async fn ts_search_empty_index_returns_empty() {
    let s = storage();

    let results = s.search_fts("nonexistent query", 10).await.unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn ts_sync_and_search_roundtrip() {
    let s = storage();

    s.sync_segment("seg-fts-001", "rust programming language")
        .await
        .unwrap();

    let results = s.search_fts("rust programming", 10).await.unwrap();
    assert!(!results.is_empty());
    assert_eq!(results[0].segment_id, "seg-fts-001");
}

#[tokio::test]
async fn ts_search_limit_respected() {
    let s = storage();

    for i in 0..5 {
        s.sync_segment(&format!("seg-fts-{i}"), "common search term here")
            .await
            .unwrap();
    }

    let results = s.search_fts("common search term", 2).await.unwrap();
    assert!(results.len() <= 2);
}

// ── 3d. MetricsStorage (3 tests) ───────────────────────────────

#[tokio::test]
async fn ms_save_and_get_metrics_roundtrip() {
    let s = storage();

    let metrics = make_metrics();
    s.save_metrics(&metrics).await.unwrap();

    let from = Utc::now() - Duration::minutes(1);
    let to = Utc::now() + Duration::minutes(1);
    let results = s.get_metrics(from, to, 100).await.unwrap();
    assert!(!results.is_empty());
    assert!((results[0].cpu_usage - 42.0).abs() < 0.1);
}

#[tokio::test]
async fn ms_get_metrics_empty_range_returns_empty() {
    let s = storage();

    let future = Utc::now() + Duration::days(365);
    let far_future = future + Duration::days(1);
    let results = s.get_metrics(future, far_future, 100).await.unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn ms_cleanup_old_metrics_returns_count() {
    let s = storage();

    let deleted = s.cleanup_old_metrics(Utc::now()).await.unwrap();
    assert_eq!(deleted, 0);
}

// ── 3e. VectorIndex (3 tests) ──────────────────────────────────

#[tokio::test]
async fn vi_build_ivf_index_on_empty_store_returns_error() {
    let s = storage();
    let index = SqliteVectorIndex::new(s.connection_arc());

    let result = index.build_ivf_index(4, 10).await;
    assert!(result.is_err(), "empty store should reject IVF build");
}

#[tokio::test]
async fn vi_count_unindexed_fresh_db() {
    let s = storage();
    let index = SqliteVectorIndex::new(s.connection_arc());

    let count = index.count_unindexed().await.unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn vi_get_index_meta_fresh_db() {
    let s = storage();
    let index = SqliteVectorIndex::new(s.connection_arc());

    let meta = index.get_index_meta().await.unwrap();
    assert!(meta.ivf_built_at.is_none());
    assert_eq!(meta.ivf_vector_count, 0);
    assert_eq!(meta.total_vector_count, 0);
}

// ── 3f. FocusStorage — sync trait (3 tests) ────────────────────

#[test]
fn fs_start_and_end_work_session_roundtrip() {
    let s = storage();

    let session = s
        .start_work_session("VSCode", AppCategory::Development)
        .unwrap();
    assert!(session.id > 0);
    assert_eq!(session.primary_app, "VSCode");

    s.end_work_session(session.id).unwrap();
}

#[test]
fn fs_get_or_create_focus_metrics_fresh_db() {
    let s = storage();

    let today = Utc::now().format("%Y-%m-%d").to_string();
    let metrics = s.get_or_create_focus_metrics(&today).unwrap();
    assert_eq!(metrics.total_active_secs, 0);
    assert_eq!(metrics.deep_work_secs, 0);
    assert_eq!(metrics.context_switches, 0);
}

#[test]
fn fs_increment_focus_metrics_accumulates() {
    let s = storage();

    let today = Utc::now().format("%Y-%m-%d").to_string();
    s.increment_focus_metrics(&today, 60, 30, 10, 2, 1).unwrap();
    s.increment_focus_metrics(&today, 40, 20, 5, 1, 0).unwrap();

    let metrics = s.get_or_create_focus_metrics(&today).unwrap();
    assert_eq!(metrics.total_active_secs, 100);
    assert_eq!(metrics.deep_work_secs, 50);
    assert_eq!(metrics.communication_secs, 15);
    assert_eq!(metrics.context_switches, 3);
    assert_eq!(metrics.interruption_count, 1);
}
