use super::helpers::*;
use super::SqliteVectorStore;
use chrono::{Duration, Utc};
use oneshim_core::models::embedding::{EmbeddingContentType, EmbeddingMetadata, SearchFilters};
use oneshim_core::ports::vector_store::VectorStore;
use rusqlite::Connection;
use std::sync::{Arc, Mutex};

use crate::migration;

fn setup_db() -> Arc<Mutex<Connection>> {
    let conn = Connection::open_in_memory().unwrap();
    migration::run_migrations(&conn).unwrap();
    Arc::new(Mutex::new(conn))
}

#[test]
fn f32_roundtrip() {
    let original = vec![1.0f32, -2.5, 3.125, 0.0];
    let bytes = f32_vec_to_bytes(&original);
    let restored = bytes_to_f32_vec(&bytes);
    assert_eq!(original, restored);
}

#[test]
fn cosine_similarity_identical() {
    let v = vec![1.0, 2.0, 3.0];
    let sim = cosine_similarity(&v, &v);
    assert!((sim - 1.0).abs() < 1e-5);
}

#[test]
fn cosine_similarity_orthogonal() {
    let a = vec![1.0, 0.0];
    let b = vec![0.0, 1.0];
    let sim = cosine_similarity(&a, &b);
    assert!(sim.abs() < 1e-5);
}

#[test]
fn cosine_similarity_opposite() {
    let a = vec![1.0, 0.0];
    let b = vec![-1.0, 0.0];
    let sim = cosine_similarity(&a, &b);
    assert!((sim - (-1.0)).abs() < 1e-5);
}

#[test]
fn cosine_similarity_zero_vector() {
    let a = vec![0.0, 0.0];
    let b = vec![1.0, 2.0];
    assert_eq!(cosine_similarity(&a, &b), 0.0);
}

#[tokio::test]
async fn store_and_search_roundtrip() {
    let conn = setup_db();
    let store = SqliteVectorStore::new(conn);

    let meta = EmbeddingMetadata {
        segment_id: "seg-001".to_string(),
        content_type: EmbeddingContentType::ContentActivity,
        content_label: Some("VSCode: main.rs".to_string()),
        timestamp: Utc::now(),
        original_text: "VSCode: main.rs".to_string(),
        model_id: "test-model".to_string(),
    };

    store.store(vec![1.0, 0.0, 0.0], meta).await.unwrap();

    let results = store.search(&[1.0, 0.0, 0.0], 10, 24.0).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].segment_id, "seg-001");
    assert!(results[0].similarity > 0.99);
}

#[tokio::test]
async fn search_returns_top_k_by_score() {
    let conn = setup_db();
    let store = SqliteVectorStore::new(conn);

    // Store three vectors: similar, less similar, and orthogonal
    let now = Utc::now();

    store
        .store(
            vec![1.0, 0.0, 0.0],
            EmbeddingMetadata {
                segment_id: "close".to_string(),
                content_type: EmbeddingContentType::ContentActivity,
                content_label: Some("close".to_string()),
                timestamp: now,
                original_text: "close".to_string(),
                model_id: "test-model".to_string(),
            },
        )
        .await
        .unwrap();

    store
        .store(
            vec![0.7, 0.7, 0.0],
            EmbeddingMetadata {
                segment_id: "medium".to_string(),
                content_type: EmbeddingContentType::ContentActivity,
                content_label: Some("medium".to_string()),
                timestamp: now,
                original_text: "medium".to_string(),
                model_id: "test-model".to_string(),
            },
        )
        .await
        .unwrap();

    store
        .store(
            vec![0.0, 0.0, 1.0],
            EmbeddingMetadata {
                segment_id: "far".to_string(),
                content_type: EmbeddingContentType::ContentActivity,
                content_label: Some("far".to_string()),
                timestamp: now,
                original_text: "far".to_string(),
                model_id: "test-model".to_string(),
            },
        )
        .await
        .unwrap();

    let results = store.search(&[1.0, 0.0, 0.0], 2, 24.0).await.unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].segment_id, "close");
    assert_eq!(results[1].segment_id, "medium");
}

#[tokio::test]
async fn time_decay_reduces_old_scores() {
    let conn = setup_db();
    let store = SqliteVectorStore::new(conn);

    let now = Utc::now();
    let old = now - Duration::hours(48);

    // Same vector, but one is old
    store
        .store(
            vec![1.0, 0.0, 0.0],
            EmbeddingMetadata {
                segment_id: "recent".to_string(),
                content_type: EmbeddingContentType::ContentActivity,
                content_label: Some("recent".to_string()),
                timestamp: now,
                original_text: "recent".to_string(),
                model_id: "test-model".to_string(),
            },
        )
        .await
        .unwrap();

    store
        .store(
            vec![1.0, 0.0, 0.0],
            EmbeddingMetadata {
                segment_id: "old".to_string(),
                content_type: EmbeddingContentType::ContentActivity,
                content_label: Some("old".to_string()),
                timestamp: old,
                original_text: "old".to_string(),
                model_id: "test-model".to_string(),
            },
        )
        .await
        .unwrap();

    let results = store.search(&[1.0, 0.0, 0.0], 10, 24.0).await.unwrap();
    assert_eq!(results.len(), 2);
    // Recent should score higher due to time decay
    assert_eq!(results[0].segment_id, "recent");
    assert!(results[0].score > results[1].score);
    // Both have similarity ~1.0
    assert!(results[0].similarity > 0.99);
    assert!(results[1].similarity > 0.99);
    // Old one has lower time_decay
    assert!(results[1].time_decay < results[0].time_decay);
}

#[tokio::test]
async fn enforce_retention_deletes_old() {
    let conn = setup_db();
    let store = SqliteVectorStore::new(conn);

    let old = Utc::now() - Duration::days(60);
    store
        .store(
            vec![1.0, 0.0],
            EmbeddingMetadata {
                segment_id: "old-seg".to_string(),
                content_type: EmbeddingContentType::SegmentSummary,
                content_label: None,
                timestamp: old,
                original_text: "".to_string(),
                model_id: "test-model".to_string(),
            },
        )
        .await
        .unwrap();

    store
        .store(
            vec![0.0, 1.0],
            EmbeddingMetadata {
                segment_id: "new-seg".to_string(),
                content_type: EmbeddingContentType::SegmentSummary,
                content_label: None,
                timestamp: Utc::now(),
                original_text: "".to_string(),
                model_id: "test-model".to_string(),
            },
        )
        .await
        .unwrap();

    let deleted = store.enforce_retention(30).await.unwrap();
    assert_eq!(deleted, 1);

    let results = store.search(&[1.0, 0.0], 10, 0.0).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].segment_id, "new-seg");
}

#[tokio::test]
async fn mark_stale_excludes_from_search() {
    let conn = setup_db();
    let store = SqliteVectorStore::new(conn);

    store
        .store(
            vec![1.0, 0.0],
            EmbeddingMetadata {
                segment_id: "seg-stale".to_string(),
                content_type: EmbeddingContentType::ContentActivity,
                content_label: Some("stale".to_string()),
                timestamp: Utc::now(),
                original_text: "stale".to_string(),
                model_id: "test-model".to_string(),
            },
        )
        .await
        .unwrap();

    // Before marking stale
    let results = store.search(&[1.0, 0.0], 10, 0.0).await.unwrap();
    assert_eq!(results.len(), 1);

    // Mark stale
    let marked = store.mark_stale("test-model").await.unwrap();
    assert_eq!(marked, 1);

    // After marking stale: search should exclude
    let results = store.search(&[1.0, 0.0], 10, 0.0).await.unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn search_filtered_by_content_type() {
    let conn = setup_db();
    let store = SqliteVectorStore::new(conn);

    let now = Utc::now();
    store
        .store(
            vec![1.0, 0.0],
            EmbeddingMetadata {
                segment_id: "s1".to_string(),
                content_type: EmbeddingContentType::SegmentSummary,
                content_label: None,
                timestamp: now,
                original_text: "".to_string(),
                model_id: "test-model".to_string(),
            },
        )
        .await
        .unwrap();

    store
        .store(
            vec![1.0, 0.0],
            EmbeddingMetadata {
                segment_id: "s2".to_string(),
                content_type: EmbeddingContentType::ContentActivity,
                content_label: Some("activity".to_string()),
                timestamp: now,
                original_text: "activity".to_string(),
                model_id: "test-model".to_string(),
            },
        )
        .await
        .unwrap();

    let filters = SearchFilters {
        content_types: Some(vec![EmbeddingContentType::SegmentSummary]),
        ..Default::default()
    };
    let results = store
        .search_filtered(&[1.0, 0.0], 10, 0.0, &filters)
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].segment_id, "s1");
}

#[tokio::test]
async fn search_filtered_by_time_range() {
    let conn = setup_db();
    let store = SqliteVectorStore::new(conn);

    let old = Utc::now() - Duration::days(5);
    let recent = Utc::now();

    store
        .store(
            vec![1.0],
            EmbeddingMetadata {
                segment_id: "old".to_string(),
                content_type: EmbeddingContentType::ContentActivity,
                content_label: Some("old".to_string()),
                timestamp: old,
                original_text: "old".to_string(),
                model_id: "test-model".to_string(),
            },
        )
        .await
        .unwrap();

    store
        .store(
            vec![1.0],
            EmbeddingMetadata {
                segment_id: "recent".to_string(),
                content_type: EmbeddingContentType::ContentActivity,
                content_label: Some("recent".to_string()),
                timestamp: recent,
                original_text: "recent".to_string(),
                model_id: "test-model".to_string(),
            },
        )
        .await
        .unwrap();

    let filters = SearchFilters {
        after: Some(Utc::now() - Duration::days(1)),
        ..Default::default()
    };
    let results = store
        .search_filtered(&[1.0], 10, 0.0, &filters)
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].segment_id, "recent");
}

#[tokio::test]
async fn store_quantized_roundtrip() {
    use oneshim_core::quantization::ScalarQuantizer;

    let conn = setup_db();
    let store = SqliteVectorStore::new(conn.clone());

    let vector = vec![0.1, 0.5, 0.9, -0.3, 0.7];
    let quantized = ScalarQuantizer::quantize(&vector).unwrap();

    let meta = EmbeddingMetadata {
        segment_id: "seg-q001".to_string(),
        content_type: EmbeddingContentType::ContentActivity,
        content_label: Some("VSCode: test.rs".to_string()),
        timestamp: Utc::now(),
        original_text: "VSCode: test.rs".to_string(),
        model_id: "test-model".to_string(),
    };

    store
        .store_quantized(vector.clone(), &quantized, meta, false)
        .await
        .unwrap();

    // Verify both f32 and INT8 columns are populated
    let guard = conn.lock().unwrap();
    let (has_f32, has_int8): (bool, bool) = guard
        .query_row(
            "SELECT vector IS NOT NULL, vector_int8 IS NOT NULL FROM embedding_vectors WHERE segment_id = 'seg-q001'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(has_f32);
    assert!(has_int8);
}

#[tokio::test]
async fn search_quantized_finds_similar() {
    use oneshim_core::quantization::ScalarQuantizer;

    let conn = setup_db();
    let store = SqliteVectorStore::new(conn);

    let now = Utc::now();

    // Store two quantized vectors: one similar, one different
    let v_close = vec![1.0, 0.1, 0.0, 0.0, 0.0];
    let v_far = vec![0.0, 0.0, 0.0, 0.1, 1.0];
    let q_close = ScalarQuantizer::quantize(&v_close).unwrap();
    let q_far = ScalarQuantizer::quantize(&v_far).unwrap();

    store
        .store_quantized(
            v_close,
            &q_close,
            EmbeddingMetadata {
                segment_id: "close".to_string(),
                content_type: EmbeddingContentType::ContentActivity,
                content_label: Some("close".to_string()),
                timestamp: now,
                original_text: "close".to_string(),
                model_id: "test-model".to_string(),
            },
            false,
        )
        .await
        .unwrap();

    store
        .store_quantized(
            v_far,
            &q_far,
            EmbeddingMetadata {
                segment_id: "far".to_string(),
                content_type: EmbeddingContentType::ContentActivity,
                content_label: Some("far".to_string()),
                timestamp: now,
                original_text: "far".to_string(),
                model_id: "test-model".to_string(),
            },
            false,
        )
        .await
        .unwrap();

    // Search with a query similar to "close"
    let query = vec![1.0, 0.0, 0.0, 0.0, 0.0];
    let q_query = ScalarQuantizer::quantize(&query).unwrap();

    let results = store
        .search_quantized(&q_query, 10, 24.0, &SearchFilters::default())
        .await
        .unwrap();

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].segment_id, "close");
    assert!(results[0].score > results[1].score);
}

#[tokio::test]
async fn search_quantized_respects_filters() {
    use oneshim_core::quantization::ScalarQuantizer;

    let conn = setup_db();
    let store = SqliteVectorStore::new(conn);

    let now = Utc::now();
    let v = vec![1.0, 0.0, 0.0];
    let qv = ScalarQuantizer::quantize(&v).unwrap();

    store
        .store_quantized(
            v.clone(),
            &qv,
            EmbeddingMetadata {
                segment_id: "summary-seg".to_string(),
                content_type: EmbeddingContentType::SegmentSummary,
                content_label: None,
                timestamp: now,
                original_text: "summary".to_string(),
                model_id: "test-model".to_string(),
            },
            false,
        )
        .await
        .unwrap();

    store
        .store_quantized(
            v.clone(),
            &qv,
            EmbeddingMetadata {
                segment_id: "activity-seg".to_string(),
                content_type: EmbeddingContentType::ContentActivity,
                content_label: Some("activity".to_string()),
                timestamp: now,
                original_text: "activity".to_string(),
                model_id: "test-model".to_string(),
            },
            false,
        )
        .await
        .unwrap();

    let query_qv = ScalarQuantizer::quantize(&[1.0, 0.0, 0.0]).unwrap();
    let filters = SearchFilters {
        content_types: Some(vec![EmbeddingContentType::SegmentSummary]),
        ..Default::default()
    };
    let results = store
        .search_quantized(&query_qv, 10, 0.0, &filters)
        .await
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].segment_id, "summary-seg");
}

#[tokio::test]
async fn search_quantized_skips_non_quantized_rows() {
    use oneshim_core::quantization::ScalarQuantizer;

    let conn = setup_db();
    let store = SqliteVectorStore::new(conn);

    // Store one vector via plain store() — no INT8 columns
    store
        .store(
            vec![1.0, 0.0, 0.0],
            EmbeddingMetadata {
                segment_id: "no-int8".to_string(),
                content_type: EmbeddingContentType::ContentActivity,
                content_label: Some("old".to_string()),
                timestamp: Utc::now(),
                original_text: "old".to_string(),
                model_id: "test-model".to_string(),
            },
        )
        .await
        .unwrap();

    let query_qv = ScalarQuantizer::quantize(&[1.0, 0.0, 0.0]).unwrap();
    let results = store
        .search_quantized(&query_qv, 10, 0.0, &SearchFilters::default())
        .await
        .unwrap();

    // The non-quantized row should be excluded
    assert!(results.is_empty());
}

#[tokio::test]
async fn backfill_quantized_converts_existing() {
    let conn = setup_db();
    let store = SqliteVectorStore::new(conn.clone());

    // Store 3 vectors via plain store() — no INT8 columns
    for i in 0..3 {
        store
            .store(
                vec![1.0, 0.0, i as f32 * 0.1],
                EmbeddingMetadata {
                    segment_id: format!("seg-{i}"),
                    content_type: EmbeddingContentType::ContentActivity,
                    content_label: Some(format!("label-{i}")),
                    timestamp: Utc::now(),
                    original_text: format!("text-{i}"),
                    model_id: "test-model".to_string(),
                },
            )
            .await
            .unwrap();
    }

    // Backfill batch of 2
    let filled = store.backfill_quantized(2).await.unwrap();
    assert_eq!(filled, 2);

    // One remaining
    let filled = store.backfill_quantized(10).await.unwrap();
    assert_eq!(filled, 1);

    // None left
    let filled = store.backfill_quantized(10).await.unwrap();
    assert_eq!(filled, 0);

    // All should now be searchable via search_quantized
    use oneshim_core::quantization::ScalarQuantizer;
    let query_qv = ScalarQuantizer::quantize(&[1.0, 0.0, 0.0]).unwrap();
    let results = store
        .search_quantized(&query_qv, 10, 0.0, &SearchFilters::default())
        .await
        .unwrap();
    assert_eq!(results.len(), 3);
}

#[tokio::test]
async fn count_unquantized_empty_table() {
    let conn = setup_db();
    let store = SqliteVectorStore::new(conn);

    let count = store.count_unquantized().await.unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn count_unquantized_tracks_non_int8_rows() {
    let conn = setup_db();
    let store = SqliteVectorStore::new(conn);

    // Store 3 plain f32 vectors — no INT8 columns
    for i in 0..3 {
        store
            .store(
                vec![1.0, 0.0, i as f32 * 0.1],
                EmbeddingMetadata {
                    segment_id: format!("seg-{i}"),
                    content_type: EmbeddingContentType::ContentActivity,
                    content_label: Some(format!("label-{i}")),
                    timestamp: Utc::now(),
                    original_text: format!("text-{i}"),
                    model_id: "test-model".to_string(),
                },
            )
            .await
            .unwrap();
    }

    assert_eq!(store.count_unquantized().await.unwrap(), 3);

    // Backfill 2
    store.backfill_quantized(2).await.unwrap();
    assert_eq!(store.count_unquantized().await.unwrap(), 1);

    // Backfill remaining
    store.backfill_quantized(10).await.unwrap();
    assert_eq!(store.count_unquantized().await.unwrap(), 0);
}

#[tokio::test]
async fn store_quantized_skip_float32_nulls_vector_column() {
    use oneshim_core::quantization::ScalarQuantizer;

    let conn = setup_db();
    let store = SqliteVectorStore::new(conn.clone());

    let vector = vec![0.1, 0.5, 0.9, -0.3, 0.7];
    let quantized = ScalarQuantizer::quantize(&vector).unwrap();

    let meta = EmbeddingMetadata {
        segment_id: "seg-skip-f32".to_string(),
        content_type: EmbeddingContentType::ContentActivity,
        content_label: Some("test skip".to_string()),
        timestamp: Utc::now(),
        original_text: "test skip f32".to_string(),
        model_id: "test-model".to_string(),
    };

    // Store with skip_float32 = true
    store
        .store_quantized(vector, &quantized, meta, true)
        .await
        .unwrap();

    // Verify: f32 column is empty BLOB (len=0), INT8 column is populated
    let guard = conn.lock().unwrap();
    let (f32_blob_len, has_int8): (usize, bool) = guard
        .query_row(
            "SELECT LENGTH(vector), vector_int8 IS NOT NULL FROM embedding_vectors WHERE segment_id = 'seg-skip-f32'",
            [],
            |row| {
                let len: i64 = row.get(0)?;
                Ok((len as usize, row.get(1)?))
            },
        )
        .unwrap();
    assert_eq!(
        f32_blob_len, 0,
        "f32 vector should be empty BLOB when skip_float32=true"
    );
    assert!(has_int8, "INT8 vector should still be populated");
}

#[tokio::test]
async fn store_quantized_skip_float32_still_searchable_via_int8() {
    use oneshim_core::quantization::ScalarQuantizer;

    let conn = setup_db();
    let store = SqliteVectorStore::new(conn);

    let v = vec![1.0, 0.0, 0.0, 0.0, 0.0];
    let qv = ScalarQuantizer::quantize(&v).unwrap();

    store
        .store_quantized(
            v,
            &qv,
            EmbeddingMetadata {
                segment_id: "int8-only".to_string(),
                content_type: EmbeddingContentType::ContentActivity,
                content_label: Some("int8 only".to_string()),
                timestamp: Utc::now(),
                original_text: "int8 only".to_string(),
                model_id: "test-model".to_string(),
            },
            true,
        )
        .await
        .unwrap();

    // Should be findable via search_quantized
    let query_qv = ScalarQuantizer::quantize(&[1.0, 0.0, 0.0, 0.0, 0.0]).unwrap();
    let results = store
        .search_quantized(&query_qv, 10, 0.0, &SearchFilters::default())
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].segment_id, "int8-only");
}

// ── Negative feedback filtering tests ────────────────────────

#[tokio::test]
async fn search_filtered_excludes_dismissed_segments() {
    let conn = setup_db();
    let store = SqliteVectorStore::new(conn);
    let now = Utc::now();

    for seg_id in &["keep-1", "dismiss-1", "keep-2", "dismiss-2"] {
        store
            .store(
                vec![1.0, 0.0, 0.0],
                EmbeddingMetadata {
                    segment_id: seg_id.to_string(),
                    content_type: EmbeddingContentType::ContentActivity,
                    content_label: Some(seg_id.to_string()),
                    timestamp: now,
                    original_text: seg_id.to_string(),
                    model_id: "test-model".to_string(),
                },
            )
            .await
            .unwrap();
    }

    let filters = SearchFilters {
        excluded_segment_ids: vec!["dismiss-1".to_string(), "dismiss-2".to_string()],
        ..Default::default()
    };
    let results = store
        .search_filtered(&[1.0, 0.0, 0.0], 10, 0.0, &filters)
        .await
        .unwrap();

    let ids: Vec<&str> = results.iter().map(|r| r.segment_id.as_str()).collect();
    assert_eq!(results.len(), 2);
    assert!(ids.contains(&"keep-1"));
    assert!(ids.contains(&"keep-2"));
    assert!(!ids.contains(&"dismiss-1"));
    assert!(!ids.contains(&"dismiss-2"));
}

#[tokio::test]
async fn search_filtered_empty_exclusion_returns_all() {
    let conn = setup_db();
    let store = SqliteVectorStore::new(conn);
    let now = Utc::now();

    for seg_id in &["seg-a", "seg-b"] {
        store
            .store(
                vec![1.0, 0.0],
                EmbeddingMetadata {
                    segment_id: seg_id.to_string(),
                    content_type: EmbeddingContentType::ContentActivity,
                    content_label: Some(seg_id.to_string()),
                    timestamp: now,
                    original_text: seg_id.to_string(),
                    model_id: "test-model".to_string(),
                },
            )
            .await
            .unwrap();
    }

    let filters = SearchFilters {
        excluded_segment_ids: vec![],
        ..Default::default()
    };
    let results = store
        .search_filtered(&[1.0, 0.0], 10, 0.0, &filters)
        .await
        .unwrap();
    assert_eq!(results.len(), 2);
}

#[tokio::test]
async fn search_quantized_excludes_dismissed_segments() {
    use oneshim_core::quantization::ScalarQuantizer;

    let conn = setup_db();
    let store = SqliteVectorStore::new(conn);
    let now = Utc::now();

    let v = vec![1.0, 0.0, 0.0];
    let qv = ScalarQuantizer::quantize(&v).unwrap();

    for seg_id in &["q-keep", "q-dismiss"] {
        store
            .store_quantized(
                v.clone(),
                &qv,
                EmbeddingMetadata {
                    segment_id: seg_id.to_string(),
                    content_type: EmbeddingContentType::ContentActivity,
                    content_label: Some(seg_id.to_string()),
                    timestamp: now,
                    original_text: seg_id.to_string(),
                    model_id: "test-model".to_string(),
                },
                false,
            )
            .await
            .unwrap();
    }

    let query_qv = ScalarQuantizer::quantize(&[1.0, 0.0, 0.0]).unwrap();
    let filters = SearchFilters {
        excluded_segment_ids: vec!["q-dismiss".to_string()],
        ..Default::default()
    };
    let results = store
        .search_quantized(&query_qv, 10, 0.0, &filters)
        .await
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].segment_id, "q-keep");
}

#[tokio::test]
async fn count_active_vectors_basic() {
    let conn = setup_db();
    let store = SqliteVectorStore::new(conn);

    assert_eq!(store.count_active_vectors().await.unwrap(), 0);

    // Store 3 vectors
    for i in 0..3 {
        store
            .store(
                vec![1.0, 0.0],
                EmbeddingMetadata {
                    segment_id: format!("seg-{i}"),
                    content_type: EmbeddingContentType::ContentActivity,
                    content_label: None,
                    timestamp: Utc::now(),
                    original_text: format!("text-{i}"),
                    model_id: "test-model".to_string(),
                },
            )
            .await
            .unwrap();
    }
    assert_eq!(store.count_active_vectors().await.unwrap(), 3);

    // Mark stale
    store.mark_stale("test-model").await.unwrap();
    assert_eq!(store.count_active_vectors().await.unwrap(), 0);
}

#[tokio::test]
async fn search_quantized_empty_exclusion_returns_all() {
    use oneshim_core::quantization::ScalarQuantizer;

    let conn = setup_db();
    let store = SqliteVectorStore::new(conn);
    let now = Utc::now();

    let v = vec![1.0, 0.0, 0.0];
    let qv = ScalarQuantizer::quantize(&v).unwrap();

    for seg_id in &["qa", "qb"] {
        store
            .store_quantized(
                v.clone(),
                &qv,
                EmbeddingMetadata {
                    segment_id: seg_id.to_string(),
                    content_type: EmbeddingContentType::ContentActivity,
                    content_label: Some(seg_id.to_string()),
                    timestamp: now,
                    original_text: seg_id.to_string(),
                    model_id: "test-model".to_string(),
                },
                false,
            )
            .await
            .unwrap();
    }

    let query_qv = ScalarQuantizer::quantize(&[1.0, 0.0, 0.0]).unwrap();
    let filters = SearchFilters {
        excluded_segment_ids: vec![],
        ..Default::default()
    };
    let results = store
        .search_quantized(&query_qv, 10, 0.0, &filters)
        .await
        .unwrap();
    assert_eq!(results.len(), 2);
}

#[tokio::test]
async fn store_quantized_rejects_empty_int8_vector() {
    use oneshim_core::quantization::QuantizedVector;

    let conn = setup_db();
    let store = SqliteVectorStore::new(conn);

    let meta = EmbeddingMetadata {
        segment_id: "seg_empty".to_string(),
        content_type: EmbeddingContentType::ContentActivity,
        content_label: None,
        original_text: "test".to_string(),
        model_id: "test-model".to_string(),
        timestamp: Utc::now(),
    };

    let empty_qv = QuantizedVector {
        data: vec![],
        scale: 1.0,
        offset: 0.0,
    };

    let result = store
        .store_quantized(vec![1.0, 2.0, 3.0], &empty_qv, meta, false)
        .await;
    assert!(result.is_err(), "should reject empty INT8 vector");
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("empty INT8 vector"),
        "error should mention empty INT8 vector, got: {err_msg}"
    );
}

#[tokio::test]
async fn store_quantized_dimension_mismatch_rejected() {
    use oneshim_core::quantization::ScalarQuantizer;

    let conn = setup_db();
    let store = SqliteVectorStore::new(conn);

    let f32_vec: Vec<f32> = vec![0.1, 0.2, 0.3]; // 3 dims
    let qv = ScalarQuantizer::quantize(&[0.1, 0.2, 0.3, 0.4, 0.5]).unwrap(); // 5 dims
    let meta = EmbeddingMetadata {
        segment_id: "dim-mismatch".to_string(),
        content_type: EmbeddingContentType::ContentActivity,
        content_label: None,
        original_text: "test".to_string(),
        model_id: "test-model".to_string(),
        timestamp: Utc::now(),
    };

    let result = store.store_quantized(f32_vec, &qv, meta, false).await;

    assert!(result.is_err(), "mismatched dimensions should be rejected");
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("dimension mismatch"),
        "error should mention dimension mismatch, got: {err_msg}"
    );
}

#[tokio::test]
async fn store_quantized_skip_float32_bypasses_dimension_check() {
    use oneshim_core::quantization::ScalarQuantizer;

    let conn = setup_db();
    let store = SqliteVectorStore::new(conn);

    let f32_vec: Vec<f32> = vec![]; // empty — skip_float32 means this is ignored
    let qv = ScalarQuantizer::quantize(&[0.1, 0.2, 0.3]).unwrap();
    let meta = EmbeddingMetadata {
        segment_id: "skip-f32".to_string(),
        content_type: EmbeddingContentType::ContentActivity,
        content_label: None,
        original_text: "test".to_string(),
        model_id: "test-model".to_string(),
        timestamp: Utc::now(),
    };

    let result = store.store_quantized(f32_vec, &qv, meta, true).await;

    assert!(
        result.is_ok(),
        "skip_float32=true should bypass dimension check"
    );
}

// ── C3a: regime_id filter via activity_segments subquery ────────────

/// Seed a row into `regimes` so the FK on `activity_segments.regime_id` is
/// satisfied when FKs are enforced. Idempotent via `INSERT OR IGNORE`.
fn ensure_regime(conn: &Arc<Mutex<Connection>>, regime_id: &str) {
    let guard = conn.lock().unwrap();
    guard
        .execute(
            "INSERT OR IGNORE INTO regimes
                (id, label, detected_at, last_seen_at, dominant_category)
             VALUES (?1, ?1, datetime('now'), datetime('now'), 'work')",
            rusqlite::params![regime_id],
        )
        .unwrap();
}

/// Inserts a row into `activity_segments` so the regime_id subquery
/// (`segment_id IN (SELECT id FROM activity_segments WHERE regime_id = ?)`)
/// can resolve.
fn insert_segment_row(conn: &Arc<Mutex<Connection>>, seg_id: &str, regime_id: Option<&str>) {
    if let Some(r) = regime_id {
        ensure_regime(conn, r);
    }
    let guard = conn.lock().unwrap();
    guard
        .execute(
            "INSERT INTO activity_segments
                (id, start_time, end_time, duration_secs, regime_id,
                 trigger_reason, dominant_category)
             VALUES (?1, datetime('now'), datetime('now'), 0, ?2, 'test', 'work')",
            rusqlite::params![seg_id, regime_id],
        )
        .unwrap();
}

async fn store_one(store: &SqliteVectorStore, seg_id: &str) {
    store
        .store(
            vec![1.0, 0.0, 0.0],
            EmbeddingMetadata {
                segment_id: seg_id.to_string(),
                content_type: EmbeddingContentType::ContentActivity,
                content_label: Some(seg_id.to_string()),
                timestamp: Utc::now(),
                original_text: seg_id.to_string(),
                model_id: "test-model".to_string(),
            },
        )
        .await
        .unwrap();
}

/// T-C3a-1 — `search_filtered` with `regime_id = "r1"` excludes other regimes.
#[tokio::test]
async fn search_filtered_excludes_other_regimes() {
    let conn = setup_db();
    insert_segment_row(&conn, "seg_r1_a", Some("r1"));
    insert_segment_row(&conn, "seg_r1_b", Some("r1"));
    insert_segment_row(&conn, "seg_r2", Some("r2"));
    let store = SqliteVectorStore::new(conn);
    store_one(&store, "seg_r1_a").await;
    store_one(&store, "seg_r1_b").await;
    store_one(&store, "seg_r2").await;

    let filters = SearchFilters {
        regime_id: Some("r1".into()),
        ..Default::default()
    };
    let results = store
        .search_filtered(&[1.0, 0.0, 0.0], 10, 0.0, &filters)
        .await
        .unwrap();

    assert_eq!(results.len(), 2);
    for r in &results {
        assert!(
            r.segment_id.starts_with("seg_r1"),
            "unexpected segment {} returned",
            r.segment_id
        );
    }
}

/// T-C3a-2 — `search_quantized` with `regime_id = "r1"` excludes other regimes.
#[tokio::test]
async fn search_quantized_excludes_other_regimes() {
    use oneshim_core::quantization::ScalarQuantizer;

    let conn = setup_db();
    insert_segment_row(&conn, "seg_r1", Some("r1"));
    insert_segment_row(&conn, "seg_r2", Some("r2"));
    let store = SqliteVectorStore::new(conn);
    let v = vec![1.0, 0.0, 0.0];
    let qv = ScalarQuantizer::quantize(&v).unwrap();

    for seg_id in &["seg_r1", "seg_r2"] {
        store
            .store_quantized(
                v.clone(),
                &qv,
                EmbeddingMetadata {
                    segment_id: seg_id.to_string(),
                    content_type: EmbeddingContentType::ContentActivity,
                    content_label: Some(seg_id.to_string()),
                    timestamp: Utc::now(),
                    original_text: seg_id.to_string(),
                    model_id: "test-model".to_string(),
                },
                false,
            )
            .await
            .unwrap();
    }

    let filters = SearchFilters {
        regime_id: Some("r1".into()),
        ..Default::default()
    };
    let results = store
        .search_quantized(&qv, 10, 0.0, &filters)
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].segment_id, "seg_r1");
}

/// T-C3a-3 — `regime_id: None` preserves pre-C3a behaviour (returns all).
#[tokio::test]
async fn regime_id_none_preserves_existing_behaviour() {
    let conn = setup_db();
    insert_segment_row(&conn, "seg_r1", Some("r1"));
    insert_segment_row(&conn, "seg_r2", Some("r2"));
    let store = SqliteVectorStore::new(conn);
    store_one(&store, "seg_r1").await;
    store_one(&store, "seg_r2").await;

    let filters = SearchFilters::default();
    let results = store
        .search_filtered(&[1.0, 0.0, 0.0], 10, 0.0, &filters)
        .await
        .unwrap();
    assert_eq!(results.len(), 2);
}

/// T-C3a-4 — a segment whose `activity_segments.regime_id` is NULL is
/// excluded when a regime filter is set.
#[tokio::test]
async fn segment_without_regime_not_returned_under_filter() {
    let conn = setup_db();
    insert_segment_row(&conn, "seg_r1", Some("r1"));
    insert_segment_row(&conn, "seg_null", None);
    let store = SqliteVectorStore::new(conn);
    store_one(&store, "seg_r1").await;
    store_one(&store, "seg_null").await;

    let filters = SearchFilters {
        regime_id: Some("r1".into()),
        ..Default::default()
    };
    let results = store
        .search_filtered(&[1.0, 0.0, 0.0], 10, 0.0, &filters)
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].segment_id, "seg_r1");
}
