use super::helpers::*;
use super::SqliteVectorStore;
use crate::sqlite::GuardedConnection;
use chrono::{Duration, Utc};
use maekon_core::models::embedding::{EmbeddingContentType, EmbeddingMetadata, SearchFilters};
use maekon_core::ports::vector_store::VectorStore;
use rusqlite::{params, Connection};
use std::sync::Arc;

use crate::sqlite::SqliteStorage;

use crate::migration;

fn setup_db() -> Arc<GuardedConnection> {
    let conn = Connection::open_in_memory().unwrap();
    migration::run_migrations(&conn).unwrap();
    Arc::new(GuardedConnection::new_unflagged(conn))
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
async fn store_stamps_monotonic_hlc_for_sync() {
    // F0/#5186: embedding_vectors writes must carry a strictly-increasing HLC + device id
    // so they propagate via cross-device sync (previously hlc=0 → never propagated).
    let conn = setup_db();
    {
        // run_migrations creates device_identity but not its row; insert one so the stamp
        // carries a real origin_device_id.
        let c = conn.test_lock();
        c.execute(
            "INSERT INTO device_identity (id, device_id, device_name) VALUES (1, 'dev-x', 'X')",
            [],
        )
        .unwrap();
    }

    let store = SqliteVectorStore::new(conn.clone());
    let meta = |seg: &str| EmbeddingMetadata {
        segment_id: seg.to_string(),
        content_type: EmbeddingContentType::ContentActivity,
        content_label: None,
        timestamp: Utc::now(),
        original_text: String::new(),
        model_id: "m".to_string(),
    };
    store.store(vec![1.0, 0.0], meta("a")).await.unwrap();
    store.store(vec![0.0, 1.0], meta("b")).await.unwrap();

    let rows: Vec<(i64, i64, String)> = {
        let c = conn.test_lock();
        let mut stmt = c
            .prepare(
                "SELECT hlc_wall_ms, hlc_counter, origin_device_id \
                 FROM embedding_vectors ORDER BY id",
            )
            .unwrap();
        let v = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        v
    };

    assert_eq!(rows.len(), 2);
    assert!(
        rows[0].0 > 0,
        "first row HLC must be stamped (>0): {:?}",
        rows[0]
    );
    assert_eq!(rows[0].2, "dev-x", "origin_device_id must be stamped");
    assert!(
        (rows[1].0, rows[1].1) > (rows[0].0, rows[0].1),
        "second write must carry a strictly-greater HLC: {:?} vs {:?}",
        rows[1],
        rows[0]
    );
}

#[tokio::test]
async fn mark_stale_preserves_peer_origin_device_id() {
    // F0/#5186 (PR5 follow-up): an embedding that arrived from a peer must NOT be
    // re-attributed to the local device when locally mark_stale'd — origin_device_id is the
    // erasure scope (DeletionEvent deletes WHERE origin_device_id), and embeddings are LWW
    // so a re-attribution would propagate and remove the row from the peer's erasure.
    let conn = setup_db();
    {
        let c = conn.test_lock();
        c.execute(
            "INSERT INTO device_identity (id, device_id, device_name) VALUES (1, 'dev-x', 'X')",
            [],
        )
        .unwrap();
    }
    let store = SqliteVectorStore::new(conn.clone());
    let meta = EmbeddingMetadata {
        segment_id: "seg".to_string(),
        content_type: EmbeddingContentType::ContentActivity,
        content_label: None,
        timestamp: Utc::now(),
        original_text: String::new(),
        model_id: "old-model".to_string(),
    };
    store.store(vec![1.0, 0.0], meta).await.unwrap();
    // Simulate the row having arrived from a peer (origin = peer-b).
    {
        let c = conn.test_lock();
        c.execute(
            "UPDATE embedding_vectors SET origin_device_id = 'peer-b'",
            [],
        )
        .unwrap();
    }

    store.mark_stale("old-model").await.unwrap();

    let (origin, stale, wall): (String, i64, i64) = {
        let c = conn.test_lock();
        c.query_row(
            "SELECT origin_device_id, is_stale, hlc_wall_ms FROM embedding_vectors",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
    };
    assert_eq!(
        origin, "peer-b",
        "mark_stale must PRESERVE the peer origin (not re-attribute to local)"
    );
    assert_eq!(stale, 1, "is_stale must be set");
    assert!(wall > 0, "HLC must be bumped");
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
async fn enforce_retention_deletes_vector_side_rows() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();
    let conn = storage.connection_arc();
    let store = SqliteVectorStore::new(conn.clone());

    store
        .store(
            vec![1.0, 0.0],
            EmbeddingMetadata {
                segment_id: "old-indexed-seg".to_string(),
                content_type: EmbeddingContentType::SegmentSummary,
                content_label: None,
                timestamp: Utc::now() - Duration::days(60),
                original_text: "old indexed vector".to_string(),
                model_id: "test-model".to_string(),
            },
        )
        .await
        .unwrap();

    let vector_id: i64 = {
        let guard = conn.test_lock();
        guard
            .query_row(
                "SELECT id FROM embedding_vectors WHERE segment_id = 'old-indexed-seg'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    };

    {
        let guard = conn.test_lock();
        let foreign_keys_enabled: i64 = guard
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(foreign_keys_enabled, 1);

        guard
            .execute(
                "INSERT INTO vector_binary_codes (vector_id, binary_code) VALUES (?1, ?2)",
                params![vector_id, vec![0b1010_1010u8]],
            )
            .unwrap();
        guard
            .execute(
                "INSERT INTO ivf_centroids (id, centroid_int8, centroid_scale, centroid_offset, vector_count) VALUES (7, ?1, 1.0, 0.0, 1)",
                params![vec![1i8 as u8, 2i8 as u8]],
            )
            .unwrap();
        guard
            .execute(
                "INSERT INTO ivf_assignments (vector_id, cluster_id) VALUES (?1, 7)",
                params![vector_id],
            )
            .unwrap();
    }

    let deleted = store.enforce_retention(30).await.unwrap();
    assert_eq!(deleted, 1);

    let (binary_rows, assignment_rows): (i64, i64) = {
        let guard = conn.test_lock();
        let binary_rows = guard
            .query_row(
                "SELECT COUNT(*) FROM vector_binary_codes WHERE vector_id = ?1",
                params![vector_id],
                |row| row.get(0),
            )
            .unwrap();
        let assignment_rows = guard
            .query_row(
                "SELECT COUNT(*) FROM ivf_assignments WHERE vector_id = ?1",
                params![vector_id],
                |row| row.get(0),
            )
            .unwrap();
        (binary_rows, assignment_rows)
    };

    assert_eq!(binary_rows, 0);
    assert_eq!(assignment_rows, 0);
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
    use maekon_core::quantization::ScalarQuantizer;

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
    let guard = conn.test_lock();
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
    use maekon_core::quantization::ScalarQuantizer;

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
    use maekon_core::quantization::ScalarQuantizer;

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
    use maekon_core::quantization::ScalarQuantizer;

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
    use maekon_core::quantization::ScalarQuantizer;
    let query_qv = ScalarQuantizer::quantize(&[1.0, 0.0, 0.0]).unwrap();
    let results = store
        .search_quantized(&query_qv, 10, 0.0, &SearchFilters::default())
        .await
        .unwrap();
    assert_eq!(results.len(), 3);
}

/// Regression (#backfill-tx): a `?` early-return inside `backfill_quantized` must
/// NOT leak an open transaction on the SHARED GuardedConnection. Previously the
/// raw `conn.execute_batch("BEGIN")` had a matching `COMMIT` only on the happy
/// path; an error arm (e.g. a corrupt/empty vector BLOB that fails
/// `ScalarQuantizer::quantize`) returned via `?` while the transaction was still
/// open, poisoning the next writer with "cannot start a transaction within a
/// transaction". The RAII `unchecked_transaction()` guard now auto-rolls-back on
/// drop, so the connection is left clean.
#[tokio::test]
async fn backfill_quantized_error_does_not_leak_open_transaction() {
    let conn = setup_db();
    let store = SqliteVectorStore::new(conn.clone());

    // Seed a corrupt row (empty `vector` BLOB → empty Vec<f32> → quantize() errors)
    // at a LOW id so it is processed first, then a valid row. Direct SQL inserts
    // bypass store()'s own validation, which is the point: we are simulating a row
    // already on disk that fails re-quantization during backfill.
    {
        let g = conn.test_lock();
        let empty_blob: Vec<u8> = Vec::new();
        g.execute(
            "INSERT INTO embedding_vectors \
             (id, segment_id, content_type, content_label, original_text, \
              vector, model_id, timestamp, hlc_wall_ms, hlc_counter, origin_device_id) \
             VALUES (1, 'corrupt', 'CONTENT_ACTIVITY', NULL, 'bad', ?1, 'm', \
                     datetime('now'), 0, 0, 'test')",
            rusqlite::params![empty_blob],
        )
        .unwrap();

        let good_blob: Vec<u8> = f32_vec_to_bytes(&[1.0_f32, 0.0, 0.0]);
        g.execute(
            "INSERT INTO embedding_vectors \
             (id, segment_id, content_type, content_label, original_text, \
              vector, model_id, timestamp, hlc_wall_ms, hlc_counter, origin_device_id) \
             VALUES (2, 'good', 'CONTENT_ACTIVITY', NULL, 'ok', ?1, 'm', \
                     datetime('now'), 0, 0, 'test')",
            rusqlite::params![good_blob],
        )
        .unwrap();
    }

    // backfill must FAIL on the corrupt row (zero-length vector cannot be quantized).
    // The quantize failure is wrapped as StorageError::Internal("Backfill quantize
    // failed …") and surfaces as CoreError::Storage; assert the variant AND message
    // so the test proves the failure is the quantize step (the path that used to
    // leak an open transaction), not an unrelated query/prepare error.
    let err = store
        .backfill_quantized(10)
        .await
        .expect_err("backfill_quantized must error on a corrupt (empty) vector BLOB");
    assert!(
        matches!(
            &err,
            maekon_core::error::CoreError::Storage { message, .. }
                if message.contains("Backfill quantize failed")
        ),
        "corrupt-BLOB backfill must surface CoreError::Storage from the failed quantize, got {err:?}"
    );

    // CRITICAL: the SHARED connection must NOT be left inside an open transaction.
    // A leaked `BEGIN` would make this fresh `BEGIN` fail with
    // "cannot start a transaction within a transaction". With the RAII fix the
    // failed backfill rolled back, so a subsequent write transaction succeeds.
    {
        let g = conn.test_lock();
        g.execute_batch("BEGIN")
            .expect("connection left in open-tx state: BEGIN should succeed after failed backfill");
        g.execute(
            "INSERT INTO embedding_vectors \
             (id, segment_id, content_type, content_label, original_text, \
              vector, model_id, timestamp, hlc_wall_ms, hlc_counter, origin_device_id) \
             VALUES (3, 'after', 'CONTENT_ACTIVITY', NULL, 'post', ?1, 'm', \
                     datetime('now'), 0, 0, 'test')",
            rusqlite::params![f32_vec_to_bytes(&[0.5_f32, 0.5, 0.0])],
        )
        .expect("write after failed backfill should succeed");
        g.execute_batch("COMMIT")
            .expect("COMMIT of the post-backfill write should succeed");
    }

    // The rollback also means the *valid* row (id=2) was NOT quantized — the whole
    // batch is atomic. count_unquantized still sees both original rows (the new
    // id=3 row also has no INT8 columns), confirming nothing partial committed.
    let unquantized = store.count_unquantized().await.unwrap();
    assert_eq!(
        unquantized, 3,
        "failed backfill must be atomic: no INT8 columns written for any row"
    );
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
    use maekon_core::quantization::ScalarQuantizer;

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
    let guard = conn.test_lock();
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
    use maekon_core::quantization::ScalarQuantizer;

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
    use maekon_core::quantization::ScalarQuantizer;

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
    use maekon_core::quantization::ScalarQuantizer;

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
    use maekon_core::quantization::QuantizedVector;

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
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("empty INT8 vector"),
        "error should mention empty INT8 vector, got: {err_msg}"
    );
}

#[tokio::test]
async fn store_quantized_dimension_mismatch_rejected() {
    use maekon_core::quantization::ScalarQuantizer;

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

    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("dimension mismatch"),
        "error should mention dimension mismatch, got: {err_msg}"
    );
}

#[tokio::test]
async fn store_quantized_skip_float32_bypasses_dimension_check() {
    use maekon_core::quantization::ScalarQuantizer;

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

    // Line 1191: store_quantized with skip_float32=true must return Ok(()) —
    // the dimension check is skipped so the empty f32_vec does not raise an error.
    store
        .store_quantized(f32_vec, &qv, meta, true)
        .await
        .expect("skip_float32=true should bypass dimension check");

    // Read-back: last_insert_id > 0 confirms the row was actually persisted.
    let id = store
        .last_insert_id()
        .await
        .expect("last_insert_id must succeed after a successful store_quantized");
    assert!(
        id > 0,
        "last_insert_id must be > 0 after store_quantized, got {id}"
    );
}

// ── C3a: regime_id filter via activity_segments subquery ────────────

/// Seed a row into `regimes` so the FK on `activity_segments.regime_id` is
/// satisfied when FKs are enforced. Idempotent via `INSERT OR IGNORE`.
fn ensure_regime(conn: &Arc<GuardedConnection>, regime_id: &str) {
    let guard = conn.test_lock();
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
fn insert_segment_row(conn: &Arc<GuardedConnection>, seg_id: &str, regime_id: Option<&str>) {
    if let Some(r) = regime_id {
        ensure_regime(conn, r);
    }
    let guard = conn.test_lock();
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
    use maekon_core::quantization::ScalarQuantizer;

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

// ── #5755 FLAG-DIM-768 dimension guard tests ────────────────────────────

/// A: First store records the corpus dims in app_meta.
#[tokio::test]
async fn corpus_dims_first_write_recorded() {
    let conn = setup_db();
    let store = SqliteVectorStore::new(conn.clone());

    store
        .store(
            vec![1.0, 0.0, 0.0], // 3-dim
            EmbeddingMetadata {
                segment_id: "d1".to_string(),
                content_type: EmbeddingContentType::ContentActivity,
                content_label: None,
                timestamp: Utc::now(),
                original_text: "t".to_string(),
                model_id: "m-384".to_string(),
            },
        )
        .await
        .unwrap();

    let stored: Option<String> = {
        let g = conn.test_lock();
        g.query_row(
            "SELECT value FROM app_meta WHERE key = 'embedding_corpus_dims'",
            [],
            |r| r.get(0),
        )
        .ok()
    };
    assert_eq!(
        stored.as_deref(),
        Some("3"),
        "corpus dims must be recorded after first store"
    );
}

/// B: Matching-dimension write is a no-op (no stale marking, no app_meta change).
#[tokio::test]
async fn corpus_dims_matching_write_no_side_effect() {
    let conn = setup_db();
    let store = SqliteVectorStore::new(conn.clone());

    // First write establishes 3-dim corpus.
    store
        .store(
            vec![1.0, 0.0, 0.0],
            EmbeddingMetadata {
                segment_id: "s1".to_string(),
                content_type: EmbeddingContentType::ContentActivity,
                content_label: None,
                timestamp: Utc::now(),
                original_text: "t".to_string(),
                model_id: "m-3".to_string(),
            },
        )
        .await
        .unwrap();

    // Second write with same dims — no stale rows expected.
    store
        .store(
            vec![0.0, 1.0, 0.0],
            EmbeddingMetadata {
                segment_id: "s2".to_string(),
                content_type: EmbeddingContentType::ContentActivity,
                content_label: None,
                timestamp: Utc::now(),
                original_text: "t".to_string(),
                model_id: "m-3".to_string(),
            },
        )
        .await
        .unwrap();

    let stale_count: i64 = {
        let g = conn.test_lock();
        g.query_row(
            "SELECT COUNT(*) FROM embedding_vectors WHERE is_stale = 1",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(
        stale_count, 0,
        "same-dim write must not mark any rows stale"
    );
}

/// C: Dimension transition marks pre-existing rows stale and updates app_meta.
#[tokio::test]
async fn corpus_dims_transition_marks_old_rows_stale() {
    let conn = setup_db();
    let store = SqliteVectorStore::new(conn.clone());

    // Two 3-dim rows stored first.
    for i in 0..2u8 {
        store
            .store(
                vec![1.0, 0.0, 0.0],
                EmbeddingMetadata {
                    segment_id: format!("seg-3d-{i}"),
                    content_type: EmbeddingContentType::ContentActivity,
                    content_label: None,
                    timestamp: Utc::now(),
                    original_text: "t".to_string(),
                    model_id: "m-384".to_string(),
                },
            )
            .await
            .unwrap();
    }

    // Verify both are active.
    {
        let g = conn.test_lock();
        let active: i64 = g
            .query_row(
                "SELECT COUNT(*) FROM embedding_vectors WHERE is_stale = 0",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(active, 2);
    }

    // Now write a 5-dim row — dimension transition.
    store
        .store(
            vec![0.1, 0.2, 0.3, 0.4, 0.5],
            EmbeddingMetadata {
                segment_id: "seg-5d".to_string(),
                content_type: EmbeddingContentType::ContentActivity,
                content_label: None,
                timestamp: Utc::now(),
                original_text: "t".to_string(),
                model_id: "m-768".to_string(),
            },
        )
        .await
        .unwrap();

    // app_meta must now record 5.
    let new_dims_stored: String = {
        let g = conn.test_lock();
        g.query_row(
            "SELECT value FROM app_meta WHERE key = 'embedding_corpus_dims'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(
        new_dims_stored, "5",
        "app_meta must reflect new corpus dims"
    );

    // The two original 3-dim rows must be stale.
    let stale_count: i64 = {
        let g = conn.test_lock();
        g.query_row(
            "SELECT COUNT(*) FROM embedding_vectors WHERE is_stale = 1",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(
        stale_count, 2,
        "old-dim rows must be marked stale on dimension transition"
    );

    // The new 5-dim row must be active.
    let active_count: i64 = {
        let g = conn.test_lock();
        g.query_row(
            "SELECT COUNT(*) FROM embedding_vectors WHERE is_stale = 0",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(
        active_count, 1,
        "new-dim row must remain active after transition"
    );
}

/// D (read-side): Mixed 384d/768d corpus — 768d query must only return 768d rows.
/// This validates the brute_force_search_quantized skip guard.
#[tokio::test]
async fn search_quantized_skips_wrong_dim_rows() {
    use maekon_core::quantization::ScalarQuantizer;

    let conn = setup_db();
    let store = SqliteVectorStore::new(conn.clone());

    // Store a 3-dim quantized row.
    let v3 = vec![1.0_f32, 0.0, 0.0];
    let qv3 = ScalarQuantizer::quantize(&v3).unwrap();
    store
        .store_quantized(
            v3.clone(),
            &qv3,
            EmbeddingMetadata {
                segment_id: "dim3".to_string(),
                content_type: EmbeddingContentType::ContentActivity,
                content_label: None,
                timestamp: Utc::now(),
                original_text: "t".to_string(),
                model_id: "m-3".to_string(),
            },
            false,
        )
        .await
        .unwrap();

    // Manually bypass the write-side guard (set is_stale=0 so it is not filtered by
    // the SQL WHERE clause) and force-insert a 5-dim row with a different model,
    // simulating a mixed corpus left over before the guard was in place.
    {
        let g = conn.test_lock();
        let v5 = vec![0.1_f32, 0.2, 0.3, 0.4, 0.5];
        let qv5 = ScalarQuantizer::quantize(&v5).unwrap();
        let int8_blob: Vec<u8> = qv5.data.iter().map(|&b| b as u8).collect();
        g.execute(
            "INSERT INTO embedding_vectors (segment_id, content_type, content_label, original_text, \
             vector, model_id, timestamp, vector_int8, quant_scale, quant_offset, \
             hlc_wall_ms, hlc_counter, origin_device_id) \
             VALUES ('dim5', 'CONTENT_ACTIVITY', NULL, 't', X'', 'm-5', \
             datetime('now'), ?1, ?2, ?3, 0, 0, 'test')",
            rusqlite::params![int8_blob, qv5.scale, qv5.offset],
        )
        .unwrap();
    }

    // Query with a 5-dim vector — only the 5-dim row must appear; 3-dim must be skipped.
    let q5 = vec![0.1_f32, 0.2, 0.3, 0.4, 0.5];
    let qq5 = ScalarQuantizer::quantize(&q5).unwrap();
    let results = store
        .search_quantized(&qq5, 10, 0.0, &SearchFilters::default())
        .await
        .unwrap();

    let ids: Vec<&str> = results.iter().map(|r| r.segment_id.as_str()).collect();
    assert_eq!(
        results.len(),
        1,
        "only the dim-matching row should be returned, got: {ids:?}"
    );
    assert_eq!(results[0].segment_id, "dim5");

    // Inverse: 3-dim query must only return the 3-dim row (5-dim skipped).
    let q3 = vec![1.0_f32, 0.0, 0.0];
    let qq3 = ScalarQuantizer::quantize(&q3).unwrap();
    let results3 = store
        .search_quantized(&qq3, 10, 0.0, &SearchFilters::default())
        .await
        .unwrap();

    let ids3: Vec<&str> = results3.iter().map(|r| r.segment_id.as_str()).collect();
    assert_eq!(
        results3.len(),
        1,
        "inverse: only the dim-matching row should appear, got: {ids3:?}"
    );
    assert_eq!(results3[0].segment_id, "dim3");
}

/// E: store_quantized dimension guard — first write records INT8 dims in app_meta.
#[tokio::test]
async fn corpus_dims_quantized_first_write_recorded() {
    use maekon_core::quantization::ScalarQuantizer;

    let conn = setup_db();
    let store = SqliteVectorStore::new(conn.clone());

    let v = vec![0.1_f32, 0.2, 0.3, 0.4]; // 4-dim
    let qv = ScalarQuantizer::quantize(&v).unwrap();

    store
        .store_quantized(
            v,
            &qv,
            EmbeddingMetadata {
                segment_id: "q1".to_string(),
                content_type: EmbeddingContentType::ContentActivity,
                content_label: None,
                timestamp: Utc::now(),
                original_text: "t".to_string(),
                model_id: "m-4".to_string(),
            },
            false,
        )
        .await
        .unwrap();

    let stored: Option<String> = {
        let g = conn.test_lock();
        g.query_row(
            "SELECT value FROM app_meta WHERE key = 'embedding_corpus_dims'",
            [],
            |r| r.get(0),
        )
        .ok()
    };
    assert_eq!(
        stored.as_deref(),
        Some("4"),
        "store_quantized must record corpus dims in app_meta"
    );
}

/// F (#5987): Batched bulk stale-mark — corpus_dims_guard marks ALL rows stale
/// when the dimension transitions, even when the row count exceeds the batch size.
///
/// Inserts rows whose count is larger than STALE_BATCH_SIZE (1000) to confirm
/// that the loop runs multiple iterations and still marks every non-stale row.
/// Uses direct SQL inserts (bypassing store()) to avoid the per-row HLC overhead
/// while still exercising the guard logic under realistic volume.
#[tokio::test]
async fn corpus_dims_transition_batched_marks_all_stale() {
    let conn = setup_db();
    let store = SqliteVectorStore::new(conn.clone());

    // Seed BATCH_SIZE + 1 rows directly at is_stale=0 with 3-dim model.
    // This forces at least two iterations of the batch loop (1000 + 1 rows).
    const ROW_COUNT: usize = 1001;
    {
        let g = conn.test_lock();
        // Use a transaction for bulk insert speed.
        g.execute_batch("BEGIN").unwrap();
        let blob: Vec<u8> = f32_vec_to_bytes(&[1.0_f32, 0.0, 0.0]);
        for i in 0..ROW_COUNT {
            g.execute(
                "INSERT INTO embedding_vectors \
                 (segment_id, content_type, content_label, original_text, \
                  vector, model_id, timestamp, hlc_wall_ms, hlc_counter, origin_device_id) \
                 VALUES (?1, 'CONTENT_ACTIVITY', NULL, 'bulk', ?2, 'm-3', \
                         datetime('now'), 0, 0, 'test')",
                rusqlite::params![format!("bulk-{i}"), blob],
            )
            .unwrap();
        }
        g.execute_batch("COMMIT").unwrap();
        // Seed the corpus dims key so the guard sees a transition (3 → 5).
        g.execute(
            "INSERT OR REPLACE INTO app_meta (key, value) VALUES ('embedding_corpus_dims', '3')",
            [],
        )
        .unwrap();
    }

    // Trigger a dimension transition by storing a 5-dim row.
    store
        .store(
            vec![0.1, 0.2, 0.3, 0.4, 0.5],
            EmbeddingMetadata {
                segment_id: "new-5d".to_string(),
                content_type: EmbeddingContentType::ContentActivity,
                content_label: None,
                timestamp: Utc::now(),
                original_text: "t".to_string(),
                model_id: "m-5".to_string(),
            },
        )
        .await
        .unwrap();

    // All ROW_COUNT bulk rows must now be stale.
    let stale_count: i64 = {
        let g = conn.test_lock();
        g.query_row(
            "SELECT COUNT(*) FROM embedding_vectors WHERE is_stale = 1",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(
        stale_count as usize, ROW_COUNT,
        "batched stale-mark must cover all {ROW_COUNT} rows, got {stale_count}"
    );

    // The newly inserted 5-dim row must be active.
    let active_count: i64 = {
        let g = conn.test_lock();
        g.query_row(
            "SELECT COUNT(*) FROM embedding_vectors WHERE is_stale = 0",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(
        active_count, 1,
        "new-dim row must remain active after batched transition"
    );
}

// ── #5642 IVF routing tests ───────────────────────────────────────────────────

use crate::sqlite::vector_index_impl::SqliteVectorIndex;
use maekon_core::ports::vector_index::VectorIndex;

/// Store N quantized vectors via SqliteVectorStore (for routing tests that need
/// both VectorStore and VectorIndex on the same GuardedConnection).
async fn store_quantized_n(conn: &Arc<GuardedConnection>, n: usize, dims: usize, model_id: &str) {
    use maekon_core::quantization::ScalarQuantizer;
    let store = SqliteVectorStore::new(conn.clone());
    for i in 0..n {
        // Vectors are spread across dims dimensions to give k-means something to cluster.
        let mut v = vec![0.0f32; dims];
        v[i % dims] = 1.0 + i as f32 * 0.01;
        let qv = ScalarQuantizer::quantize(&v).unwrap();
        store
            .store_quantized(
                v,
                &qv,
                EmbeddingMetadata {
                    segment_id: format!("ivf-rt-{model_id}-{i}"),
                    content_type: EmbeddingContentType::ContentActivity,
                    content_label: None,
                    timestamp: Utc::now(),
                    original_text: format!("text-{i}"),
                    model_id: model_id.to_string(),
                },
                false,
            )
            .await
            .unwrap();
    }
}

/// R-IVF-1: Before IVF build, `search` must use brute-force (existing behaviour
/// is unchanged — all stored vectors returned by a matching query).
#[tokio::test]
async fn ivf_routing_not_built_falls_back_to_brute_force() {
    let conn = setup_db();
    let store = SqliteVectorStore::new(conn.clone());

    // Store two vectors.
    for seg in &["bf-a", "bf-b"] {
        store
            .store(
                vec![1.0, 0.0, 0.0],
                EmbeddingMetadata {
                    segment_id: seg.to_string(),
                    content_type: EmbeddingContentType::ContentActivity,
                    content_label: None,
                    timestamp: Utc::now(),
                    original_text: seg.to_string(),
                    model_id: "m".to_string(),
                },
            )
            .await
            .unwrap();
    }

    // IVF not built — brute-force must return both rows.
    let results = store.search(&[1.0, 0.0, 0.0], 10, 0.0).await.unwrap();
    assert_eq!(
        results.len(),
        2,
        "before IVF build, brute-force must return all matching rows"
    );
}

/// R-IVF-2: After IVF build, `search` routes through IVF and top-1 result
/// matches the brute-force top-1 (accuracy regression gate).
#[tokio::test]
async fn ivf_routing_after_build_top1_matches_brute_force() {
    use maekon_core::quantization::ScalarQuantizer;

    let dims = 8;
    let n_vectors = 30;
    let conn = setup_db();
    store_quantized_n(&conn, n_vectors, dims, "m1").await;

    let index = SqliteVectorIndex::new(conn.clone());
    let n_clusters = 3;
    index.build_ivf_index(n_clusters, 10).await.unwrap();

    let store = SqliteVectorStore::new(conn.clone());

    // Query near dimension 0 (cluster 0 territory).
    let query: Vec<f32> = {
        let mut v = vec![0.0f32; dims];
        v[0] = 1.0;
        v
    };

    // Brute-force baseline via search_quantized.
    let q_query = ScalarQuantizer::quantize(&query).unwrap();
    let bf_results = store
        .search_quantized(&q_query, 5, 0.0, &SearchFilters::default())
        .await
        .unwrap();

    // IVF-routed path via `search`.
    let ivf_results = store.search(&query, 5, 0.0).await.unwrap();

    assert!(
        !ivf_results.is_empty(),
        "IVF routing must return at least one result"
    );
    assert!(
        !bf_results.is_empty(),
        "brute-force baseline must return at least one result"
    );

    // Top-1 must agree between IVF and brute-force.
    assert_eq!(
        ivf_results[0].segment_id, bf_results[0].segment_id,
        "IVF top-1 ({}) must match brute-force top-1 ({})",
        ivf_results[0].segment_id, bf_results[0].segment_id
    );
}

/// R-IVF-3: Unindexed-delta vectors (inserted after build) are included in the
/// IVF-routed result.  We verify this by ensuring the delta vector is visible in
/// a large `limit` result set (limit=50 >> corpus of 21 vectors), which must
/// include ALL vectors regardless of the search path.
#[tokio::test]
async fn ivf_routing_includes_unindexed_delta() {
    use maekon_core::quantization::ScalarQuantizer;

    let dims = 4;
    let n_initial = 20usize;
    let conn = setup_db();
    // Use a single store instance throughout so corpus_dims_cache is warm.
    let store = SqliteVectorStore::new(conn.clone());

    // Store initial vectors via the store (warms corpus_dims_cache to 4).
    for i in 0..n_initial {
        let mut v = vec![0.0f32; dims];
        v[i % dims] = 1.0 + i as f32 * 0.01;
        let qv = ScalarQuantizer::quantize(&v).unwrap();
        store
            .store_quantized(
                v,
                &qv,
                EmbeddingMetadata {
                    segment_id: format!("ivf-rt-m1-{i}"),
                    content_type: EmbeddingContentType::ContentActivity,
                    content_label: None,
                    timestamp: Utc::now(),
                    original_text: format!("text-{i}"),
                    model_id: "m1".to_string(),
                },
                false,
            )
            .await
            .unwrap();
    }

    let index = SqliteVectorIndex::new(conn.clone());
    index.build_ivf_index(3, 10).await.unwrap();

    // Insert a post-build vector (unindexed delta).
    let delta_v: Vec<f32> = vec![0.0, 0.0, 0.0, 1.0];
    let delta_qv = ScalarQuantizer::quantize(&delta_v).unwrap();
    store
        .store_quantized(
            delta_v.clone(),
            &delta_qv,
            EmbeddingMetadata {
                segment_id: "post-build-delta".to_string(),
                content_type: EmbeddingContentType::ContentActivity,
                content_label: None,
                timestamp: Utc::now(),
                original_text: "delta".to_string(),
                model_id: "m1".to_string(),
            },
            false,
        )
        .await
        .unwrap();

    // Verify the delta is unindexed (not in ivf_assignments).
    let unindexed: i64 = {
        let g = conn.test_lock();
        g.query_row(
            "SELECT COUNT(*) FROM embedding_vectors \
             WHERE is_stale = 0 AND id NOT IN (SELECT vector_id FROM ivf_assignments)",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(unindexed, 1, "exactly one unindexed-delta vector expected");

    // With limit >> corpus size (50 > 21), ALL vectors must be returned regardless
    // of the search path (IVF+delta or brute-force fallback).
    let results = store.search(&delta_v, 50, 0.0).await.unwrap();
    let ids: Vec<&str> = results.iter().map(|r| r.segment_id.as_str()).collect();

    // The delta must appear in the result set.
    assert!(
        ids.contains(&"post-build-delta"),
        "unindexed-delta vector must appear in IVF-routed search results (limit=50 covers all 21 vectors); got {ids:?}"
    );

    // Total result count must equal the full corpus (all 21 vectors are relevant
    // to the query direction; no vector is orthogonal).
    assert_eq!(
        results.len(),
        n_initial + 1,
        "expected all {} vectors (IVF+delta merge), got {}; ids={ids:?}",
        n_initial + 1,
        results.len()
    );
}

/// R-IVF-4: `search_filtered` also routes through IVF and honours filters.
#[tokio::test]
async fn ivf_routing_filtered_respects_content_type_filter() {
    use maekon_core::quantization::ScalarQuantizer;

    let dims = 4;
    let conn = setup_db();

    // Store 20 ContentActivity vectors via quantized path.
    let store_tmp = SqliteVectorStore::new(conn.clone());
    for i in 0..20 {
        let mut v = vec![0.0f32; dims];
        v[i % dims] = 1.0 + i as f32 * 0.01;
        let qv = ScalarQuantizer::quantize(&v).unwrap();
        store_tmp
            .store_quantized(
                v,
                &qv,
                EmbeddingMetadata {
                    segment_id: format!("filt-act-{i}"),
                    content_type: EmbeddingContentType::ContentActivity,
                    content_label: None,
                    timestamp: Utc::now(),
                    original_text: format!("act-{i}"),
                    model_id: "m1".to_string(),
                },
                false,
            )
            .await
            .unwrap();
    }
    // One SegmentSummary vector — must be excluded by the filter.
    let v_sum = vec![1.0, 0.0, 0.0, 0.0];
    let qv_sum = ScalarQuantizer::quantize(&v_sum).unwrap();
    store_tmp
        .store_quantized(
            v_sum.clone(),
            &qv_sum,
            EmbeddingMetadata {
                segment_id: "filt-summary".to_string(),
                content_type: EmbeddingContentType::SegmentSummary,
                content_label: None,
                timestamp: Utc::now(),
                original_text: "summary".to_string(),
                model_id: "m1".to_string(),
            },
            false,
        )
        .await
        .unwrap();

    let index = SqliteVectorIndex::new(conn.clone());
    index.build_ivf_index(3, 10).await.unwrap();

    let store = SqliteVectorStore::new(conn.clone());
    let query = vec![1.0, 0.0, 0.0, 0.0];
    let filters = SearchFilters {
        content_types: Some(vec![EmbeddingContentType::ContentActivity]),
        ..Default::default()
    };
    let results = store
        .search_filtered(&query, 10, 0.0, &filters)
        .await
        .unwrap();

    let ids: Vec<&str> = results.iter().map(|r| r.segment_id.as_str()).collect();
    assert!(
        !ids.contains(&"filt-summary"),
        "SegmentSummary must be excluded by content_type filter; got {ids:?}"
    );
    assert!(
        results.iter().all(|r| r.segment_id.starts_with("filt-act")),
        "all results must be ContentActivity; got {ids:?}"
    );
}

/// R-IVF-5: Dimension mismatch (corpus 4d, query 3d) must fall back to brute-force
/// without error (the IVF gate guards on cached_dims == query_dims).
#[tokio::test]
async fn ivf_routing_dim_mismatch_falls_back_gracefully() {
    let dims = 4;
    let conn = setup_db();
    store_quantized_n(&conn, 10, dims, "m1").await;

    let index = SqliteVectorIndex::new(conn.clone());
    index.build_ivf_index(2, 5).await.unwrap();

    let store = SqliteVectorStore::new(conn.clone());

    // Query with wrong dimension — must not error, returns (possibly empty) results.
    let wrong_dim_query = vec![1.0, 0.0, 0.0]; // 3d vs corpus 4d
    let hits = store
        .search(&wrong_dim_query, 5, 0.0)
        .await
        .expect("dim-mismatch query must not return Err");
    // The result may be empty when dims differ — the invariant is no Err.
    let _ = hits;
}

/// R-IVF-6: cold-start routing regression guard (#5642).
///
/// When the app restarts against a DB that already has a corpus + a built IVF
/// index, a new `SqliteVectorStore` instance initializes `corpus_dims_cache` to
/// `DIMS_CACHE_UNSET`(0). Even with no write path, the first `search()` call must
/// have `try_hydrate_dims_cache` read the persisted dims from `app_meta` and
/// activate the IVF path.
///
/// Assertion: store2's top-1 (no writes) matches the brute-force top-1
/// → proves the IVF path activated correctly or returned the same answer.
#[tokio::test]
async fn ivf_routing_cold_start_hydrates_cache_from_app_meta() {
    use maekon_core::quantization::ScalarQuantizer;

    let dims = 8;
    let n_vectors = 30;
    let conn = setup_db();

    // store1: insert vectors (populates both corpus_dims_cache + app_meta).
    store_quantized_n(&conn, n_vectors, dims, "cs").await;

    // Build the IVF index.
    let index = SqliteVectorIndex::new(conn.clone());
    index.build_ivf_index(3, 10).await.unwrap();

    // store2: same DB connection, no writes → corpus_dims_cache = DIMS_CACHE_UNSET.
    // Simulates the "cold-start restart" scenario.
    let store2 = SqliteVectorStore::new(conn.clone());

    let query: Vec<f32> = {
        let mut v = vec![0.0f32; dims];
        v[0] = 1.0;
        v
    };

    // store2.search() → try_hydrate_dims_cache() → read app_meta → activate IVF path.
    let cold_results = store2.search(&query, 5, 0.0).await.unwrap();

    // brute-force baseline (using search_quantized).
    let store_bf = SqliteVectorStore::new(conn.clone());
    let q_query = ScalarQuantizer::quantize(&query).unwrap();
    let bf_results = store_bf
        .search_quantized(&q_query, 5, 0.0, &SearchFilters::default())
        .await
        .unwrap();

    assert!(
        !cold_results.is_empty(),
        "cold-start search must return results (IVF or brute-force)"
    );
    assert!(
        !bf_results.is_empty(),
        "brute-force baseline results must not be empty"
    );

    // top-1 match: either IVF activated or brute-force returned the same answer.
    // Both cases prove there is no accuracy regression.
    assert_eq!(
        cold_results[0].segment_id, bf_results[0].segment_id,
        "cold-start top-1 ({}) must match brute-force top-1 ({})",
        cold_results[0].segment_id, bf_results[0].segment_id,
    );
}

// ── #5811 compute_nprobe unit tests ──────────────────────────────────────────

use super::trait_impl::compute_nprobe;

/// Boundary: n_clusters == 0 must return 0 (no index; routing gate does not fire).
#[test]
fn nprobe_zero_clusters_returns_zero() {
    assert_eq!(compute_nprobe(0), 0);
}

/// n_clusters == 1: single cluster; must return 1 (probe the only cluster).
#[test]
fn nprobe_one_cluster_returns_one() {
    assert_eq!(compute_nprobe(1), 1);
}

/// n_clusters <= MIN_NPROBE (10): every cluster should be probed.
#[test]
fn nprobe_small_corpus_probes_all_clusters() {
    // For n_clusters 1..=10 the formula yields 30 % which is < 10, so
    // the MIN_NPROBE floor kicks in; but the .min(n_clusters) cap means
    // the result is exactly n_clusters when n_clusters <= MIN_NPROBE.
    for k in 1usize..=10 {
        assert_eq!(
            compute_nprobe(k),
            k,
            "n_clusters={k}: should probe all clusters"
        );
    }
}

/// n_clusters == 33 (scheduler builds sqrt(1000) ≈ 33): nprobe must be 10
/// (MIN_NPROBE floor) since ceil(33 * 0.3) = 10.
#[test]
fn nprobe_33_clusters_is_ten() {
    // ceil(33 * 0.3) = ceil(9.9) = 10, clamped to [10, 33] → 10.
    assert_eq!(compute_nprobe(33), 10);
}

/// n_clusters == 100: nprobe must be ceil(100 * 0.3) = 30.
#[test]
fn nprobe_100_clusters_is_thirty() {
    assert_eq!(compute_nprobe(100), 30);
}

/// n_clusters == 1000: nprobe must be ceil(1000 * 0.3) = 300.
#[test]
fn nprobe_1000_clusters_is_three_hundred() {
    assert_eq!(compute_nprobe(1000), 300);
}

/// Result must never exceed n_clusters regardless of NPROBE_RATIO.
#[test]
fn nprobe_never_exceeds_n_clusters() {
    for k in [1, 2, 5, 10, 11, 33, 100, 500, 1000] {
        let p = compute_nprobe(k);
        assert!(p <= k, "nprobe {p} must not exceed n_clusters {k}");
    }
}

/// n_clusters == 11: the proportional floor (MIN_NPROBE) applies because
/// ceil(11 * 0.3) = ceil(3.3) = 4 < MIN_NPROBE(10) → clamped to 10,
/// then .min(11) → 10.
#[test]
fn nprobe_11_clusters_hits_min_floor() {
    assert_eq!(compute_nprobe(11), 10);
}

// ── #6113: atomic rowid return (HNSW TOCTOU fix) ──────────────────────────────

fn meta_for(seg: &str) -> EmbeddingMetadata {
    EmbeddingMetadata {
        segment_id: seg.to_string(),
        content_type: EmbeddingContentType::ContentActivity,
        content_label: Some(seg.to_string()),
        timestamp: Utc::now(),
        original_text: seg.to_string(),
        model_id: "test-model".to_string(),
    }
}

/// `store_returning_id` must return the rowid of the row it just inserted, read
/// inside the same write lock as the INSERT. With interleaved writes the IDs
/// must be distinct and strictly increasing — never the same value that a
/// separate `last_insert_id()` could observe after another write raced in.
#[tokio::test]
async fn store_returning_id_matches_inserted_row() {
    let conn = setup_db();
    let store = SqliteVectorStore::new(conn.clone());

    let id1 = store
        .store_returning_id(vec![1.0, 0.0, 0.0], meta_for("seg-a"))
        .await
        .unwrap();
    let id2 = store
        .store_returning_id(vec![0.0, 1.0, 0.0], meta_for("seg-b"))
        .await
        .unwrap();
    let id3 = store
        .store_returning_id(vec![0.0, 0.0, 1.0], meta_for("seg-c"))
        .await
        .unwrap();

    // Strictly increasing, distinct rowids.
    assert!(id1 >= 1, "first rowid must be a real id, got {id1}");
    assert!(id2 > id1, "rowids must increase: {id1} -> {id2}");
    assert!(id3 > id2, "rowids must increase: {id2} -> {id3}");

    // Each returned id must map back to the correct segment in the table.
    let guard = conn.test_lock();
    let seg_for = |id: u64| -> String {
        guard
            .query_row(
                "SELECT segment_id FROM embedding_vectors WHERE id = ?1",
                params![id as i64],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(seg_for(id1), "seg-a");
    assert_eq!(seg_for(id2), "seg-b");
    assert_eq!(seg_for(id3), "seg-c");
}

/// `store_quantized_returning_id` carries the same atomic-rowid contract on the
/// quantized write path used by the embedding pipeline's HNSW sync.
#[tokio::test]
async fn store_quantized_returning_id_matches_inserted_row() {
    use maekon_core::quantization::ScalarQuantizer;

    let conn = setup_db();
    let store = SqliteVectorStore::new(conn.clone());

    let v1 = vec![0.1, 0.5, 0.9];
    let v2 = vec![0.9, 0.5, 0.1];
    let q1 = ScalarQuantizer::quantize(&v1).unwrap();
    let q2 = ScalarQuantizer::quantize(&v2).unwrap();

    let id1 = store
        .store_quantized_returning_id(v1.clone(), &q1, meta_for("q-a"), false)
        .await
        .unwrap();
    let id2 = store
        .store_quantized_returning_id(v2.clone(), &q2, meta_for("q-b"), false)
        .await
        .unwrap();

    assert!(id1 >= 1, "first quantized rowid must be real, got {id1}");
    assert!(id2 > id1, "quantized rowids must increase: {id1} -> {id2}");

    let guard = conn.test_lock();
    let seg_for = |id: u64| -> String {
        guard
            .query_row(
                "SELECT segment_id FROM embedding_vectors WHERE id = ?1",
                params![id as i64],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(seg_for(id1), "q-a");
    assert_eq!(seg_for(id2), "q-b");
}

/// Concurrent writers must each receive their own rowid. This is the core of
/// the #6113 TOCTOU fix: the rowid is read under the same lock as the INSERT,
/// so no two interleaved writes can be handed the same `last_insert_rowid()`.
#[tokio::test]
async fn concurrent_store_returning_id_yields_distinct_ids() {
    let conn = setup_db();
    let store = Arc::new(SqliteVectorStore::new(conn.clone()));

    let mut handles = Vec::new();
    for i in 0..16u32 {
        let s = store.clone();
        handles.push(tokio::spawn(async move {
            let mut v = vec![0.0f32; 4];
            v[(i % 4) as usize] = 1.0 + i as f32 * 0.01;
            s.store_returning_id(v, meta_for(&format!("c-{i}")))
                .await
                .unwrap()
        }));
    }

    let mut ids = Vec::new();
    for h in handles {
        ids.push(h.await.unwrap());
    }
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(
        ids.len(),
        16,
        "all 16 concurrent writes must return distinct rowids"
    );
}
